//! JavaScript property bundle for the official React Gallery widget.
//!
//! This is the first independent modern-widget bundle from mxrs Onda 3.
//! It covers XPath, microflow, and nanoflow list sources, page-parameter
//! XPath arguments, selection, primitive options, translated captions,
//! templated content, and filter placeholders.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document, extract_id, parse_array};
use mxrs_compiler_support::{operation_id, widget_data_source_id};

use crate::WebListDataSource;
use crate::web_operation::{object_page_parameters, xpath_variables};

pub const GALLERY_WIDGET_ID: &str = "com.mendix.widget.web.gallery.Gallery";

#[derive(Debug, Clone)]
struct PropertyValue {
    key: String,
    kind: String,
    value: Document,
}

/// Compiles one raw `CustomWidgets$CustomWidget` Gallery instance into the
/// React property expression consumed by the Mendix web client.
pub struct GalleryBundleCompiler<'a> {
    page_name: &'a str,
    widget: &'a Document,
    properties: Vec<PropertyValue>,
    data_source: WebListDataSource,
    xpath_arguments: Option<Vec<String>>,
}

impl<'a> GalleryBundleCompiler<'a> {
    pub fn new(
        documents: &'a [(String, Document)],
        page_name: &'a str,
        widget: &'a Document,
    ) -> Self {
        let properties = property_values(widget);
        let data_source = WebListDataSource::from_documents(documents, widget);
        let xpath_arguments = resolve_xpath_arguments(documents, page_name, &data_source);
        Self {
            page_name,
            widget,
            properties,
            data_source,
            xpath_arguments,
        }
    }

    pub fn supported(&self) -> bool {
        widget_id(self.widget).as_deref() == Some(GALLERY_WIDGET_ID)
            && self.data_source.supported()
            && !self.data_source.entity.is_empty()
            && self.xpath_arguments.is_some()
    }

    pub fn entity_name(&self) -> &str {
        &self.data_source.entity
    }

    pub fn content_widgets(&self) -> Vec<Document> {
        self.value("content")
            .map(|value| array_docs(value, "Widgets"))
            .unwrap_or_default()
    }

    pub fn widget_key(&self) -> String {
        format!(
            "p.{}.{}",
            self.page_name,
            self.widget.get_str("Name").unwrap_or_default()
        )
    }

    pub fn data_source_id(&self) -> String {
        widget_data_source_id(&self.widget_key())
    }

    /// `content` and `rendered_filters` are already-rendered JavaScript
    /// arrays supplied by the surrounding page renderer.
    pub fn render(
        &self,
        content: &str,
        nanoflow_reference: Option<&str>,
        rendered_filters: Option<&str>,
    ) -> String {
        let mut properties = self.primitive_properties();
        let key = self.widget_key();
        let data_source_id = self.data_source_id();
        set(&mut properties, "key", js_string(&key));
        set(&mut properties, "$widgetId", js_string(&key));
        set(
            &mut properties,
            "datasource",
            self.data_source_js(nanoflow_reference),
        );
        set(
            &mut properties,
            "content",
            format!(
                "TemplatedWidgetProperty({{ children: () => {content}, dataSourceId: {}, editable: false }})",
                js_string(&data_source_id)
            ),
        );
        set(
            &mut properties,
            "filtersPlaceholder",
            self.filters_placeholder(rendered_filters),
        );
        let selection = self
            .value("itemSelection")
            .and_then(|value| value.get_str("Selection").ok())
            .unwrap_or("None");
        set(
            &mut properties,
            "itemSelection",
            format!(
                "SelectionProperty({})",
                js_object(&[
                    ("selectionType", js_string(selection)),
                    ("dataSourceId", js_string(&data_source_id)),
                ])
            ),
        );
        set(&mut properties, "class", js_string(&css_class(self.widget)));
        format!(
            "React.createElement($Gallery, {})",
            js_object_owned(&properties)
        )
    }

    fn value(&self, key: &str) -> Option<&Document> {
        self.properties
            .iter()
            .find(|property| property.key == key)
            .map(|property| &property.value)
    }

