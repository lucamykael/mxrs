//! Builds a project via `mxrs-dsl`, persists it via `mxrs-writer`, and reads
//! it back via `mxrs-model` — the Phase 3 done-definition's readback half
//! (structural fidelity in mxrs's own reader). Cross-checking against a real
//! Studio Pro / `mxrb compare` run is not available in this environment; see
//! `mxrs-writer`'s crate doc for what's deliberately not covered yet
//! (ProjectSettings/Security/Navigation scaffolding).

use mxrs_dsl::{CallArgument, ProjectBuilder, decimal, integer, string};
use mxrs_expr::attribute;
use mxrs_ir::declaration::{AssociationDecl, EntityDecl};
use mxrs_ir::{
    AssociationOwner, AssociationStorage, AssociationType, AttributeType, MicroflowRef, Ref,
};
use mxrs_model::Project;
use mxrs_model::association::{AssociationType as ModelAssociationType, Owner as ModelOwner};
use mxrs_model::attribute::AttributeType as ModelAttributeType;

/// Hand-written marker types (not `mxrs-typegen`-generated — these tests
/// don't need a manifest/build.rs, just something implementing
/// `EntityMarker`) covering every entity these tests declare associations
/// to. `mxrs-typegen`'s own crate proves the codegen path; this proves the
/// trait contract alone is enough for `EntityBuilder::association` to work.
#[allow(dead_code, non_snake_case, non_camel_case_types)]
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
        pub struct Order_Number;
        impl mxrs_ir::AttributeMarker for Order_Number {
            type Entity = Order;
            const NAME: &'static str = "Number";
        }
        impl mxrs_expr::TypedAttributeMarker for Order_Number {
            type Value = mxrs_expr::MxString;
        }
        pub struct Order_Total;
        impl mxrs_ir::AttributeMarker for Order_Total {
            type Entity = Order;
            const NAME: &'static str = "Total";
        }
        impl mxrs_expr::TypedAttributeMarker for Order_Total {
            type Value = mxrs_expr::MxDecimal;
        }
        pub struct ACT_Notify;
        impl mxrs_ir::MicroflowMarker for ACT_Notify {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "ACT_Notify";
        }
        pub struct Order_Order_Customer;
        impl mxrs_ir::AssociationMarker for Order_Order_Customer {
            type From = Order;
            type To = Customer;
            const NAME: &'static str = "Order_Customer";
            const ASSOCIATION_TYPE: mxrs_ir::AssociationType = mxrs_ir::AssociationType::Reference;
        }
        pub struct Order_Order_Account;
        impl mxrs_ir::AssociationMarker for Order_Order_Account {
            type From = Order;
            type To = super::CRM::Account;
            const NAME: &'static str = "Order_Account";
            const ASSOCIATION_TYPE: mxrs_ir::AssociationType = mxrs_ir::AssociationType::Reference;
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

#[test]
fn lowers_every_ir_attribute_type_at_the_storage_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AttributeTypes.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Demo", |module| {
        module.entity("Record", |entity| {
            entity.string("StringValue");
            entity.integer("IntegerValue");
            entity.long("LongValue");
            entity.float("FloatValue");
            entity.decimal("DecimalValue");
            entity.boolean("BooleanValue");
            entity.datetime("DateTimeValue");
            entity.autonumber("AutoNumberValue");
            entity.hash_string("HashStringValue");
            entity.binary("BinaryValue");
            entity.enumeration("EnumValue", "Demo.State");
        });
    });

    let definition = project.build();
    let declared = &definition.modules[0].entities[0].attributes;
    assert_eq!(declared[3].attribute_type, AttributeType::Float);
    assert_eq!(declared[8].attribute_type, AttributeType::HashString);
    assert_eq!(declared[9].attribute_type, AttributeType::Binary);
    assert_eq!(declared[10].attribute_type, AttributeType::Enumeration);

    mxrs_writer::write_project(&path, &definition).unwrap();
    let read = Project::open(&path, true).unwrap();
    let module = &read.modules().unwrap()[0];
    let attributes = &module.entities()[0].attributes;
    let actual: Vec<ModelAttributeType> = attributes
        .iter()
        .map(|attribute| attribute.attribute_type)
        .collect();
    assert_eq!(
        actual,
        vec![
            ModelAttributeType::String,
            ModelAttributeType::Integer,
            ModelAttributeType::Long,
            ModelAttributeType::Float,
            ModelAttributeType::Decimal,
            ModelAttributeType::Boolean,
            ModelAttributeType::DateTime,
            ModelAttributeType::AutoNumber,
            ModelAttributeType::HashString,
            ModelAttributeType::Binary,
            ModelAttributeType::Enum,
        ]
    );
    assert_eq!(attributes[10].enumeration.as_deref(), Some("Demo.State"));
}

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
            e.association::<markers::Sales::Order_Order_Customer>()
                .owner = AssociationOwner::Default;
        });
        m.microflow("ACT_CreateOrder", |f| {
            let order = f.create_object(
                "order",
                Ref::<markers::Sales::Order>::new(),
                vec![attribute::<markers::Sales::Order_Number>(string("A-1"))],
                false,
            );
            f.change_object(
                &order,
                vec![attribute::<markers::Sales::Order_Total>(decimal(100.0))],
                false,
            );
            f.decision(
                order.attribute::<markers::Sales::Order_Total>().gt(0.0),
                |t| {
                    t.commit(&order);
                },
                |_f| {},
            );
            f.call_microflow(
                MicroflowRef::<markers::Sales::ACT_Notify>::new(),
                None,
                false,
                vec![CallArgument::new("Order", order.clone())],
            );
            f.return_value(order);
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
    let order = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap();
    assert_eq!(order.qualified_name.as_deref(), Some("Sales.Order"));
    assert_eq!(order.documentation, "A customer order");
    let number = order
        .attributes
        .iter()
        .find(|a| a.name.as_deref() == Some("Number"))
        .unwrap();
    assert_eq!(number.default_value.as_deref(), Some("A-0000"));

    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    let assoc = associations[0];
    assert_eq!(assoc.name.as_deref(), Some("Order_Customer"));
    let customer = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Customer"))
        .unwrap();
    assert_eq!(assoc.from_entity_id.as_deref(), order.id.as_deref());
    assert_eq!(assoc.to_entity_id.as_deref(), customer.id.as_deref());

    assert_eq!(sales.microflows.len(), 1);
    let mf = &sales.microflows[0];
    assert_eq!(mf.name.as_deref(), Some("ACT_CreateOrder"));
    // StartEvent, CreateObject activity, ChangeObject activity,
    // ExclusiveSplit, Commit activity (true branch), ExclusiveMerge,
    // CallMicroflow activity, EndEvent.
    assert_eq!(mf.objects.len(), 8);
    let flow_types: Vec<String> = mf
        .objects
        .iter()
        .map(|o| o.get_str("$Type").unwrap().to_string())
        .collect();
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
            e.association::<markers::Sales::Order_Order_Account>().owner =
                AssociationOwner::Default;
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
    let sales = modules
        .iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();

    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    let assoc = associations[0];
    assert_eq!(assoc.name.as_deref(), Some("Order_Account"));
    assert!(assoc.is_cross_module());
    assert_eq!(assoc.to_entity_id.as_deref(), Some("CRM.Account"));
    let order = sales
        .entities()
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap();
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
    let types: Vec<String> = read
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|u| read.mpr().parse_contents(u).ok())
        .filter_map(|d| d.get_str("$Type").ok().map(String::from))
        .collect();

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
            e.association::<markers::Sales::Order_Order_Account>();
        });
    });
    let definition = project.build();

    let err = mxrs_writer::write_project(&path, &definition).unwrap_err();
    assert!(
        matches!(err, mxrs_writer::WriterError::UnknownCrossModuleAssociationTarget(target) if target == "CRM.Account")
    );
}

