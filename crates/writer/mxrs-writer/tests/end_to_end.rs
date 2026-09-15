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
    AssociationOwner, AssociationStorage, AssociationType, AttributeType, ConstantType,
    ExportLevel, MemberRights, MicroflowRef, OnOverlap, Ref, ScheduleUnit, ScheduledEventSchedule,
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
        pub struct ACT_Missing;
        impl mxrs_ir::MicroflowMarker for ACT_Missing {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "ACT_Missing";
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
    let return_type = mf
        .return_type_document
        .as_ref()
        .expect("object return type is persisted");
    assert_eq!(
        return_type.get_str("$Type").unwrap(),
        "DataTypes$ObjectType"
    );
    assert_eq!(return_type.get_str("Entity").unwrap(), "Sales.Order");
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
fn project_write_rejects_a_lifecycle_handler_missing_from_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("MissingLifecycleHandler.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.entity("Order", |entity| {
            entity.before_commit::<markers::Sales::ACT_Missing>(|_| {});
        });
    });
    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::UnknownLifecycleHandler { handler, .. }
            if handler == "Sales.ACT_Missing"
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

/// Reads back one `Documents` unit of `document_type` by `Name`. Constants
/// have no typed `mxrs-model` representation (mxrb materializes them
/// separately from the artifact document compiler, and this mirrors that), so
/// the assertions below work on the raw document.
fn document_by_name(project: &Project, document_type: &str, name: &str) -> mxrs_bson::Document {
    project
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .find(|document| {
            document.get_str("$Type").ok() == Some(document_type)
                && document.get_str("Name").ok() == Some(name)
        })
        .unwrap_or_else(|| panic!("no {document_type} named {name:?} was written"))
}

#[test]
fn constants_persist_with_the_type_and_value_mxrb_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Constants.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.constant("ApiEndpoint", |c| {
            c.documentation("Base URL of the catalog service")
                .value("https://example.invalid/api");
        });
        m.constant("MaxRetries", |c| {
            c.value_type(ConstantType::Integer)
                .value("3")
                .exposed_to_client(true);
        });
    });

    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let read = Project::open(&path, true).unwrap();
    let endpoint = document_by_name(&read, "Constants$Constant", "ApiEndpoint");
    assert_eq!(
        endpoint.get_str("DefaultValue").unwrap(),
        "https://example.invalid/api"
    );
    assert_eq!(
        endpoint.get_str("Documentation").unwrap(),
        "Base URL of the catalog service"
    );
    assert_eq!(endpoint.get_str("ExportLevel").unwrap(), "Hidden");
    assert!(!endpoint.get_bool("ExposedToClient").unwrap());
    assert!(!endpoint.get_bool("Excluded").unwrap());
    assert_eq!(
        endpoint
            .get_document("Type")
            .unwrap()
            .get_str("$Type")
            .unwrap(),
        "DataTypes$StringType"
    );

    let retries = document_by_name(&read, "Constants$Constant", "MaxRetries");
    assert_eq!(retries.get_str("DefaultValue").unwrap(), "3");
    assert!(retries.get_bool("ExposedToClient").unwrap());
    assert_eq!(
        retries
            .get_document("Type")
            .unwrap()
            .get_str("$Type")
            .unwrap(),
        "DataTypes$IntegerType"
    );
    // The constant and its `Type` sub-document are separately identified, and
    // neither ID may collide with the other's. Read through `extract_id`
    // because the codec stores `$ID` as an MS-GUID blob, not a string.
    assert_ne!(
        mxrs_bson::extract_id(retries.get("$ID").unwrap()),
        mxrs_bson::extract_id(retries.get_document("Type").unwrap().get("$ID").unwrap())
    );
}

