//! Property bundle for the official React Combo Box widget.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document, extract_id, parse_array};
use mxrs_compiler_support::{operation_id, widget_data_source_id};

use crate::WebListDataSource;

pub const COMBO_BOX_WIDGET_ID: &str = "com.mendix.widget.web.combobox.Combobox";

#[derive(Debug, Clone)]
struct PropertyValue {
    key: String,
    kind: String,
    value: Document,
}

pub struct ComboBoxBundleCompiler<'a> {
    documents: &'a [(String, Document)],
    page_name: &'a str,
    widget: &'a Document,
    scope: Option<&'a str>,
    entity: &'a str,
    values: Vec<PropertyValue>,
    data_source: WebListDataSource,
}

impl<'a> ComboBoxBundleCompiler<'a> {
    pub fn new(
        documents: &'a [(String, Document)],
        page_name: &'a str,
        widget: &'a Document,
        scope: Option<&'a str>,
        entity: &'a str,
    ) -> Self {
        Self {
            documents,
            page_name,
            widget,
            scope,
            entity,
            values: property_values(widget),
            data_source: WebListDataSource::from_documents(documents, widget),
        }
    }

    pub fn supported(&self) -> bool {
        widget_id(self.widget).as_deref() == Some(COMBO_BOX_WIDGET_ID)
            && self.resolved_scope().is_some()
            && self.supported_source()
    }

    pub fn render(&self) -> String {
        let mut properties = self.primitive_properties();
        let key = self.widget_key();
        set(&mut properties, "key", js_string(&key));
        set(&mut properties, "$widgetId", js_string(&key));
        set(&mut properties, "class", js_string(&css_class(self.widget)));
        set(&mut properties, "id", js_string(&key));
        set(
            &mut properties,
            "optionsSourceStaticDataSource",
            "[]".to_string(),
        );
        set(&mut properties, "ariaRequired", expression_bool(false));
        for (key, value) in self.source_properties() {
            set(&mut properties, &key, value);
        }
        let control = format!(
            "React.createElement($Combobox, {})",
            js_object_owned(&properties)
        );
        let caption = translated_text(self.widget.get_document("LabelTemplate").ok());
        let group_key = format!("{key}$formGroup");
        format!(
            "React.createElement($FormGroup, {})",
            js_object(&[
                ("key", js_string(&group_key)),
                ("$widgetId", js_string(&group_key)),
                (
                    "class",
                    js_string(&format!("{} mx-combobox", css_class(self.widget))),
                ),
                ("control", format!("[{control}]")),
                ("width", "3".to_string()),
                ("orientation", js_string("horizontal")),
                ("labelFor", js_string(&key)),
                ("caption", expression(&caption)),
                ("hasError", expression_bool(false)),
            ])
        )
    }

    fn supported_source(&self) -> bool {
        if self.primitive("source") == Some("database") {
            return self.target_attribute().is_some()
                && self.database_caption_attribute().is_some()
                && self.database_value_attribute().is_some()
                && self.list_source_supported();
        }
        if self.primitive("optionsSourceType") == Some("enumeration") {
            return self.primitive("source") == Some("context")
                && self.enumeration_attribute().is_some();
        }
        self.primitive("source") == Some("context")
            && self.primitive("optionsSourceType") == Some("association")
            && self.association_step().is_some()
            && (self.association_caption_attribute().is_some()
                || self.association_caption_expression().is_some())
            && self.list_source_supported()
    }

    fn list_source_supported(&self) -> bool {
        self.data_source.supported() && !self.data_source.entity.is_empty()
    }

    fn source_properties(&self) -> Vec<(String, String)> {
        if self.primitive("source") == Some("database") {
            return self.database_properties();
        }
        if self.primitive("optionsSourceType") == Some("enumeration") {
            return vec![(
                "attributeEnumeration".to_string(),
                self.attribute_property(self.enumeration_attribute().unwrap_or_default()),
            )];
        }
        self.association_properties()
    }

