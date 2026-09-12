//! Property bundle for the official React Image widget.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document, extract_id, parse_array};

use crate::image_format::image_format;

pub const IMAGE_WIDGET_ID: &str = "com.mendix.widget.web.image.Image";

type ActionRenderer<'a> = dyn Fn(&Document) -> Option<String> + 'a;

#[derive(Debug, Clone, Default)]
pub struct StaticImageOptions {
    pub width_unit: String,
    pub width: Option<i64>,
    pub height_unit: String,
    pub height: Option<i64>,
    pub responsive: bool,
}

#[derive(Debug, Clone)]
struct PropertyValue {
    key: String,
    kind: String,
    value: Document,
}

pub struct ImageBundleCompiler<'a> {
    documents: &'a [(String, Document)],
    page_name: &'a str,
    widget: &'a Document,
    scope: Option<&'a str>,
    key_prefix: &'a str,
    action_renderer: Option<&'a ActionRenderer<'a>>,
    values: Vec<PropertyValue>,
}

impl<'a> ImageBundleCompiler<'a> {
    pub fn new(
        documents: &'a [(String, Document)],
        page_name: &'a str,
        widget: &'a Document,
    ) -> Self {
        Self {
            documents,
            page_name,
            widget,
            scope: None,
            key_prefix: "p",
            action_renderer: None,
            values: property_values(widget),
        }
    }

    pub fn with_scope(mut self, scope: &'a str) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn with_key_prefix(mut self, key_prefix: &'a str) -> Self {
        self.key_prefix = key_prefix;
        self
    }

    pub fn with_action_renderer(mut self, renderer: &'a ActionRenderer<'a>) -> Self {
        self.action_renderer = Some(renderer);
        self
    }

    pub fn supported(&self) -> bool {
        widget_id(self.widget).as_deref() == Some(IMAGE_WIDGET_ID)
            && self.supported_source()
            && self.supported_action()
    }

    pub fn render(&self) -> String {
        let mut properties = self.primitive_properties();
        let key = self.widget_key();
        set(&mut properties, "key", js_string(&key));
        set(&mut properties, "$widgetId", js_string(&key));
        set(
            &mut properties,
            "alternativeText",
            expression(&self.text_value("alternativeText")),
        );
        set(&mut properties, "class", js_string(&css_class(self.widget)));
        if self.primitive("datasource") == Some("image") {
            let uri = self.image_uri().unwrap_or_default();
            set(
                &mut properties,
                "imageObject",
                format!(
                    "WebStaticImageProperty({})",
                    js_object(&[("image", js_object(&[("uri", js_string(&uri))]),)])
                ),
            );
            set(
                &mut properties,
                "imageUrl",
                expression(&self.text_value("imageUrl")),
            );
        } else if let Some(url) = self.dynamic_image_url() {
            set(&mut properties, "imageUrl", url);
        }
        if let Some(action) = self.compiled_action() {
            set(&mut properties, "onClick", action);
        }
        format!(
            "React.createElement($Image, {})",
            js_object_owned(&properties)
        )
    }

    pub fn render_static(
        key: &str,
        css_class: &str,
        uri: &str,
        options: &StaticImageOptions,
    ) -> String {
        let unit = |value: &str| {
            if value.is_empty() {
                "auto".to_string()
            } else {
                value.to_ascii_lowercase()
            }
        };
        let properties = vec![
            ("key".to_string(), js_string(key)),
            ("$widgetId".to_string(), js_string(key)),
            ("datasource".to_string(), js_string("image")),
            (
                "imageObject".to_string(),
                format!(
                    "WebStaticImageProperty({})",
                    js_object(&[("image", js_object(&[("uri", js_string(uri))]),)])
                ),
            ),
            ("imageUrl".to_string(), expression("")),
            ("isBackgroundImage".to_string(), "false".to_string()),
            ("onClickType".to_string(), js_string("action")),
            ("alternativeText".to_string(), expression("")),
            (
                "widthUnit".to_string(),
                js_string(&unit(&options.width_unit)),
            ),
            (
                "width".to_string(),
                options.width.unwrap_or(100).to_string(),
            ),
            (
                "heightUnit".to_string(),
                js_string(&unit(&options.height_unit)),
            ),
            (
                "height".to_string(),
                options.height.unwrap_or(100).to_string(),
            ),
            ("iconSize".to_string(), "14".to_string()),
            ("displayAs".to_string(), js_string("fullImage")),
            ("responsive".to_string(), options.responsive.to_string()),
            ("minHeightUnit".to_string(), js_string("none")),
            ("minHeight".to_string(), "0".to_string()),
            ("maxHeightUnit".to_string(), js_string("none")),
            ("maxHeight".to_string(), "0".to_string()),
            ("class".to_string(), js_string(css_class)),
        ];
        format!(
            "React.createElement($Image, {})",
            js_object_owned(&properties)
        )
    }

