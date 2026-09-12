//! JavaScript property bundle for the official React Data Grid 2 widget.
//!
//! Covers the XPath-backed grid subset emitted by mxrb: attribute and
//! dynamic-text columns, custom content, selection, filters, and association
//! filter metadata.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document, extract_id, parse_array};
use mxrs_compiler_support::{operation_id, widget_data_source_id};

use crate::WebListDataSource;
use crate::web_operation::DATA_GRID_WIDGET_ID;

type WidgetRenderer<'a> = dyn Fn(&[Document], &str, &str) -> String + 'a;

#[derive(Debug, Clone)]
struct PropertyValue {
    key: String,
    kind: String,
    value: Document,
}

/// Compiles one raw Data Grid 2 custom widget into its React property bundle.
pub struct DataGridBundleCompiler<'a> {
    documents: &'a [(String, Document)],
    page_name: &'a str,
    widget: &'a Document,
    index: HashMap<String, Document>,
    properties: Vec<PropertyValue>,
    data_source: WebListDataSource,
    scope: Option<&'a str>,
    render_widgets: Option<&'a WidgetRenderer<'a>>,
}

impl<'a> DataGridBundleCompiler<'a> {
    pub fn new(
        documents: &'a [(String, Document)],
        page_name: &'a str,
        widget: &'a Document,
    ) -> Self {
        let mut index = HashMap::new();
        index_documents(widget, &mut index);
        let properties = property_values(widget, &index);
        Self {
            documents,
            page_name,
            widget,
            index,
            properties,
            data_source: WebListDataSource::from_documents(documents, widget),
            scope: None,
            render_widgets: None,
        }
    }

    /// Supplies the surrounding page renderer needed by custom-content
    /// columns and non-empty widget placeholders.
    pub fn with_widget_renderer(mut self, renderer: &'a WidgetRenderer<'a>) -> Self {
        self.render_widgets = Some(renderer);
        self
    }

    pub fn with_scope(mut self, scope: &'a str) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn supported(&self) -> bool {
        widget_id(self.widget).as_deref() == Some(DATA_GRID_WIDGET_ID)
            && self.data_source.supported()
            && (!self.data_source.association() || self.scope.is_some())
            && !self.data_source.entity.is_empty()
            && !self.columns().is_empty()
            && self
                .columns()
                .iter()
                .all(|column| self.supported_column(column))
    }