    fn database_properties(&self) -> Vec<(String, String)> {
        let target = self.target_attribute().unwrap_or_default();
        if !entity_steps(&target).is_empty() {
            return vec![
                ("source".to_string(), js_string("context")),
                (
                    "optionsSourceAssociationCaptionAttribute".to_string(),
                    self.list_attribute_property(
                        self.database_caption_attribute().unwrap_or_default(),
                    ),
                ),
                (
                    "attributeAssociation".to_string(),
                    self.association_property(&entity_steps(&target)[0]),
                ),
                (
                    "optionsSourceAssociationDataSource".to_string(),
                    self.list_property(),
                ),
            ];
        }
        vec![
            (
                "databaseAttributeString".to_string(),
                self.attribute_property(target),
            ),
            (
                "optionsSourceDatabaseCaptionAttribute".to_string(),
                self.list_attribute_property(self.database_caption_attribute().unwrap_or_default()),
            ),
            (
                "optionsSourceDatabaseValueAttribute".to_string(),
                self.list_attribute_property(self.database_value_attribute().unwrap_or_default()),
            ),
            (
                "optionsSourceDatabaseDataSource".to_string(),
                self.list_property(),
            ),
            (
                "optionsSourceDatabaseItemSelection".to_string(),
                self.selection_property(),
            ),
        ]
    }

    fn association_properties(&self) -> Vec<(String, String)> {
        let mut result = vec![
            (
                "attributeAssociation".to_string(),
                self.association_property(&self.association_step().unwrap_or_default()),
            ),
            (
                "optionsSourceAssociationDataSource".to_string(),
                self.list_property(),
            ),
        ];
        if let Some(attribute) = self.association_caption_attribute() {
            result.push((
                "optionsSourceAssociationCaptionAttribute".to_string(),
                self.list_attribute_property(attribute),
            ));
        } else {
            result.push((
                "optionsSourceAssociationCaptionExpression".to_string(),
                self.list_expression_property(),
            ));
        }
        result
    }

    fn list_expression_property(&self) -> String {
        let source = self.association_caption_expression().unwrap_or_default();
        let path = source.strip_prefix("$currentObject/").unwrap_or_default();
        self.list_expression(js_object(&[
            ("type", js_string("variable")),
            ("variable", js_string("currentObject")),
            ("path", js_string(path)),
        ]))
    }

    fn list_expression(&self, expr: String) -> String {
        format!(
            "ListExpressionProperty({})",
            js_object(&[
                (
                    "expression",
                    js_object(&[
                        ("expr", expr),
                        (
                            "args",
                            js_object(&[(
                                "currentObject",
                                js_object(&[
                                    ("widget", js_string(&self.widget_key())),
                                    ("source", js_string("object")),
                                ]),
                            )]),
                        ),
                    ]),
                ),
                ("dataSourceId", js_string(&self.data_source_id())),
            ])
        )
    }

    fn association_property(&self, step: &Document) -> String {
        format!(
            "AssociationProperty({})",
            js_object(&[
                ("type", js_string("Reference")),
                ("entity", js_string(&self.resolved_entity())),
                ("path", js_string("")),
                (
                    "attribute",
                    js_string(step.get_str("Association").unwrap_or_default()),
                ),
                (
                    "endpointEntity",
                    js_string(step.get_str("DestinationEntity").unwrap_or_default()),
                ),
                ("selectableObjectsId", js_string(&self.data_source_id())),
                (
                    "scope",
                    self.resolved_scope()
                        .map(|scope| js_string(&scope))
                        .unwrap_or_else(|| "null".to_string()),
                ),
                ("onChange", do_nothing()),
            ])
        )
    }

