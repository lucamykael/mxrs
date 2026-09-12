//! Decodes the *schema* half of a pluggable widget's BSON — the
//! `CustomWidgets$CustomWidgetType` document a `CustomWidgets$CustomWidget`
//! instance's `Type` field carries inline. Ports the schema-decode
//! functions (`decode_widget_type`/`decode_object_type`/
//! `decode_property_type`/`decode_value_type`, plus their small decode
//! helpers for enumerations/action variables/return types/translations)
//! from `Mxrb::Pluggable::MprCodec` in `lib/mxrb/pluggable/mpr_codec.rb`.
//!
//! This is deliberately a *slice* of that file, not the whole thing.
//! `Mxrb::Pluggable::MprCodec` also decodes/encodes the widget *instance*
//! (`decode_object`/`decode_value`/`encode_*`, `Object`/`Properties` on the
//! `CustomWidgets$CustomWidget` document itself) — that half is **not**
//! ported here. Two reasons, both concrete:
//!
//! 1. It needs the instance-value model (`Mxrb::Pluggable::Node`/
//!    `ObjectNode`), not built yet — `node.rb`'s own API is Ruby
//!    `method_missing`-ergonomics building-block code (same category
//!    `mxrs-forms` already declined to port for its own `Node`), so this
//!    would be a from-scratch Rust design, not a direct port.
//! 2. Several `WidgetValue` kinds (`Attribute`, `Entity`, `TranslatableString`,
//!    `Expression`, `EntityConstraint`, embedded `Icon`/`Action`/`TextTemplate`/
//!    non-custom `Widgets` children) delegate to `Mxrb::Forms::MprCodec`'s
//!    *private* decode helpers in mxrb — `mxrs-forms::MprCodec` only exposes
//!    whole-`Node` `decode`/`encode` today (see its `lib.rs`), not the
//!    individual primitives this would need to call. Wiring this in is a
//!    small, targeted expansion of `mxrs-forms`'s public surface (name the
//!    specific functions needed when picking this up), not a blocker in
//!    the "impossible" sense — just genuinely separate follow-up work.
//!
//! Concretely still open, in dependency order: an instance value/`Node`
//! model here, `mxrs-forms` exposing the primitives above, then
//! `decode_object`/`decode_value`/`encode_object`/`encode_value` here, and
//! finally wiring `mxrs-forms::mpr_codec`'s own
//! `FormsError::PluggableNotSupported` case to delegate here instead of
//! erroring — that last step is what actually unblocks a real Data Grid
//! 2/Gallery/ComboBox page decoding end to end.

use mxrs_bson::{Bson, Document, extract_id, parse_array};

use crate::catalog::{
    ActionVariable, EnumerationValue, ObjectType, PropertyType, ReturnType, Translation, ValueType,
    WidgetType,
};
use crate::error::{PluggableError, Result};

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
