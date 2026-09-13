//! Decodes the *schema* half of a pluggable widget's BSON — the
//! `CustomWidgets$CustomWidgetType` document a `CustomWidgets$CustomWidget`
//! instance's `Type` field carries inline. Ports the schema-decode
//! functions (`decode_widget_type`/`decode_object_type`/
//! `decode_property_type`/`decode_value_type`, plus their small decode
//! helpers for enumerations/action variables/return types/translations)
//! from `Mxrb::Pluggable::MprCodec` in `lib/mxrb/pluggable/mpr_codec.rb`.
//!
//! This is a *slice* of that file, not the whole thing. The widget
//! *instance* half — `decode_object`/`decode_value` — is now ported for
//! every value kind that's self-contained (`Boolean`/`Integer`/`Decimal`/
//! `String`/`Enumeration`/`Selection`/`Expression`/`EntityConstraint`/
//! `TranslatableString`/`System`/`Object` recursion, plus the
//! `File`/`Form`/`Image`/`Microflow`/`Nanoflow` string-path references)
//! *and* for `Attribute`/`Entity`/`Association` — these three route
//! through `mxrs-forms-refs::{decode_attribute_reference,
//! decode_entity_reference}` (extracted out of `mxrs-forms` specifically
//! so this crate could reach them without creating a dependency cycle —
//! see that crate's doc comment for why). See [`node`](crate::node) for
//! the decoded shape, including [`node::ReferenceTarget`] for how an
//! `Association`'s two possible reference shapes are represented.
//!
//! **`DataSource`/`Widgets`/`TextTemplate`/`Action`/`Icon` are all closed
//! in full now**, via [`crate::embedded::EmbeddedFormsDecoder`] — a
//! dependency-inversion trait this crate defines and `mxrs-forms::MprCodec`
//! implements, instead of the full `Node`/`Catalog` crate-extraction this
//! doc previously proposed (that plan is superseded: it would have moved
//! ~850 lines for no reason beyond avoiding a trait, when mxrb's own
//! architecture already names the exact right seam — `Pluggable::MprCodec`
//! is constructed with a `forms_codec:` callback object it calls
//! `decode_embedded` on. The trait here is that callback, typed).
//! `decode_value`'s `TextTemplate`/`Action`/`Icon` arms decode straight
//! through [`decode_embedded_field`]; `decode_data_source_value` ports
//! `decode_data_source` in full (an XPath/database source decoded
//! field-by-field, `sort_bar`/`source_variable` included, or a source of
//! any other `$Type` decoded whole via the same decoder — mirroring
//! mxrb's untyped `forms_codec.decode_embedded(source, ...)` return with
//! the explicit [`node::DataSourceValue`] sum type); `decode_widgets_value`
//! ports `decode_widgets` in full (`CustomWidgets$CustomWidget` items
//! self-recurse, any other item routes through the decoder — see
//! [`node::WidgetItem`]). `Value`/`ObjectNode`/`Assignment` are generic
//! over the embedded-node type (`N` / `D::Node`) as a direct consequence.
//!
//! **`PluggableError::NeedsFormsIntegration` is consequently dead code as
//! of this pass** — every `ValueType::kind` mxrb's own schema can produce
//! now has a real decode path. Left in the enum (not removed) as a
//! documented, still-typed escape hatch should a genuinely new kind show
//! up in a Mendix version this crate hasn't seen yet; nothing constructs
//! it today.
//!
//! Real-world weight, measured (not guessed) by `tests/instance_oracle.rs`
//! against every real `CustomWidgets$CustomWidget` instance in QRQC/SPC,
//! across every pass that landed this crate's instance-decode support:
//!
//! | | schema+refs only | + TextTemplate/Action/Icon | + DataSource/Widgets (this pass) |
//! |---|---|---|---|
//! | QRQC (378 instances) | 23 fully decoded | 174 fully decoded | **358** fully decoded |
//! | SPC (682 instances) | 13 fully decoded | 460 fully decoded | **657** fully decoded |
//!
//! The remaining 20 (QRQC) / 25 (SPC) instances are **not** a
//! `mxrs-pluggable` gap: `instance_oracle.rs` categorizes every one of
//! them as `PluggableError::EmbeddedDecodeFailed` — a real `TextTemplate`/
//! `Action`/`Icon`/`DataSource`/`Widgets` element exists and this crate
//! correctly handed it to `mxrs-forms`, but `mxrs-forms` itself hit one of
//! its *own*, separately-tracked gaps decoding whatever native Forms
//! element was nested inside (not diagnosed further here — that's
//! `mxrs-forms`'s backlog, not this crate's). Zero unexpected errors (a
//! panic, or any error variant other than `EmbeddedDecodeFailed`) in
//! either file — every failure is named and expected.
//!
//! The writer side is ported as well: [`encode_widget_type`],
//! [`encode_object`], and [`encode_widget`] cover every value kind above
//! and route embedded Forms nodes through
//! [`crate::embedded::EmbeddedFormsEncoder`]. A decoded [`WidgetNode`]
//! retains its original storage baseline so encoding preserves exact
//! inline-schema identity and outer widget fields; a newly-built node gets
//! a complete canonical schema with fresh, internally-consistent UUID
//! pointers. `mxrs-forms::MprCodec` implements both embedded traits and
//! delegates its `Value::Pluggable` encode/decode arms here.

use mxrs_bson::{Bson, Document, extract_id, parse_array};

use crate::catalog::{
    ActionVariable, EnumerationValue, ObjectType, PropertyType, ReturnType, Translation, ValueType,
    WidgetType,
};
use crate::embedded::{EmbeddedFormsDecoder, EmbeddedFormsEncoder};
use crate::error::{PluggableError, Result};
use crate::node::{
    Assignment, DataSource, DataSourceValue, ObjectNode, ReferenceTarget, Value, WidgetItem,
    WidgetNode,
};

/// Decoded alongside a widget type: storage-id -> property/object-type,
/// resolved by identity while decoding a single `CustomWidgets$CustomWidget`
/// instance's `Properties` against `TypePointer`s (mxrb's `context` hash).
#[derive(Debug, Default)]
pub struct SchemaContext {
    pub property_types: std::collections::HashMap<String, PropertyType>,
    pub object_types: std::collections::HashMap<String, ObjectType>,
}

/// UUID pointers generated while encoding one inline widget schema.
///
/// The keys are addresses inside the borrowed [`WidgetType`], matching
/// mxrb's identity-keyed hashes. The lifetime prevents this context from
/// outliving (or being used after moving) the schema it was built from.
#[derive(Debug)]
pub struct EncodeContext<'schema> {
    property_ids: std::collections::HashMap<*const PropertyType, String>,
    value_type_ids: std::collections::HashMap<*const ValueType, String>,
    object_type_ids: std::collections::HashMap<*const ObjectType, String>,
    _schema: std::marker::PhantomData<&'schema WidgetType>,
}

impl EncodeContext<'_> {
    fn new() -> Self {
        Self {
            property_ids: std::collections::HashMap::new(),
            value_type_ids: std::collections::HashMap::new(),
            object_type_ids: std::collections::HashMap::new(),
            _schema: std::marker::PhantomData,
        }
    }
}