    fn list_property(&self) -> String {
        let mut config = vec![
            (
                "dataSourceId".to_string(),
                js_string(&self.data_source_id()),
            ),
            ("entity".to_string(), js_string(&self.data_source.entity)),
            (
                "scope".to_string(),
                self.resolved_scope()
                    .map(|scope| js_string(&scope))
                    .unwrap_or_else(|| "null".to_string()),
            ),
            (
                "operationId".to_string(),
                js_string(&operation_id(
                    self.page_name,
                    self.widget.get_str("Name").unwrap_or_default(),
                )),
            ),
        ];
        if self.data_source.xpath() {
            config.push(("sort".to_string(), "[]".to_string()));
            format!("DatabaseObjectListProperty({})", js_object_owned(&config))
        } else {
            config.push(("argMap".to_string(), "{  }".to_string()));
            config.push(("fetchOnlyWithAllParams".to_string(), "false".to_string()));
            format!("MicroflowObjectListProperty({})", js_object_owned(&config))
        }
    }

    fn selection_property(&self) -> String {
        let selection = self
            .value("optionsSourceDatabaseItemSelection")
            .and_then(|value| value.get_str("Selection").ok())
            .unwrap_or("Single");
        format!(
            "SelectionProperty({})",
            js_object(&[
                ("selectionType", js_string(selection)),
                ("dataSourceId", js_string(&self.data_source_id())),
            ])
        )
    }

    fn attribute_property(&self, attribute: Document) -> String {
        let qualified = attribute.get_str("Attribute").unwrap_or_default();
        let (entity, name) = qualified.rsplit_once('.').unwrap_or(("", ""));
        let path = entity_steps(&attribute)
            .iter()
            .flat_map(|step| {
                [
                    step.get_str("Association").ok(),
                    step.get_str("DestinationEntity").ok(),
                ]
            })
            .flatten()
            .collect::<Vec<_>>()
            .join("/");
        format!(
            "AttributeProperty({})",
            js_object(&[
                (
                    "scope",
                    self.resolved_scope()
                        .map(|scope| js_string(&scope))
                        .unwrap_or_else(|| "null".to_string()),
                ),
                ("path", js_string(&path)),
                ("entity", js_string(entity)),
                ("attribute", js_string(name)),
                ("onChange", do_nothing()),
                ("isList", "false".to_string()),
                ("validation", "null".to_string()),
                ("formatting", "{  }".to_string()),
            ])
        )
    }

    fn list_attribute_property(&self, attribute: Document) -> String {
        let qualified = attribute.get_str("Attribute").unwrap_or_default();
        let (entity, name) = qualified.rsplit_once('.').unwrap_or(("", ""));
        format!(
            "ListAttributeProperty({})",
            js_object(&[
                ("path", js_string("")),
                ("entity", js_string(entity)),
                ("attribute", js_string(name)),
                ("attributeType", js_string("String")),
                ("sortable", "true".to_string()),
                ("filterable", "true".to_string()),
                ("dataSourceId", js_string(&self.data_source_id())),
                ("isList", "false".to_string()),
            ])
        )
    }

    fn resolved_scope(&self) -> Option<String> {
        self.page_parameter()
            .map(|parameter| format!("${parameter}"))
            .or_else(|| self.scope.map(str::to_string))
    }

    fn resolved_entity(&self) -> String {
        if !self.entity.is_empty() {
            return self.entity.to_string();
        }
        let Some(parameter) = self.page_parameter() else {
            return String::new();
        };
        let Some((module, page_name)) = self.page_name.split_once('.') else {
            return String::new();
        };
        self.documents
            .iter()
            .find_map(|(owner, document)| {
                (owner == module
                    && document.get_str("$Type").ok() == Some("Forms$Page")
                    && document.get_str("Name").ok() == Some(page_name))
                .then(|| {
                    array_docs(document, "Parameters")
                        .into_iter()
                        .find(|candidate| candidate.get_str("Name").ok() == Some(parameter))
                        .and_then(|parameter| {
                            parameter
                                .get_document("ParameterType")
                                .ok()
                                .and_then(|type_| type_.get_str("Entity").ok())
                                .map(str::to_string)
                        })
                })
                .flatten()
            })
            .unwrap_or_default()
    }

    fn page_parameter(&self) -> Option<&str> {
        self.value("databaseAttributeString")
            .and_then(|value| value.get_document("SourceVariable").ok())
            .and_then(|source| source.get_str("PageParameter").ok())
            .filter(|name| !name.is_empty())
    }

