//! Exercises `#[derive(MxEntity)]` (M8.3) through the real pipeline:
//! derive expansion → `mxrs_dsl::ModuleBuilder::entity` call → written via
//! `mxrs-writer` → read back via `mxrs-model` — the same end-to-end
//! standard `project! {}` (M8.1) is held to, proving this second front end
//! genuinely produces the same kind of output, not a parallel shape.

use mxrs_macros::MxEntity;
use mxrs_model::Project;

#[derive(MxEntity)]
#[allow(dead_code)]
struct Order {
    #[mx_attribute(kind = "string", default = "A-0000")]
    number: String,
    #[mx_attribute(kind = "decimal")]
    total: String,
    #[mx_attribute(kind = "float")]
    score: f64,
    #[mx_attribute(kind = "hash_string")]
    password: String,
    #[mx_attribute(kind = "binary")]
    payload: Vec<u8>,
    #[mx_attribute(kind = "enumeration", enumeration = "Sales.State", default = "Open")]
    state: String,
}

#[derive(MxEntity)]
#[mx_entity(name = "Client")]
#[allow(dead_code)]
struct Customer {
    #[mx_attribute(kind = "string")]
    full_name: String,
}

#[test]
fn derived_entities_write_and_read_back_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Derived.mpr");

    let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        Order::mx_register(m);
        Customer::mx_register(m);
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let read = Project::open(&path, true).unwrap();
    let sales = read
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let entities = sales.entities();
    assert_eq!(entities.len(), 2);

    let order = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap();
    let number = order
        .attributes
        .iter()
        .find(|a| a.name.as_deref() == Some("Number"))
        .unwrap();
    assert_eq!(number.default_value.as_deref(), Some("A-0000"));
    assert!(
        order
            .attributes
            .iter()
            .any(|a| a.name.as_deref() == Some("Total"))
    );
    let state = order
        .attributes
        .iter()
        .find(|a| a.name.as_deref() == Some("State"))
        .unwrap();
    assert_eq!(
        state.attribute_type,
        mxrs_model::attribute::AttributeType::Enum
    );
    assert_eq!(state.enumeration.as_deref(), Some("Sales.State"));
    assert_eq!(state.default_value.as_deref(), Some("Open"));

    // #[mx_entity(name = "Client")] overrides the struct's own name.
    assert!(entities.iter().any(|e| e.name.as_deref() == Some("Client")));
    let client = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Client"))
        .unwrap();
    assert!(
        client
            .attributes
            .iter()
            .any(|a| a.name.as_deref() == Some("FullName"))
    );
}
