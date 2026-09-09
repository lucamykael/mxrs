//! Builds a project via `mxrs-dsl`, persists it via `mxrs-writer`, and reads
//! it back via `mxrs-model` — the Phase 3 done-definition's readback half
//! (structural fidelity in mxrs's own reader). Cross-checking against a real
//! Studio Pro / `mxrb compare` run is not available in this environment; see
//! `mxrs-writer`'s crate doc for what's deliberately not covered yet
//! (ProjectSettings/Security/Navigation scaffolding).

use mxrs_dsl::ProjectBuilder;
use mxrs_ir::declaration::{AssociationDecl, EntityDecl};
use mxrs_ir::flow::MicroflowCallMapping;
use mxrs_ir::Member;
use mxrs_model::association::{AssociationType, Owner, StorageFormat};
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

#[test]
fn synchronize_domain_associations_preserves_ids_adds_and_removes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Sync.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.string("Number");
            e.association("Order_Customer", "Customer", AssociationType::Reference).owner = Owner::Default;
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let sales = before.modules().unwrap().into_iter().find(|m| m.name.as_deref() == Some("Sales")).unwrap();
    let module_id = sales.id.clone();
    let order = sales.entities().iter().find(|e| e.name.as_deref() == Some("Order")).unwrap().clone();
    let original_assoc_id = sales.associations()[0].id.clone().unwrap();
    let original_number_default = order.attributes.iter().find(|a| a.name.as_deref() == Some("Number")).unwrap().default_value.clone();

    // Re-declare the module: keep `Order_Customer` (same name, tweaked
    // documentation) and add a brand-new `Order_Customer_Set` association.
    let entities = vec![
        EntityDecl::new("Customer"),
        EntityDecl {
            associations: vec![
                AssociationDecl {
                    name: "Order_Customer".into(),
                    target: "Customer".into(),
                    association_type: AssociationType::Reference,
                    owner: Owner::Default,
                    storage_format: StorageFormat::Column,
                    documentation: "updated docs".into(),
                },
                AssociationDecl {
                    name: "Order_Customer_Set".into(),
                    target: "Customer".into(),
                    association_type: AssociationType::ReferenceSet,
                    owner: Owner::Both,
                    storage_format: StorageFormat::Table,
                    documentation: String::new(),
                },
            ],
            ..EntityDecl::new("Order")
        },
    ];
    let known_entities = ["Sales.Customer", "Sales.Order"].into_iter().map(String::from).collect();

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::domain::synchronize_domain_associations(&mut mpr, &module_id, "Sales", &entities, &known_entities)
        .unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after.modules().unwrap().into_iter().find(|m| m.name.as_deref() == Some("Sales")).unwrap();
    let order = sales.entities().iter().find(|e| e.name.as_deref() == Some("Order")).unwrap();

    // Entity content (untouched by association sync) survives byte-for-byte.
    assert_eq!(order.attributes.iter().find(|a| a.name.as_deref() == Some("Number")).unwrap().default_value, original_number_default);

    let associations = sales.associations();
    assert_eq!(associations.len(), 2);
    let kept = associations.iter().find(|a| a.name.as_deref() == Some("Order_Customer")).unwrap();
    assert_eq!(kept.id.as_deref(), Some(original_assoc_id.as_str()));
    assert_eq!(kept.documentation, "updated docs");
    let added = associations.iter().find(|a| a.name.as_deref() == Some("Order_Customer_Set")).unwrap();
    assert_eq!(added.association_type, AssociationType::ReferenceSet);
    assert_eq!(added.owner, Owner::Both);

    // Second sync drops `Order_Customer_Set` again; `Order_Customer` keeps its id.
    let entities = vec![
        EntityDecl::new("Customer"),
        EntityDecl {
            associations: vec![AssociationDecl {
                name: "Order_Customer".into(),
                target: "Customer".into(),
                association_type: AssociationType::Reference,
                owner: Owner::Default,
                storage_format: StorageFormat::Column,
                documentation: "updated docs".into(),
            }],
            ..EntityDecl::new("Order")
        },
    ];
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::domain::synchronize_domain_associations(&mut mpr, &module_id, "Sales", &entities, &known_entities)
        .unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after.modules().unwrap().into_iter().find(|m| m.name.as_deref() == Some("Sales")).unwrap();
    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    assert_eq!(associations[0].id.as_deref(), Some(original_assoc_id.as_str()));
}

#[test]
fn synchronize_domain_associations_rejects_unknown_entity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SyncMissingEntity.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |_e| {});
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let module_id = before.modules().unwrap().into_iter().find(|m| m.name.as_deref() == Some("Sales")).unwrap().id;

    let entities = vec![EntityDecl::new("Order"), EntityDecl::new("Ghost")];
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let err = mxrs_writer::domain::synchronize_domain_associations(&mut mpr, &module_id, "Sales", &entities, &Default::default())
        .unwrap_err();
    assert!(matches!(
        err,
        mxrs_writer::WriterError::EntitiesMissingFromDomainModel { module_name, missing }
            if module_name == "Sales" && missing == vec!["Ghost".to_string()]
    ));
}
