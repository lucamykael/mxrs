//! Schema-driven fallback compiler for pluggable widget bundles.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use mxrs_bson::{Bson, Document, extract_id, parse_array};
use mxrs_compiler_flow::nanoflow::expression::{Expression, LiteralValue, parse_expression};
use mxrs_compiler_flow::nanoflow::js_value::JsValue;
use mxrs_compiler_support::operation_id;

type WidgetRenderer<'a> = dyn Fn(&[Document]) -> String + 'a;
type ActionRenderer<'a> = dyn Fn(&Document) -> Option<String> + 'a;
type DataSourceRenderer<'a> = dyn Fn(&Document) -> Option<String> + 'a;

#[derive(Debug, Clone)]
struct PropertyValue {
    key: String,
    kind: String,
    value: Document,
    metadata: Document,
}

/// Compiles any pluggable widget whose schema maps entirely onto standard
/// Mendix client properties and whose JavaScript module is installed.
pub struct GenericWidgetBundleCompiler<'a> {
    documents: &'a [(String, Document)],
    project_path: &'a Path,
    page_name: &'a str,
    widget: &'a Document,
    scope: Option<&'a str>,
    key_prefix: &'a str,
    renderer: Option<&'a WidgetRenderer<'a>>,
    action_renderer: Option<&'a ActionRenderer<'a>>,
    data_source_renderer: Option<&'a DataSourceRenderer<'a>>,
    package_available: Option<bool>,
    index: HashMap<String, Document>,
    values: Vec<PropertyValue>,
    component_name: String,
    module_path: String,
}

impl<'a> GenericWidgetBundleCompiler<'a> {
    pub fn new(
        documents: &'a [(String, Document)],
        project_path: &'a Path,
        page_name: &'a str,
        widget: &'a Document,
        scope: Option<&'a str>,
    ) -> Self {
        let mut index = HashMap::new();
        index_documents(widget, &mut index);
        let values = property_values(widget, &index);
        let widget_id = widget_id(widget).unwrap_or_default();
        Self {
            documents,
            project_path,
            page_name,
            widget,
            scope,
            key_prefix: "p",
            renderer: None,
            action_renderer: None,
            data_source_renderer: None,
            package_available: None,
            index,
            values,
            component_name: widget_id.rsplit('.').next().unwrap_or_default().to_string(),
            module_path: widget_id.replace('.', "/"),
        }
    }

    pub fn with_widget_renderer(mut self, renderer: &'a WidgetRenderer<'a>) -> Self {
        self.renderer = Some(renderer);
        self
    }

    pub fn with_action_renderer(mut self, renderer: &'a ActionRenderer<'a>) -> Self {
        self.action_renderer = Some(renderer);
        self
    }

    pub fn with_data_source_renderer(mut self, renderer: &'a DataSourceRenderer<'a>) -> Self {
        self.data_source_renderer = Some(renderer);
        self
    }

    pub fn with_key_prefix(mut self, prefix: &'a str) -> Self {
        self.key_prefix = prefix;
        self
    }

    pub fn with_package_available(mut self, available: bool) -> Self {
        self.package_available = Some(available);
        self
    }

    pub fn module_available(project_path: &Path, module_path: &str) -> bool {
        package_module(project_path, module_path)
    }

    pub fn component_name(&self) -> &str {
        &self.component_name
    }

    pub fn module_path(&self) -> &str {
        &self.module_path
    }

    pub fn supported(&self) -> bool {
        identifier(&self.component_name)
            && self
                .package_available
                .unwrap_or_else(|| self.package_module())
            && self
                .values
                .iter()
                .all(|property| self.supported_property(property, self.list_source(&self.values)))
    }

    pub fn render(&self) -> String {
        let list_source = self.list_source(&self.values);
        let mut properties = self.compile_values(&self.values, list_source.as_ref());
        properties.push(("key".to_string(), js_string(&self.widget_key())));
        properties.push(("$widgetId".to_string(), js_string(&self.widget_key())));
        properties.push(("class".to_string(), js_string(&css_class(self.widget))));
        format!(
            "React.createElement(${}, {})",
            self.component_name,
            js_object_owned(&properties)
        )
    }