/// Decodes a complete `CustomWidgets$CustomWidget`, retaining both its
/// inline type and object so the result can be encoded again.
pub fn decode_widget<D: EmbeddedFormsDecoder>(
    document: &Document,
    path: &str,
    decoder: &D,
) -> Result<WidgetNode<D::Node>> {
    require_type(document, "CustomWidgets$CustomWidget", path)?;
    let type_path = format!("{path}.Type");
    let (widget_type, context) =
        decode_widget_type(get_doc(document, "Type", &type_path)?, &type_path)?;
    let object_path = format!("{path}.Object");
    let object = decode_object(
        get_doc(document, "Object", &object_path)?,
        &context,
        &object_path,
        decoder,
    )?;
    Ok(WidgetNode {
        widget_type,
        object,
        storage_baseline: Some(document.clone()),
        // `storage_baseline` already carries every original field verbatim
        // — see `WidgetNode::extra`'s doc comment for why this only needs
        // to be populated on the from-scratch authoring path.
        extra: Document::new(),
    })
}

/// Decodes a `CustomWidgets$CustomWidgetType` document (the `Type` field of
/// a `CustomWidgets$CustomWidget` instance) into a [`WidgetType`] schema.
/// Ports `decode_widget_type`.
pub fn decode_widget_type(document: &Document, path: &str) -> Result<(WidgetType, SchemaContext)> {
    require_type(document, "CustomWidgets$CustomWidgetType", path)?;
    let mut context = SchemaContext::default();
    let object_type_path = format!("{path}.ObjectType");
    let object_type = decode_object_type(
        get_doc(document, "ObjectType", &object_type_path)?,
        &mut context,
        &object_type_path,
    )?;
    let widget_type = WidgetType {
        id: get_str_required(document, "WidgetId", path)?,
        name: get_str_fallback(document, "WidgetName", "Name", ""),
        description: get_str_fallback(document, "WidgetDescription", "Description", ""),
        prompt: get_str_or(document, "Prompt", ""),
        studio_pro_category: get_str_or(document, "StudioProCategory", ""),
        studio_category: get_str_or(document, "StudioCategory", ""),
        platform: get_str_or(document, "SupportedPlatform", "Web"),
        offline: get_bool_or(document, "OfflineCapable", false),
        needs_context: get_bool_fallback(
            document,
            "WidgetNeedsEntityContext",
            "NeedsEntityContext",
            false,
        ),
        plugin: get_bool_fallback(document, "WidgetPluginWidget", "PluginWidget", false),
        help_url: get_str_or(document, "HelpUrl", ""),
        object_type,
    };
    assert_known(
        document,
        &[
            "$ID",
            "$Type",
            "WidgetId",
            "WidgetName",
            "Name",
            "WidgetDescription",
            "Description",
            "Prompt",
            "StudioProCategory",
            "StudioCategory",
            "SupportedPlatform",
            "OfflineCapable",
            "WidgetNeedsEntityContext",
            "NeedsEntityContext",
            "WidgetPluginWidget",
            "PluginWidget",
            "HelpUrl",
            "ObjectType",
        ],
        path,
    )?;
    Ok((widget_type, context))
}

/// Ports `decode_object_type`.
fn decode_object_type(
    document: &Document,
    context: &mut SchemaContext,
    path: &str,
) -> Result<ObjectType> {
    require_type(document, "CustomWidgets$WidgetObjectType", path)?;
    let object_type_id = document.get("$ID").and_then(extract_id);
    let mut properties = Vec::new();
    for (index, item) in array_docs(document, "PropertyTypes")
        .into_iter()
        .enumerate()
    {
        let item_path = format!("{path}.PropertyTypes[{index}]");
        properties.push(decode_property_type(&item, context, &item_path)?);
    }
    let object_type = ObjectType { properties };
    if let Some(id) = object_type_id {
        context.object_types.insert(id, object_type.clone());
    }
    assert_known(document, &["$ID", "$Type", "PropertyTypes"], path)?;
    Ok(object_type)
}

/// Ports `decode_property_type`.
fn decode_property_type(
    document: &Document,
    context: &mut SchemaContext,
    path: &str,
) -> Result<PropertyType> {
    require_type(document, "CustomWidgets$WidgetPropertyType", path)?;
    let key = ["PropertyKey", "_Key", "Key"]
        .iter()
        .find_map(|field| document.get_str(field).ok())
        .unwrap_or("")
        .to_string();
    let value_type_path = format!("{path}.ValueType");
    let value_type = decode_value_type(
        get_doc(document, "ValueType", &value_type_path)?,
        context,
        &value_type_path,
    )?;
    let property = PropertyType {
        key,
        category: get_str_or(document, "Category", ""),
        caption: get_str_or(document, "Caption", ""),
        description: get_str_or(document, "Description", ""),
        prompt: get_str_or(document, "Prompt", ""),
        default: get_bool_or(document, "IsDefault", false),
        value_type,
    };
    if let Some(id) = document.get("$ID").and_then(extract_id) {
        context.property_types.insert(id, property.clone());
    }
    assert_known(
        document,
        &[
            "$ID",
            "$Type",
            "PropertyKey",
            "_Key",
            "Key",
            "Category",
            "Caption",
            "Description",
            "Prompt",
            "IsDefault",
            "ValueType",
        ],
        path,
    )?;
    Ok(property)
}

/// Ports `decode_value_type`.
fn decode_value_type(
    document: &Document,
    context: &mut SchemaContext,
    path: &str,
) -> Result<ValueType> {
    require_type(document, "CustomWidgets$WidgetValueType", path)?;
    let object_type = match document.get_document("ObjectType") {
        Ok(nested) => Some(Box::new(decode_object_type(
            nested,
            context,
            &format!("{path}.ObjectType"),
        )?)),
        Err(_) => None,
    };
    let value_type = ValueType {
        kind: get_str_or(document, "Type", "String"),
        list: get_bool_or(document, "IsList", false),
        linked: get_bool_or(document, "IsLinked", false),
        metadata: get_bool_or(document, "IsMetaData", false),
        entity_property: get_str_or(document, "EntityProperty", ""),
        allow_non_persistable_entities: get_bool_or(document, "AllowNonPersistableEntities", false),
        path_kind: get_str_or(document, "IsPath", "No"),
        path_type: get_str_or(document, "PathType", "None"),
        parameter_list: get_bool_or(document, "ParameterIsList", false),
        multiline: get_bool_or(document, "Multiline", false),
        default_value: get_str_or(document, "DefaultValue", ""),
        required: get_bool_or(document, "Required", false),
        on_change_property: get_str_or(document, "OnChangeProperty", ""),
        data_source_property: get_str_or(document, "DataSourceProperty", ""),
        selectable_objects_property: get_str_or(document, "SelectableObjectsProperty", ""),
        attribute_types: string_array_fallback(document, "AttributeTypes", "AllowedTypes"),
        association_types: string_array(document, "AssociationTypes"),
        selection_types: string_array(document, "SelectionTypes"),
        enumeration_values: decode_enumerations(document, "EnumerationValues", path)?,
        action_variables: decode_action_variables(document, "ActionVariables", path)?,
        object_type,
        return_type: decode_return_type(document, path)?,
        translations: decode_translations(document, "Translations", path)?,
        set_label: get_bool_or(document, "SetLabel", false),
        default_type: get_str_or(document, "DefaultType", "None"),
        allow_upload: get_bool_or(document, "AllowUpload", false),
    };
    assert_known(
        document,
        &[
            "$ID",
            "$Type",
            "Type",
            "IsList",
            "IsLinked",
            "IsMetaData",
            "EntityProperty",
            "AllowNonPersistableEntities",
            "IsPath",
            "PathType",
            "ParameterIsList",
            "Multiline",
            "DefaultValue",
            "Required",
            "OnChangeProperty",
            "DataSourceProperty",
            "SelectableObjectsProperty",
            "AttributeTypes",
            "AllowedTypes",
            "AssociationTypes",
            "SelectionTypes",
            "EnumerationValues",
            "ActionVariables",
            "ObjectType",
            "ReturnType",
            "Translations",
            "SetLabel",
            "DefaultType",
            "AllowUpload",
        ],
        path,
    )?;
    Ok(value_type)
}

