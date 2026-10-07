//! The documents an installed widget package stands for: the inline
//! `CustomWidgets$CustomWidgetType` a widget stores, the object that
//! configures nothing yet, and the value each property holds before it is
//! configured.
//!
//! These are the shapes `Mxrb::WidgetPackage.template` builds, kept field
//! for field, because the migration compares them with what a model stores:
//! a widget whose schema already is the package's must compare equal, and
//! a migrated one must store what mxrb would. `mxrs_pluggable`'s encoder
//! writes the complete Studio Pro schema (its `Prompt` fields included) and
//! is the writer's; this is the migration's.

use mxrs_bson::{Bson, Document, doc};
use mxrs_pluggable::{ObjectType, PropertyType, ValueType, WidgetType};

use super::tree::{self, array};

fn new_id() -> Bson {
    Bson::String(uuid::Uuid::new_v4().to_string())
}

/// The schema and the unconfigured object of the widget `definition`
/// declares, freshly identified.
pub(super) fn template(definition: &WidgetType) -> (Document, Document) {
    let object_type_id = new_id();
    let mut property_types = Vec::new();
    let mut properties = Vec::new();
    for property in &definition.object_type.properties {
        let (property_type, value) = property_pair(property);
        property_types.push(Bson::Document(property_type));
        properties.extend(value.map(Bson::Document));
    }
    let widget_type = doc! {
        "$ID": new_id(),
        "$Type": "CustomWidgets$CustomWidgetType",
        "HelpUrl": &definition.help_url,
        "OfflineCapable": definition.offline,
        "StudioCategory": &definition.studio_category,
        "StudioProCategory": &definition.studio_pro_category,
        "SupportedPlatform": &definition.platform,
        "WidgetDescription": &definition.description,
        "WidgetId": &definition.id,
        "WidgetName": &definition.name,
        "WidgetNeedsEntityContext": definition.needs_context,
        "WidgetPluginWidget": definition.plugin,
        // Studio Pro fingerprints the embedded definition by member order as
        // well as value, and writes ObjectType last.
        "ObjectType": {
            "$ID": object_type_id.clone(),
            "$Type": "CustomWidgets$WidgetObjectType",
            "PropertyTypes": array(property_types, 2),
        },
    };
    let object = doc! {
        "$ID": new_id(),
        "$Type": "CustomWidgets$WidgetObject",
        "TypePointer": object_type_id,
        "Properties": array(properties, 2),
    };
    (widget_type, object)
}

/// A property's type and, unless it is a system property, its value.
fn property_pair(property: &PropertyType) -> (Document, Option<Document>) {
    let property_type_id = new_id();
    let value_type = value_type(&property.value_type);
    let value = (!property.is_system()).then(|| {
        doc! {
            "$ID": new_id(),
            "$Type": "CustomWidgets$WidgetProperty",
            "TypePointer": property_type_id.clone(),
            "Value": widget_value(
                value_type.get("$ID").cloned().unwrap_or(Bson::Null),
                &ValueSpec::declared(&property.value_type),
            ),
        }
    });
    let property_type = doc! {
        "$ID": property_type_id,
        "$Type": "CustomWidgets$WidgetPropertyType",
        "Caption": &property.caption,
        "Category": &property.category,
        "Description": &property.description,
        "IsDefault": property.default,
        "PropertyKey": &property.key,
        "ValueType": value_type,
    };
    (property_type, value)
}