/// The marker type for a target proves it *exists somewhere* (per whatever
/// manifest declared it) — it does not prove *this* `ProjectBuilder`
/// invocation actually declares that entity. Both checks matter: a
/// compile-time-valid `Ref<M>` referencing an entity this specific project
/// never declares still has to fail at write time, same as it always has.
#[test]
fn a_valid_marker_for_an_entity_absent_from_this_project_still_fails_at_write_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("MarkerWithoutDeclaration.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.association::<markers::Sales::Order_Order_Customer>();
        });
    });
    let definition = project.build();

    let err = mxrs_writer::write_project(&path, &definition).unwrap_err();
    assert!(
        matches!(err, mxrs_writer::WriterError::UnknownAssociationTarget(target) if target == "Sales.Customer")
    );
}

/// Regression test for the local/cross routing fix `resolve_association`
/// makes: `EntityBuilder::association` now always emits a fully-qualified
/// `"Module.Entity"` target (via `Ref<M>::qualified_name()`), including for
/// a same-module association — this must still resolve as local (a plain
/// `ChildID` pointer, `DomainModels$Association`), not get misrouted into
/// the cross-module `DomainModels$CrossAssociation` shape just because the
/// target string happens to contain a dot.
#[test]
fn a_same_module_association_stays_local_even_though_its_target_is_fully_qualified() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("QualifiedSameModule.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.association::<markers::Sales::Order_Order_Customer>();
        });
    });
    let definition = project.build();
    assert_eq!(
        definition.modules[0].entities[1].associations[0].target,
        "Sales.Customer"
    );

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let sales = read
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    let assoc = associations[0];
    assert!(
        !assoc.is_cross_module(),
        "a same-module association was misrouted as cross-module"
    );
    let customer = sales
        .entities()
        .iter()
        .find(|e| e.name.as_deref() == Some("Customer"))
        .unwrap();
    assert_eq!(assoc.to_entity_id.as_deref(), customer.id.as_deref());
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
            e.association::<markers::Sales::Order_Order_Customer>()
                .owner = AssociationOwner::Default;
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let sales = before
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let module_id = sales.id.clone();
    let order = sales
        .entities()
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap()
        .clone();
    let original_assoc_id = sales.associations()[0].id.clone().unwrap();
    let original_number_default = order
        .attributes
        .iter()
        .find(|a| a.name.as_deref() == Some("Number"))
        .unwrap()
        .default_value
        .clone();

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
                    owner: AssociationOwner::Default,
                    storage: AssociationStorage::Column,
                    documentation: "updated docs".into(),
                },
                AssociationDecl {
                    name: "Order_Customer_Set".into(),
                    target: "Customer".into(),
                    association_type: AssociationType::ReferenceSet,
                    owner: AssociationOwner::Both,
                    storage: AssociationStorage::Table,
                    documentation: String::new(),
                },
            ],
            ..EntityDecl::new("Order")
        },
    ];
    let known_entities = ["Sales.Customer", "Sales.Order"]
        .into_iter()
        .map(String::from)
        .collect();

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::domain::synchronize_domain_associations(
        &mut mpr,
        &module_id,
        "Sales",
        &entities,
        &known_entities,
    )
    .unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let order = sales
        .entities()
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap();

    // Entity content (untouched by association sync) survives byte-for-byte.
    assert_eq!(
        order
            .attributes
            .iter()
            .find(|a| a.name.as_deref() == Some("Number"))
            .unwrap()
            .default_value,
        original_number_default
    );

    let associations = sales.associations();
    assert_eq!(associations.len(), 2);
    let kept = associations
        .iter()
        .find(|a| a.name.as_deref() == Some("Order_Customer"))
        .unwrap();
    assert_eq!(kept.id.as_deref(), Some(original_assoc_id.as_str()));
    assert_eq!(kept.documentation, "updated docs");
    let added = associations
        .iter()
        .find(|a| a.name.as_deref() == Some("Order_Customer_Set"))
        .unwrap();
    assert_eq!(added.association_type, ModelAssociationType::ReferenceSet);
    assert_eq!(added.owner, ModelOwner::Both);

    // Second sync drops `Order_Customer_Set` again; `Order_Customer` keeps its id.
    let entities = vec![
        EntityDecl::new("Customer"),
        EntityDecl {
            associations: vec![AssociationDecl {
                name: "Order_Customer".into(),
                target: "Customer".into(),
                association_type: AssociationType::Reference,
                owner: AssociationOwner::Default,
                storage: AssociationStorage::Column,
                documentation: "updated docs".into(),
            }],
            ..EntityDecl::new("Order")
        },
    ];
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::domain::synchronize_domain_associations(
        &mut mpr,
        &module_id,
        "Sales",
        &entities,
        &known_entities,
    )
    .unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    assert_eq!(
        associations[0].id.as_deref(),
        Some(original_assoc_id.as_str())
    );
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
    let module_id = before
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap()
        .id;

    let entities = vec![EntityDecl::new("Order"), EntityDecl::new("Ghost")];
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let err = mxrs_writer::domain::synchronize_domain_associations(
        &mut mpr,
        &module_id,
        "Sales",
        &entities,
        &Default::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        mxrs_writer::WriterError::EntitiesMissingFromDomainModel { module_name, missing }
            if module_name == "Sales" && missing == vec!["Ghost".to_string()]
    ));
}