fn decode_enumerations(
    document: &Document,
    field: &str,
    path: &str,
) -> Result<Vec<EnumerationValue>> {
    array_docs(document, field)
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let item_path = format!("{path}.{field}[{index}]");
            assert_known(
                &item,
                &["$ID", "$Type", "_Key", "Key", "Caption"],
                &item_path,
            )?;
            let key = item
                .get_str("_Key")
                .or_else(|_| item.get_str("Key"))
                .unwrap_or("")
                .to_string();
            Ok(EnumerationValue {
                key,
                caption: get_str_or(&item, "Caption", ""),
            })
        })
        .collect()
}

fn decode_action_variables(
    document: &Document,
    field: &str,
    path: &str,
) -> Result<Vec<ActionVariable>> {
    array_docs(document, field)
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let item_path = format!("{path}.{field}[{index}]");
            assert_known(
                &item,
                &["$ID", "$Type", "Key", "_Key", "Type", "Caption"],
                &item_path,
            )?;
            let key = item
                .get_str("Key")
                .or_else(|_| item.get_str("_Key"))
                .unwrap_or("")
                .to_string();
            Ok(ActionVariable {
                key,
                kind: get_str_or(&item, "Type", "None"),
                caption: get_str_or(&item, "Caption", ""),
            })
        })
        .collect()
}

fn decode_return_type(document: &Document, path: &str) -> Result<Option<ReturnType>> {
    let Ok(item) = document.get_document("ReturnType") else {
        return Ok(None);
    };
    let item_path = format!("{path}.ReturnType");
    assert_known(
        item,
        &[
            "$ID",
            "$Type",
            "Type",
            "IsList",
            "EntityProperty",
            "AssignableTo",
        ],
        &item_path,
    )?;
    Ok(Some(ReturnType {
        kind: get_str_or(item, "Type", "None"),
        list: get_bool_or(item, "IsList", false),
        entity_property: get_str_or(item, "EntityProperty", ""),
        assignable_to: get_str_or(item, "AssignableTo", ""),
    }))
}

fn decode_translations(document: &Document, field: &str, path: &str) -> Result<Vec<Translation>> {
    array_docs(document, field)
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let item_path = format!("{path}.{field}[{index}]");
            assert_known(&item, &["$ID", "$Type", "LanguageCode", "Text"], &item_path)?;
            Ok(Translation {
                language: get_str_or(&item, "LanguageCode", ""),
                text: get_str_or(&item, "Text", ""),
            })
        })
        .collect()
}

/// Encodes an inline `CustomWidgets$CustomWidgetType` and returns the
/// identity context required to encode its corresponding object.
pub fn encode_widget_type<'schema>(
    widget_type: &'schema WidgetType,
) -> (Document, EncodeContext<'schema>) {
    let mut context = EncodeContext::new();
    let object_type = encode_object_type(&widget_type.object_type, &mut context);
    let mut document = identified("CustomWidgets$CustomWidgetType");
    document.insert("HelpUrl", widget_type.help_url.clone());
    document.insert("OfflineCapable", widget_type.offline);
    document.insert("StudioCategory", widget_type.studio_category.clone());
    document.insert("StudioProCategory", widget_type.studio_pro_category.clone());
    document.insert("SupportedPlatform", widget_type.platform.clone());
    document.insert("WidgetDescription", widget_type.description.clone());
    document.insert("WidgetId", widget_type.id.clone());
    document.insert("WidgetName", widget_type.name.clone());
    document.insert("Prompt", widget_type.prompt.clone());
    document.insert("WidgetNeedsEntityContext", widget_type.needs_context);
    document.insert("WidgetPluginWidget", widget_type.plugin);
    document.insert("ObjectType", object_type);
    (document, context)
}

fn encode_object_type<'schema>(
    object_type: &'schema ObjectType,
    context: &mut EncodeContext<'schema>,
) -> Document {
    let mut document = identified("CustomWidgets$WidgetObjectType");
    let id = document
        .get_str("$ID")
        .expect("identified document")
        .to_string();
    context
        .object_type_ids
        .insert(std::ptr::from_ref(object_type), id);
    let properties = object_type
        .properties
        .iter()
        .map(|property| Bson::Document(encode_property_type(property, context)))
        .collect();
    document.insert("PropertyTypes", marked(properties, 2));
    document
}

fn encode_property_type<'schema>(
    property: &'schema PropertyType,
    context: &mut EncodeContext<'schema>,
) -> Document {
    let mut document = identified("CustomWidgets$WidgetPropertyType");
    let id = document
        .get_str("$ID")
        .expect("identified document")
        .to_string();
    context
        .property_ids
        .insert(std::ptr::from_ref(property), id);
    document.insert("Caption", property.caption.clone());
    document.insert("Category", property.category.clone());
    document.insert("Description", property.description.clone());
    document.insert("IsDefault", property.default);
    document.insert("PropertyKey", property.key.clone());
    document.insert("Prompt", property.prompt.clone());
    document.insert(
        "ValueType",
        encode_value_type(&property.value_type, context),
    );
    document
}

fn encode_value_type<'schema>(
    value_type: &'schema ValueType,
    context: &mut EncodeContext<'schema>,
) -> Document {
    let mut document = identified("CustomWidgets$WidgetValueType");
    let id = document
        .get_str("$ID")
        .expect("identified document")
        .to_string();
    context
        .value_type_ids
        .insert(std::ptr::from_ref(value_type), id);
    document.insert(
        "ActionVariables",
        marked(
            value_type
                .action_variables
                .iter()
                .map(|value| {
                    let mut item = identified("CustomWidgets$WidgetActionVariable");
                    item.insert("Caption", value.caption.clone());
                    item.insert("Key", value.key.clone());
                    item.insert("Type", value.kind.clone());
                    Bson::Document(item)
                })
                .collect(),
            2,
        ),
    );
    document.insert(
        "AllowedTypes",
        marked_strings(&value_type.attribute_types, 1),
    );
    document.insert(
        "AllowNonPersistableEntities",
        value_type.allow_non_persistable_entities,
    );
    document.insert("AllowUpload", value_type.allow_upload);
    document.insert(
        "AssociationTypes",
        marked_strings(&value_type.association_types, 1),
    );
    document.insert(
        "DataSourceProperty",
        value_type.data_source_property.clone(),
    );
    document.insert("DefaultType", value_type.default_type.clone());
    document.insert("DefaultValue", value_type.default_value.clone());
    document.insert("EntityProperty", value_type.entity_property.clone());
    document.insert(
        "EnumerationValues",
        marked(
            value_type
                .enumeration_values
                .iter()
                .map(|value| {
                    let mut item = identified("CustomWidgets$WidgetEnumerationValue");
                    item.insert("_Key", value.key.clone());
                    item.insert("Caption", value.caption.clone());
                    Bson::Document(item)
                })
                .collect(),
            2,
        ),
    );
    document.insert("IsLinked", value_type.linked);
    document.insert("IsList", value_type.list);
    document.insert("IsMetaData", value_type.metadata);
    document.insert("IsPath", value_type.path_kind.clone());
    document.insert("Multiline", value_type.multiline);
    document.insert(
        "ObjectType",
        value_type
            .object_type
            .as_deref()
            .map(|object| Bson::Document(encode_object_type(object, context)))
            .unwrap_or(Bson::Null),
    );
    document.insert("OnChangeProperty", value_type.on_change_property.clone());
    document.insert("ParameterIsList", value_type.parameter_list);
    document.insert("PathType", value_type.path_type.clone());
    document.insert("Required", value_type.required);
    document.insert(
        "ReturnType",
        value_type
            .return_type
            .as_ref()
            .map(|value| {
                let mut item = identified("CustomWidgets$WidgetReturnType");
                item.insert("Type", value.kind.clone());
                item.insert("IsList", value.list);
                item.insert("EntityProperty", value.entity_property.clone());
                item.insert("AssignableTo", value.assignable_to.clone());
                Bson::Document(item)
            })
            .unwrap_or(Bson::Null),
    );
    document.insert(
        "SelectableObjectsProperty",
        value_type.selectable_objects_property.clone(),
    );
    document.insert(
        "SelectionTypes",
        marked_strings(&value_type.selection_types, 1),
    );
    document.insert("SetLabel", value_type.set_label);
    document.insert(
        "Translations",
        marked(
            value_type
                .translations
                .iter()
                .map(|value| {
                    let mut item = identified("CustomWidgets$WidgetTranslation");
                    item.insert("LanguageCode", value.language.clone());
                    item.insert("Text", value.text.clone());
                    Bson::Document(item)
                })
                .collect(),
            2,
        ),
    );
    document.insert("Type", value_type.kind.clone());
    document
}

