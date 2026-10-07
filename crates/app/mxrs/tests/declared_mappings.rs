//! An export mapping the project declares, with the JSON structure it maps,
//! is the mapping the boundary applies: `mxrs::mapping::declared` reads it
//! from the declarations.

use std::collections::BTreeMap;

use mxrs::ports::{FlowValue, ObjectRef};
use mxrs::prelude::*;
use mxrs::{Store, StoreSchema};

#[declaration(module = "Integration")]
pub fn json_order(module: &mut ModuleBuilder) {
    module.json_structure(
        "JSON_Order",
        r#"{"number": "A-1", "lines": [{"sku": "S"}]}"#,
        |_json| {},
    );
}

#[declaration(module = "Sales")]
pub fn em_order(module: &mut ModuleBuilder) {
    module.export_mapping("EM_Order", "Integration.JSON_Order", |mapping| {
        mapping.object("(Object)", "Sales.Order", |order| {
            order.value("number").attribute("OrderNumber");
            order.array("lines", |lines| {
                lines.object("(Object)", "Sales.Line", |line| {
                    line.value("sku");
                });
            });
        });
    });
}

#[test]
fn a_declared_export_mapping_is_the_one_applied() {
    let mapping = mxrs::mapping::declared(env!("CARGO_CRATE_NAME"), "Sales.EM_Order");
    let root = mapping.root();
    assert_eq!(root.entity(), "Sales.Order");
    assert!(!root.is_multiple());
    assert_eq!(root.values()[0].key(), "number");
    assert_eq!(root.values()[0].attribute(), "OrderNumber");
    let lines = &root.children()[0];
    assert_eq!(lines.key(), "lines");
    assert_eq!(lines.association(), "Sales.Line_Order");
    assert!(lines.is_multiple());

    let schema = StoreSchema::default()
        .entity("Sales.Order", BTreeMap::new(), false)
        .entity("Sales.Line", BTreeMap::new(), false);
    let mut store = Store::new(schema);
    let order = store.create("Sales.Order").unwrap();
    store
        .set_member("Sales.Order", &order.id, "OrderNumber", "A-7".into())
        .unwrap();
    let document = mapping.apply(
        &store,
        &FlowValue::Object(ObjectRef {
            entity: "Sales.Order".into(),
            id: order.id,
        }),
    );
    assert_eq!(document["number"], "A-7");
}

#[test]
#[should_panic(expected = "export mapping Sales.EM_Missing is not declared")]
fn a_mapping_nothing_declares_is_named() {
    mxrs::mapping::declared(env!("CARGO_CRATE_NAME"), "Sales.EM_Missing");
}