    pub fn entity_name(&self) -> &str {
        &self.data_source.entity
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

    pub fn render(&self) -> String {
        let mut properties = self.primitive_properties(&self.properties);
        let key = self.widget_key();
        set(&mut properties, "key", js_string(&key));
        set(&mut properties, "$widgetId", js_string(&key));
        set(&mut properties, "advanced", "false".to_string());
        set(&mut properties, "datasource", self.data_source_js());
        set(
            &mut properties,
            "columns",
            format!(
                "[{}]",
                self.columns()
                    .iter()
                    .map(|column| js_object_owned(&self.compile_column(column)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        set(&mut properties, "itemSelection", self.selection());
        set(
            &mut properties,
            "filtersPlaceholder",
            self.compile_widgets(self.value("filtersPlaceholder")),
        );
        set(&mut properties, "class", js_string(&css_class(self.widget)));
        format!(
            "React.createElement($Datagrid, {})",
            js_object_owned(&properties)
        )
    }

    fn value(&self, key: &str) -> Option<&Document> {
        self.properties
            .iter()
            .find(|property| property.key == key)
            .map(|property| &property.value)
    }

    fn columns(&self) -> Vec<Document> {
        self.value("columns")
            .map(|value| array_docs(value, "Objects"))
            .unwrap_or_default()
    }

    fn supported_column(&self, column: &Document) -> bool {
        let values = property_values(column, &self.index);
        let mode = value(&values, "showContentAs")
            .and_then(|value| value.get_str("PrimitiveValue").ok())
            .unwrap_or_default();
        match mode {
            "attribute" | "dynamicText" => attribute_name(&values).contains('.'),
            "customContent" => self.render_widgets.is_some() && value(&values, "content").is_some(),
            _ => false,
        }
    }

    fn primitive_properties(&self, values: &[PropertyValue]) -> Vec<(String, String)> {
        values
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

    fn data_source_js(&self) -> String {
        let data_source_id = self.data_source_id();
        let operation = operation_id(
            self.page_name,
            self.widget.get_str("Name").unwrap_or_default(),
        );
        if self.data_source.association() {
            return format!(
                "AssociationObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&data_source_id)),
                    ("operationId", js_string(&operation)),
                    ("scope", js_string(self.scope.unwrap_or_default())),
                    ("directPath", js_string(&self.data_source.association_path)),
                    ("sort", "[]".to_string()),
                ])
            );
        }
        if self.data_source.microflow() {
            return format!(
                "MicroflowObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&data_source_id)),
                    ("operationId", js_string(&operation)),
                    ("argMap", "{}".to_string()),
                    ("fetchOnlyWithAllParams", "false".to_string()),
                ])
            );
        }
        format!(
            "DatabaseObjectListProperty({})",
            js_object(&[
                ("dataSourceId", js_string(&data_source_id)),
                ("entity", js_string(&self.data_source.entity)),
                ("operationId", js_string(&operation)),
                ("sort", "[]".to_string()),
            ])
        )
    }

    fn selection(&self) -> String {
        let selection = self
            .value("itemSelection")
            .and_then(|value| value.get_str("Selection").ok())
            .unwrap_or("None");
        format!(
            "SelectionProperty({})",
            js_object(&[
                ("selectionType", js_string(selection)),
                ("dataSourceId", js_string(&self.data_source_id())),
            ])
        )
    }

    fn compile_column(&self, column: &Document) -> Vec<(String, String)> {
        let values = property_values(column, &self.index);
        let mode = value(&values, "showContentAs")
            .and_then(|value| value.get_str("PrimitiveValue").ok())
            .unwrap_or_default();
        let attribute = attribute_name(&values);
        let (entity, name) = attribute.rsplit_once('.').unwrap_or(("", ""));
        let mut result = self.primitive_properties(&values);
        set(&mut result, "showContentAs", js_string(mode));
        set(
            &mut result,
            "attribute",
            if attribute.is_empty() {
                "undefined".to_string()
            } else {
                self.raw_attribute(entity, name)
            },
        );
        set(&mut result, "dynamicText", "undefined".to_string());
        set(
            &mut result,
            "header",
            expression(&translated_text(
                value(&values, "header").and_then(|value| value.get_document("TextTemplate").ok()),
            )),
        );
        set(&mut result, "tooltip", "undefined".to_string());
        set(
            &mut result,
            "filter",
            self.compile_widgets(value(&values, "filter")),
        );
        set(&mut result, "visible", expression_bool(true));
        set(&mut result, "exportValue", "undefined".to_string());

        if mode == "dynamicText" {
            set(
                &mut result,
                "dynamicText",
                self.dynamic_text(&values, entity, name),
            );
        }
        if mode == "customContent" {
            set(&mut result, "content", self.templated_content(&values));
        }
        if self.association_filter_supported(&values) {
            result.extend(self.association_filter(&values));
        }
        result
    }

    fn raw_attribute(&self, entity: &str, name: &str) -> String {
        format!(
            "ListAttributeProperty({})",
            js_object(&[
                ("path", js_string("")),
                ("entity", js_string(entity)),
                ("attribute", js_string(name)),
                (
                    "attributeType",
                    js_string(&self.attribute_type(entity, name))
                ),
                ("sortable", "true".to_string()),
                ("filterable", "true".to_string()),
                ("dataSourceId", js_string(&self.data_source_id())),
                ("isList", "false".to_string()),
            ])
        )
    }

    fn dynamic_text(&self, values: &[PropertyValue], entity: &str, name: &str) -> String {
        let template =
            value(values, "dynamicText").and_then(|value| value.get_document("TextTemplate").ok());
        if template.is_none_or(|template| array_docs(template, "Parameters").len() != 1)
            || name.is_empty()
        {
            return "undefined".to_string();
        }
        let variable = js_object(&[
            ("type", js_string("variable")),
            ("variable", js_string("currentObject")),
            ("path", js_string(name)),
        ]);
        let expr = if self.attribute_type(entity, name) == "DateTime" {
            js_object(&[
                ("type", js_string("function")),
                ("name", js_string("_format")),
                (
                    "parameters",
                    format!(
                        "[{variable}, {}]",
                        js_object(&[
                            ("type", js_string("literal")),
                            ("value", js_string("{\"type\":\"datetime\"}")),
                        ])
                    ),
                ),
            ])
        } else {
            variable
        };
        self.list_expression(expr, &self.data_source_id())
    }

    fn list_expression(&self, expr: String, data_source_id: &str) -> String {
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
                ("dataSourceId", js_string(data_source_id)),
            ])
        )
    }

    fn templated_content(&self, values: &[PropertyValue]) -> String {
        let widgets = value(values, "content")
            .map(|value| array_docs(value, "Widgets"))
            .unwrap_or_default();
        let rendered = self
            .render_widgets
            .map(|renderer| renderer(&widgets, &self.widget_key(), self.entity_name()))
            .unwrap_or_else(|| "undefined".to_string());
        format!(
            "TemplatedWidgetProperty({})",
            js_object(&[
                ("dataSourceId", js_string(&self.data_source_id())),
                ("editable", "false".to_string()),
                ("children", format!("() => {rendered}")),
            ])
        )
    }

    fn compile_widgets(&self, value: Option<&Document>) -> String {
        let widgets = value
            .map(|value| array_docs(value, "Widgets"))
            .unwrap_or_default();
        if widgets.is_empty() {
            return "[]".to_string();
        }
        self.render_widgets
            .map(|renderer| renderer(&widgets, &self.widget_key(), self.entity_name()))
            .unwrap_or_else(|| "undefined".to_string())
    }

    fn association_filter_supported(&self, values: &[PropertyValue]) -> bool {
        !association_steps(values).is_empty() && filter_widget(values).is_some()
    }

    fn association_filter(&self, values: &[PropertyValue]) -> Vec<(String, String)> {
        let steps = association_steps(values);
        let Some(filter) = filter_widget(values) else {
            return Vec::new();
        };
        let association = steps
            .first()
            .and_then(|step| step.get_str("Association").ok())
            .unwrap_or_default();
        let endpoint = steps
            .last()
            .and_then(|step| step.get_str("DestinationEntity").ok())
            .unwrap_or_default();
        let attribute = attribute_name(values);
        let caption = attribute
            .rsplit_once('.')
            .map(|(_, name)| name)
            .unwrap_or_default();
        let filter_name = filter.get_str("Name").unwrap_or_default();
        let selectable_id = format!("{}${filter_name}", self.data_source_id());
        let operation = operation_id(
            self.page_name,
            &format!(
                "{}${filter_name}",
                self.widget.get_str("Name").unwrap_or_default()
            ),
        );
        let filter_values = property_values(&filter, &self.index);
        let multi_select = value(&filter_values, "multiSelect")
            .and_then(|value| value.get_str("PrimitiveValue").ok())
            == Some("true");
        vec![
            (
                "filterAssociation".to_string(),
                format!(
                    "ListAssociationProperty({})",
                    js_object(&[
                        (
                            "type",
                            js_string(if multi_select {
                                "ReferenceSet"
                            } else {
                                "Reference"
                            }),
                        ),
                        ("entity", "undefined".to_string()),
                        ("path", js_string("")),
                        ("attribute", js_string(association)),
                        ("endpointEntity", js_string(endpoint)),
                        ("selectableObjectsId", js_string(&selectable_id)),
                        ("filterable", "true".to_string()),
                        ("dataSourceId", js_string(&self.data_source_id())),
                    ])
                ),
            ),
            (
                "filterAssociationOptions".to_string(),
                format!(
                    "DatabaseObjectListProperty({})",
                    js_object(&[
                        ("dataSourceId", js_string(&selectable_id)),
                        ("entity", js_string(endpoint)),
                        ("operationId", js_string(&operation)),
                        (
                            "sort",
                            format!("[[{}, {}]]", js_string(caption), js_string("asc")),
                        ),
                    ])
                ),
            ),
            (
                "filterAssociationOptionLabel".to_string(),
                self.list_expression(
                    js_object(&[
                        ("type", js_string("variable")),
                        ("variable", js_string("currentObject")),
                        ("path", js_string(caption)),
                    ]),
                    &selectable_id,
                ),
            ),
        ]
    }

    fn attribute_type(&self, entity: &str, name: &str) -> String {
        let Some((module_name, entity_name)) = entity.split_once('.') else {
            return "String".to_string();
        };
        let attribute_type = self.documents.iter().find_map(|(module, document)| {
            if module != module_name
                || document.get_str("$Type").ok() != Some("DomainModels$DomainModel")
            {
                return None;
            }
            array_docs(document, "Entities")
                .into_iter()
                .find(|candidate| candidate.get_str("Name").ok() == Some(entity_name))
                .and_then(|entity| {
                    array_docs(&entity, "Attributes")
                        .into_iter()
                        .find(|attribute| attribute.get_str("Name").ok() == Some(name))
                })
                .and_then(|attribute| attribute.get_document("NewType").ok().cloned())
        });
        attribute_type
            .as_ref()
            .and_then(|type_| type_.get_str("$Type").ok())
            .unwrap_or("DomainModels$StringAttributeType")
            .trim_start_matches("DomainModels$")
            .trim_end_matches("AttributeType")
            .to_string()
    }
}