/// Encodes a complete custom widget, including its inline schema.
pub fn encode_widget<E: EmbeddedFormsEncoder>(
    widget: &WidgetNode<E::Node>,
    path: &str,
    encoder: &E,
) -> Result<Document> {
    let (widget_type, context, mut document) = match &widget.storage_baseline {
        Some(baseline) => {
            require_type(baseline, "CustomWidgets$CustomWidget", path)?;
            let type_path = format!("{path}.Type");
            let type_document = get_doc(baseline, "Type", &type_path)?;
            (
                type_document.clone(),
                encode_context_from_document(&widget.widget_type, type_document, &type_path)?,
                baseline.clone(),
            )
        }
        None => {
            let (type_document, context) = encode_widget_type(&widget.widget_type);
            let mut document = identified("CustomWidgets$CustomWidget");
            // See `WidgetNode::extra`'s doc comment: only meaningful on this
            // from-scratch path — a decoded widget's `storage_baseline`
            // already carries these fields (if any) verbatim.
            for (key, value) in &widget.extra {
                document.insert(key, value.clone());
            }
            (type_document, context, document)
        }
    };
    let object = encode_object(
        &widget.object,
        &widget.widget_type.object_type,
        &context,
        &format!("{path}.Object"),
        encoder,
    )?;
    document.insert("Type", widget_type);
    document.insert("Object", object);
    Ok(document)
}

/// Rebuilds mxrb's identity-keyed encode context from an original inline
/// schema. This is the Rust equivalent of `restore_schema_identity!`: it
/// lets a decoded widget retain the baseline's UUIDs, field aliases/order,
/// and outer properties instead of making Studio Pro see a new schema.
fn encode_context_from_document<'schema>(
    widget_type: &'schema WidgetType,
    document: &Document,
    path: &str,
) -> Result<EncodeContext<'schema>> {
    require_type(document, "CustomWidgets$CustomWidgetType", path)?;
    let mut context = EncodeContext::new();
    let object_path = format!("{path}.ObjectType");
    collect_encode_context(
        &widget_type.object_type,
        get_doc(document, "ObjectType", &object_path)?,
        &mut context,
        &object_path,
    )?;
    Ok(context)
}

fn collect_encode_context<'schema>(
    schema: &'schema ObjectType,
    document: &Document,
    context: &mut EncodeContext<'schema>,
    path: &str,
) -> Result<()> {
    let object_id = document.get("$ID").and_then(extract_id).ok_or_else(|| {
        PluggableError::MissingSchemaPointer {
            item: "object type baseline ID",
            path: path.to_string(),
        }
    })?;
    context
        .object_type_ids
        .insert(std::ptr::from_ref(schema), object_id);

    let stored_properties = array_docs(document, "PropertyTypes");
    for property in &schema.properties {
        let Some(stored) = stored_properties.iter().find(|stored| {
            ["PropertyKey", "_Key", "Key"]
                .iter()
                .find_map(|field| stored.get_str(field).ok())
                == Some(property.key.as_str())
        }) else {
            return Err(PluggableError::MissingSchemaPointer {
                item: "property type baseline",
                path: format!("{path}.{}", property.key),
            });
        };
        let property_path = format!("{path}.{}", property.key);
        let property_id = stored.get("$ID").and_then(extract_id).ok_or_else(|| {
            PluggableError::MissingSchemaPointer {
                item: "property type baseline ID",
                path: property_path.clone(),
            }
        })?;
        context
            .property_ids
            .insert(std::ptr::from_ref(property), property_id);
        let value_path = format!("{property_path}.ValueType");
        let value_document = get_doc(stored, "ValueType", &value_path)?;
        let value_id = value_document
            .get("$ID")
            .and_then(extract_id)
            .ok_or_else(|| PluggableError::MissingSchemaPointer {
                item: "value type baseline ID",
                path: value_path.clone(),
            })?;
        context
            .value_type_ids
            .insert(std::ptr::from_ref(&property.value_type), value_id);
        if let Some(nested_schema) = property.value_type.object_type.as_deref() {
            let nested_path = format!("{value_path}.ObjectType");
            collect_encode_context(
                nested_schema,
                get_doc(value_document, "ObjectType", &nested_path)?,
                context,
                &nested_path,
            )?;
        }
    }
    Ok(())
}

/// Encodes one `CustomWidgets$WidgetObject`, resolving its schema pointers
/// through the context returned by [`encode_widget_type`].
pub fn encode_object<'schema, E: EmbeddedFormsEncoder>(
    object: &ObjectNode<E::Node>,
    schema: &'schema ObjectType,
    context: &EncodeContext<'schema>,
    path: &str,
    encoder: &E,
) -> Result<Document> {
    let object_pointer = context
        .object_type_ids
        .get(&std::ptr::from_ref(schema))
        .ok_or_else(|| PluggableError::MissingSchemaPointer {
            item: "object type",
            path: path.to_string(),
        })?;
    let mut properties = Vec::new();
    for assignment in object.assignments() {
        let property = schema.fetch_property(&assignment.property.key)?;
        let property_pointer = context
            .property_ids
            .get(&std::ptr::from_ref(property))
            .ok_or_else(|| PluggableError::MissingSchemaPointer {
                item: "property type",
                path: format!("{path}.{}", property.key),
            })?;
        let value_type_pointer = context
            .value_type_ids
            .get(&std::ptr::from_ref(&property.value_type))
            .ok_or_else(|| PluggableError::MissingSchemaPointer {
                item: "value type",
                path: format!("{path}.{}", property.key),
            })?;
        let value_path = format!("{path}.{}", property.key);
        let mut value = encode_value(
            &assignment.value,
            &property.value_type,
            value_type_pointer,
            context,
            &value_path,
            encoder,
        )?;
        if let Some(source_variable) = &assignment.source_variable {
            value.insert("SourceVariable", source_variable.clone());
        }
        let mut stored = identified("CustomWidgets$WidgetProperty");
        stored.insert("TypePointer", property_pointer.clone());
        stored.insert("Value", value);
        properties.push(Bson::Document(stored));
    }
    let mut document = identified("CustomWidgets$WidgetObject");
    document.insert("TypePointer", object_pointer.clone());
    document.insert("Properties", marked(properties, 2));
    Ok(document)
}