fn value_type(value_type: &ValueType) -> Document {
    let strings = |values: &[String]| array(values.iter().cloned().map(Bson::String).collect(), 1);
    doc! {
        "$ID": new_id(),
        "$Type": "CustomWidgets$WidgetValueType",
        "ActionVariables": array(
            value_type
                .action_variables
                .iter()
                .map(|variable| {
                    Bson::Document(doc! {
                        "$ID": new_id(),
                        "$Type": "CustomWidgets$WidgetActionVariable",
                        "Caption": &variable.caption,
                        "Key": &variable.key,
                        "Type": &variable.kind,
                    })
                })
                .collect(),
            2,
        ),
        "AllowedTypes": strings(&value_type.attribute_types),
        "AllowNonPersistableEntities": value_type.allow_non_persistable_entities,
        "AllowUpload": value_type.allow_upload,
        "AssociationTypes": strings(&value_type.association_types),
        "DataSourceProperty": &value_type.data_source_property,
        "DefaultType": &value_type.default_type,
        "DefaultValue": &value_type.default_value,
        "EntityProperty": &value_type.entity_property,
        "EnumerationValues": array(
            value_type
                .enumeration_values
                .iter()
                .map(|value| {
                    Bson::Document(doc! {
                        "$ID": new_id(),
                        "$Type": "CustomWidgets$WidgetEnumerationValue",
                        "_Key": &value.key,
                        "Caption": &value.caption,
                    })
                })
                .collect(),
            2,
        ),
        "IsLinked": value_type.linked,
        "IsList": value_type.list,
        "IsMetaData": value_type.metadata,
        "IsPath": &value_type.path_kind,
        "Multiline": value_type.multiline,
        "ObjectType": value_type
            .object_type
            .as_deref()
            .map_or(Bson::Null, |object_type| Bson::Document(object_type_document(object_type))),
        "OnChangeProperty": &value_type.on_change_property,
        "ParameterIsList": value_type.parameter_list,
        "PathType": &value_type.path_type,
        "Required": value_type.required,
        "ReturnType": value_type.return_type.as_ref().map_or(Bson::Null, |return_type| {
            Bson::Document(doc! {
                "$ID": new_id(),
                "$Type": "CustomWidgets$WidgetReturnType",
                "AssignableTo": &return_type.assignable_to,
                "EntityProperty": &return_type.entity_property,
                "IsList": return_type.list,
                "Type": &return_type.kind,
            })
        }),
        "SelectableObjectsProperty": &value_type.selectable_objects_property,
        "SelectionTypes": strings(&value_type.selection_types),
        "SetLabel": value_type.set_label,
        "Translations": array(
            value_type
                .translations
                .iter()
                .map(|translation| {
                    Bson::Document(doc! {
                        "$ID": new_id(),
                        "$Type": "CustomWidgets$WidgetTranslation",
                        "LanguageCode": &translation.language,
                        "Text": &translation.text,
                    })
                })
                .collect(),
            2,
        ),
        "Type": &value_type.kind,
    }
}

fn object_type_document(object_type: &ObjectType) -> Document {
    doc! {
        "$ID": new_id(),
        "$Type": "CustomWidgets$WidgetObjectType",
        "PropertyTypes": array(
            object_type
                .properties
                .iter()
                .map(|property| Bson::Document(property_pair(property).0))
                .collect(),
            2,
        ),
    }
}

/// What decides the value a property holds before it is configured.
pub(super) struct ValueSpec {
    kind: Option<String>,
    default: String,
    translations: Vec<(String, String)>,
    data_source: String,
    selection_types: Vec<Bson>,
}

impl ValueSpec {
    /// The spec a package's property declares.
    fn declared(value_type: &ValueType) -> Self {
        Self {
            kind: Some(value_type.kind.clone()),
            default: value_type.default_value.clone(),
            translations: value_type
                .translations
                .iter()
                .map(|translation| (translation.language.clone(), translation.text.clone()))
                .collect(),
            data_source: value_type.data_source_property.clone(),
            selection_types: value_type
                .selection_types
                .iter()
                .cloned()
                .map(Bson::String)
                .collect(),
        }
    }

