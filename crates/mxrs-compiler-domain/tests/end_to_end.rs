//! Exercises `DomainCompiler` against a real project: built via `mxrs-dsl`,
//! persisted via `mxrs-writer`, read back via `mxrs-model` — the same
//! writer→reader round trip `mxrs-writer`'s own end-to-end tests use, one
//! stage further (editor shape → Runtime shape).

use mxrs_bson::Bson;
use mxrs_compiler_domain::{DomainCompiler, SecurityCompiler};
use mxrs_dsl::ProjectBuilder;
use mxrs_ir::{AssociationType, Ref};
use mxrs_model::Project;
use mxrs_model::entity::{AccessMember, AccessMemberKind, AccessRule};

#[allow(dead_code, non_snake_case)]
mod markers {
    pub mod Sales {
        pub struct Customer;
        impl mxrs_ir::EntityMarker for Customer {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Customer";
        }
        pub struct Order;
        impl mxrs_ir::EntityMarker for Order {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Order";
        }
    }
    pub mod CRM {
        pub struct Account;
        impl mxrs_ir::EntityMarker for Account {
            const MODULE: &'static str = "CRM";
            const NAME: &'static str = "Account";
        }
    }
}

fn write_fixture(path: &std::path::Path) {
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.persistable(true);
            e.string("Number");
            e.association(
                "Order_Customer",
                Ref::<markers::Sales::Customer>::new(),
                AssociationType::Reference,
            );
            e.association(
                "Order_Account",
                Ref::<markers::CRM::Account>::new(),
                AssociationType::Reference,
            );
        });
    });
    project.module("CRM", |m| {
        m.entity("Account", |e| {
            e.string("Name");
        });
    });
    mxrs_writer::write_project(path, &project.build()).unwrap();
}

#[test]
fn compiles_entities_attributes_and_associations_to_runtime_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Compiled.mpr");
    write_fixture(&path);

    let project = Project::open(&path, true).unwrap();
    let modules = project.modules().unwrap();
    let compiler = DomainCompiler::new(&project, &modules).unwrap();

    let sales = modules
        .iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let compiled = compiler.compile_module(sales).unwrap().unwrap();

    assert_eq!(
        compiled.get_str("$Type").unwrap(),
        "DomainModels$DomainModel"
    );
    assert_eq!(
        compiled.get_str("$ID").unwrap(),
        sales.domain_model.as_ref().unwrap().id.as_deref().unwrap()
    );

    let entities = compiled.get_array("Entities").unwrap();
    assert_eq!(entities.len(), 2);
    let order = entities
        .iter()
        .find_map(|e| match e {
            Bson::Document(d) if d.get_str("UnqualifiedName").ok() == Some("Order") => Some(d),
            _ => None,
        })
        .expect("Order entity should be present");
    assert_eq!(order.get_str("QualifiedName").unwrap(), "Sales.Order");

    let attributes = order.get_array("Attributes").unwrap();
    let number = attributes
        .iter()
        .find_map(|a| match a {
            Bson::Document(d) if d.get_str("Name").ok() == Some("Number") => Some(d),
            _ => None,
        })
        .expect("Number attribute should be present");
    let Bson::Document(attr_type) = number.get("Type").unwrap() else {
        panic!("expected a document")
    };
    assert_eq!(
        attr_type.get_str("$Type").unwrap(),
        "DomainModels$StringAttributeType"
    );
    assert_eq!(
        attr_type.get_i32("Length").unwrap(),
        mxrs_model::attribute::DEFAULT_STRING_LENGTH
    );

    let generalization = order.get_document("MaybeGeneralization").unwrap();
    assert!(generalization.get_bool("Persistable").unwrap());

    let associations = compiled.get_array("Associations").unwrap();
    assert_eq!(
        associations.len(),
        1,
        "the same-module Order->Customer association"
    );
    let Bson::Document(order_customer) = &associations[0] else {
        panic!("expected a document")
    };
    assert_eq!(
        order_customer.get_str("QualifiedName").unwrap(),
        "Sales.Order_Customer"
    );
    assert!(order_customer.get("ChildPointer").is_some());

    let cross_associations = compiled.get_array("CrossAssociations").unwrap();
    assert_eq!(
        cross_associations.len(),
        1,
        "the cross-module Order->Account association"
    );
    let Bson::Document(order_account) = &cross_associations[0] else {
        panic!("expected a document")
    };
    assert_eq!(
        order_account.get_str("$Type").unwrap(),
        "DomainModels$CrossAssociation"
    );
    assert_eq!(order_account.get_str("Child").unwrap(), "CRM.Account");
}

#[test]
fn a_module_without_a_domain_model_compiles_to_none() {
    // Every module `mxrs-dsl` builds gets an (empty) domain model, so this
    // exercises the `None` branch directly against a hand-built `Module`
    // rather than needing a project fixture with a domain-model-less module.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Empty.mpr");
    write_fixture(&path);
    let project = Project::open(&path, true).unwrap();
    let modules = project.modules().unwrap();
    let compiler = DomainCompiler::new(&project, &modules).unwrap();

    let module = mxrs_model::Module {
        id: "test-module".to_string(),
        name: Some("Empty".to_string()),
        sort_index: None,
        from_app_store: false,
        app_store_guid: None,
        app_store_version: None,
        export_level: "Hidden".to_string(),
        domain_model: None,
        pages: vec![],
        microflows: vec![],
        nanoflows: vec![],
        rules: vec![],
        menus: vec![],
        module_roles: vec![],
        artifact_units: vec![],
    };
    assert!(compiler.compile_module(&module).unwrap().is_none());
}