    fn supported_source(&self) -> bool {
        match self.primitive("datasource") {
            Some("image") => self.image_uri().is_some(),
            Some("imageUrl") => self.dynamic_image_url().is_some(),
            _ => false,
        }
    }

    fn supported_action(&self) -> bool {
        let action = self
            .value("onClick")
            .and_then(|value| value.get_document("Action").ok());
        action.is_none_or(|action| {
            action.get_str("$Type").ok() == Some("Forms$NoAction")
                || self
                    .action_renderer
                    .is_some_and(|renderer| renderer(action).is_some())
        })
    }

    fn compiled_action(&self) -> Option<String> {
        let action = self
            .value("onClick")
            .and_then(|value| value.get_document("Action").ok())?;
        if action.get_str("$Type").ok() == Some("Forms$NoAction") {
            return None;
        }
        self.action_renderer.and_then(|renderer| renderer(action))
    }

    fn dynamic_image_url(&self) -> Option<String> {
        let parameter = self.image_url_parameter()?;
        let reference = parameter.get_document("AttributeRef").ok()?;
        let attribute = reference.get_str("Attribute").ok()?;
        if self.scope.is_none() || !attribute.contains('.') {
            return None;
        }
        let path = reference
            .get_document("EntityRef")
            .ok()
            .map(entity_ref_path)
            .unwrap_or_default();
        let name = attribute.rsplit_once('.').map(|(_, name)| name)?;
        let member = [path.as_str(), name]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        Some(format!(
            "ExpressionProperty({})",
            js_object(&[(
                "expression",
                js_object(&[
                    (
                        "expr",
                        js_object(&[
                            ("type", js_string("variable")),
                            ("variable", js_string("currentObject")),
                            ("path", js_string(&member)),
                        ]),
                    ),
                    (
                        "args",
                        js_object(&[(
                            "currentObject",
                            js_object(&[
                                ("widget", js_string(self.scope.unwrap_or_default())),
                                ("source", js_string("object")),
                            ]),
                        )]),
                    ),
                ]),
            )])
        ))
    }

    fn image_url_parameter(&self) -> Option<Document> {
        let template = self.value("imageUrl")?.get_document("TextTemplate").ok()?;
        let parameters = array_docs(template, "Parameters");
        (parameters.len() == 1).then(|| parameters[0].clone())
    }