    fn supported_property(&self, property: &PropertyValue, list_source: Option<Document>) -> bool {
        match property.kind.as_str() {
            "Boolean" | "Integer" | "Decimal" | "Enumeration" | "String" | "System"
            | "Association" | "Selection" => true,
            "DataSource" => {
                property
                    .value
                    .get("DataSource")
                    .is_none_or(|value| matches!(value, Bson::Null))
                    || self.compiled_data_source(&property.value).is_some()
            }
            "Attribute"
                if property
                    .value
                    .get("AttributeRef")
                    .is_none_or(|value| matches!(value, Bson::Null)) =>
            {
                true
            }
            "Attribute" => self
                .attribute_property(property, list_source.as_ref())
                .is_some(),
            "Icon" => property
                .value
                .get("Icon")
                .is_none_or(|value| matches!(value, Bson::Null)),
            "Image" => property
                .value
                .get_str("Image")
                .unwrap_or_default()
                .is_empty(),
            "Expression" | "TextTemplate" => self
                .expression_property(property, list_source.as_ref())
                .is_some(),
            "Action" => {
                self.no_action(&property.value) || self.compiled_action(&property.value).is_some()
            }
            "Widgets" => {
                array_docs(&property.value, "Widgets").is_empty() || self.renderer.is_some()
            }
            "Object" => array_docs(&property.value, "Objects").iter().all(|object| {
                let values = property_values(object, &self.index);
                !values.is_empty()
                    && values.iter().all(|property| {
                        self.supported_property(property, self.list_source(&values))
                    })
            }),
            _ => false,
        }
    }

    fn compile_values(
        &self,
        values: &[PropertyValue],
        list_source: Option<&Document>,
    ) -> Vec<(String, String)> {
        values
            .iter()
            .filter_map(|property| {
                self.compile_property(property, list_source)
                    .map(|value| (property.key.clone(), value))
            })
            .collect()
    }

    fn compile_property(
        &self,
        property: &PropertyValue,
        list_source: Option<&Document>,
    ) -> Option<String> {
        let primitive = || property.value.get_str("PrimitiveValue").unwrap_or_default();
        match property.kind.as_str() {
            "Boolean" => Some((primitive() == "true").to_string()),
            "Integer" => Some(primitive().parse::<i64>().unwrap_or_default().to_string()),
            "Decimal" => Some(primitive().parse::<f64>().unwrap_or_default().to_string()),
            "Enumeration" | "String" => Some(js_string(primitive())),
            "Expression" | "TextTemplate" => self.expression_property(property, list_source),
            "DataSource" => self.compiled_data_source(&property.value),
            "Action" => self.compiled_action(&property.value),
            "Attribute" => self.attribute_property(property, list_source),
            "Selection" => Some(format!(
                "SelectionProperty({})",
                js_object(&[
                    (
                        "selectionType",
                        js_string(property.value.get_str("Selection").unwrap_or("None")),
                    ),
                    ("dataSourceId", js_string(&self.widget_key())),
                ])
            )),
            "Widgets" => self.compile_widgets(property, list_source),
            "Object" => Some(self.compile_objects(&property.value)),
            "Association" | "Icon" | "Image" | "System" => None,
            _ => None,
        }
    }

    fn compile_widgets(
        &self,
        property: &PropertyValue,
        list_source: Option<&Document>,
    ) -> Option<String> {
        let widgets = array_docs(&property.value, "Widgets");
        if widgets.is_empty() {
            return Some("[]".to_string());
        }
        let rendered = self.renderer.map(|renderer| renderer(&widgets))?;
        if data_source_bound(&property.metadata) && list_source.is_some() {
            Some(format!(
                "TemplatedWidgetProperty({})",
                js_object(&[
                    ("children", format!("() => {rendered}")),
                    ("dataSourceId", js_string(&self.widget_key())),
                    ("editable", "false".to_string()),
                ])
            ))
        } else {
            Some(rendered)
        }
    }