#[test]
fn resolves_module_roles_to_user_roles_via_the_default_security_scaffold() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Security.mpr");
    write_fixture(&path);

    let project = Project::open(&path, true).unwrap();
    let security = SecurityCompiler::new(&project).unwrap();

    let rule = AccessRule {
        id: None,
        roles: vec!["System.Administrator".to_string()],
        create: true,
        delete: false,
        documentation: String::new(),
        default_rights: "None".to_string(),
        members: vec![AccessMember {
            id: None,
            name: "Number".to_string(),
            reference: "Sales.Order/Number".to_string(),
            rights: "ReadWrite".to_string(),
            kind: AccessMemberKind::Attribute,
        }],
        xpath: String::new(),
        xpath_caption: None,
    };

    let compiled = security.access_rule(&rule);
    let allowed = compiled.get_array("AllowedUserRoles").unwrap();
    assert_eq!(allowed, &vec![Bson::String("Administrator".to_string())]);

    let members = compiled.get_array("MemberAccesses").unwrap();
    let Bson::Document(member) = &members[0] else {
        panic!("expected a document")
    };
    assert_eq!(member.get_str("Attribute").unwrap(), "Sales.Order/Number");
    assert_eq!(member.get_str("Association").unwrap(), "");
}

#[test]
fn resolves_oql_views_and_preserves_opaque_domain_runtime_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Oql.mpr");
    write_fixture(&path);

    let project = Project::open(&path, true).unwrap();
    let sales_id = project
        .modules()
        .unwrap()
        .into_iter()
        .find(|module| module.name.as_deref() == Some("Sales"))
        .unwrap()
        .id;
    drop(project);

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let domain_unit = mpr
        .units_by_containment("DomainModel")
        .unwrap()
        .into_iter()
        .find(|unit| unit.container_id == sales_id)
        .unwrap();
    let mut domain = mpr.parse_contents(&domain_unit).unwrap();
    let entities = domain.get_array_mut("entities").unwrap();
    let order = entities
        .iter_mut()
        .filter_map(Bson::as_document_mut)
        .find(|entity| entity.get_str("name").ok() == Some("Order"))
        .unwrap();
    order.insert("$Type", "DomainModels$ViewEntity");
    order.insert("dataStorageGuid", "order-guid");
    order.insert("image", "Sales.OrderIcon");
    order.insert(
        "source",
        mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "DomainModels$OqlViewEntitySource",
            "SourceDocument": "Sales.OrderSource",
        },
    );
    order.insert(
        "eventHandlers",
        mxrs_bson::build_array(
            vec![Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "DomainModels$EventHandler",
                "Moment": "Before",
                "Event": "Create",
                "Microflow": "Sales.ACT_BeforeCreate",
                "Messages": mxrs_bson::build_array(vec![Bson::String("kept".into())], 3),
            })],
            3,
        ),
    );
    let associations = domain.get_array_mut("associations").unwrap();
    let association = associations
        .iter_mut()
        .filter_map(Bson::as_document_mut)
        .next()
        .unwrap();
    association.insert("Source", "Sales.Order");
    association.insert("GUID", "association-guid");
    mpr.update_unit(&domain_unit.unit_id, domain).unwrap();
    mpr.insert_unit(
        &sales_id,
        "Documents",
        mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "DomainModels$ViewEntitySourceDocument",
            "Name": "OrderSource",
            "Oql": "SELECT Number FROM Sales.Order",
        },
        None,
    )
    .unwrap();
    drop(mpr);

    let project = Project::open(&path, true).unwrap();
    let modules = project.modules().unwrap();
    let compiler = DomainCompiler::new(&project, &modules).unwrap();
    let sales = modules
        .iter()
        .find(|module| module.name.as_deref() == Some("Sales"))
        .unwrap();
    let compiled = compiler.compile_module(sales).unwrap().unwrap();
    let entities = compiled.get_array("Entities").unwrap();
    let order = entities
        .iter()
        .filter_map(Bson::as_document)
        .find(|entity| entity.get_str("UnqualifiedName").ok() == Some("Order"))
        .unwrap();
    assert_eq!(order.get_str("GUID").unwrap(), "order-guid");
    assert_eq!(order.get_str("Image").unwrap(), "Sales.OrderIcon");
    let source = order.get_document("Source").unwrap();
    assert_eq!(
        source.get_str("OqlRuntime").unwrap(),
        "SELECT Number FROM Sales.Order"
    );
    assert_eq!(source.get_str("SourceType").unwrap(), "OQL");
    let events = order.get_array("Events").unwrap();
    let event = events[0].as_document().unwrap();
    assert_eq!(event.get_array("Messages").unwrap().len(), 1);
    assert_eq!(
        event.get_array("Messages").unwrap()[0].as_str(),
        Some("kept")
    );

    let associations = compiled.get_array("Associations").unwrap();
    let association = associations[0].as_document().unwrap();
    assert_eq!(association.get_str("Source").unwrap(), "Sales.Order");
    assert_eq!(association.get_str("GUID").unwrap(), "association-guid");
}