fn find_domain_model_doc(
    mpr: &mxrs_mpr::MprFile,
    module_id: &str,
) -> (String, mxrs_bson::Document) {
    let unit = mpr
        .units_by_containment("DomainModel")
        .unwrap()
        .into_iter()
        .find(|u| u.container_id == module_id)
        .unwrap();
    let doc = mpr.parse_contents(&unit).unwrap();
    (unit.unit_id, doc)
}

#[test]
fn synchronize_domain_entities_preserves_ids_reconciles_attributes_adds_and_removes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SyncEntities.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.string("Number");
            e.decimal("Total");
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let sales = before
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let module_id = sales.id.clone();
    let customer_id = sales
        .entities()
        .iter()
        .find(|e| e.name.as_deref() == Some("Customer"))
        .unwrap()
        .id
        .clone()
        .unwrap();
    let order = sales
        .entities()
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap();
    let order_id = order.id.clone().unwrap();
    let number_id = order
        .attributes
        .iter()
        .find(|a| a.name.as_deref() == Some("Number"))
        .unwrap()
        .id
        .clone()
        .unwrap();

    // Hand-mutate Order's on-disk location to a marker value that no fresh
    // construction would ever produce, proving it survives reconciliation.
    {
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let (dm_id, mut doc) = find_domain_model_doc(&mpr, &module_id);
        let entities_key = if doc.contains_key("entities") {
            "entities"
        } else {
            "Entities"
        };
        let parsed = mxrs_bson::parse_array(match doc.get(entities_key) {
            Some(mxrs_bson::Bson::Array(items)) => Some(items.as_slice()),
            _ => None,
        });
        let items: Vec<mxrs_bson::Bson> = parsed
            .items
            .into_iter()
            .map(|item| match item {
                mxrs_bson::Bson::Document(mut d) if d.get_str("name").ok() == Some("Order") => {
                    d.insert("location", mxrs_bson::doc! { "x": 999, "y": 888 });
                    mxrs_bson::Bson::Document(d)
                }
                other => other,
            })
            .collect();
        doc.insert(
            entities_key,
            mxrs_bson::Bson::Array(mxrs_bson::build_array(items, parsed.marker)),
        );
        mpr.update_unit(&dm_id, doc).unwrap();
    }

    // Re-declare: Customer unchanged; Order drops `Total`, gains `Code`; a
    // brand-new `Shipment` entity is added.
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.string("Number");
            e.string("Code");
        });
        m.entity("Shipment", |e| {
            e.string("TrackingCode");
        });
    });
    let entities = project.build().modules.remove(0).entities;

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let ids =
        mxrs_writer::domain::synchronize_domain_entities(&mut mpr, &module_id, "Sales", &entities)
            .unwrap();
    assert_eq!(ids.get("Customer"), Some(&customer_id));
    assert_eq!(ids.get("Order"), Some(&order_id));
    assert!(ids.contains_key("Shipment"));
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let entities = sales.entities();
    assert_eq!(entities.len(), 3);

    let customer = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Customer"))
        .unwrap();
    assert_eq!(customer.id.as_deref(), Some(customer_id.as_str()));

    let order = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap();
    assert_eq!(order.id.as_deref(), Some(order_id.as_str()));
    assert_eq!(
        order.location,
        mxrs_model::entity::Location { x: 999, y: 888 }
    );
    assert!(
        order
            .attributes
            .iter()
            .find(|a| a.name.as_deref() == Some("Total"))
            .is_none()
    );
    let number = order
        .attributes
        .iter()
        .find(|a| a.name.as_deref() == Some("Number"))
        .unwrap();
    assert_eq!(number.id.as_deref(), Some(number_id.as_str()));
    assert!(
        order
            .attributes
            .iter()
            .any(|a| a.name.as_deref() == Some("Code"))
    );

    let shipment = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Shipment"))
        .unwrap();
    assert!(shipment.id.is_some());
    assert_ne!(shipment.id.as_deref(), Some(order_id.as_str()));
    assert_ne!(shipment.id.as_deref(), Some(customer_id.as_str()));

    // A second sync that drops `Shipment` entirely removes it, while `Order`
    // (still declared) keeps its id and its hand-mutated location.
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.string("Number");
            e.string("Code");
        });
    });
    let entities = project.build().modules.remove(0).entities;
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::domain::synchronize_domain_entities(&mut mpr, &module_id, "Sales", &entities)
        .unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let entities = sales.entities();
    assert_eq!(entities.len(), 2);
    assert!(
        entities
            .iter()
            .find(|e| e.name.as_deref() == Some("Shipment"))
            .is_none()
    );
    let order = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap();
    assert_eq!(order.id.as_deref(), Some(order_id.as_str()));
    assert_eq!(
        order.location,
        mxrs_model::entity::Location { x: 999, y: 888 }
    );
}