    fn primitive_properties(&self) -> Vec<(String, String)> {
        self.properties
            .iter()
            .filter_map(|property| {
                let value = match property.kind.as_str() {
                    "Boolean" => Some(
                        (property.value.get_str("PrimitiveValue").unwrap_or_default() == "true")
                            .to_string(),
                    ),
                    "Integer" => Some(
                        property
                            .value
                            .get_str("PrimitiveValue")
                            .unwrap_or_default()
                            .parse::<i64>()
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    "Enumeration" => Some(js_string(
                        property.value.get_str("PrimitiveValue").unwrap_or_default(),
                    )),
                    "TextTemplate" => Some(format!(
                        "ExpressionProperty({})",
                        js_object(&[(
                            "expression",
                            js_object(&[
                                (
                                    "expr",
                                    js_object(&[
                                        ("type", js_string("literal")),
                                        (
                                            "value",
                                            js_string(&translated_text(
                                                property.value.get_document("TextTemplate").ok(),
                                            )),
                                        ),
                                    ]),
                                ),
                                ("args", "{  }".to_string())
                            ]),
                        )])
                    )),
                    "Widgets" if array_docs(&property.value, "Widgets").is_empty() => {
                        Some("[]".to_string())
                    }
                    "Object" if array_docs(&property.value, "Objects").is_empty() => {
                        Some("[]".to_string())
                    }
                    _ => None,
                }?;
                Some((property.key.clone(), value))
            })
            .collect()
    }

    fn data_source_js(&self, nanoflow_reference: Option<&str>) -> String {
        let data_source_id = self.data_source_id();
        let operation = operation_id(
            self.page_name,
            self.widget.get_str("Name").unwrap_or_default(),
        );
        if self.data_source.xpath() {
            let mut config = vec![
                ("dataSourceId".to_string(), js_string(&data_source_id)),
                ("operationId".to_string(), js_string(&operation)),
                ("entity".to_string(), js_string(&self.data_source.entity)),
                ("sort".to_string(), "[]".to_string()),
            ];
            if let Some(arguments) = &self.xpath_arguments
                && !arguments.is_empty()
            {
                let args: Vec<(String, String)> = arguments
                    .iter()
                    .map(|name| {
                        (
                            name.clone(),
                            format!("[{}, undefined, false]", js_string(&format!("${name}"))),
                        )
                    })
                    .collect();
                config.push(("arguments".to_string(), js_object_owned(&args)));
                config.push(("fetchOnlyWithAllParams".to_string(), "true".to_string()));
            }
            return format!("DatabaseObjectListProperty({})", js_object_owned(&config));
        }
        if self.data_source.microflow() {
            return format!(
                "MicroflowObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&data_source_id)),
                    ("operationId", js_string(&operation)),
                    ("argMap", "{  }".to_string()),
                    ("fetchOnlyWithAllParams", "false".to_string()),
                ])
            );
        }
        format!(
            "NanoflowObjectListProperty({})",
            js_object(&[
                ("dataSourceId", js_string(&data_source_id)),
                (
                    "source",
                    js_object(&[(
                        "nanoflow",
                        nanoflow_reference.unwrap_or("undefined").to_string(),
                    )]),
                ),
                ("argMap", "{  }".to_string()),
                ("fetchOnlyWithAllParams", "false".to_string()),
            ])
        )
    }

    fn filters_placeholder(&self, rendered_filters: Option<&str>) -> String {
        let filters = self
            .value("filtersPlaceholder")
            .map(|value| array_docs(value, "Widgets"))
            .unwrap_or_default();
        if filters.is_empty() {
            "[]".to_string()
        } else {
            rendered_filters.unwrap_or("undefined").to_string()
        }
    }
}

fn resolve_xpath_arguments(
    documents: &[(String, Document)],
    page_name: &str,
    data_source: &WebListDataSource,
) -> Option<Vec<String>> {
    if !data_source.xpath() {
        return Some(Vec::new());
    }
    let names = xpath_variables(&data_source.xpath_constraint);
    if names.is_empty() {
        return Some(Vec::new());
    }
    let (module, name) = page_name.split_once('.')?;
    let page = documents.iter().find_map(|(owner, document)| {
        (owner == module
            && document.get_str("$Type").ok() == Some("Forms$Page")
            && document.get_str("Name").ok() == Some(name))
        .then_some(document)
    })?;
    object_page_parameters(page, &names).map(|parameters| parameters.into_keys().collect())
}