fn encode_value<'schema, E: EmbeddedFormsEncoder>(
    value: &Value<E::Node>,
    value_type: &'schema ValueType,
    type_id: &str,
    context: &EncodeContext<'schema>,
    path: &str,
    encoder: &E,
) -> Result<Document> {
    let mut document = empty_widget_value(type_id);
    if matches!(value, Value::Null) {
        // `Action` is the sole field whose storage baseline is non-null
        // (`Forms$NoAction`); an explicitly absent semantic action must
        // overwrite it or decode would change `Null` into `Action`.
        if value_type.kind == "Action" {
            document.insert("Action", Bson::Null);
        }
        return Ok(document);
    }
    let mismatch = || PluggableError::ValueKindMismatch {
        kind: value_type.kind.clone(),
        path: path.to_string(),
    };
    match value_type.kind.as_str() {
        "Boolean" => match value {
            Value::Boolean(value) => document.insert("PrimitiveValue", value.to_string()),
            _ => return Err(mismatch()),
        },
        "Integer" => match value {
            Value::Integer(value) => document.insert("PrimitiveValue", value.to_string()),
            _ => return Err(mismatch()),
        },
        "Decimal" | "String" | "Enumeration" => match value {
            Value::Primitive(value) => document.insert("PrimitiveValue", value.clone()),
            _ => return Err(mismatch()),
        },
        "Selection" => match value {
            Value::Selection(value) => document.insert("Selection", value.clone()),
            _ => return Err(mismatch()),
        },
        "Expression" => match value {
            Value::Expression(value) => document.insert("Expression", value.clone()),
            _ => return Err(mismatch()),
        },
        "EntityConstraint" => match value {
            Value::XPathConstraint(value) => document.insert("XPathConstraint", value.clone()),
            _ => return Err(mismatch()),
        },
        "Attribute" => match value {
            Value::AttributeReference(value) => document.insert(
                "AttributeRef",
                mxrs_forms_refs::encode_attribute_reference(value),
            ),
            _ => return Err(mismatch()),
        },
        "Entity" => match value {
            Value::EntityReference(value) => {
                document.insert("EntityRef", mxrs_forms_refs::encode_entity_reference(value))
            }
            _ => return Err(mismatch()),
        },
        "TranslatableString" => match value {
            Value::Text(value) => document.insert("TranslatableValue", encode_text(value)),
            _ => return Err(mismatch()),
        },
        "TextTemplate" => match value {
            Value::TextTemplate(value) => document.insert(
                "TextTemplate",
                encode_embedded(value, &format!("{path}.TextTemplate"), encoder)?,
            ),
            _ => return Err(mismatch()),
        },
        "Action" => match value {
            Value::Action(value) => document.insert(
                "Action",
                encode_embedded(value, &format!("{path}.Action"), encoder)?,
            ),
            _ => return Err(mismatch()),
        },
        "Icon" => match value {
            Value::Icon(value) => document.insert(
                "Icon",
                encode_embedded(value, &format!("{path}.Icon"), encoder)?,
            ),
            _ => return Err(mismatch()),
        },
        "DataSource" => {
            encode_data_source(&mut document, value, path, encoder)?;
            None
        }
        "Object" => {
            encode_objects(&mut document, value, value_type, context, path, encoder)?;
            None
        }
        "Widgets" => {
            encode_widgets(&mut document, value, path, encoder)?;
            None
        }
        "System" if matches!(value, Value::System) => None,
        "System" => return Err(mismatch()),
        kind => {
            encode_semantic_reference(&mut document, value, kind, path)?;
            None
        }
    };
    Ok(document)
}

fn empty_widget_value(type_id: &str) -> Document {
    let mut document = identified("CustomWidgets$WidgetValue");
    let mut no_action = identified("Forms$NoAction");
    no_action.insert("DisabledDuringExecution", true);
    document.insert("Action", no_action);
    document.insert("AttributeRef", Bson::Null);
    document.insert("DataSource", Bson::Null);
    document.insert("EntityRef", Bson::Null);
    document.insert("Expression", "");
    document.insert("Form", "");
    document.insert("Icon", Bson::Null);
    document.insert("Image", "");
    document.insert("Microflow", "");
    document.insert("Nanoflow", "");
    document.insert("Objects", marked(Vec::new(), 2));
    document.insert("PrimitiveValue", "");
    document.insert("Selection", "None");
    document.insert("SourceVariable", Bson::Null);
    document.insert("TextTemplate", Bson::Null);
    document.insert("TranslatableValue", Bson::Null);
    document.insert("TypePointer", type_id);
    document.insert("Widgets", marked(Vec::new(), 2));
    document.insert("XPathConstraint", "");
    document
}

fn encode_objects<'schema, E: EmbeddedFormsEncoder>(
    document: &mut Document,
    value: &Value<E::Node>,
    value_type: &'schema ValueType,
    context: &EncodeContext<'schema>,
    path: &str,
    encoder: &E,
) -> Result<()> {
    let schema =
        value_type
            .object_type
            .as_deref()
            .ok_or_else(|| PluggableError::MissingSchemaPointer {
                item: "nested object type",
                path: path.to_string(),
            })?;
    let objects: Vec<&ObjectNode<E::Node>> = match (value_type.list, value) {
        (true, Value::ObjectList(values)) => values.iter().collect(),
        (false, Value::Object(Some(value))) => vec![value],
        (false, Value::Object(None)) => Vec::new(),
        _ => {
            return Err(PluggableError::ValueKindMismatch {
                kind: value_type.kind.clone(),
                path: path.to_string(),
            });
        }
    };
    let encoded = objects
        .into_iter()
        .enumerate()
        .map(|(index, object)| {
            encode_object(
                object,
                schema,
                context,
                &format!("{path}[{index}]"),
                encoder,
            )
            .map(Bson::Document)
        })
        .collect::<Result<Vec<_>>>()?;
    document.insert("Objects", marked(encoded, 2));
    Ok(())
}

fn encode_widgets<E: EmbeddedFormsEncoder>(
    document: &mut Document,
    value: &Value<E::Node>,
    path: &str,
    encoder: &E,
) -> Result<()> {
    let Value::Widgets(values) = value else {
        return Err(PluggableError::ValueKindMismatch {
            kind: "Widgets".to_string(),
            path: path.to_string(),
        });
    };
    let encoded = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let item_path = format!("{path}.Widgets[{index}]");
            match value {
                WidgetItem::Pluggable(widget) => encode_widget(widget, &item_path, encoder),
                WidgetItem::Native(node) => encode_embedded(node, &item_path, encoder),
            }
            .map(Bson::Document)
        })
        .collect::<Result<Vec<_>>>()?;
    document.insert("Widgets", marked(encoded, 2));
    Ok(())
}

