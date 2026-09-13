//! Validates the Forms codec against a document produced by mxrb's *own*
//! `Forms::MprCodec` (via `Mxrb::Forms.build`), rather than mxrb's DSL/
//! `writer.rb` path — see the comment on `real_fixture.rs` for why that
//! distinction matters: mxrb's DSL-generated pages go through a separate,
//! older hand-rolled writer that even mxrb's own `Forms::MprCodec` doesn't
//! fully decode (exporter.rb explicitly rescues and falls back for them).
//! This fixture is in the shape `Forms::MprCodec` is actually designed for.

use std::rc::Rc;

use mxrs_forms::{Catalog, MprCodec, node::Value};

#[test]
fn decodes_and_round_trips_a_native_forms_mprcodec_document() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native_page.bson"
    ))
    .unwrap();
    let document = mxrs_bson::parse(&bytes).unwrap();

    let catalog = Rc::new(Catalog::for_version("11.12.1").unwrap());
    let codec = MprCodec::new(catalog);

    let node = codec.decode(&document).unwrap();
    assert_eq!(node.schema_type().name, "Page");
    assert!(matches!(node.fetch("name").unwrap(), Some(Value::String(s)) if s == "OrderOverview"));

    let re_encoded = codec.decode(&codec.encode(&node).unwrap()).unwrap();
    assert_eq!(node, re_encoded);
}

#[test]
fn custom_widget_round_trips_through_the_forms_codec() {
    let catalog = Rc::new(Catalog::for_version("11.12.1").unwrap());
    let codec = MprCodec::new(catalog.clone());
    let mut column = mxrs_forms::Node::new("LayoutGridColumn", catalog).unwrap();
    let widget_type = mxrs_pluggable::WidgetType {
        id: "test.empty".to_string(),
        name: "Empty".to_string(),
        description: String::new(),
        prompt: String::new(),
        studio_pro_category: String::new(),
        studio_category: String::new(),
        platform: "Web".to_string(),
        offline: false,
        needs_context: false,
        plugin: false,
        help_url: String::new(),
        object_type: mxrs_pluggable::ObjectType::default(),
    };
    column
        .set(
            "widgets",
            Value::List(vec![Value::Pluggable(Box::new(
                mxrs_pluggable::WidgetNode::new(widget_type, mxrs_pluggable::ObjectNode::new()),
            ))]),
        )
        .unwrap();

    let encoded = codec.encode(&column).unwrap();
    let decoded = codec.decode(&encoded).unwrap();
    assert_eq!(decoded, column);
}