#[test]
fn scheduled_events_persist_with_a_schedule_matching_their_interval_type() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Jobs.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.microflow("ACT_ExpireCarts", |_f| {});
        m.microflow("ACT_SyncCatalog", |_f| {});
        m.scheduled_event(
            "SE_ExpireCarts",
            "ACT_ExpireCarts",
            ScheduleUnit::Days,
            |e| {
                e.documentation("Drops carts nobody came back for");
            },
        );
        m.scheduled_event(
            "SE_SyncCatalog",
            "ACT_SyncCatalog",
            ScheduleUnit::Hours,
            |e| {
                e.every(6)
                    .time_zone("America/Sao_Paulo")
                    .on_overlap(OnOverlap::DelayNext)
                    .enabled(false);
            },
        );
    });

    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let read = Project::open(&path, true).unwrap();
    let daily = document_by_name(&read, "ScheduledEvents$ScheduledEvent", "SE_ExpireCarts");
    // The unqualified declaration is stored qualified against its own module.
    assert_eq!(daily.get_str("Microflow").unwrap(), "Sales.ACT_ExpireCarts");
    assert_eq!(daily.get_str("IntervalType").unwrap(), "Day");
    // `Interval` reads back as Int64: the MPR codec widens integer
    // properties, leaving only array markers at Int32.
    assert_eq!(daily.get_i64("Interval").unwrap(), 1);
    assert_eq!(daily.get_str("TimeZone").unwrap(), "UTC");
    assert_eq!(daily.get_str("OnOverlap").unwrap(), "SkipNext");
    assert!(daily.get_bool("Enabled").unwrap());
    let schedule = daily.get_document("Schedule").unwrap();
    assert_eq!(
        schedule.get_str("$Type").unwrap(),
        "ScheduledEvents$DaySchedule"
    );
    assert_eq!(schedule.get_i64("HourOfDay").unwrap(), 0);
    assert_eq!(schedule.get_i64("MinuteOfHour").unwrap(), 0);
    // Fixed epoch rather than "now", so writing the same declaration twice
    // produces the same bytes.
    assert_eq!(
        daily
            .get_datetime("StartDateTime")
            .unwrap()
            .timestamp_millis(),
        946_684_800_000
    );

    let hourly = document_by_name(&read, "ScheduledEvents$ScheduledEvent", "SE_SyncCatalog");
    assert_eq!(hourly.get_str("IntervalType").unwrap(), "Hour");
    assert_eq!(hourly.get_i64("Interval").unwrap(), 6);
    assert_eq!(hourly.get_str("TimeZone").unwrap(), "America/Sao_Paulo");
    assert_eq!(hourly.get_str("OnOverlap").unwrap(), "DelayNext");
    assert!(!hourly.get_bool("Enabled").unwrap());
    let schedule = hourly.get_document("Schedule").unwrap();
    assert_eq!(
        schedule.get_str("$Type").unwrap(),
        "ScheduledEvents$HourSchedule"
    );
    assert_eq!(schedule.get_i64("Multiplier").unwrap(), 6);
    assert_eq!(schedule.get_i64("MinuteOffset").unwrap(), 0);
}

#[test]
fn legacy_interval_is_independent_from_the_modern_day_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("BadSchedule.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.microflow("ACT_Nightly", |_f| {});
        m.scheduled_event("SE_Nightly", "ACT_Nightly", ScheduleUnit::Days, |e| {
            e.every(3);
        });
    });

    mxrs_writer::write_project(&path, &project.build()).unwrap();
    let read = Project::open(&path, true).unwrap();
    let event = document_by_name(&read, "ScheduledEvents$ScheduledEvent", "SE_Nightly");
    assert_eq!(event.get_i64("Interval").unwrap(), 3);
    assert_eq!(
        event
            .get_document("Schedule")
            .unwrap()
            .get_str("$Type")
            .unwrap(),
        "ScheduledEvents$DaySchedule"
    );
}

#[test]
fn a_negative_interval_fails_instead_of_producing_an_invalid_legacy_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ZeroInterval.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.microflow("ACT_Poll", |_f| {});
        m.scheduled_event("SE_Poll", "ACT_Poll", ScheduleUnit::Minutes, |e| {
            e.every(-1);
        });
    });

    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::InvalidScheduleInterval { name, interval }
            if name == "SE_Poll" && interval == -1
    ));
}