fn encode_data_source<E: EmbeddedFormsEncoder>(
    document: &mut Document,
    value: &Value<E::Node>,
    path: &str,
    encoder: &E,
) -> Result<()> {
    let Value::DataSource(source) = value else {
        return Err(PluggableError::ValueKindMismatch {
            kind: "DataSource".to_string(),
            path: path.to_string(),
        });
    };
    let Some(source) = source else {
        return Ok(());
    };
    match source {
        DataSourceValue::Embedded(node) => {
            document.insert(
                "DataSource",
                encode_embedded(node, &format!("{path}.DataSource"), encoder)?,
            );
        }
        DataSourceValue::XPath(source)
            if source.entity.is_none()
                && source.constraint.is_empty()
                && source.sort_bar.is_none()
                && !source.force_full_objects =>
        {
            if let Some(variable) = &source.source_variable {
                document.insert(
                    "SourceVariable",
                    encode_embedded(variable, &format!("{path}.SourceVariable"), encoder)?,
                );
            }
        }
        DataSourceValue::XPath(source) => {
            let mut data_source = identified("CustomWidgets$CustomWidgetXPathSource");
            data_source.insert(
                "EntityRef",
                source
                    .entity
                    .as_ref()
                    .map(|value| Bson::Document(mxrs_forms_refs::encode_entity_reference(value)))
                    .unwrap_or(Bson::Null),
            );
            data_source.insert("XPathConstraint", source.constraint.clone());
            data_source.insert(
                "SortBar",
                source
                    .sort_bar
                    .as_ref()
                    .map(|node| {
                        encode_embedded(node, &format!("{path}.DataSource.SortBar"), encoder)
                            .map(Bson::Document)
                    })
                    .transpose()?
                    .unwrap_or(Bson::Null),
            );
            data_source.insert(
                "SourceVariable",
                source
                    .source_variable
                    .as_ref()
                    .map(|node| {
                        encode_embedded(node, &format!("{path}.DataSource.SourceVariable"), encoder)
                            .map(Bson::Document)
                    })
                    .transpose()?
                    .unwrap_or(Bson::Null),
            );
            data_source.insert("ForceFullObjects", source.force_full_objects);
            document.insert("DataSource", data_source);
        }
    }
    Ok(())
}

fn encode_semantic_reference<N>(
    document: &mut Document,
    value: &Value<N>,
    kind: &str,
    path: &str,
) -> Result<()> {
    let Value::Reference {
        kind: value_kind,
        target,
    } = value
    else {
        return Err(PluggableError::ValueKindMismatch {
            kind: kind.to_string(),
            path: path.to_string(),
        });
    };
    if value_kind != kind {
        return Err(PluggableError::ValueKindMismatch {
            kind: kind.to_string(),
            path: path.to_string(),
        });
    }
    match (kind, target) {
        ("Association", ReferenceTarget::Entity(value)) => {
            document.insert("EntityRef", mxrs_forms_refs::encode_entity_reference(value));
        }
        ("Association", ReferenceTarget::Attribute(value)) => {
            document.insert(
                "AttributeRef",
                mxrs_forms_refs::encode_attribute_reference(value),
            );
        }
        ("Association", _) => {
            return Err(PluggableError::ValueKindMismatch {
                kind: kind.to_string(),
                path: path.to_string(),
            });
        }
        (_, ReferenceTarget::Path(value)) => {
            let field = if kind == "File" { "Image" } else { kind };
            document.insert(field, value.clone());
        }
        _ => {
            return Err(PluggableError::ValueKindMismatch {
                kind: kind.to_string(),
                path: path.to_string(),
            });
        }
    }
    Ok(())
}

fn encode_text(value: &[(String, String)]) -> Document {
    let items = value
        .iter()
        .map(|(language, text)| {
            let mut item = identified("Texts$Translation");
            item.insert("LanguageCode", language.clone());
            item.insert("Text", text.clone());
            Bson::Document(item)
        })
        .collect();
    let mut document = identified("Texts$Text");
    document.insert("Items", marked(items, 3));
    document
}

fn encode_embedded<E: EmbeddedFormsEncoder>(
    node: &E::Node,
    path: &str,
    encoder: &E,
) -> Result<Document> {
    encoder
        .encode_embedded(node, path)
        .map_err(|source| PluggableError::EmbeddedEncodeFailed {
            path: path.to_string(),
            source: Box::new(source),
        })
}

fn identified(type_name: &str) -> Document {
    let mut document = Document::new();
    document.insert("$ID", uuid::Uuid::new_v4().to_string());
    document.insert("$Type", type_name);
    document
}

fn marked(values: Vec<Bson>, marker: i32) -> Bson {
    Bson::Array(mxrs_bson::build_array(values, marker))
}

fn marked_strings(values: &[String], marker: i32) -> Bson {
    marked(values.iter().cloned().map(Bson::String).collect(), marker)
}

/// Ports mxrb's `WIDGET_VALUE_FIELDS` — every storage field a
/// `CustomWidgets$WidgetValue` document may carry, across every value
/// kind (most are unused for any given kind; that's normal, mxrb's own
/// codec is written the same way).
const WIDGET_VALUE_FIELDS: &[&str] = &[
    "$ID",
    "$Type",
    "Action",
    "AttributeRef",
    "DataSource",
    "EntityRef",
    "Expression",
    "Form",
    "Icon",
    "Image",
    "Microflow",
    "Nanoflow",
    "Objects",
    "PrimitiveValue",
    "Selection",
    "SourceVariable",
    "TextTemplate",
    "TranslatableValue",
    "TypePointer",
    "Widgets",
    "XPathConstraint",
];

/// Decodes a `CustomWidgets$WidgetObject` document's assigned properties
/// into an [`ObjectNode`], resolving each stored `TypePointer` against
/// `context` (as populated by [`decode_widget_type`] for the same widget).
/// Ports the read side of `decode_object`.
///
/// Native Forms values are delegated through [`EmbeddedFormsDecoder`]; no
/// currently-known pluggable value kind is left unhandled here.
pub fn decode_object<D: EmbeddedFormsDecoder>(
    document: &Document,
    context: &SchemaContext,
    path: &str,
    decoder: &D,
) -> Result<ObjectNode<D::Node>> {
    require_type(document, "CustomWidgets$WidgetObject", path)?;
    let mut node = ObjectNode::new();
    for (index, stored) in array_docs(document, "Properties").into_iter().enumerate() {
        let item_path = format!("{path}.Properties[{index}]");
        let property = stored
            .get("TypePointer")
            .and_then(extract_id)
            .as_deref()
            .and_then(|id| context.property_types.get(id))
            .ok_or_else(|| PluggableError::UnresolvedPropertyPointer {
                path: item_path.clone(),
            })?
            .clone();
        let value_doc = get_doc(&stored, "Value", &item_path)?;
        let value_path = format!("{item_path}.{}", property.key);
        let value = decode_value(
            value_doc,
            &property.value_type,
            context,
            &value_path,
            decoder,
        )?;
        let source_variable = if property.value_type.kind != "DataSource" {
            value_doc.get_document("SourceVariable").ok().cloned()
        } else {
            None
        };
        node.push(Assignment {
            property,
            value,
            source_variable,
        });
        assert_known(
            &stored,
            &["$ID", "$Type", "TypePointer", "Value"],
            &item_path,
        )?;
    }
    assert_known(
        document,
        &["$ID", "$Type", "TypePointer", "Properties"],
        path,
    )?;
    Ok(node)
}