    fn compile_objects(&self, value: &Document) -> String {
        format!(
            "[{}]",
            array_docs(value, "Objects")
                .iter()
                .map(|object| {
                    let values = property_values(object, &self.index);
                    let list_source = self.list_source(&values);
                    js_object_owned(&self.compile_values(&values, list_source.as_ref()))
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn list_source(&self, values: &[PropertyValue]) -> Option<Document> {
        values.iter().find_map(|property| {
            (property.kind == "DataSource")
                .then(|| {
                    self.compiled_data_source(&property.value)
                        .map(|_| property.metadata.clone())
                })
                .flatten()
        })
    }

    fn compiled_data_source(&self, value: &Document) -> Option<String> {
        self.data_source_renderer
            .and_then(|renderer| renderer(value))
            .or_else(|| self.database_list_property(value))
    }

    fn expression_property(
        &self,
        property: &PropertyValue,
        list_source: Option<&Document>,
    ) -> Option<String> {
        let expression = if property.kind == "Expression" {
            compile_expression(property.value.get_str("Expression").unwrap_or_default())?
        } else {
            self.compile_template(&property.value)?
        };
        let list_bound = data_source_bound(&property.metadata) && list_source.is_some();
        let variables = expression_variables(&expression);
        let args = variables
            .iter()
            .map(|variable| {
                (
                    variable.clone(),
                    js_object(&[
                        (
                            "widget",
                            if list_bound {
                                js_string(&self.widget_key())
                            } else {
                                self.scope
                                    .map(js_string)
                                    .unwrap_or_else(|| "null".to_string())
                            },
                        ),
                        ("source", js_string("object")),
                    ]),
                )
            })
            .collect::<Vec<_>>();
        let mut config = vec![(
            "expression".to_string(),
            js_object(&[("expr", expression), ("args", js_object_owned(&args))]),
        )];
        if list_bound {
            config.push(("dataSourceId".to_string(), js_string(&self.widget_key())));
        }
        Some(format!(
            "{}({})",
            if list_bound {
                "ListExpressionProperty"
            } else {
                "ExpressionProperty"
            },
            js_object_owned(&config)
        ))
    }

    fn database_list_property(&self, value: &Document) -> Option<String> {
        let source = value.get_document("DataSource").ok()?;
        if source.get_str("$Type").ok() != Some("CustomWidgets$CustomWidgetXPathSource") {
            return None;
        }
        let entity = source
            .get_document("EntityRef")
            .ok()?
            .get_str("Entity")
            .ok()?;
        if !qualified_name(entity) {
            return None;
        }
        Some(format!(
            "DatabaseObjectListProperty({})",
            js_object(&[
                ("dataSourceId", js_string(&self.widget_key())),
                ("entity", js_string(entity)),
                (
                    "operationId",
                    js_string(&operation_id(
                        self.page_name,
                        self.widget.get_str("Name").unwrap_or_default(),
                    )),
                ),
                ("sort", sort_items(source)),
            ])
        ))
    }

    fn attribute_property(
        &self,
        property: &PropertyValue,
        list_source: Option<&Document>,
    ) -> Option<String> {
        let attribute = property
            .value
            .get_document("AttributeRef")
            .ok()?
            .get_str("Attribute")
            .ok()?;
        let (entity, name) = attribute.rsplit_once('.')?;
        let list_bound = data_source_bound(&property.metadata) && list_source.is_some();
        if list_bound {
            return Some(format!(
                "ListAttributeProperty({})",
                js_object(&[
                    ("path", js_string("")),
                    ("entity", js_string(entity)),
                    ("attribute", js_string(name)),
                    ("dataSourceId", js_string(&self.widget_key())),
                    ("isList", "false".to_string()),
                ])
            ));
        }
        let scope = self.scope?;
        Some(format!(
            "AttributeProperty({})",
            js_object(&[
                ("scope", js_string(scope)),
                ("path", js_string("")),
                ("entity", js_string(entity)),
                ("attribute", js_string(name)),
                ("onChange", do_nothing()),
                ("isList", "false".to_string()),
                ("validation", "null".to_string()),
                ("formatting", "{  }".to_string()),
            ])
        ))
    }

    fn compile_template(&self, value: &Document) -> Option<String> {
        let template = value.get_document("TextTemplate").ok();
        let text = translated_text(template);
        let parameters = template
            .map(|template| array_docs(template, "Parameters"))
            .unwrap_or_default();
        if parameters.is_empty() {
            return Some(literal(&text));
        }
        let expressions = parameters
            .iter()
            .map(|parameter| self.template_parameter(parameter))
            .collect::<Option<Vec<_>>>()?;
        if text == "{1}" && expressions.len() == 1 {
            return expressions.into_iter().next();
        }
        format_expression(&text, &expressions)
    }

    fn template_parameter(&self, parameter: &Document) -> Option<String> {
        let attribute = parameter
            .get_document("AttributeRef")
            .ok()
            .and_then(|reference| reference.get_str("Attribute").ok())
            .unwrap_or_default();
        if attribute.is_empty() {
            return compile_expression(parameter.get_str("Expression").unwrap_or_default());
        }
        let name = attribute.rsplit('.').next().unwrap_or_default();
        let variable = variable(name);
        self.enumeration_name(attribute)
            .map_or(Some(variable.clone()), |enumeration| {
                Some(js_object(&[
                    ("type", js_string("function")),
                    ("name", js_string("getCaption")),
                    (
                        "parameters",
                        format!("[{variable}, {}]", literal(&enumeration)),
                    ),
                ]))
            })
    }

    fn enumeration_name(&self, attribute: &str) -> Option<String> {
        let mut parts = attribute.splitn(3, '.');
        let module = parts.next()?;
        let entity = parts.next()?;
        let name = parts.next()?;
        self.documents.iter().find_map(|(owner, document)| {
            (owner == module && document.get_str("$Type").ok() == Some("DomainModels$DomainModel"))
                .then(|| {
                    array_docs(document, "Entities")
                        .into_iter()
                        .find(|candidate| candidate.get_str("Name").ok() == Some(entity))
                        .and_then(|entity| {
                            array_docs(&entity, "Attributes")
                                .into_iter()
                                .find(|candidate| candidate.get_str("Name").ok() == Some(name))
                        })
                        .and_then(|attribute| attribute.get_document("NewType").ok().cloned())
                        .and_then(|type_| type_.get_str("Enumeration").ok().map(str::to_string))
                        .filter(|name| !name.is_empty())
                })
                .flatten()
        })
    }

    fn compiled_action(&self, value: &Document) -> Option<String> {
        let action = value.get_document("Action").ok()?;
        if action.get_str("$Type").unwrap_or_default().is_empty()
            || action.get_str("$Type").ok() == Some("Forms$NoAction")
        {
            return None;
        }
        self.action_renderer.and_then(|renderer| renderer(action))
    }

    fn no_action(&self, value: &Document) -> bool {
        value.get_document("Action").is_err()
            || value
                .get_document("Action")
                .ok()
                .and_then(|action| action.get_str("$Type").ok())
                == Some("Forms$NoAction")
    }

    fn package_module(&self) -> bool {
        package_module(self.project_path, &self.module_path)
    }

    fn widget_key(&self) -> String {
        format!(
            "{}.{}.{}",
            self.key_prefix,
            self.page_name,
            self.widget.get_str("Name").unwrap_or_default()
        )
    }
}

fn property_values(document: &Document, index: &HashMap<String, Document>) -> Vec<PropertyValue> {
    let object = document.get_document("Object").ok().unwrap_or(document);
    array_docs(object, "Properties")
        .into_iter()
        .filter_map(|property| {
            let schema = index.get(&property.get("TypePointer").and_then(extract_id)?)?;
            let metadata = schema.get_document("ValueType").ok()?.clone();
            Some(PropertyValue {
                key: schema.get_str("PropertyKey").ok()?.to_string(),
                kind: metadata.get_str("Type").unwrap_or_default().to_string(),
                value: property.get_document("Value").ok()?.clone(),
                metadata,
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
            Bson::Array(values) => values.iter().for_each(|value| {
                if let Bson::Document(document) = value {
                    index_documents(document, index);
                }
            }),
            _ => {}
        }
    }
}

fn compile_expression(source: &str) -> Option<String> {
    let value = source.trim();
    let expression = if let Some(inner) = function_argument(value, "toString") {
        Expression::Function("toString".to_string(), vec![compile_expression_ast(inner)?])
    } else {
        compile_expression_ast(value)?
    };
    Some(JsValue::from(normalize_expression_variables(expression)).render())
}

fn compile_expression_ast(source: &str) -> Option<Expression> {
    let (expression, _) = parse_expression(source);
    (!contains_opaque(&expression)).then_some(expression)
}

fn contains_opaque(expression: &Expression) -> bool {
    match expression {
        Expression::Literal(LiteralValue::Opaque(_)) => true,
        Expression::Conditional(condition, then, otherwise) => {
            contains_opaque(condition) || contains_opaque(then) || contains_opaque(otherwise)
        }
        Expression::Function(_, parameters) => parameters.iter().any(contains_opaque),
        _ => false,
    }
}

fn normalize_expression_variables(expression: Expression) -> Expression {
    match expression {
        Expression::Variable { path, .. } => Expression::Variable {
            name: "currentObject".to_string(),
            path,
        },
        Expression::Conditional(condition, then, otherwise) => Expression::Conditional(
            Box::new(normalize_expression_variables(*condition)),
            Box::new(normalize_expression_variables(*then)),
            Box::new(normalize_expression_variables(*otherwise)),
        ),
        Expression::Function(name, parameters) => Expression::Function(
            name,
            parameters
                .into_iter()
                .map(normalize_expression_variables)
                .collect(),
        ),
        expression => expression,
    }
}

fn function_argument<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    source
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn variable(path: &str) -> String {
    let mut fields = vec![
        ("type".to_string(), js_string("variable")),
        ("variable".to_string(), js_string("currentObject")),
    ];
    if !path.is_empty() {
        fields.push(("path".to_string(), js_string(path)));
    }
    js_object_owned(&fields)
}

fn literal(value: &str) -> String {
    literal_value(&js_string(value))
}

fn literal_value(value: &str) -> String {
    js_object(&[("type", js_string("literal")), ("value", value.to_string())])
}

fn format_expression(text: &str, expressions: &[String]) -> Option<String> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        if start > 0 {
            pieces.push(literal(&rest[..start]));
        }
        let end = rest[start..].find('}')? + start;
        let index = rest[start + 1..end].parse::<usize>().ok()?;
        pieces.push(expressions.get(index.checked_sub(1)?)?.clone());
        rest = &rest[end + 1..];
    }
    if !rest.is_empty() {
        pieces.push(literal(rest));
    }
    pieces.into_iter().reduce(|left, right| {
        js_object(&[
            ("type", js_string("function")),
            ("name", js_string("+")),
            ("parameters", format!("[{left}, {right}]")),
        ])
    })
}

fn expression_variables(expression: &str) -> Vec<String> {
    if expression.contains("\"variable\": \"currentObject\"") {
        vec!["currentObject".to_string()]
    } else {
        Vec::new()
    }
}

fn sort_items(source: &Document) -> String {
    let items = source
        .get_document("SortBar")
        .ok()
        .map(|bar| array_docs(bar, "SortItems"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let attribute = item
                .get_document("AttributeRef")
                .ok()?
                .get_str("Attribute")
                .ok()?
                .rsplit('.')
                .next()?;
            identifier(attribute).then(|| {
                format!(
                    "[{}, {}]",
                    js_string(attribute),
                    js_string(
                        if item
                            .get_str("SortOrder")
                            .unwrap_or_default()
                            .eq_ignore_ascii_case("descending")
                        {
                            "desc"
                        } else {
                            "asc"
                        }
                    )
                )
            })
        })
        .collect::<Vec<_>>();
    format!("[{}]", items.join(", "))
}

fn data_source_bound(metadata: &Document) -> bool {
    !metadata
        .get_str("DataSourceProperty")
        .unwrap_or_default()
        .is_empty()
}

fn archive_contains(path: &Path, entry: &str) -> bool {
    fs::read(path).is_ok_and(|bytes| {
        bytes
            .windows(entry.len())
            .any(|window| window == entry.as_bytes())
    })
}

fn package_module(project_path: &Path, module_path: &str) -> bool {
    let root = project_path.parent().unwrap_or_else(|| Path::new("."));
    let entry = format!("{module_path}.mjs");
    if root.join("widgets").join(&entry).is_file() {
        return true;
    }
    let Ok(entries) = fs::read_dir(root.join("widgets")) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry_path| {
        let path = entry_path.path();
        path.extension().is_some_and(|extension| extension == "mpk")
            && archive_contains(&path, &entry)
    })
}

fn identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
}

fn qualified_name(value: &str) -> bool {
    value.split('.').count() >= 2 && value.split('.').all(identifier)
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

fn array_docs(document: &Document, field: &str) -> Vec<Document> {
    parse_array(document.get_array(field).ok().map(Vec::as_slice))
        .items
        .into_iter()
        .filter_map(|value| value.as_document().cloned())
        .collect()
}

fn do_nothing() -> String {
    js_object(&[
        ("type", js_string("doNothing")),
        ("argMap", "{  }".to_string()),
        ("config", "{  }".to_string()),
        ("disabledDuringExecution", "true".to_string()),
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
    use tempfile::tempdir;

    use super::*;

    fn property_type(id: &str, key: &str, kind: &str, source: Option<&str>) -> Bson {
        let mut value_type = doc! { "Type": kind };
        if let Some(source) = source {
            value_type.insert("DataSourceProperty", source);
        }
        Bson::Document(doc! {
            "$ID": id, "PropertyKey": key, "ValueType": value_type,
        })
    }

    fn property(id: &str, value: Document) -> Bson {
        Bson::Document(doc! { "TypePointer": id, "Value": value })
    }

    fn generic_widget(types: Vec<Bson>, properties: Vec<Bson>) -> Document {
        doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Name": "language",
            "Appearance": { "Class": "language-picker" },
            "Type": {
                "WidgetId": "example.LanguageSelector",
                "ObjectType": { "$ID": "generic-object", "PropertyTypes": mxrs_bson::build_array(types, 2) },
            },
            "Object": {
                "TypePointer": "generic-object",
                "Properties": mxrs_bson::build_array(properties, 2),
            },
        }
    }

    #[test]
    fn compiles_primitives_data_text_action_content_and_nested_objects() {
        let temp = tempdir().unwrap();
        let widgets = temp.path().join("widgets/example");
        fs::create_dir_all(&widgets).unwrap();
        fs::write(widgets.join("LanguageSelector.mjs"), "export default {};").unwrap();
        let types = vec![
            property_type("enabled", "enabled", "Boolean", None),
            property_type("options", "options", "DataSource", None),
            property_type("label", "label", "Expression", Some("options")),
            property_type("caption", "caption", "TextTemplate", None),
            property_type("action", "onClick", "Action", None),
            property_type("content", "content", "Widgets", Some("options")),
            property_type("config", "config", "Object", None),
            property_type("name", "name", "String", None),
        ];
        let mut options = doc! {};
        options.insert(
            "DataSource",
            doc! {
                "$Type": "CustomWidgets$CustomWidgetXPathSource",
                "EntityRef": { "Entity": "System.Language" },
                "SortBar": { "SortItems": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "AttributeRef": { "Attribute": "System.Language.Description" },
                    "SortOrder": "Ascending",
                })], 2) },
            },
        );
        let mut content = doc! {};
        content.insert(
            "Widgets",
            mxrs_bson::build_array(
                vec![Bson::Document(doc! { "$Type": "Forms$DynamicText" })],
                2,
            ),
        );
        let nested = doc! {
            "$Type": "CustomWidgets$WidgetObject",
            "Properties": mxrs_bson::build_array(vec![property("name", doc! {
                "PrimitiveValue": "Series",
            })], 2),
        };
        let widget = generic_widget(
            types,
            vec![
                property("enabled", doc! { "PrimitiveValue": "true" }),
                property("options", options),
                property("label", doc! { "Expression": "$Item/Description" }),
                property(
                    "caption",
                    doc! { "TextTemplate": {
                        "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                            "LanguageCode": "en_US", "Text": "Choose language",
                        })], 3) },
                        "Parameters": mxrs_bson::build_array(Vec::new(), 2),
                    } },
                ),
                property(
                    "action",
                    doc! { "Action": { "$Type": "Forms$SignOutClientAction" } },
                ),
                property("content", content),
                property(
                    "config",
                    doc! {
                        "Objects": mxrs_bson::build_array(vec![Bson::Document(nested)], 2),
                    },
                ),
            ],
        );
        let project = temp.path().join("App.mpr");
        let documents = Vec::new();
        let render_widgets = |_widgets: &[Document]| "[DynamicText]".to_string();
        let render_action = |_action: &Document| {
            Some("ActionProperty({ action: { type: \"signOut\" } })".to_string())
        };
        let compiler = GenericWidgetBundleCompiler::new(
            &documents,
            &project,
            "Demo.Home",
            &widget,
            Some("p.Demo.Home.context"),
        )
        .with_widget_renderer(&render_widgets)
        .with_action_renderer(&render_action);
        assert!(compiler.supported());
        assert_eq!(compiler.component_name(), "LanguageSelector");
        assert_eq!(compiler.module_path(), "example/LanguageSelector");
        let rendered = compiler.render();
        for expected in [
            "React.createElement($LanguageSelector",
            "\"enabled\": true",
            "DatabaseObjectListProperty",
            "System.Language",
            "ListExpressionProperty",
            "Choose language",
            "ActionProperty",
            "TemplatedWidgetProperty",
            "DynamicText",
            "\"name\": \"Series\"",
            "mx-name-language language-picker",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected}: {rendered}"
            );
        }
    }

    #[test]
    fn fails_closed_and_covers_expression_and_package_fallbacks() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("App.mpr");
        let widget = generic_widget(
            vec![property_type("bad", "bad", "Unknown", None)],
            vec![property("bad", doc! {})],
        );
        let documents = Vec::new();
        assert!(
            !GenericWidgetBundleCompiler::new(&documents, &project, "Demo.Home", &widget, None,)
                .supported()
        );

        for (source, expected) in [
            ("", "\"value\": null"),
            ("$Item/Name", "\"path\": \"Name\""),
            ("-1.5", "literalNumeric"),
            ("true", "\"value\": true"),
            ("'text'", "\"value\": \"text\""),
            ("toString(false)", "\"name\": \"toString\""),
            (
                "if $Item/Active = true then 'yes' else 'no'",
                "\"type\": \"conditional\"",
            ),
            (
                "$Item/Sales.Order_Customer/Sales.Customer/Name",
                "Sales.Order_Customer/Sales.Customer/Name",
            ),
        ] {
            assert!(compile_expression(source).unwrap().contains(expected));
        }
        assert!(compile_expression("invalid +").is_none());

        fs::create_dir_all(temp.path().join("widgets")).unwrap();
        fs::write(
            temp.path().join("widgets/packed.mpk"),
            b"zip-prefix example/LanguageSelector.mjs zip-suffix",
        )
        .unwrap();
        let widget = generic_widget(Vec::new(), Vec::new());
        let compiler =
            GenericWidgetBundleCompiler::new(&documents, &project, "Demo.Home", &widget, None);
        assert!(compiler.supported());
    }
}
