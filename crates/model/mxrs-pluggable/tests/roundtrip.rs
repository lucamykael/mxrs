use std::convert::Infallible;

use mxrs_bson::{Document, doc};
use mxrs_forms_refs::{AttributeReference, EntityReference};
use mxrs_pluggable::{
    Assignment, DataSource, DataSourceValue, EmbeddedFormsDecoder, EmbeddedFormsEncoder,
    ObjectNode, ObjectType, PropertyType, ReferenceTarget, Value, ValueType, WidgetItem,
    WidgetNode, WidgetType, decode_widget, encode_widget,
};

struct DocumentCodec;

impl EmbeddedFormsDecoder for DocumentCodec {
    type Node = Document;
    type Error = Infallible;

    fn decode_embedded(&self, document: &Document, _path: &str) -> Result<Document, Infallible> {
        Ok(document.clone())
    }
}

impl EmbeddedFormsEncoder for DocumentCodec {
    type Node = Document;
    type Error = Infallible;

    fn encode_embedded(&self, node: &Document, _path: &str) -> Result<Document, Infallible> {
        Ok(node.clone())
    }
}

fn value_type(kind: &str) -> ValueType {
    ValueType {
        kind: kind.to_string(),
        list: false,
        linked: false,
        metadata: false,
        entity_property: String::new(),
        allow_non_persistable_entities: false,
        path_kind: "No".to_string(),
        path_type: "None".to_string(),
        parameter_list: false,
        multiline: false,
        default_value: String::new(),
        required: false,
        on_change_property: String::new(),
        data_source_property: String::new(),
        selectable_objects_property: String::new(),
        attribute_types: Vec::new(),
        association_types: Vec::new(),
        selection_types: Vec::new(),
        enumeration_values: Vec::new(),
        action_variables: Vec::new(),
        object_type: None,
        return_type: None,
        translations: Vec::new(),
        set_label: false,
        default_type: "None".to_string(),
        allow_upload: false,
    }
}

fn property(key: &str, kind: &str) -> PropertyType {
    PropertyType {
        key: key.to_string(),
        category: "General".to_string(),
        caption: key.to_string(),
        description: format!("{key} description"),
        prompt: format!("{key} prompt"),
        default: false,
        value_type: value_type(kind),
    }
}

fn widget_type(id: &str, properties: Vec<PropertyType>) -> WidgetType {
    WidgetType {
        id: id.to_string(),
        name: id.to_string(),
        description: "round-trip fixture".to_string(),
        prompt: "Choose".to_string(),
        studio_pro_category: "Tests".to_string(),
        studio_category: "Tests".to_string(),
        platform: "Web".to_string(),
        offline: true,
        needs_context: true,
        plugin: false,
        help_url: "https://example.invalid/widget".to_string(),
        object_type: ObjectType { properties },
    }
}

fn embedded(type_name: &str) -> Document {
    doc! { "$ID": "embedded-id", "$Type": type_name, "Name": "embedded" }
}