#[test]
fn synchronize_domain_entities_rejects_duplicate_declared_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SyncDuplicateEntity.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |_e| {});
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let module_id = before
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap()
        .id;

    let entities = vec![EntityDecl::new("Order"), EntityDecl::new("Order")];
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let err =
        mxrs_writer::domain::synchronize_domain_entities(&mut mpr, &module_id, "Sales", &entities)
            .unwrap_err();
    assert!(matches!(
        err,
        mxrs_writer::WriterError::DuplicateEntity { module_name, name }
            if module_name == "Sales" && name == "Order"
    ));
}

#[test]
fn synchronize_domain_model_adds_an_entity_and_an_association_to_it_in_one_pass() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SyncModel.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let module_id = before
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap()
        .id;

    // Declare a brand-new `Customer` entity *and* an association from the
    // existing `Order` to it, in the same call — proves entities sync runs
    // before associations sync so the new entity is a valid target.
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
            e.association::<markers::Sales::Order_Order_Customer>();
        });
        m.entity("Customer", |e| {
            e.string("Name");
        });
    });
    let entities = project.build().modules.remove(0).entities;
    let known_entities = ["Sales.Order", "Sales.Customer"]
        .into_iter()
        .map(String::from)
        .collect();

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::domain::synchronize_domain_model(
        &mut mpr,
        &module_id,
        "Sales",
        &entities,
        &known_entities,
    )
    .unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert_eq!(sales.entities().len(), 2);
    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    assert_eq!(associations[0].name.as_deref(), Some("Order_Customer"));
    let customer = sales
        .entities()
        .iter()
        .find(|e| e.name.as_deref() == Some("Customer"))
        .unwrap();
    assert_eq!(
        associations[0].to_entity_id.as_deref(),
        customer.id.as_deref()
    );
}