/// Ports mxrb's complete `decode_value` dispatch.
fn decode_value<D: EmbeddedFormsDecoder>(
    document: &Document,
    value_type: &ValueType,
    context: &SchemaContext,
    path: &str,
    decoder: &D,
) -> Result<Value<D::Node>> {
    require_type(document, "CustomWidgets$WidgetValue", path)?;
    assert_known(document, WIDGET_VALUE_FIELDS, path)?;
    match value_type.kind.as_str() {
        "Boolean" => Ok(Value::Boolean(
            get_str_or(document, "PrimitiveValue", "false") == "true",
        )),
        "Integer" => {
            let raw = get_str_or(document, "PrimitiveValue", "0");
            raw.parse::<i64>().map(Value::Integer).map_err(|source| {
                PluggableError::InvalidPrimitive {
                    kind: "Integer",
                    value: raw,
                    path: path.to_string(),
                    source,
                }
            })
        }
        "Decimal" => Ok(Value::Primitive(get_str_or(
            document,
            "PrimitiveValue",
            "0",
        ))),
        "String" | "Enumeration" => {
            Ok(Value::Primitive(get_str_or(document, "PrimitiveValue", "")))
        }
        "Selection" => Ok(Value::Selection(get_str_or(document, "Selection", "None"))),
        "Expression" => Ok(Value::Expression(get_str_or(document, "Expression", ""))),
        "EntityConstraint" => Ok(Value::XPathConstraint(get_str_or(
            document,
            "XPathConstraint",
            "",
        ))),
        "TranslatableString" => Ok(Value::Text(decode_translatable(document, path)?)),
        "System" => Ok(Value::System),
        "Object" => decode_object_value(document, value_type, context, path, decoder),
        "Attribute" => decode_attribute_value(document, path),
        "Entity" => decode_entity_value(document, path),
        "Association" => decode_association_value(document, path),
        "DataSource" => decode_data_source_value(document, path, decoder),
        "Widgets" => decode_widgets_value(document, path, decoder),
        "TextTemplate" => decode_embedded_field(document, "TextTemplate", path, decoder)
            .map(|opt| opt.map_or(Value::Null, Value::TextTemplate)),
        "Action" => decode_embedded_field(document, "Action", path, decoder)
            .map(|opt| opt.map_or(Value::Null, Value::Action)),
        "Icon" => decode_embedded_field(document, "Icon", path, decoder)
            .map(|opt| opt.map_or(Value::Null, Value::Icon)),
        other => Ok(decode_reference(other, document)),
    }
}

/// Ports `decode_optional_forms(value) = value && forms_codec.decode_embedded(value)`
/// — `None` when the field is absent/null, `Some` wrapping the embedded
/// element decoded via the injected [`EmbeddedFormsDecoder`] otherwise.
/// Shared by the `TextTemplate`/`Action`/`Icon` kinds, which differ only
/// in which storage field they read and which [`Value`] variant they wrap
/// the result in.
fn decode_embedded_field<D: EmbeddedFormsDecoder>(
    document: &Document,
    field: &str,
    path: &str,
    decoder: &D,
) -> Result<Option<D::Node>> {
    match document.get(field) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::Document(embedded)) => {
            let embedded_path = format!("{path}.{field}");
            decoder
                .decode_embedded(embedded, &embedded_path)
                .map(Some)
                .map_err(|source| PluggableError::EmbeddedDecodeFailed {
                    path: embedded_path,
                    source: Box::new(source),
                })
        }
        Some(_) => Err(PluggableError::ExpectedEmbeddedDocument {
            path: format!("{path}.{field}"),
        }),
    }
}

/// Ports the `when 'Attribute'` arm of mxrb's `decode_value`:
/// `document['AttributeRef'] && forms_codec.decode_attribute_reference_value(...)`.
fn decode_attribute_value<N>(document: &Document, path: &str) -> Result<Value<N>> {
    match document.get("AttributeRef") {
        None | Some(Bson::Null) => Ok(Value::Null),
        Some(raw) => {
            mxrs_forms_refs::decode_attribute_reference(raw, &format!("{path}.AttributeRef"))
                .map(Value::AttributeReference)
                .map_err(PluggableError::from)
        }
    }
}

/// Ports the `when 'Entity'` arm the same way, over `EntityRef`.
fn decode_entity_value<N>(document: &Document, path: &str) -> Result<Value<N>> {
    match document.get("EntityRef") {
        None | Some(Bson::Null) => Ok(Value::Null),
        Some(raw) => mxrs_forms_refs::decode_entity_reference(raw, &format!("{path}.EntityRef"))
            .map(Value::EntityReference)
            .map_err(PluggableError::from),
    }
}

/// Ports the `Association` half of `decode_semantic_reference`: routed
/// through `EntityRef` when present, else through `AttributeRef` (both
/// wrapped as `Pluggable.reference('Association', target)` in mxrb —
/// see [`crate::node::ReferenceTarget`]).
fn decode_association_value<N>(document: &Document, path: &str) -> Result<Value<N>> {
    if let Some(raw) = document
        .get("EntityRef")
        .filter(|v| !matches!(v, Bson::Null))
    {
        let target = mxrs_forms_refs::decode_entity_reference(raw, &format!("{path}.EntityRef"))?;
        return Ok(Value::Reference {
            kind: "Association".to_string(),
            target: crate::node::ReferenceTarget::Entity(target),
        });
    }
    match document
        .get("AttributeRef")
        .filter(|v| !matches!(v, Bson::Null))
    {
        None => Ok(Value::Null),
        Some(raw) => {
            let target =
                mxrs_forms_refs::decode_attribute_reference(raw, &format!("{path}.AttributeRef"))?;
            Ok(Value::Reference {
                kind: "Association".to_string(),
                target: crate::node::ReferenceTarget::Attribute(target),
            })
        }
    }
}

/// Ports `decode_data_source` in full: an XPath/database source is
/// decoded field-by-field (`sort_bar`/`source_variable` now routed
/// through the injected [`EmbeddedFormsDecoder`], same as
/// `TextTemplate`/`Action`/`Icon`); a source of any other `$Type` is
/// decoded whole via the same decoder (mxrb's
/// `forms_codec.decode_embedded(source, ...)`); no source and no
/// `SourceVariable` at all is `None` (mxrb's `nil`).
fn decode_data_source_value<D: EmbeddedFormsDecoder>(
    document: &Document,
    path: &str,
    decoder: &D,
) -> Result<Value<D::Node>> {
    let source = document.get_document("DataSource").ok();
    // Mirrors `document['SourceVariable'] || source&.fetch('SourceVariable', nil)`:
    // the value's own `SourceVariable` wins over the nested source's.
    let source_variable = match document.get("SourceVariable") {
        None | Some(Bson::Null) => match source {
            Some(source) => decode_embedded_field(
                source,
                "SourceVariable",
                &format!("{path}.DataSource"),
                decoder,
            )?,
            None => None,
        },
        Some(_) => decode_embedded_field(document, "SourceVariable", path, decoder)?,
    };

    let Some(source) = source else {
        return Ok(match source_variable {
            Some(variable) => Value::DataSource(Some(DataSourceValue::XPath(DataSource {
                entity: None,
                constraint: String::new(),
                sort_bar: None,
                source_variable: Some(variable),
                force_full_objects: false,
            }))),
            None => Value::DataSource(None),
        });
    };

    let source_path = format!("{path}.DataSource");
    let type_name = source.get_str("$Type").unwrap_or("");
    if !matches!(
        type_name,
        "CustomWidgets$CustomWidgetXPathSource" | "CustomWidgets$CustomWidgetDatabaseSource"
    ) {
        let node = decoder
            .decode_embedded(source, &source_path)
            .map_err(|source| PluggableError::EmbeddedDecodeFailed {
                path: source_path.clone(),
                source: Box::new(source),
            })?;
        return Ok(Value::DataSource(Some(DataSourceValue::Embedded(node))));
    }

    let entity = match source.get("EntityRef") {
        None | Some(Bson::Null) => None,
        Some(raw) => Some(mxrs_forms_refs::decode_entity_reference(
            raw,
            &format!("{source_path}.EntityRef"),
        )?),
    };
    let sort_bar = decode_embedded_field(source, "SortBar", &source_path, decoder)?;
    assert_known(
        source,
        &[
            "$ID",
            "$Type",
            "EntityRef",
            "XPathConstraint",
            "DatabaseConstraints",
            "SortBar",
            "SourceVariable",
            "ForceFullObjects",
        ],
        &source_path,
    )?;
    Ok(Value::DataSource(Some(DataSourceValue::XPath(
        DataSource {
            entity,
            constraint: get_str_fallback(source, "XPathConstraint", "DatabaseConstraints", ""),
            sort_bar,
            source_variable,
            force_full_objects: get_bool_or(source, "ForceFullObjects", false),
        },
    ))))
}