#[test]
fn a_scheduled_event_without_a_microflow_fails_rather_than_writing_a_job_that_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("NoHandler.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.scheduled_event("SE_Orphan", "", ScheduleUnit::Minutes, |_e| {});
    });

    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::ScheduledEventWithoutMicroflow(name) if name == "SE_Orphan"
    ));
}

#[test]
fn a_disabled_unbound_event_and_a_week_schedule_keep_their_typed_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CompleteSchedules.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.microflow("ACT_Weekly", |_| {});
        module.scheduled_event("Disabled", "", ScheduleUnit::Minutes, |event| {
            event
                .every(0)
                .enabled(false)
                .excluded(true)
                .schedule(ScheduledEventSchedule::None);
        });
        module.scheduled_event("Weekly", "ACT_Weekly", ScheduleUnit::Weeks, |event| {
            event.schedule(ScheduledEventSchedule::Week {
                hour_of_day: 23,
                minute_of_hour: 59,
                monday: true,
                tuesday: false,
                wednesday: false,
                thursday: false,
                friday: true,
                saturday: false,
                sunday: false,
            });
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let read = Project::open(&path, true).unwrap();
    let disabled = document_by_name(&read, "ScheduledEvents$ScheduledEvent", "Disabled");
    assert_eq!(disabled.get_str("Microflow").unwrap(), "");
    assert_eq!(disabled.get("Schedule"), Some(&mxrs_bson::Bson::Null));
    assert_eq!(disabled.get_i64("Interval").unwrap(), 0);
    assert!(!disabled.get_bool("Enabled").unwrap());
    assert!(disabled.get_bool("Excluded").unwrap());

    let weekly = document_by_name(&read, "ScheduledEvents$ScheduledEvent", "Weekly");
    assert_eq!(weekly.get_str("IntervalType").unwrap(), "Week");
    let schedule = weekly.get_document("Schedule").unwrap();
    assert_eq!(
        schedule.get_str("$Type").unwrap(),
        "ScheduledEvents$WeekSchedule"
    );
    assert_eq!(schedule.get_i64("HourOfDay").unwrap(), 23);
    assert_eq!(schedule.get_i64("MinuteOfHour").unwrap(), 59);
    assert!(schedule.get_bool("Monday").unwrap());
    assert!(schedule.get_bool("Friday").unwrap());
    assert!(!schedule.get_bool("Sunday").unwrap());
}

#[test]
fn scheduled_event_start_and_schedule_ranges_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let invalid_start = dir.path().join("InvalidStart.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.microflow("ACT_Run", |_| {});
        module.scheduled_event("Run", "ACT_Run", ScheduleUnit::Hours, |event| {
            event.start_at("tomorrow morning");
        });
    });
    let error = mxrs_writer::write_project(&invalid_start, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::InvalidScheduledEventStart { name, value }
            if name == "Run" && value == "tomorrow morning"
    ));

    let invalid_schedule = dir.path().join("InvalidSchedule.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.microflow("ACT_Run", |_| {});
        module.scheduled_event("Run", "ACT_Run", ScheduleUnit::Hours, |event| {
            event.schedule(ScheduledEventSchedule::Hour {
                multiplier: 1,
                minute_offset: 60,
            });
        });
    });
    let error = mxrs_writer::write_project(&invalid_schedule, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::InvalidScheduledEventScheduleValue {
            name,
            field: "minute offset",
            value: 60,
        } if name == "Run"
    ));
}