    fn primitive_properties(&self) -> Vec<(String, String)> {
        self.values
            .iter()
            .filter_map(|property| {
                let compiled = match property.kind.as_str() {
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
                    "TextTemplate" => Some(expression(&translated_text(
                        property.value.get_document("TextTemplate").ok(),
                    ))),
                    "Widgets" if array_docs(&property.value, "Widgets").is_empty() => {
                        Some("[]".to_string())
                    }
                    _ => None,
                }?;
                Some((property.key.clone(), compiled))
            })
            .collect()
    }

    fn value(&self, key: &str) -> Option<&Document> {
        self.values
            .iter()
            .find(|property| property.key == key)
            .map(|property| &property.value)
    }

    fn primitive(&self, key: &str) -> Option<&str> {
        self.value(key)?.get_str("PrimitiveValue").ok()
    }

    fn target_attribute(&self) -> Option<Document> {
        self.value("databaseAttributeString")?
            .get_document("AttributeRef")
            .ok()
            .cloned()
    }

    fn enumeration_attribute(&self) -> Option<Document> {
        self.value("attributeEnumeration")?
            .get_document("AttributeRef")
            .ok()
            .cloned()
    }

    fn database_caption_attribute(&self) -> Option<Document> {
        self.value("optionsSourceDatabaseCaptionAttribute")?
            .get_document("AttributeRef")
            .ok()
            .cloned()
    }

    fn database_value_attribute(&self) -> Option<Document> {
        self.value("optionsSourceDatabaseValueAttribute")?
            .get_document("AttributeRef")
            .ok()
            .cloned()
    }

    fn association_caption_attribute(&self) -> Option<Document> {
        self.value("optionsSourceAssociationCaptionAttribute")?
            .get_document("AttributeRef")
            .ok()
            .cloned()
    }

    fn association_caption_expression(&self) -> Option<&str> {
        self.value("optionsSourceAssociationCaptionExpression")?
            .get_str("Expression")
            .ok()
    }

    fn association_step(&self) -> Option<Document> {
        let value = self.value("attributeAssociation")?;
        entity_steps(value).into_iter().next()
    }

    fn widget_key(&self) -> String {
        format!(
            "p.{}.{}",
            self.page_name,
            self.widget.get_str("Name").unwrap_or_default()
        )
    }

    fn data_source_id(&self) -> String {
        widget_data_source_id(&self.widget_key())
    }
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
            let schema = index.get(&property.get("TypePointer").and_then(extract_id)?)?;
            Some(PropertyValue {
                key: schema.get_str("PropertyKey").ok()?.to_string(),
                kind: schema
                    .get_document("ValueType")
                    .ok()
                    .and_then(|type_| type_.get_str("Type").ok())
                    .unwrap_or_default()
                    .to_string(),
                value: property.get_document("Value").ok()?.clone(),
            })
        })
        .collect()
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

fn widget_id(widget: &Document) -> Option<String> {
    widget
        .get_document("Type")
        .ok()
        .and_then(|type_| type_.get_str("WidgetId").ok())
        .map(str::to_string)
}