fn property_values(document: &Document, index: &HashMap<String, Document>) -> Vec<PropertyValue> {
    let object = if document.get_str("$Type").ok() == Some("CustomWidgets$WidgetObject") {
        document
    } else if let Ok(object) = document.get_document("Object") {
        object
    } else {
        return Vec::new();
    };
    array_docs(object, "Properties")
        .into_iter()
        .filter_map(|property| {
            let id = property.get("TypePointer").and_then(extract_id)?;
            let schema = index.get(&id)?;
            Some(PropertyValue {
                key: schema.get_str("PropertyKey").ok()?.to_string(),
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

fn value<'a>(values: &'a [PropertyValue], key: &str) -> Option<&'a Document> {
    values
        .iter()
        .find(|property| property.key == key)
        .map(|property| &property.value)
}

fn attribute_name(values: &[PropertyValue]) -> String {
    value(values, "attribute")
        .and_then(|value| value.get_document("AttributeRef").ok())
        .and_then(|reference| reference.get_str("Attribute").ok())
        .unwrap_or_default()
        .to_string()
}

fn association_steps(values: &[PropertyValue]) -> Vec<Document> {
    value(values, "attribute")
        .and_then(|value| value.get_document("AttributeRef").ok())
        .and_then(|reference| reference.get_document("EntityRef").ok())
        .map(|reference| array_docs(reference, "Steps"))
        .unwrap_or_default()
}

fn filter_widget(values: &[PropertyValue]) -> Option<Document> {
    value(values, "filter")
        .map(|value| array_docs(value, "Widgets"))
        .and_then(|widgets| widgets.into_iter().next())
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

fn expression(value: &str) -> String {
    format!(
        "ExpressionProperty({})",
        js_object(&[(
            "expression",
            js_object(&[
                (
                    "expr",
                    js_object(&[("type", js_string("literal")), ("value", js_string(value)),]),
                ),
                ("args", "{  }".to_string()),
            ]),
        )])
    )
}

fn expression_bool(value: bool) -> String {
    format!(
        "ExpressionProperty({})",
        js_object(&[(
            "expression",
            js_object(&[
                (
                    "expr",
                    js_object(&[("type", js_string("literal")), ("value", value.to_string()),]),
                ),
                ("args", "{  }".to_string()),
            ]),
        )])
    )
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
            "Objects": mxrs_bson::build_array(Vec::new(), 2),
            "Selection": "None",
        }
    }

    fn text(value: &str) -> Document {
        doc! { "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
            "LanguageCode": "en_US", "Text": value,
        })], 3) } }
    }

    fn column(mode: &str, attribute: &str) -> Document {
        let mut show = value(mode);
        show.insert("PrimitiveValue", mode);
        let mut attr = value("");
        attr.insert("AttributeRef", doc! { "Attribute": attribute });
        let mut header = value("");
        header.insert("TextTemplate", text("Name"));
        doc! {
            "$Type": "CustomWidgets$WidgetObject",
            "Properties": mxrs_bson::build_array(vec![
                property("show", show),
                property("attribute", attr),
                property("header", header),
                property("sortable", value("true")),
            ], 2),
        }
    }

    fn grid(columns: Vec<Document>) -> Document {
        let types = vec![
            property_type("datasource", "datasource", "DataSource"),
            property_type("columns", "columns", "Object"),
            property_type("refresh", "refreshInterval", "Integer"),
            property_type("enabled", "refreshIndicator", "Boolean"),
            property_type("pagination", "pagination", "Enumeration"),
            property_type("caption", "loadMoreButtonCaption", "TextTemplate"),
            property_type("filters", "filtersPlaceholder", "Widgets"),
            property_type("selection", "itemSelection", "Selection"),
            property_type("show", "showContentAs", "Enumeration"),
            property_type("attribute", "attribute", "Attribute"),
            property_type("header", "header", "TextTemplate"),
            property_type("sortable", "sortable", "Boolean"),
            property_type("dynamic", "dynamicText", "TextTemplate"),
            property_type("content", "content", "Widgets"),
            property_type("filter", "filter", "Widgets"),
            property_type("multi", "multiSelect", "Boolean"),
        ];
        let mut source = value("");
        source.insert(
            "DataSource",
            doc! {
                "$Type": "CustomWidgets$CustomWidgetXPathSource",
                "EntityRef": { "Entity": "Demo.Item" },
            },
        );
        let mut column_value = value("");
        column_value.insert(
            "Objects",
            mxrs_bson::build_array(columns.into_iter().map(Bson::Document).collect(), 2),
        );
        let mut caption = value("");
        caption.insert("TextTemplate", text("Load More"));
        doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Name": "grid",
            "Appearance": { "Class": "table" },
            "Type": {
                "$Type": "CustomWidgets$CustomWidgetType",
                "WidgetId": DATA_GRID_WIDGET_ID,
                "ObjectType": {
                    "$ID": "grid-object",
                    "PropertyTypes": mxrs_bson::build_array(types, 2),
                },
            },
            "Object": {
                "$Type": "CustomWidgets$WidgetObject",
                "TypePointer": "grid-object",
                "Properties": mxrs_bson::build_array(vec![
                    property("datasource", source),
                    property("columns", column_value),
                    property("refresh", value("20")),
                    property("enabled", value("false")),
                    property("pagination", value("buttons")),
                    property("caption", caption),
                    property("filters", value("")),
                    property("selection", value("")),
                ], 2),
            },
        }
    }

    fn documents() -> Vec<(String, Document)> {
        vec![(
            "Demo".to_string(),
            doc! {
                "$Type": "DomainModels$DomainModel",
                "Entities": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "Name": "Item",
                    "Attributes": mxrs_bson::build_array(vec![
                        Bson::Document(doc! {
                            "Name": "Name",
                            "NewType": { "$Type": "DomainModels$StringAttributeType" },
                        }),
                        Bson::Document(doc! {
                            "Name": "CreatedAt",
                            "NewType": { "$Type": "DomainModels$DateTimeAttributeType" },
                        }),
                    ], 2),
                })], 2),
            },
        )]
    }

    #[test]
    fn compiles_xpath_grid_and_attribute_column() {
        let documents = documents();
        let widget = grid(vec![column("attribute", "Demo.Item.Name")]);
        let compiler = DataGridBundleCompiler::new(&documents, "Demo.Home", &widget);
        assert!(compiler.supported());
        assert_eq!(compiler.entity_name(), "Demo.Item");
        let rendered = compiler.render();
        for expected in [
            "React.createElement($Datagrid",
            "DatabaseObjectListProperty",
            "ListAttributeProperty",
            "ExpressionProperty",
            "\"attributeType\": \"String\"",
            "\"refreshIndicator\": false",
            "SelectionProperty",
            "mx-name-grid table",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected}: {rendered}"
            );
        }
    }

    #[test]
    fn compiles_microflow_and_association_grid_sources() {
        let set_source = |widget: &mut Document, source: Document| {
            widget
                .get_document_mut("Object")
                .unwrap()
                .get_array_mut("Properties")
                .unwrap()[1]
                .as_document_mut()
                .unwrap()
                .get_document_mut("Value")
                .unwrap()
                .insert("DataSource", source);
        };
        let mut documents = documents();
        documents.push((
            "Demo".to_string(),
            doc! {
                "$Type": "Microflows$Microflow",
                "Name": "LoadItems",
                "MicroflowReturnType": { "Entity": "Demo.Item" },
            },
        ));
        let mut microflow = grid(vec![column("attribute", "Demo.Item.Name")]);
        set_source(
            &mut microflow,
            doc! {
                "$Type": "Forms$MicroflowSource",
                "MicroflowSettings": { "Microflow": "Demo.LoadItems" },
            },
        );
        let compiler = DataGridBundleCompiler::new(&documents, "Demo.Home", &microflow);
        assert!(compiler.supported());
        assert!(compiler.render().contains("MicroflowObjectListProperty"));

        let mut association = grid(vec![column("attribute", "Demo.Item.Name")]);
        set_source(
            &mut association,
            doc! {
                "$Type": "Forms$AssociationSource",
                "EntityRef": { "Steps": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "Association": "Demo.Order_Items",
                    "DestinationEntity": "Demo.Item",
                })], 2) },
            },
        );
        let compiler = DataGridBundleCompiler::new(&documents, "Demo.Home", &association)
            .with_scope("p.Demo.Home.order");
        assert!(compiler.supported());
        let rendered = compiler.render();
        assert!(rendered.contains("AssociationObjectListProperty"));
        assert!(rendered.contains("Demo.Order_Items/Demo.Item"));
    }

    #[test]
    fn accepts_an_empty_custom_content_column() {
        let mut custom = column("customContent", "");
        custom
            .get_array_mut("Properties")
            .unwrap()
            .push(property("content", value("")));
        let documents = documents();
        let widget = grid(vec![custom]);
        let renderer = |_widgets: &[Document], _scope: &str, _entity: &str| "[]".to_string();
        let compiler = DataGridBundleCompiler::new(&documents, "Demo.Home", &widget)
            .with_widget_renderer(&renderer);
        assert!(compiler.supported());
        assert!(compiler.render().contains("TemplatedWidgetProperty"));
    }

    #[test]
    fn compiles_dynamic_custom_content_and_association_filter_columns() {
        let mut dynamic = column("dynamicText", "Demo.Item.CreatedAt");
        let mut dynamic_value = value("");
        let mut template = text("");
        template.insert(
            "Parameters",
            mxrs_bson::build_array(vec![Bson::Document(doc! {})], 2),
        );
        dynamic_value.insert("TextTemplate", template);
        dynamic
            .get_array_mut("Properties")
            .unwrap()
            .push(property("dynamic", dynamic_value));

        let mut custom = column("customContent", "");
        let mut content = value("");
        content.insert(
            "Widgets",
            mxrs_bson::build_array(vec![Bson::Document(doc! { "Name": "Child" })], 2),
        );
        custom
            .get_array_mut("Properties")
            .unwrap()
            .push(property("content", content));

        let mut association = column("attribute", "Demo.Category.Caption");
        let properties = association.get_array_mut("Properties").unwrap();
        let attribute = properties[2]
            .as_document_mut()
            .unwrap()
            .get_document_mut("Value")
            .unwrap()
            .get_document_mut("AttributeRef")
            .unwrap();
        attribute.insert(
            "EntityRef",
            doc! { "Steps": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "Association": "Demo.Item_Category",
                "DestinationEntity": "Demo.Category",
            })], 2) },
        );
        let filter_widget = doc! {
            "Name": "associationFilter",
            "Object": {
                "$Type": "CustomWidgets$WidgetObject",
                "Properties": mxrs_bson::build_array(vec![property("multi", value("true"))], 2),
            },
        };
        let mut filter_value = value("");
        filter_value.insert(
            "Widgets",
            mxrs_bson::build_array(vec![Bson::Document(filter_widget)], 2),
        );
        properties.push(property("filter", filter_value));

        let documents = documents();
        let widget = grid(vec![dynamic, custom, association]);
        let renderer = |widgets: &[Document], scope: &str, entity: &str| {
            format!("[nested({}, {scope:?}, {entity:?})]", widgets.len())
        };
        let compiler = DataGridBundleCompiler::new(&documents, "Demo.Home", &widget)
            .with_widget_renderer(&renderer);
        assert!(compiler.supported());
        let rendered = compiler.render();
        for expected in [
            "_format",
            "TemplatedWidgetProperty",
            "ListAssociationProperty",
            "\"type\": \"ReferenceSet\"",
            "Demo.Item_Category",
            "filterAssociationOptions",
            "filterAssociationOptionLabel",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected}: {rendered}"
            );
        }
    }

    #[test]
    fn rejects_unknown_widget_and_unsupported_column() {
        let documents = documents();
        let mut widget = grid(vec![column("attribute", "")]);
        assert!(!DataGridBundleCompiler::new(&documents, "Demo.Home", &widget).supported());
        widget
            .get_document_mut("Type")
            .unwrap()
            .insert("WidgetId", "example.Other");
        assert!(!DataGridBundleCompiler::new(&documents, "Demo.Home", &widget).supported());
    }
}