#[test]
fn regular_expressions_round_trip_every_semantic_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("RegularExpression.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.regular_expression("OrderCode", "[A-Z]{3}-[0-9]+", |expression| {
            expression
                .documentation("Public order code")
                .excluded(true)
                .export_level(ExportLevel::Published);
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let model = Project::open(&path, true).unwrap();
    let document = model
        .all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let document = model.mpr().parse_contents(&unit).ok()?;
            (document.get_str("$Type").ok() == Some("RegularExpressions$RegularExpression"))
                .then_some(document)
        })
        .unwrap();
    assert_eq!(document.get_str("Name").unwrap(), "OrderCode");
    assert_eq!(document.get_str("Expression").unwrap(), "[A-Z]{3}-[0-9]+");
    assert_eq!(
        document.get_str("Documentation").unwrap(),
        "Public order code"
    );
    assert!(document.get_bool("Excluded").unwrap());
    assert_eq!(document.get_str("ExportLevel").unwrap(), "Published");
}

#[test]
fn entity_access_rules_persist_and_read_back_with_qualified_roles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Access.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.role("User", "Regular user");
        m.role("Manager", "Sales manager");
        m.entity("Order", |e| {
            e.string("Number");
            e.decimal("Total");
            e.access_rule(["User"], |rule| {
                rule.documentation("Own orders, read only")
                    .xpath("[System.owner = '[%CurrentUser%]']")
                    .attribute::<markers::Sales::Order_Number>(MemberRights::ReadOnly)
                    .attribute::<markers::Sales::Order_Total>(MemberRights::ReadOnly);
            });
            e.access_rule(["Manager", "CRM.Admin"], |rule| {
                rule.allow_create(true)
                    .allow_delete(true)
                    .default_rights(MemberRights::ReadWrite)
                    .association::<markers::Sales::Order_Order_Customer>(MemberRights::ReadWrite);
            });
        });
    });

    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let read = Project::open(&path, true).unwrap();
    let order = read.modules().unwrap()[0].entities()[0].clone();
    assert_eq!(order.access_rules.len(), 2);

    let user = &order.access_rules[0];
    // An unqualified role resolves against the declaring module; an already
    // qualified one is left alone.
    assert_eq!(user.roles, ["Sales.User"]);
    assert!(!user.create && !user.delete);
    assert_eq!(user.default_rights, "None");
    assert_eq!(user.xpath, "[System.owner = '[%CurrentUser%]']");
    assert_eq!(user.documentation, "Own orders, read only");
    assert_eq!(
        user.members
            .iter()
            .map(|member| (member.reference.as_str(), member.rights.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Sales.Order.Number", "ReadOnly"),
            ("Sales.Order.Total", "ReadOnly")
        ]
    );

    let manager = &order.access_rules[1];
    assert_eq!(manager.roles, ["Sales.Manager", "CRM.Admin"]);
    assert!(manager.create && manager.delete);
    assert_eq!(manager.default_rights, "ReadWrite");
    assert_eq!(manager.members.len(), 1);
    // An association member is stored under `Association`, not `Attribute`,
    // and keeps the association's own qualified name.
    assert_eq!(manager.members[0].reference, "Sales.Order_Customer");
    assert_eq!(
        manager.members[0].kind,
        mxrs_model::entity::AccessMemberKind::Association
    );
}