fn entity_steps(value: &Document) -> Vec<Document> {
    let reference = value.get_document("EntityRef").ok().or_else(|| {
        value
            .get_document("AttributeRef")
            .ok()
            .and_then(|r| r.get_document("EntityRef").ok())
    });
    reference
        .map(|reference| array_docs(reference, "Steps"))
        .unwrap_or_default()
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

fn expression(value: &str) -> String {
    expression_value(js_string(value))
}

fn expression_bool(value: bool) -> String {
    expression_value(value.to_string())
}

fn expression_value(value: String) -> String {
    format!(
        "ExpressionProperty({})",
        js_object(&[(
            "expression",
            js_object(&[
                (
                    "expr",
                    js_object(&[("type", js_string("literal")), ("value", value)]),
                ),
                ("args", "{  }".to_string()),
            ]),
        )])
    )
}

fn do_nothing() -> String {
    js_object(&[
        ("type", js_string("doNothing")),
        ("argMap", "{  }".to_string()),
        ("config", "{  }".to_string()),
        ("disabledDuringExecution", "false".to_string()),
    ])
}

fn css_class(widget: &Document) -> String {
    let name = widget.get_str("Name").unwrap_or_default();
    let appearance = widget
        .get_document("Appearance")
        .ok()
        .and_then(|appearance| appearance.get_str("Class").ok())
        .unwrap_or_default();
    [format!("mx-name-{name}"), appearance.to_string()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
            "$ID": id, "PropertyKey": key, "ValueType": { "Type": kind },
        })
    }

    fn property(id: &str, value: Document) -> Bson {
        Bson::Document(doc! { "TypePointer": id, "Value": value })
    }

    fn value(primitive: &str) -> Document {
        doc! {
            "PrimitiveValue": primitive,
            "Widgets": mxrs_bson::build_array(Vec::new(), 2),
            "Selection": "None",
        }
    }

    fn xpath(entity: &str) -> Document {
        doc! {
            "$Type": "CustomWidgets$CustomWidgetXPathSource",
            "EntityRef": { "Entity": entity },
            "XPathConstraint": "",
        }
    }

    fn combo_widget(name: &str, properties: Vec<Bson>, extra_types: Vec<Bson>) -> Document {
        let mut types = vec![
            property_type("source", "source", "Enumeration"),
            property_type("options", "optionsSourceType", "Enumeration"),
            property_type(
                "caption",
                "optionsSourceAssociationCaptionAttribute",
                "Attribute",
            ),
            property_type(
                "caption-expr",
                "optionsSourceAssociationCaptionExpression",
                "Expression",
            ),
            property_type("association", "attributeAssociation", "Association"),
            property_type(
                "association-source",
                "optionsSourceAssociationDataSource",
                "DataSource",
            ),
            property_type("target", "databaseAttributeString", "Attribute"),
            property_type(
                "db-caption",
                "optionsSourceDatabaseCaptionAttribute",
                "Attribute",
            ),
            property_type(
                "db-value",
                "optionsSourceDatabaseValueAttribute",
                "Attribute",
            ),
            property_type("db-source", "optionsSourceDatabaseDataSource", "DataSource"),
            property_type(
                "db-selection",
                "optionsSourceDatabaseItemSelection",
                "Selection",
            ),
            property_type("clearable", "clearable", "Boolean"),
            property_type("interval", "filterInputDebounceInterval", "Integer"),
        ];
        types.extend(extra_types);
        doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Name": name,
            "Appearance": { "Class": "selector" },
            "LabelTemplate": { "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "LanguageCode": "en_US", "Text": "Location",
            })], 3) } },
            "Type": {
                "WidgetId": COMBO_BOX_WIDGET_ID,
                "ObjectType": { "$ID": "combo-object", "PropertyTypes": mxrs_bson::build_array(types, 2) },
            },
            "Object": {
                "TypePointer": "combo-object",
                "Properties": mxrs_bson::build_array(properties, 2),
            },
        }
    }

    fn attribute(name: &str) -> Document {
        doc! { "AttributeRef": { "Attribute": name } }
    }

    fn association() -> Document {
        doc! { "EntityRef": { "Steps": mxrs_bson::build_array(vec![Bson::Document(doc! {
            "Association": "Demo.Order_Location",
            "DestinationEntity": "Demo.Location",
        })], 2) } }
    }

    #[test]
    fn compiles_context_association_and_caption_expression() {
        let mut source = value("");
        source.insert("DataSource", xpath("Demo.Location"));
        let widget = combo_widget(
            "location",
            vec![
                property("source", value("context")),
                property("options", value("association")),
                property("caption", attribute("Demo.Location.Name")),
                property("association", association()),
                property("association-source", source),
                property("clearable", value("true")),
            ],
            Vec::new(),
        );
        let documents = Vec::new();
        let compiler = ComboBoxBundleCompiler::new(
            &documents,
            "Demo.Edit",
            &widget,
            Some("p.Demo.Edit.editor"),
            "Demo.Order",
        );
        assert!(compiler.supported());
        let rendered = compiler.render();
        for expected in [
            "React.createElement($FormGroup",
            "React.createElement($Combobox",
            "AssociationProperty",
            "DatabaseObjectListProperty",
            "ListAttributeProperty",
            "Demo.Order_Location",
            "mx-name-location selector",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected}: {rendered}"
            );
        }

        let mut source = value("");
        source.insert("DataSource", xpath("Demo.Location"));
        let widget = combo_widget(
            "location",
            vec![
                property("source", value("context")),
                property("options", value("association")),
                property("caption-expr", doc! { "Expression": "$currentObject/Name" }),
                property("association", association()),
                property("association-source", source),
            ],
            Vec::new(),
        );
        let compiler = ComboBoxBundleCompiler::new(
            &documents,
            "Demo.Edit",
            &widget,
            Some("p.Demo.Edit.editor"),
            "Demo.Order",
        );
        assert!(compiler.supported());
        assert!(compiler.render().contains("ListExpressionProperty"));
        assert!(compiler.render().contains("\"path\": \"Name\""));
    }

    #[test]
    fn compiles_direct_and_association_database_targets() {
        let documents = vec![(
            "Demo".to_string(),
            doc! {
                "$Type": "Forms$Page",
                "Name": "Edit",
                "Parameters": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "Name": "Order", "ParameterType": { "Entity": "Demo.Order" },
                })], 2),
            },
        )];
        let make = |stepped: bool| {
            let mut target = attribute("Demo.Order.Code");
            target.insert("SourceVariable", doc! { "PageParameter": "Order" });
            if stepped {
                target.get_document_mut("AttributeRef").unwrap().insert(
                    "EntityRef",
                    association().get_document("EntityRef").unwrap().clone(),
                );
            } else {
                target.get_document_mut("AttributeRef").unwrap().insert(
                    "EntityRef",
                    doc! { "Steps": mxrs_bson::build_array(Vec::new(), 2) },
                );
            }
            let mut source = value("");
            source.insert("DataSource", xpath("Demo.Order"));
            combo_widget(
                "code",
                vec![
                    property("source", value("database")),
                    property("options", value("database")),
                    property("target", target),
                    property("db-caption", attribute("Demo.Order.Code")),
                    property("db-value", attribute("Demo.Order.Code")),
                    property("db-source", source),
                    property("db-selection", doc! { "Selection": "Single" }),
                    property("interval", value("250")),
                ],
                Vec::new(),
            )
        };
        let direct = make(false);
        let compiler = ComboBoxBundleCompiler::new(&documents, "Demo.Edit", &direct, None, "");
        assert!(compiler.supported());
        let rendered = compiler.render();
        assert!(rendered.contains("AttributeProperty"));
        assert!(rendered.contains("SelectionProperty"));
        assert!(rendered.contains("\"scope\": \"$Order\""));

        let stepped = make(true);
        let compiler = ComboBoxBundleCompiler::new(&documents, "Demo.Edit", &stepped, None, "");
        assert!(compiler.supported());
        let rendered = compiler.render();
        assert!(rendered.contains("\"source\": \"context\""));
        assert!(rendered.contains("Demo.Order_Location"));
    }

    #[test]
    fn compiles_context_enumeration_without_list_source() {
        let widget = combo_widget(
            "status",
            vec![
                property("source", value("context")),
                property("options", value("enumeration")),
                property("enum", attribute("Demo.Order.Status")),
            ],
            vec![property_type("enum", "attributeEnumeration", "Attribute")],
        );
        let documents = Vec::new();
        let compiler = ComboBoxBundleCompiler::new(
            &documents,
            "Demo.Edit",
            &widget,
            Some("p.Demo.Edit.editor"),
            "Demo.Order",
        );
        assert!(compiler.supported());
        let rendered = compiler.render();
        assert!(rendered.contains("\"optionsSourceType\": \"enumeration\""));
        assert!(rendered.contains("\"attributeEnumeration\": AttributeProperty"));
    }
}
