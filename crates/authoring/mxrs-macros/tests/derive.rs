//! Exercises `#[derive(MxEntity)]` (M8.3) through the real pipeline:
//! derive expansion → `mxrs_dsl::ModuleBuilder::entity` call → written via
//! `mxrs-writer` → read back via `mxrs-model` — the same end-to-end
//! standard `project! {}` (M8.1) is held to, proving this second front end
//! genuinely produces the same kind of output, not a parallel shape.

use mxrs_macros::{MxEntity, MxEnumeration};
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
#[mxrs(module = "Sales")]
#[allow(dead_code)]
struct Customer {
    #[mx_attribute(kind = "string")]
    full_name: String,
}

#[derive(MxEntity)]
#[mxrs(
    module = "Sales",
    documentation = "A Cargo-native order",
    persistable = false
)]
#[allow(dead_code)]
struct TypedOrder {
    #[mxrs(
        name = "OrderNumber",
        default = "A-0001",
        documentation = "Human-readable order number",
        length = 80,
        required,
        unique
    )]
    number: mxrs_expr::MxString,
    total: mxrs_expr::MxDecimal,
    #[mxrs(localize_date = false)]
    submitted_at: Option<mxrs_expr::MxDateTime>,
    status: OrderStatus,
    #[mxrs(association = "TypedOrder_Customer", documentation = "Owning customer")]
    customer: mxrs_ir::Reference<Customer>,
    #[mxrs(association = "TypedOrder_Watchers", owner = "Both", storage = "Table")]
    watchers: mxrs_ir::ReferenceSet<Customer>,
    approver: mxrs_ir::Reference<Customer>,
}

#[derive(MxEnumeration)]
#[mxrs(module = "Sales", documentation = "Order lifecycle")]
#[allow(dead_code)]
enum OrderStatus {
    #[mxrs(caption = "Open order")]
    Open,
    Closed,
}

#[test]
fn derived_entities_write_and_read_back_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Derived.mpr");

    let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        Order::mx_register(m);
        Customer::mx_register(m);
        TypedOrder::mx_register(m);
        OrderStatus::mx_register(m);
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
    assert_eq!(entities.len(), 3);

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

    assert_eq!(
        <TypedOrder as mxrs_ir::EntityMarker>::qualified_name(),
        "Sales.TypedOrder"
    );
    let typed = entities
        .iter()
        .find(|entity| entity.name.as_deref() == Some("TypedOrder"))
        .unwrap();
    assert_eq!(typed.documentation, "A Cargo-native order");
    assert!(!typed.persistable);
    let number = typed
        .attributes
        .iter()
        .find(|attribute| attribute.name.as_deref() == Some("OrderNumber"))
        .unwrap();
    assert_eq!(number.default_value.as_deref(), Some("A-0001"));
    assert_eq!(number.documentation, "Human-readable order number");
    assert_eq!(number.length, Some(80));
    assert!(number.required);
    assert!(number.unique);
    let submitted_at = typed
        .attributes
        .iter()
        .find(|attribute| attribute.name.as_deref() == Some("SubmittedAt"))
        .unwrap();
    assert_eq!(submitted_at.localize_date, Some(false));
    let status = typed
        .attributes
        .iter()
        .find(|attribute| attribute.name.as_deref() == Some("Status"))
        .unwrap();
    assert_eq!(status.enumeration.as_deref(), Some("Sales.OrderStatus"));
    let associations = sales.associations();
    let customer = associations
        .iter()
        .find(|association| association.name.as_deref() == Some("TypedOrder_Customer"))
        .unwrap();
    assert_eq!(customer.documentation, "Owning customer");
    assert_eq!(
        customer.association_type,
        mxrs_model::association::AssociationType::Reference
    );
    let watchers = associations
        .iter()
        .find(|association| association.name.as_deref() == Some("TypedOrder_Watchers"))
        .unwrap();
    assert_eq!(
        watchers.association_type,
        mxrs_model::association::AssociationType::ReferenceSet
    );
    assert_eq!(watchers.owner, mxrs_model::association::Owner::Both);
    assert_eq!(
        watchers.storage_format,
        mxrs_model::association::StorageFormat::Table
    );
    assert!(
        associations
            .iter()
            .any(|association| association.name.as_deref() == Some("TypedOrder_Approver")),
        "an unannotated Reference<T> should infer its association name"
    );

    let enumeration = read
        .mpr()
        .all_units()
        .unwrap()
        .into_iter()
        .find(|unit| {
            read.mpr()
                .parse_contents(unit)
                .ok()
                .is_some_and(|document| {
                    document.get_str("$Type").ok() == Some("Enumerations$Enumeration")
                        && document.get_str("Name").ok() == Some("OrderStatus")
                })
        })
        .unwrap();
    let enumeration = read.mpr().parse_contents(&enumeration).unwrap();
    assert_eq!(
        enumeration.get_str("Documentation").unwrap(),
        "Order lifecycle"
    );
    let values = mxrs_bson::parse_array(enumeration.get_array("Values").ok().map(Vec::as_slice));
    assert_eq!(values.items.len(), 2);
}