#[test]
fn re_synchronizing_preserves_access_rule_identities_and_undeclared_rules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AccessResync.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.role("User", "Regular user");
        m.entity("Order", |e| {
            e.string("Number");
            e.access_rule(["User"], |rule| {
                rule.attribute::<markers::Sales::Order_Number>(MemberRights::ReadOnly);
            });
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let original = Project::open(&path, true).unwrap().modules().unwrap()[0].entities()[0].clone();
    let original_rule_id = original.access_rules[0].id.clone().unwrap();
    let original_member_id = original.access_rules[0].members[0].id.clone().unwrap();

    // Re-declaring the same rule with wider rights keeps both identities: the
    // role set, not array position, is what identifies a rule.
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
            e.access_rule(["User"], |rule| {
                rule.allow_create(true)
                    .attribute::<markers::Sales::Order_Number>(MemberRights::ReadWrite);
            });
        });
    });
    mxrs_writer::synchronize_project(&path, &project.build()).unwrap();

    let updated = Project::open(&path, true).unwrap().modules().unwrap()[0].entities()[0].clone();
    assert_eq!(updated.access_rules.len(), 1);
    assert_eq!(
        updated.access_rules[0].id.as_deref(),
        Some(original_rule_id.as_str())
    );
    assert!(updated.access_rules[0].create);
    assert_eq!(updated.access_rules[0].members[0].rights, "ReadWrite");
    assert_eq!(
        updated.access_rules[0].members[0].id.as_deref(),
        Some(original_member_id.as_str())
    );

    // An entity that declares no rules at all leaves the existing ones alone
    // rather than clearing them.
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
        });
    });
    mxrs_writer::synchronize_project(&path, &project.build()).unwrap();
    let untouched = Project::open(&path, true).unwrap().modules().unwrap()[0].entities()[0].clone();
    assert_eq!(untouched.access_rules.len(), 1);
    assert_eq!(
        untouched.access_rules[0].id.as_deref(),
        Some(original_rule_id.as_str())
    );

    // Clearing is possible, but only by saying so.
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
            e.clear_access_rules();
        });
    });
    mxrs_writer::synchronize_project(&path, &project.build()).unwrap();
    let cleared = Project::open(&path, true).unwrap().modules().unwrap()[0].entities()[0].clone();
    assert!(cleared.access_rules.is_empty());
}

#[test]
fn an_access_rule_with_no_roles_fails_instead_of_granting_rights_to_nobody() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("NoRoles.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Order", |e| {
            e.access_rule(Vec::<String>::new(), |rule| {
                rule.allow_create(true);
            });
        });
    });

    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::AccessRuleWithoutRoles { module_name, name }
            if module_name == "Sales" && name == "Order"
    ));
}

#[test]
fn standalone_menu_writes_all_typed_actions_icons_and_localized_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Menus.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.menu("Main", |menu| {
            menu.documentation("Application menu").item("Home", |item| {
                item.caption("pt_BR", "Início")
                    .image("Atlas_Core.Atlas_Filled.home")
                    .page("Home");
            });
            menu.item("Create", |item| {
                let mut title = mxrs_ir::LocalizedText::new();
                title.insert("en_US".into(), "New order".into());
                item.glyph(57_377)
                    .action(mxrs_ir::MenuActionDecl::CreateObjectAndOpenPage {
                        entity: "Order".into(),
                        page: "NewOrder".into(),
                        disabled_during_execution: true,
                        pages_to_close: Some(1),
                        title_override: Some(title),
                    });
            });
        });
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    let menu = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .map(|unit| mpr.parse_contents(&unit).unwrap())
        .find(|document| {
            document.get_str("$Type").ok() == Some("Menus$MenuDocument")
                && document.get_str("Name").ok() == Some("Main")
        })
        .unwrap();
    assert_eq!(menu.get_str("Documentation").unwrap(), "Application menu");
    let items = mxrs_bson::parse_array(Some(
        menu.get_document("ItemCollection")
            .unwrap()
            .get_array("Items")
            .unwrap(),
    ));
    assert_eq!(items.marker, 3);
    let home = items.items[0].as_document().unwrap();
    assert_eq!(
        home.get_document("Action")
            .unwrap()
            .get_document("FormSettings")
            .unwrap()
            .get_str("Form")
            .unwrap(),
        "Sales.Home"
    );
    assert_eq!(
        home.get_document("Icon").unwrap().get_str("Image").unwrap(),
        "Atlas_Core.Atlas_Filled.home"
    );
    let create = items.items[1].as_document().unwrap();
    let action = create.get_document("Action").unwrap();
    assert_eq!(
        action.get_str("$Type").unwrap(),
        "Forms$CreateObjectClientAction"
    );
    assert_eq!(
        action
            .get_document("EntityRef")
            .unwrap()
            .get_str("Entity")
            .unwrap(),
        "Sales.Order"
    );
    assert_eq!(action.get_str("NumberOfPagesToClose2").unwrap(), "1");
}
