//! Schema-driven bridge for pre-pluggable (Dojo) custom widgets.

use std::{collections::HashMap, fs, path::Path};

use mxrs_bson::{Bson, Document, extract_id, parse_array};
use serde_json::{Map, Number, Value};

const SUPPORTED_TYPES: &[&str] = &[
    "Attribute",
    "Boolean",
    "Decimal",
    "Entity",
    "EntityConstraint",
    "Enumeration",
    "Form",
    "Image",
    "Integer",
    "Microflow",
    "Object",
    "String",
    "System",
    "TranslatableString",
];

pub struct LegacyWidgetBundleCompiler<'a> {
    documents: &'a [(String, Document)],
    project_path: &'a Path,
    widget: &'a Document,
    widget_id: String,
    schema: HashMap<String, Document>,
}

impl<'a> LegacyWidgetBundleCompiler<'a> {
    pub fn new(
        documents: &'a [(String, Document)],
        project_path: &'a Path,
        widget: &'a Document,
    ) -> Self {
        let widget_type = widget.get_document("Type").ok();
        let widget_id = widget_type
            .and_then(|type_| type_.get_str("WidgetId").ok())
            .unwrap_or_default()
            .to_string();
        let mut schema = HashMap::new();
        if let Some(widget_type) = widget_type {
            index_documents(widget_type, &mut schema);
        }
        Self {
            documents,
            project_path,
            widget,
            widget_id,
            schema,
        }
    }

    pub fn supported(&self) -> bool {
        !self.widget_id.is_empty()
            && self.module_available()
            && self
                .properties_map(self.widget.get_document("Object").ok())
                .is_some()
    }

    pub fn widget_id(&self) -> &str {
        &self.widget_id
    }

    pub fn properties(&self) -> Option<Value> {
        self.properties_map(self.widget.get_document("Object").ok())
            .map(Value::Object)
    }

    fn properties_map(&self, object: Option<&Document>) -> Option<Map<String, Value>> {
        let object = object?;
        array_docs(object, "Properties")
            .into_iter()
            .map(|property| {
                let schema = self
                    .schema
                    .get(&property.get("TypePointer").and_then(extract_id)?)?;
                let key = schema.get_str("PropertyKey").ok()?.to_string();
                let kind = schema
                    .get_document("ValueType")
                    .ok()?
                    .get_str("Type")
                    .ok()?;
                if !SUPPORTED_TYPES.contains(&kind) {
                    return None;
                }
                let value = property.get_document("Value").ok()?;
                Some((key, self.compile_value(kind, value)?))
            })
            .collect()
    }

    fn compile_value(&self, kind: &str, value: &Document) -> Option<Value> {
        let primitive = || value.get_str("PrimitiveValue").unwrap_or_default();
        match kind {
            "Boolean" => Some(Value::Bool(primitive() == "true")),
            "Integer" => Some(Value::Number(
                primitive().parse::<i64>().unwrap_or_default().into(),
            )),
            "Decimal" => {
                Number::from_f64(primitive().parse::<f64>().unwrap_or_default()).map(Value::Number)
            }
            "Enumeration" | "String" | "System" => Some(Value::String(primitive().to_string())),
            "TranslatableString" => Some(Value::String(translated(
                value.get_document("TranslatableValue").ok(),
            ))),
            "Entity" => Some(Value::String(
                value.get_str("EntityPath").unwrap_or_default().to_string(),
            )),
            "EntityConstraint" => Some(Value::String(
                value
                    .get_str("XPathConstraint")
                    .unwrap_or_default()
                    .to_string(),
            )),
            "Attribute" => {
                let path = value
                    .get_str("AttributePath")
                    .ok()
                    .or_else(|| {
                        value
                            .get_document("AttributeRef")
                            .ok()
                            .and_then(|reference| reference.get_str("Attribute").ok())
                    })
                    .unwrap_or_default();
                Some(Value::String(
                    path.rsplit('.').next().unwrap_or_default().to_string(),
                ))
            }
            "Microflow" => Some(Value::String(
                value.get_str("Microflow").unwrap_or_default().to_string(),
            )),
            "Form" => Some(Value::String(
                value.get_str("Form").unwrap_or_default().to_string(),
            )),
            "Image" => {
                let reference = value.get_str("Image").unwrap_or_default();
                if reference.is_empty() {
                    Some(Value::String(String::new()))
                } else {
                    self.image_uri(reference).map(Value::String)
                }
            }
            "Object" => array_docs(value, "Objects")
                .into_iter()
                .map(|object| self.properties_map(Some(&object)).map(Value::Object))
                .collect::<Option<Vec<_>>>()
                .map(Value::Array),
            _ => None,
        }
    }

