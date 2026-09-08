//! Builds a project via `mxrs-dsl`, persists it via `mxrs-writer`, and reads
//! it back via `mxrs-model` — the Phase 3 done-definition's readback half
//! (structural fidelity in mxrs's own reader). Cross-checking against a real
//! Studio Pro / `mxrb compare` run is not available in this environment; see
//! `mxrs-writer`'s crate doc for what's deliberately not covered yet
//! (ProjectSettings/Security/Navigation scaffolding).

use mxrs_dsl::ProjectBuilder;
use mxrs_ir::flow::MicroflowCallMapping;
use mxrs_ir::Member;
use mxrs_model::association::{AssociationType, Owner};
use mxrs_model::Project;

#[test]
fn writes_a_domain_model_and_microflow_that_reads_back_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Written.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.documentation("A customer order");
            e.string("Number").default_value = Some("A-0000".into());
            e.decimal("Total");
            e.association("Order_Customer", "Customer", AssociationType::Reference).owner = Owner::Default;
        });
        m.microflow("ACT_CreateOrder", |f| {
            f.create_object("order", "Sales.Order", vec![Member::attribute("Number", "'A-1'")], false);
            f.change_object("order", "Sales.Order", vec![Member::attribute("Total", "100")], false);
            f.decision(
                "$order/Total > 0",
                |t| {
                    t.commit("order");
                },
                |_f| {},
            );
            f.call_microflow(
                "Sales.ACT_Notify",
                None,
                false,
                vec![MicroflowCallMapping { parameter: "Order".into(), value: "$order".into() }],
            );
            f.return_value("$order");
        });
    });
    let definition = project.build();

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let modules = read.modules().unwrap();
    assert_eq!(modules.len(), 1);
    let sales = &modules[0];
    assert_eq!(sales.name.as_deref(), Some("Sales"));

    let entities = sales.entities();
    assert_eq!(entities.len(), 2);
    let order = entities.iter().find(|e| e.name.as_deref() == Some("Order")).unwrap();
    assert_eq!(order.qualified_name.as_deref(), Some("Sales.Order"));
    assert_eq!(order.documentation, "A customer order");
    let number = order.attributes.iter().find(|a| a.name.as_deref() == Some("Number")).unwrap();
    assert_eq!(number.default_value.as_deref(), Some("A-0000"));

    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    let assoc = associations[0];
    assert_eq!(assoc.name.as_deref(), Some("Order_Customer"));
    let customer = entities.iter().find(|e| e.name.as_deref() == Some("Customer")).unwrap();
    assert_eq!(assoc.from_entity_id.as_deref(), order.id.as_deref());
    assert_eq!(assoc.to_entity_id.as_deref(), customer.id.as_deref());

    assert_eq!(sales.microflows.len(), 1);
    let mf = &sales.microflows[0];
    assert_eq!(mf.name.as_deref(), Some("ACT_CreateOrder"));
    // StartEvent, CreateObject activity, ChangeObject activity,
    // ExclusiveSplit, Commit activity (true branch), ExclusiveMerge,
    // CallMicroflow activity, EndEvent.
    assert_eq!(mf.objects.len(), 8);
    let flow_types: Vec<String> = mf.objects.iter().map(|o| o.get_str("$Type").unwrap().to_string()).collect();
    assert!(flow_types.contains(&"Microflows$StartEvent".to_string()));
    assert!(flow_types.contains(&"Microflows$EndEvent".to_string()));
    assert!(flow_types.contains(&"Microflows$ExclusiveSplit".to_string()));
}

#[test]
fn writes_a_cross_module_association_that_reads_back_as_a_qualified_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CrossModule.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.association("Order_Account", "CRM.Account", AssociationType::Reference).owner = Owner::Default;
        });
    });
    project.module("CRM", |m| {
        m.entity("Account", |e| {
            e.string("Name");
        });
    });
    let definition = project.build();

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let modules = read.modules().unwrap();
    let sales = modules.iter().find(|m| m.name.as_deref() == Some("Sales")).unwrap();

    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    let assoc = associations[0];
    assert_eq!(assoc.name.as_deref(), Some("Order_Account"));
    assert!(assoc.is_cross_module());
    assert_eq!(assoc.to_entity_id.as_deref(), Some("CRM.Account"));
    let order = sales.entities().iter().find(|e| e.name.as_deref() == Some("Order")).unwrap();
    assert_eq!(assoc.from_entity_id.as_deref(), order.id.as_deref());
}

#[test]
fn a_fresh_project_has_settings_security_navigation_and_system_texts_units() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Scaffolded.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |_e| {});
    });
    let definition = project.build();

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let types: Vec<String> =
        read.all_units().unwrap().iter().filter_map(|u| read.mpr().parse_contents(u).ok()).filter_map(|d| d.get_str("$Type").ok().map(String::from)).collect();

    assert!(types.contains(&"Settings$ProjectSettings".to_string()));
    assert!(types.contains(&"Texts$SystemTextCollection".to_string()));
    assert!(types.contains(&"Projects$ProjectConversion".to_string()));
    assert!(types.contains(&"Security$ProjectSecurity".to_string()));
    assert!(types.contains(&"Navigation$NavigationDocument".to_string()));

    let navigation = read.navigation().unwrap();
    assert!(navigation.profiles.is_empty());
}

#[test]
fn unknown_cross_module_association_target_fails_at_write_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("BadCrossModule.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.association("Order_Account", "CRM.Account", AssociationType::Reference);
        });
    });
    let definition = project.build();

    let err = mxrs_writer::write_project(&path, &definition).unwrap_err();
    assert!(matches!(err, mxrs_writer::WriterError::UnknownCrossModuleAssociationTarget(target) if target == "CRM.Account"));
}