#[test]
fn every_value_kind_round_trips_with_schema_and_embedded_nodes() {
    let nested_property = property("nestedText", "String");
    let nested_schema = ObjectType {
        properties: vec![nested_property.clone()],
    };
    let mut object_property = property("objects", "Object");
    object_property.value_type.list = true;
    object_property.value_type.object_type = Some(Box::new(nested_schema));

    let nested_widget_property = property("childText", "String");
    let nested_widget_type = widget_type("test.child", vec![nested_widget_property.clone()]);
    let mut nested_widget_object = ObjectNode::new();
    nested_widget_object.push(Assignment {
        property: nested_widget_property,
        value: Value::Primitive("child".to_string()),
        source_variable: None,
    });

    let mut properties = vec![
        property("boolean", "Boolean"),
        property("integer", "Integer"),
        property("decimal", "Decimal"),
        property("string", "String"),
        property("enumeration", "Enumeration"),
        property("selection", "Selection"),
        property("expression", "Expression"),
        property("constraint", "EntityConstraint"),
        property("attribute", "Attribute"),
        property("entity", "Entity"),
        property("associationAttribute", "Association"),
        property("associationEntity", "Association"),
        property("text", "TranslatableString"),
        property("template", "TextTemplate"),
        property("action", "Action"),
        property("emptyAction", "Action"),
        property("icon", "Icon"),
        property("dataSource", "DataSource"),
        object_property,
        property("widgets", "Widgets"),
        property("system", "System"),
        property("file", "File"),
        property("form", "Form"),
        property("image", "Image"),
        property("microflow", "Microflow"),
        property("nanoflow", "Nanoflow"),
    ];
    let schema = widget_type("test.every-kind", properties.clone());

    let attribute = AttributeReference {
        attribute: "Sales.Order/Number".to_string(),
        entity_reference: Some(EntityReference::direct("Sales.Order")),
    };
    let entity = EntityReference::direct("Sales.Order");
    let mut nested_object = ObjectNode::new();
    nested_object.push(Assignment {
        property: nested_property,
        value: Value::Primitive("nested".to_string()),
        source_variable: None,
    });
    let values = vec![
        Value::Boolean(true),
        Value::Integer(42),
        Value::Primitive("12.50".to_string()),
        Value::Primitive("hello".to_string()),
        Value::Primitive("Open".to_string()),
        Value::Selection("All".to_string()),
        Value::Expression("$currentObject/Name".to_string()),
        Value::XPathConstraint("[Active = true()]".to_string()),
        Value::AttributeReference(attribute.clone()),
        Value::EntityReference(entity.clone()),
        Value::Reference {
            kind: "Association".to_string(),
            target: ReferenceTarget::Attribute(attribute),
        },
        Value::Reference {
            kind: "Association".to_string(),
            target: ReferenceTarget::Entity(entity.clone()),
        },
        Value::Text(vec![("en_US".to_string(), "Hello".to_string())]),
        Value::TextTemplate(embedded("Texts$TextTemplate")),
        Value::Action(embedded("Forms$CallMicroflowAction")),
        Value::Null,
        Value::Icon(embedded("Forms$GlyphIcon")),
        Value::DataSource(Some(DataSourceValue::XPath(DataSource {
            entity: Some(entity),
            constraint: "[Active = true()]".to_string(),
            sort_bar: Some(embedded("Forms$GridSortBar")),
            source_variable: Some(embedded("Forms$WidgetSource")),
            force_full_objects: true,
        }))),
        Value::ObjectList(vec![nested_object]),
        Value::Widgets(vec![
            WidgetItem::Native(embedded("Forms$DynamicText")),
            WidgetItem::Pluggable(Box::new(WidgetNode::new(
                nested_widget_type,
                nested_widget_object,
            ))),
        ]),
        Value::System,
        Value::Reference {
            kind: "File".to_string(),
            target: ReferenceTarget::Path("Images.Logo".to_string()),
        },
        Value::Reference {
            kind: "Form".to_string(),
            target: ReferenceTarget::Path("Sales.Order_NewEdit".to_string()),
        },
        Value::Reference {
            kind: "Image".to_string(),
            target: ReferenceTarget::Path("Images.Banner".to_string()),
        },
        Value::Reference {
            kind: "Microflow".to_string(),
            target: ReferenceTarget::Path("Sales.ACT_Save".to_string()),
        },
        Value::Reference {
            kind: "Nanoflow".to_string(),
            target: ReferenceTarget::Path("Sales.NAN_Refresh".to_string()),
        },
    ];
    assert_eq!(properties.len(), values.len());

    let mut object = ObjectNode::new();
    for (index, (property, value)) in properties.drain(..).zip(values).enumerate() {
        object.push(Assignment {
            property,
            value,
            source_variable: (index == 3)
                .then(|| doc! { "$ID": "source-id", "$Type": "Forms$WidgetSource" }),
        });
    }
    let widget = WidgetNode::new(schema, object);

    let mut encoded = encode_widget(&widget, "$", &DocumentCodec).expect("encode every kind");
    encoded.insert("Name", "widget-under-test");
    let decoded = decode_widget(&encoded, "$", &DocumentCodec).expect("decode encoded widget");
    assert_eq!(decoded, widget);
    let reencoded = encode_widget(&decoded, "$", &DocumentCodec).expect("encode decoded baseline");
    assert_eq!(reencoded.get("Type"), encoded.get("Type"));
    assert_eq!(reencoded.get_str("Name").unwrap(), "widget-under-test");
}

#[test]
fn source_variable_only_data_source_round_trips_in_value_field() {
    let data_source_property = property("source", "DataSource");
    let schema = widget_type("test.variable-source", vec![data_source_property.clone()]);
    let mut object = ObjectNode::new();
    object.push(Assignment {
        property: data_source_property,
        value: Value::DataSource(Some(DataSourceValue::XPath(DataSource {
            entity: None,
            constraint: String::new(),
            sort_bar: None,
            source_variable: Some(embedded("Forms$WidgetSource")),
            force_full_objects: false,
        }))),
        source_variable: None,
    });
    let widget = WidgetNode::new(schema, object);

    let encoded = encode_widget(&widget, "$", &DocumentCodec).unwrap();
    let decoded = decode_widget(&encoded, "$", &DocumentCodec).unwrap();
    assert_eq!(decoded, widget);
}

/// Ports a real fallback behavior of `Mxrb::Writer#pluggable_widget_doc`
/// (`lib/mxrb/writer.rb`'s `configure_fallback_data_grid!`): without the
/// real Studio-Pro-side widget package to hydrate a Data Grid 2's actual
/// property schema from, mxrb still emits a recognizable widget by writing
/// native-widget-shaped fields (a `DataSource`, here) directly onto the
/// outer `CustomWidgets$CustomWidget` document, alongside an otherwise
/// empty pluggable schema.
#[test]
fn a_freshly_authored_widget_carries_its_extra_fallback_fields_into_the_encoded_document() {
    let schema = widget_type("com.mendix.widget.web.datagrid.Datagrid", vec![]);
    let mut widget = WidgetNode::new(schema, ObjectNode::new());
    widget.extra = doc! {
        "DataSource": { "$ID": "ds-id", "$Type": "Forms$DatabaseSource", "Entity": "Sales.Order" },
    };

    let encoded = encode_widget(&widget, "$", &DocumentCodec).unwrap();
    let data_source = encoded.get_document("DataSource").unwrap();
    assert_eq!(data_source.get_str("Entity").unwrap(), "Sales.Order");
    // `Type`/`Object` still come from the real widget-type schema, not `extra`.
    assert_eq!(
        encoded
            .get_document("Type")
            .unwrap()
            .get_str("WidgetId")
            .unwrap(),
        "com.mendix.widget.web.datagrid.Datagrid"
    );
}