    fn image_uri(&self, reference: &str) -> Option<String> {
        let mut parts = reference.splitn(3, '.');
        let module = parts.next()?;
        let collection = parts.next()?;
        let name = parts.next()?;
        if module == "System" && collection == "Images" {
            let extension = if matches!(name, "Error" | "Running" | "Completed" | "Module") {
                "gif"
            } else {
                "png"
            };
            return Some(format!("img/System${name}.{extension}"));
        }
        let image = self.documents.iter().find_map(|(owner, document)| {
            (owner == module
                && document.get_str("$Type").ok() == Some("Images$ImageCollection")
                && document.get_str("Name").ok() == Some(collection))
            .then(|| {
                array_docs(document, "Images")
                    .into_iter()
                    .find(|image| image.get_str("Name").ok() == Some(name))
            })
            .flatten()
        })?;
        let extension = crate::image_format::image_format(&image).ok()?;
        Some(format!("img/{module}${name}.{extension}"))
    }

    fn module_available(&self) -> bool {
        let root = self.project_path.parent().unwrap_or_else(|| Path::new("."));
        let entry = format!("{}.js", self.widget_id.replace('.', "/"));
        if root.join("widgets").join(&entry).is_file() {
            return true;
        }
        fs::read_dir(root.join("widgets")).is_ok_and(|entries| {
            entries.filter_map(Result::ok).any(|candidate| {
                let path = candidate.path();
                path.extension().is_some_and(|extension| extension == "mpk")
                    && fs::read(path).is_ok_and(|bytes| {
                        bytes
                            .windows(entry.len())
                            .any(|window| window == entry.as_bytes())
                    })
            })
        })
    }
}

fn index_documents(document: &Document, index: &mut HashMap<String, Document>) {
    if let Some(id) = document.get("$ID").and_then(extract_id) {
        index.insert(id, document.clone());
    }
    for value in document.values() {
        match value {
            Bson::Document(document) => index_documents(document, index),
            Bson::Array(values) => values.iter().for_each(|value| {
                if let Bson::Document(document) = value {
                    index_documents(document, index);
                }
            }),
            _ => {}
        }
    }
}

fn array_docs(document: &Document, field: &str) -> Vec<Document> {
    parse_array(document.get_array(field).ok().map(Vec::as_slice))
        .items
        .into_iter()
        .filter_map(|value| value.as_document().cloned())
        .collect()
}

fn translated(text: Option<&Document>) -> String {
    let items = text
        .map(|text| array_docs(text, "Items"))
        .unwrap_or_default();
    items
        .iter()
        .find(|item| item.get_str("LanguageCode").ok() == Some("en_US"))
        .or_else(|| items.first())
        .and_then(|item| item.get_str("Text").ok())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use mxrs_bson::doc;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn compiles_schema_described_dojo_properties_when_the_module_exists() {
        let temp = tempdir().unwrap();
        let module = temp.path().join("widgets/Legacy/widget");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("Sample.js"), "define([], function () {});").unwrap();
        let widget = doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Type": {
                "WidgetId": "Legacy.widget.Sample",
                "ObjectType": {
                    "$ID": "object-type",
                    "PropertyTypes": mxrs_bson::build_array(vec![
                        Bson::Document(doc! { "$ID": "enabled", "PropertyKey": "enabled", "ValueType": { "Type": "Boolean" } }),
                        Bson::Document(doc! { "$ID": "caption", "PropertyKey": "caption", "ValueType": { "Type": "TranslatableString" } }),
                    ], 2),
                },
            },
            "Object": {
                "TypePointer": "object-type",
                "Properties": mxrs_bson::build_array(vec![
                    Bson::Document(doc! { "TypePointer": "enabled", "Value": { "PrimitiveValue": "true" } }),
                    Bson::Document(doc! { "TypePointer": "caption", "Value": { "TranslatableValue": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "LanguageCode": "en_US", "Text": "Hello",
                    })], 3) } } }),
                ], 2),
            },
        };
        let project = temp.path().join("App.mpr");
        let compiler = LegacyWidgetBundleCompiler::new(&[], &project, &widget);

        assert!(compiler.supported());
        assert_eq!(
            compiler.properties().unwrap(),
            serde_json::json!({ "enabled": true, "caption": "Hello" })
        );
    }
}