fn property_values(widget: &Document) -> Vec<PropertyValue> {
    let mut index = HashMap::new();
    index_documents(widget, &mut index);
    let Some(object) = widget.get_document("Object").ok() else {
        return Vec::new();
    };
    array_docs(object, "Properties")
        .into_iter()
        .filter_map(|property| {
            let id = property.get("TypePointer").and_then(extract_id)?;
            let schema = index.get(&id)?;
            Some(PropertyValue {
                key: schema
                    .get_str("PropertyKey")
                    .unwrap_or_default()
                    .to_string(),
                kind: schema
                    .get_document("ValueType")
                    .ok()
                    .and_then(|value_type| value_type.get_str("Type").ok())
                    .unwrap_or_default()
                    .to_string(),
                value: property.get_document("Value").ok()?.clone(),
            })
        })
        .collect()
}

fn widget_id(widget: &Document) -> Option<String> {
    widget
        .get_document("Type")
        .ok()
        .and_then(|type_| type_.get_str("WidgetId").ok())
        .map(str::to_string)
}

fn index_documents(document: &Document, index: &mut HashMap<String, Document>) {
    if let Some(id) = document.get("$ID").and_then(extract_id) {
        index.insert(id, document.clone());
    }
    for value in document.values() {
        match value {
            Bson::Document(document) => index_documents(document, index),
            Bson::Array(values) => {
                for value in values {
                    if let Bson::Document(document) = value {
                        index_documents(document, index);
                    }
                }
            }
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

fn translated_text(template: Option<&Document>) -> String {
    let items = template
        .and_then(|template| template.get_document("Template").ok())
        .map(|template| array_docs(template, "Items"))
        .unwrap_or_default();
    items
        .iter()
        .find(|item| item.get_str("LanguageCode").ok() == Some("en_US"))
        .or_else(|| items.first())
        .and_then(|item| item.get_str("Text").ok())
        .unwrap_or_default()
        .to_string()
}

fn css_class(widget: &Document) -> String {
    let name = widget.get_str("Name").unwrap_or_default();
    let appearance = widget
        .get_document("Appearance")
        .ok()
        .and_then(|appearance| appearance.get_str("Class").ok())
        .unwrap_or_default();
    let mut values = vec![format!("mx-name-{name}")];
    if !appearance.is_empty() && !values.iter().any(|value| value == appearance) {
        values.push(appearance.to_string());
    }
    values.join(" ")
}

fn set(values: &mut Vec<(String, String)>, key: &str, value: String) {
    if let Some((_, previous)) = values.iter_mut().find(|(name, _)| name == key) {
        *previous = value;
    } else {
        values.push((key.to_string(), value));
    }
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings always serialize")
}

fn js_object(values: &[(&str, String)]) -> String {
    format!(
        "{{ {} }}",
        values
            .iter()
            .map(|(key, value)| format!("{}: {value}", js_string(key)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn js_object_owned(values: &[(String, String)]) -> String {
    format!(
        "{{ {} }}",
        values
            .iter()
            .map(|(key, value)| format!("{}: {value}", js_string(key)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[cfg(test)]
mod tests {
    use mxrs_bson::doc;

    use super::*;

    fn property_type(id: &str, key: &str, kind: &str) -> Bson {
        Bson::Document(doc! {
            "$ID": id,
            "$Type": "CustomWidgets$WidgetPropertyType",
            "PropertyKey": key,
            "ValueType": { "$Type": "CustomWidgets$WidgetValueType", "Type": kind },
        })
    }

    fn property(id: &str, value: Document) -> Bson {
        Bson::Document(doc! {
            "$Type": "CustomWidgets$WidgetProperty",
            "TypePointer": id,
            "Value": value,
        })
    }

    fn value(primitive: &str) -> Document {
        doc! {
            "$Type": "CustomWidgets$WidgetValue",
            "PrimitiveValue": primitive,
            "Widgets": mxrs_bson::build_array(Vec::new(), 2),
            "Selection": "None",
        }
    }

    fn gallery(source: Document) -> Document {
        let types = vec![
            property_type("datasource", "datasource", "DataSource"),
            property_type("content", "content", "Widgets"),
            property_type("page-size", "pageSize", "Integer"),
            property_type("pagination", "pagination", "Enumeration"),
            property_type("caption", "loadMoreButtonCaption", "TextTemplate"),
            property_type("selection", "itemSelection", "Selection"),
        ];
        let mut data_source = value("");
        data_source.insert("DataSource", source);
        let mut content = value("");
        content.insert(
            "Widgets",
            mxrs_bson::build_array(
                vec![Bson::Document(doc! { "$Type": "Forms$DivContainer" })],
                2,
            ),
        );
        let mut caption = value("");
        caption.insert(
            "TextTemplate",
            doc! { "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "LanguageCode": "en_US", "Text": "More",
            })], 3) } },
        );
        doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Name": "gallery1",
            "Appearance": { "Class": "cards" },
            "Type": {
                "$Type": "CustomWidgets$CustomWidgetType",
                "WidgetId": GALLERY_WIDGET_ID,
                "ObjectType": { "$ID": "gallery-object", "PropertyTypes": mxrs_bson::build_array(types, 2) },
            },
            "Object": {
                "$Type": "CustomWidgets$WidgetObject",
                "TypePointer": "gallery-object",
                "Properties": mxrs_bson::build_array(vec![
                    property("datasource", data_source),
                    property("content", content),
                    property("page-size", value("20")),
                    property("pagination", value("buttons")),
                    property("caption", caption),
                    property("selection", value("")),
                ], 2),
            },
        }
    }

    #[test]
    fn compiles_xpath_gallery_with_page_arguments() {
        let widget = gallery(doc! {
            "$Type": "CustomWidgets$CustomWidgetXPathSource",
            "EntityRef": { "Entity": "Demo.Item" },
            "XPathConstraint": "[Demo.Item_Parent = $Parent]",
        });
        let documents = vec![(
            "Demo".to_string(),
            doc! {
                "$Type": "Forms$Page",
                "Name": "Home",
                "Parameters": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "$Type": "Forms$PageParameter",
                    "Name": "Parent",
                    "ParameterType": { "$Type": "DataTypes$ObjectType", "Entity": "Demo.Parent" },
                })], 2),
            },
        )];
        let compiler = GalleryBundleCompiler::new(&documents, "Demo.Home", &widget);
        assert!(compiler.supported());
        assert_eq!(compiler.entity_name(), "Demo.Item");
        assert_eq!(compiler.content_widgets().len(), 1);
        let rendered = compiler.render("[card]", None, None);
        for expected in [
            "React.createElement($Gallery",
            "DatabaseObjectListProperty",
            "TemplatedWidgetProperty({ children: () => [card]",
            "SelectionProperty",
            "ExpressionProperty",
            "mx-name-gallery1 cards",
            "\"arguments\": { \"Parent\": [\"$Parent\", undefined, false] }",
            "\"fetchOnlyWithAllParams\": true",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected}: {rendered}"
            );
        }
    }

    #[test]
    fn compiles_microflow_and_nanoflow_sources_and_rejects_unknown_widget() {
        let flows = vec![
            (
                "Demo".to_string(),
                doc! {
                    "$Type": "Microflows$Microflow",
                    "Name": "LoadItems",
                    "MicroflowReturnType": { "Entity": "Demo.Item" },
                },
            ),
            (
                "Demo".to_string(),
                doc! {
                    "$Type": "Microflows$Nanoflow",
                    "Name": "LoadItemsClient",
                    "MicroflowReturnType": { "Entity": "Demo.Item" },
                },
            ),
        ];
        let microflow = gallery(doc! {
            "$Type": "Forms$MicroflowSource",
            "MicroflowSettings": { "Microflow": "Demo.LoadItems" },
        });
        let compiler = GalleryBundleCompiler::new(&flows, "Demo.Home", &microflow);
        assert!(compiler.supported());
        assert!(
            compiler
                .render("[]", None, None)
                .contains("MicroflowObjectListProperty")
        );

        let nanoflow = gallery(doc! {
            "$Type": "Forms$NanoflowSource",
            "Nanoflow": "Demo.LoadItemsClient",
        });
        let compiler = GalleryBundleCompiler::new(&flows, "Demo.Home", &nanoflow);
        assert!(compiler.supported());
        let rendered = compiler.render("[]", Some("() => DemoLoadItems"), None);
        assert!(rendered.contains("NanoflowObjectListProperty"));
        assert!(rendered.contains("\"nanoflow\": () => DemoLoadItems"));

        let mut unknown = microflow;
        unknown
            .get_document_mut("Type")
            .unwrap()
            .insert("WidgetId", "example.Other");
        assert!(!GalleryBundleCompiler::new(&flows, "Demo.Home", &unknown).supported());
    }
}
