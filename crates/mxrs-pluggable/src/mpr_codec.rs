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
//! `File`/`Form`/`Image`/`Microflow`/`Nanoflow` string-path references).
//! See [`node`](crate::node) for the decoded shape.
//!
//! Still open, correctly erroring rather than guessing
//! (`PluggableError::NeedsFormsIntegration`): `Attribute`, `Entity`,
//! `Association` (both of mxrb's own branches for it need `forms_codec`),
//! `TextTemplate`, `Action`, `Icon`, `DataSource`, and `Widgets` — every
//! one of these delegates to `Mxrb::Forms::MprCodec`'s *private* decode
//! helpers in mxrb (`decode_attribute_reference_value`/
//! `decode_entity_reference_value`/`decode_embedded`), which
//! `mxrs-forms::MprCodec` doesn't expose yet (it only exposes whole-`Node`
//! `decode`/`encode` today — see its `lib.rs`). Real-world weight of this
//! gap, measured (not guessed) by `tests/instance_oracle.rs` against
//! every real `CustomWidgets$CustomWidget` instance in QRQC/SPC: only
//! 10/378 (QRQC) and 11/682 (SPC) decode *fully* end-to-end today — the
//! rest each hit one of these kinds first (`TextTemplate`, `DataSource`,
//! and `Attribute` dominate). What's landed here is real, independently
//! useful, zero-panic progress (every self-contained property decodes
//! correctly, and every blocked instance fails with this exact named
//! error, never a guess or a silent drop), not the finish line.
//!
//! `encode_object`/`encode_value` (the writer-side counterpart) are also
//! not ported yet — decode was the priority (it's what
//! `mxrs-compiler-widgets` needs to read real pages), encode is symmetric
//! follow-up work once decode's remaining gaps close.
//!
//! Wiring `mxrs-forms::mpr_codec`'s own `FormsError::PluggableNotSupported`
//! case to delegate here is deferred until decode covers enough real
//! instances end-to-end to be worth it — today it would just trade one
//! named error for another on nearly every real widget.

use mxrs_bson::{Bson, Document, extract_id, parse_array};

use crate::catalog::{
    ActionVariable, EnumerationValue, ObjectType, PropertyType, ReturnType, Translation, ValueType,
    WidgetType,
};
use crate::error::{PluggableError, Result};
use crate::node::{Assignment, ObjectNode, Value};

/// Decoded alongside a widget type: storage-id -> property/object-type,
/// resolved by identity while decoding a single `CustomWidgets$CustomWidget`
/// instance's `Properties` against `TypePointer`s (mxrb's `context` hash).
/// Left for the instance-decode follow-up to populate/consume — the schema
/// decode below still threads it through (matching mxrb's own
/// `decode_widget_type` return shape) so that follow-up doesn't need to
/// touch this module's signatures.
#[derive(Debug, Default)]
pub struct SchemaContext {
    pub property_types: std::collections::HashMap<String, PropertyType>,
    pub object_types: std::collections::HashMap<String, ObjectType>,
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
/// Stops at the first property whose value kind isn't self-contained yet
/// — see this module's doc for exactly which kinds those are and why —
/// returning [`PluggableError::NeedsFormsIntegration`] rather than
/// guessing or dropping data, same as every other unported gap in this
/// workspace.
pub fn decode_object(
    document: &Document,
    context: &SchemaContext,
    path: &str,
) -> Result<ObjectNode> {
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
        let value = decode_value(value_doc, &property.value_type, context, &value_path)?;
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

/// Ports the self-contained slice of `decode_value` — see this module's
/// doc for which kinds are covered and which return
/// [`PluggableError::NeedsFormsIntegration`] instead.
fn decode_value(
    document: &Document,
    value_type: &ValueType,
    context: &SchemaContext,
    path: &str,
) -> Result<Value> {
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
        "Object" => decode_object_value(document, value_type, context, path),
        "Attribute" | "Entity" | "TextTemplate" | "Action" | "Icon" | "DataSource" | "Widgets"
        | "Association" => Err(PluggableError::NeedsFormsIntegration {
            kind: value_type.kind.clone(),
            path: path.to_string(),
        }),
        other => Ok(decode_reference(other, document)),
    }
}

/// Ports the `Object`-kind arm of `decode_value` (`decode_objects`):
/// self-contained recursion into nested `ObjectNode`s, no `mxrs-forms`
/// involved.
fn decode_object_value(
    document: &Document,
    value_type: &ValueType,
    context: &SchemaContext,
    path: &str,
) -> Result<Value> {
    let mut nodes = Vec::new();
    for (index, item) in array_docs(document, "Objects").into_iter().enumerate() {
        nodes.push(decode_object(&item, context, &format!("{path}[{index}]"))?);
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
fn decode_reference(kind: &str, document: &Document) -> Value {
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
            target,
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