    /// The spec a stored `CustomWidgets$WidgetValueType` declares; its
    /// translations only when `with_translations`.
    fn stored(value_type: &Document, with_translations: bool) -> Self {
        let translations = if with_translations {
            tree::items(value_type.get("Translations"))
                .iter()
                .map(|translation| {
                    (
                        tree::to_text(tree::field(translation, "LanguageCode")),
                        tree::to_text(tree::field(translation, "Text")),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            kind: value_type.get_str("Type").ok().map(str::to_string),
            default: tree::to_text(value_type.get("DefaultValue")),
            translations,
            data_source: tree::to_text(value_type.get("DataSourceProperty")),
            selection_types: tree::items(value_type.get("SelectionTypes")).to_vec(),
        }
    }
}

/// The value a stored value type holds before it is configured.
pub(super) fn default_value(value_type: &Document) -> Document {
    widget_value(
        value_type.get("$ID").cloned().unwrap_or(Bson::Null),
        &ValueSpec::stored(value_type, true),
    )
}

/// The object a nested object type holds before it is configured — the
/// object a configured nested object is rebound onto. Unlike a top-level
/// value, its texts start without the schema's translations.
pub(super) fn object_template(object_type: &Document) -> Document {
    let properties = tree::items(object_type.get("PropertyTypes"))
        .iter()
        .filter(|property_type| {
            !tree::is_str(
                tree::dig(Some(property_type), &["ValueType", "Type"]),
                "System",
            )
        })
        .map(|property_type| {
            let value_type = tree::document(tree::field(property_type, "ValueType"))
                .cloned()
                .unwrap_or_default();
            Bson::Document(doc! {
                "$ID": new_id(),
                "$Type": "CustomWidgets$WidgetProperty",
                "TypePointer": tree::field(property_type, "$ID").cloned().unwrap_or(Bson::Null),
                "Value": widget_value(
                    value_type.get("$ID").cloned().unwrap_or(Bson::Null),
                    &ValueSpec::stored(&value_type, false),
                ),
            })
        })
        .collect();
    doc! {
        "$ID": new_id(),
        "$Type": "CustomWidgets$WidgetObject",
        "Properties": array(properties, 2),
        "TypePointer": object_type.get("$ID").cloned().unwrap_or(Bson::Null),
    }
}

/// A `CustomWidgets$WidgetValue` of the value type `type_pointer`
/// identifies, holding what `spec` says it holds unconfigured.
fn widget_value(type_pointer: Bson, spec: &ValueSpec) -> Document {
    let mut value = doc! {
        "$ID": new_id(),
        "$Type": "CustomWidgets$WidgetValue",
        "Action": {
            "$ID": new_id(),
            "$Type": "Forms$NoAction",
            "DisabledDuringExecution": true,
        },
        "AttributeRef": Bson::Null,
        "DataSource": Bson::Null,
        "EntityRef": Bson::Null,
        "Expression": "",
        "Form": "",
        "Icon": Bson::Null,
        "Image": "",
        "Microflow": "",
        "Nanoflow": "",
        "Objects": array(Vec::new(), 2),
        "PrimitiveValue": "",
        "Selection": "None",
        "SourceVariable": Bson::Null,
        "TextTemplate": Bson::Null,
        "TranslatableValue": Bson::Null,
        "TypePointer": type_pointer,
        "Widgets": array(Vec::new(), 2),
        "XPathConstraint": "",
    };
    let or = |fallback: &str| {
        if spec.default.is_empty() {
            fallback.to_string()
        } else {
            spec.default.clone()
        }
    };
    match spec.kind.as_deref() {
        Some("Boolean") => {
            value.insert("PrimitiveValue", or("false"));
        }
        Some("Integer") => {
            value.insert("PrimitiveValue", or("0"));
        }
        Some("Enumeration" | "String" | "Decimal") => {
            value.insert("PrimitiveValue", spec.default.clone());
        }
        Some("Expression") => {
            value.insert("Expression", spec.default.clone());
        }
        Some("Selection") => {
            let selection = if spec.default.is_empty() {
                spec.selection_types
                    .first()
                    .filter(|first| tree::truthy(Some(first)))
                    .cloned()
                    .unwrap_or_else(|| Bson::String("None".to_string()))
            } else {
                Bson::String(spec.default.clone())
            };
            value.insert("Selection", selection);
        }
        Some("TextTemplate") if spec.data_source.is_empty() => {
            value.insert("TextTemplate", client_template(&spec.translations));
        }
        _ => {}
    }
    value
}

fn client_template(translations: &[(String, String)]) -> Document {
    let items = translations
        .iter()
        .map(|(language, text)| {
            Bson::Document(doc! {
                "$ID": new_id(),
                "$Type": "Texts$Translation",
                "LanguageCode": language,
                "Text": text,
            })
        })
        .collect();
    doc! {
        "$ID": new_id(),
        "$Type": "Forms$ClientTemplate",
        "Fallback": {
            "$ID": new_id(),
            "$Type": "Texts$Text",
            "Items": array(Vec::new(), 3),
        },
        "Parameters": array(Vec::new(), 2),
        "Template": {
            "$ID": new_id(),
            "$Type": "Texts$Text",
            "Items": array(items, 3),
        },
    }
}