#[test]
fn synchronize_microflows_preserves_id_on_a_name_match_and_upserts_new_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SyncMicroflows.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |_e| {});
        m.microflow("ACT_A", |f| {
            f.return_value(integer(1));
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let sales = before
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let module_id = sales.id.clone();
    let original_id = sales.microflows[0].id.clone().unwrap();

    // Re-declare ACT_A with a different body (same name -> same $ID) and
    // add a brand-new ACT_B.
    let mut redeclare = ProjectBuilder::new("11.12.1");
    redeclare.module("Sales", |m| {
        m.microflow("ACT_A", |f| {
            f.return_value(integer(2));
        });
        m.microflow("ACT_B", |f| {
            f.return_value(integer(3));
        });
    });
    let microflows = redeclare.build().modules.remove(0).microflows;

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::documents::synchronize_microflows(&mut mpr, &module_id, &microflows).unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert_eq!(sales.microflows.len(), 2);
    let act_a = sales
        .microflows
        .iter()
        .find(|f| f.name.as_deref() == Some("ACT_A"))
        .unwrap();
    assert_eq!(act_a.id.as_deref(), Some(original_id.as_str()));
    let act_b = sales
        .microflows
        .iter()
        .find(|f| f.name.as_deref() == Some("ACT_B"))
        .unwrap();
    assert_ne!(act_b.id.as_deref(), Some(original_id.as_str()));
}

/// Unlike domain-model entity/association sync, microflow sync is
/// upsert-only: a microflow that already exists but isn't named in a given
/// `synchronize_microflows` call is left alone, not deleted — matching
/// mxrb's own `write_documents` (see `documents.rs`'s doc comment).
#[test]
fn synchronize_microflows_does_not_delete_an_undeclared_existing_microflow() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SyncMicroflowsUpsertOnly.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |_e| {});
        m.microflow("ACT_Keep", |f| {
            f.return_value(integer(1));
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let module_id = before
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap()
        .id;

    let mut redeclare = ProjectBuilder::new("11.12.1");
    redeclare.module("Sales", |m| {
        m.microflow("ACT_New", |f| {
            f.return_value(integer(2));
        });
    });
    let microflows = redeclare.build().modules.remove(0).microflows;

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::documents::synchronize_microflows(&mut mpr, &module_id, &microflows).unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let names: Vec<Option<&str>> = sales.microflows.iter().map(|f| f.name.as_deref()).collect();
    assert!(names.contains(&Some("ACT_Keep")));
    assert!(names.contains(&Some("ACT_New")));
}

#[test]
fn writes_a_page_with_native_widgets_that_reads_back_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("WrittenPage.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.page("OrderOverview", |p| {
            p.title("Orders");
            p.url("orderoverview");
            p.layout("Atlas_Core.ApplicationLayout", "Main");
            p.container(|c| {
                c.class("row");
                c.text("Manage your orders");
                c.button("Close", |b| {
                    b.close_page();
                });
            });
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let read = Project::open(&path, true).unwrap();
    let sales = read
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert_eq!(sales.pages.len(), 1);
    let page = &sales.pages[0];
    assert_eq!(page.name.as_deref(), Some("OrderOverview"));
    assert_eq!(page.url, "orderoverview");
    assert_eq!(page.widgets.len(), 1);
    let container = &page.widgets[0];
    assert_eq!(container.widget_type, "container");
    assert_eq!(container.children.len(), 2);
    assert_eq!(container.children[0].widget_type, "text");
    assert_eq!(container.children[1].widget_type, "button");
}

#[test]
fn synchronize_pages_preserves_id_on_a_name_match_and_upserts_new_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SyncPages.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.page("Home", |p| {
            p.layout("Atlas_Core.ApplicationLayout", "Main");
            p.text("v1");
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let before = Project::open(&path, true).unwrap();
    let sales = before
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let module_id = sales.id.clone();
    let original_id = sales.pages[0].id.clone().unwrap();

    let mut redeclare = ProjectBuilder::new("11.12.1");
    redeclare.module("Sales", |m| {
        m.page("Home", |p| {
            p.layout("Atlas_Core.ApplicationLayout", "Main");
            p.text("v2");
        });
        m.page("About", |p| {
            p.layout("Atlas_Core.ApplicationLayout", "Main");
            p.text("about");
        });
    });
    let pages = redeclare.build().modules.remove(0).pages;

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mxrs_writer::documents::synchronize_pages(&mut mpr, &module_id, "11.12.1", &pages).unwrap();
    drop(mpr);

    let after = Project::open(&path, true).unwrap();
    let sales = after
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert_eq!(sales.pages.len(), 2);
    let home = sales
        .pages
        .iter()
        .find(|p| p.name.as_deref() == Some("Home"))
        .unwrap();
    assert_eq!(home.id.as_deref(), Some(original_id.as_str()));
    assert_eq!(home.widgets[0].options.get_str("caption").unwrap(), "v2");
    let about = sales
        .pages
        .iter()
        .find(|p| p.name.as_deref() == Some("About"))
        .unwrap();
    assert_ne!(about.id.as_deref(), Some(original_id.as_str()));
}

#[test]
fn a_page_declaring_widgets_without_a_layout_fails_loudly_at_write_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("NoLayout.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.page("Broken", |p| {
            p.text("orphaned widget, no layout declared");
        });
    });

    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::PageWidgetsRequireLayout(name) if name == "Broken"
    ));
}