/// Ports `decode_widgets` in full: a nested `CustomWidgets$CustomWidget`
/// item recurses into this crate's own `decode_widget_type`/
/// `decode_object` (self-recursion, no `mxrs-forms` involved); any other
/// item routes through the injected [`EmbeddedFormsDecoder`] (mxrb's
/// `forms_codec.decode_embedded`).
fn decode_widgets_value<D: EmbeddedFormsDecoder>(
    document: &Document,
    path: &str,
    decoder: &D,
) -> Result<Value<D::Node>> {
    let mut items = Vec::new();
    for (index, item) in array_docs(document, "Widgets").into_iter().enumerate() {
        let item_path = format!("{path}.Widgets[{index}]");
        if item.get_str("$Type").ok() != Some("CustomWidgets$CustomWidget") {
            let node = decoder
                .decode_embedded(&item, &item_path)
                .map_err(|source| PluggableError::EmbeddedDecodeFailed {
                    path: item_path.clone(),
                    source: Box::new(source),
                })?;
            items.push(WidgetItem::Native(node));
            continue;
        }
        items.push(WidgetItem::Pluggable(Box::new(decode_widget(
            &item, &item_path, decoder,
        )?)));
    }
    Ok(Value::Widgets(items))
}

/// Ports the `Object`-kind arm of `decode_value` (`decode_objects`):
/// self-contained recursion into nested `ObjectNode`s, no `mxrs-forms`
/// involved.
fn decode_object_value<D: EmbeddedFormsDecoder>(
    document: &Document,
    value_type: &ValueType,
    context: &SchemaContext,
    path: &str,
    decoder: &D,
) -> Result<Value<D::Node>> {
    let mut nodes = Vec::new();
    for (index, item) in array_docs(document, "Objects").into_iter().enumerate() {
        nodes.push(decode_object(
            &item,
            context,
            &format!("{path}[{index}]"),
            decoder,
        )?);
    }
    if value_type.list {
        Ok(Value::ObjectList(nodes))
    } else {
        Ok(Value::Object(nodes.into_iter().next().map(Box::new)))
    }
}

/// Ports `decode_text` (self-contained: `Texts$Text`/`Texts$Translation`
/// are read directly, no `forms_codec` call in mxrb either).
fn decode_translatable(document: &Document, _path: &str) -> Result<Vec<(String, String)>> {
    let Ok(text) = document.get_document("TranslatableValue") else {
        return Ok(Vec::new());
    };
    Ok(array_docs(text, "Items")
        .into_iter()
        .map(|item| {
            (
                get_str_or(&item, "LanguageCode", ""),
                get_str_or(&item, "Text", ""),
            )
        })
        .collect())
}

/// Ports the string-path half of `decode_semantic_reference` —
/// `File`/`Form`/`Image`/`Microflow`/`Nanoflow` (an `Association` is
/// handled by the caller before reaching here: it always needs
/// `forms_codec`, in both of mxrb's own branches for it). A `Hash`-shaped
/// field (mxrb's defensive `value[key] || value["#{key}Path"]` case) is
/// checked before falling back to a plain string.
fn decode_reference<N>(kind: &str, document: &Document) -> Value<N> {
    let field = if kind == "File" { "Image" } else { kind };
    let target = match document.get(field) {
        Some(Bson::String(value)) => value.clone(),
        Some(Bson::Document(nested)) => nested
            .get_str(field)
            .or_else(|_| nested.get_str(format!("{field}Path")))
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    };
    if target.is_empty() {
        Value::Null
    } else {
        Value::Reference {
            kind: kind.to_string(),
            target: crate::node::ReferenceTarget::Path(target),
        }
    }
}

fn require_type(document: &Document, expected: &'static str, path: &str) -> Result<()> {
    if document.get_str("$Type").ok() == Some(expected) {
        Ok(())
    } else {
        Err(PluggableError::UnexpectedType {
            expected,
            path: path.to_string(),
        })
    }
}

fn assert_known(document: &Document, known: &[&str], path: &str) -> Result<()> {
    match document.keys().find(|key| !known.contains(&key.as_str())) {
        Some(unknown) => Err(PluggableError::UnmappedField {
            field: unknown.clone(),
            path: path.to_string(),
        }),
        None => Ok(()),
    }
}

fn get_doc<'a>(document: &'a Document, key: &'static str, path: &str) -> Result<&'a Document> {
    document
        .get_document(key)
        .map_err(|_| PluggableError::MissingField {
            field: key,
            path: path.to_string(),
        })
}

fn get_str_required(document: &Document, key: &'static str, path: &str) -> Result<String> {
    document
        .get_str(key)
        .map(str::to_string)
        .map_err(|_| PluggableError::MissingField {
            field: key,
            path: path.to_string(),
        })
}

fn get_str_or(document: &Document, key: &str, default: &str) -> String {
    document.get_str(key).unwrap_or(default).to_string()
}

fn get_str_fallback(document: &Document, primary: &str, secondary: &str, default: &str) -> String {
    document
        .get_str(primary)
        .or_else(|_| document.get_str(secondary))
        .unwrap_or(default)
        .to_string()
}

fn get_bool_or(document: &Document, key: &str, default: bool) -> bool {
    document.get_bool(key).unwrap_or(default)
}

fn get_bool_fallback(document: &Document, primary: &str, secondary: &str, default: bool) -> bool {
    document
        .get_bool(primary)
        .or_else(|_| document.get_bool(secondary))
        .unwrap_or(default)
}

fn array_docs(document: &Document, key: &str) -> Vec<Document> {
    let raw = document.get_array(key).ok().map(Vec::as_slice);
    parse_array(raw)
        .items
        .into_iter()
        .filter_map(|item| match item {
            Bson::Document(doc) => Some(doc),
            _ => None,
        })
        .collect()
}

fn string_array(document: &Document, key: &str) -> Vec<String> {
    let raw = document.get_array(key).ok().map(Vec::as_slice);
    parse_array(raw)
        .items
        .into_iter()
        .map(|item| match item {
            Bson::String(s) => s,
            other => other.to_string(),
        })
        .collect()
}

fn string_array_fallback(document: &Document, primary: &str, secondary: &str) -> Vec<String> {
    if document.get_array(primary).is_ok() {
        string_array(document, primary)
    } else {
        string_array(document, secondary)
    }
}