    fn image_uri(&self) -> Option<String> {
        let reference = self.value("imageObject")?.get_str("Image").ok()?;
        let mut parts = reference.splitn(3, '.');
        let module = parts.next()?;
        let collection = parts.next()?;
        let name = parts.next()?;
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
        let format = image_format(&image).ok()?;
        Some(format!("img/{module}${collection}${name}.{format}"))
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

    fn text_value(&self, key: &str) -> String {
        translated_text(
            self.value(key)
                .and_then(|value| value.get_document("TextTemplate").ok()),
        )
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

fn entity_ref_path(reference: &Document) -> String {
    array_docs(reference, "Steps")
        .iter()
        .flat_map(|step| {
            [
                step.get_str("Association").ok(),
                step.get_str("DestinationEntity").ok(),
            ]
        })
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
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
                    js_object(&[("type", js_string("literal")), ("value", js_string(value))]),
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
    use mxrs_bson::{Binary, BinarySubtype, doc};

    use super::*;

    fn property_type(id: &str, key: &str, kind: &str) -> Bson {
        Bson::Document(doc! {
            "$ID": id,
            "PropertyKey": key,
            "ValueType": { "Type": kind },
        })
    }

    fn property(id: &str, mut value: Document) -> Bson {
        value
            .entry("Widgets")
            .or_insert(Bson::Array(mxrs_bson::build_array(Vec::new(), 2)));
        Bson::Document(doc! { "TypePointer": id, "Value": value })
    }

    fn widget(properties: Vec<Bson>) -> Document {
        let types = vec![
            property_type("source", "datasource", "Enumeration"),
            property_type("object", "imageObject", "Image"),
            property_type("url", "imageUrl", "TextTemplate"),
            property_type("alt", "alternativeText", "TextTemplate"),
            property_type("responsive", "responsive", "Boolean"),
            property_type("click", "onClick", "Action"),
        ];
        doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Name": "logo",
            "Appearance": { "Class": "hero" },
            "Type": {
                "WidgetId": IMAGE_WIDGET_ID,
                "ObjectType": { "$ID": "image-object", "PropertyTypes": mxrs_bson::build_array(types, 2) },
            },
            "Object": {
                "TypePointer": "image-object",
                "Properties": mxrs_bson::build_array(properties, 2),
            },
        }
    }

    fn template(text: &str, parameters: Vec<Bson>) -> Document {
        doc! {
            "TextTemplate": {
                "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "LanguageCode": "en_US", "Text": text,
                })], 3) },
                "Parameters": mxrs_bson::build_array(parameters, 2),
            },
        }
    }

    #[test]
    fn compiles_static_and_standalone_images() {
        let image = Binary {
            subtype: BinarySubtype::Generic,
            bytes: b"\x89PNG\r\n\x1A\nrest".to_vec(),
        };
        let documents = vec![(
            "Demo".to_string(),
            doc! {
                "$Type": "Images$ImageCollection",
                "Name": "Brand",
                "Images": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "Name": "Logo", "Image": image,
                })], 2),
            },
        )];
        let widget = widget(vec![
            property("source", doc! { "PrimitiveValue": "image" }),
            property("object", doc! { "Image": "Demo.Brand.Logo" }),
            property("url", template("", Vec::new())),
            property("alt", template("Company", Vec::new())),
            property("responsive", doc! { "PrimitiveValue": "true" }),
        ]);
        let compiler = ImageBundleCompiler::new(&documents, "Demo.Home", &widget);
        assert!(compiler.supported());
        let rendered = compiler.render();
        for expected in [
            "React.createElement($Image",
            "WebStaticImageProperty",
            "img/Demo$Brand$Logo.png",
            "Company",
            "mx-name-logo hero",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected}: {rendered}"
            );
        }

        let rendered = ImageBundleCompiler::render_static(
            "p.Demo.Home.logo",
            "logo",
            "img/logo.svg",
            &StaticImageOptions {
                responsive: true,
                ..StaticImageOptions::default()
            },
        );
        assert!(rendered.contains("\"widthUnit\": \"auto\""));
        assert!(rendered.contains("\"responsive\": true"));
    }

    #[test]
    fn compiles_dynamic_url_path_and_optional_action() {
        let parameter = Bson::Document(doc! {
            "AttributeRef": {
                "Attribute": "Demo.Photo.Url",
                "EntityRef": { "Steps": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "Association": "Demo.Order_Photo",
                    "DestinationEntity": "Demo.Photo",
                })], 2) },
            },
        });
        let widget = widget(vec![
            property("source", doc! { "PrimitiveValue": "imageUrl" }),
            property("url", template("{1}", vec![parameter])),
            property("alt", template("Photo", Vec::new())),
            property(
                "click",
                doc! { "Action": { "$Type": "Forms$CallMicroflowAction" } },
            ),
        ]);
        let documents = Vec::new();
        let action = |_action: &Document| Some("ActionProperty({})".to_string());
        let compiler = ImageBundleCompiler::new(&documents, "Demo.Home", &widget)
            .with_scope("p.Demo.Home.view")
            .with_action_renderer(&action);
        assert!(compiler.supported());
        let rendered = compiler.render();
        assert!(rendered.contains("Demo.Order_Photo/Demo.Photo/Url"));
        assert!(rendered.contains("ActionProperty({})"));
    }
}
