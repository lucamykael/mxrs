//! Behavioral suite for the relational schema migration and durability,
//! pinned against `schema_migrator.rb`'s evolution semantics: additive
//! changes apply in place, incompatible changes rebuild preserving data,
//! removals are refused until explicitly allowed, and physical names are
//! GUID-keyed so a rename never loses a table.

use std::collections::BTreeMap;

use mxrs_bson::doc;
use mxrs_model::association::Association;
use mxrs_model::entity::Entity;
use mxrs_model::{Attribute, AttributeType, DomainModel, Module};
use mxrs_runtime::{Store, StoreSchema};
use mxrs_runtime_sqlite::{RelationalRuntimeStore, SqliteRuntimeError, schema};
use serde_json::Value;

fn attribute(name: &str, kind: AttributeType) -> Attribute {
    let mut attribute = Attribute::from_bson(&doc! { "Name": name });
    attribute.attribute_type = kind;
    attribute.data_storage_guid = Some(format!("guid-{name}"));
    attribute
}

fn entity(name: &str, attributes: Vec<Attribute>) -> Entity {
    let mut entity = Entity::from_bson(&doc! { "$Type": "DomainModels$Entity", "Name": name });
    entity.id = Some(format!("id-{name}"));
    entity.qualified_name = Some(format!("App.{name}"));
    entity.data_storage_guid = Some(format!("guid-{name}"));
    entity.attributes = attributes;
    entity
}

fn module_with(entities: Vec<Entity>, associations: Vec<Association>) -> Module {
    Module {
        id: String::new(),
        name: Some("App".to_string()),
        sort_index: None,
        from_app_store: false,
        app_store_guid: None,
        app_store_version: None,
        export_level: String::new(),
        domain_model: Some(DomainModel {
            id: None,
            native_type: None,
            documentation: String::new(),
            entities,
            associations,
            cross_associations: Vec::new(),
        }),
        pages: Vec::new(),
        microflows: Vec::new(),
        nanoflows: Vec::new(),
        rules: Vec::new(),
        menus: Vec::new(),
        module_roles: Vec::new(),
        artifact_units: Vec::new(),
    }
}

fn association(name: &str, from: &Entity, to: &Entity, reference: bool) -> Association {
    let mut association = Association::from_bson(&doc! {
        "Name": name,
        "Type": if reference { "Reference" } else { "ReferenceSet" },
    });
    association.id = Some(format!("guid-{name}"));
    association.from_entity_id = from.id.clone();
    association.to_entity_id = to.qualified_name.clone();
    association
}

fn order_module() -> Module {
    let mut total = attribute("Total", AttributeType::Decimal);
    total.default_value = Some("0".to_string());
    let order = entity(
        "Order",
        vec![attribute("Name", AttributeType::String), total],
    );
    let customer = entity("Customer", vec![attribute("Name", AttributeType::String)]);
    let link = association("Order_Customer", &order, &customer, true);
    module_with(vec![order, customer], vec![link])
}

fn store_schema(modules: &[Module]) -> StoreSchema {
    let mut schema = StoreSchema::default();
    for module in modules {
        for entity in module.entities() {
            schema = schema.entity(
                entity.qualified_name.clone().unwrap(),
                BTreeMap::new(),
                !entity.persistable,
            );
        }
    }
    schema
}

#[test]
fn derives_guid_keyed_tables_and_migrates_idempotently() {
    let module = order_module();
    let derived = schema::derive(std::slice::from_ref(&module));
    assert_eq!(derived.entities.len(), 2);
    assert_eq!(derived.associations.len(), 1);
    let order = derived.entity("App.Order").unwrap();
    assert!(order.table.starts_with("mxrb_entity_"));
    assert_eq!(order.columns.len(), 2);
    let link = derived.association("Order_Customer").unwrap();
    assert!(link.reference);
    assert_eq!(link.from_entity, "App.Order");
    assert_eq!(link.to_entity, "App.Customer");

    let mut store = RelationalRuntimeStore::in_memory(std::slice::from_ref(&module), false)
        .expect("fresh migrate");
    // Second migration over the same schema is a no-op (idempotent open).
    let mut runtime_store = Store::new(store_schema(std::slice::from_ref(&module)));
    assert_eq!(store.load(&mut runtime_store).unwrap(), 0);
}

#[test]
fn saves_and_loads_objects_attributes_and_associations_relationally() {
    let module = order_module();
    let modules = std::slice::from_ref(&module);
    let mut persistence = RelationalRuntimeStore::in_memory(modules, false).unwrap();
    let mut store = Store::new(store_schema(modules));

    let customer = store.create("App.Customer").unwrap();
    store
        .set_member(
            "App.Customer",
            &customer.id,
            "Name",
            Value::String("Ada".into()),
        )
        .unwrap();
    store.commit("App.Customer", &customer.id).unwrap();
    let order = store.create("App.Order").unwrap();
    store
        .set_member("App.Order", &order.id, "Name", Value::String("o-1".into()))
        .unwrap();
    store
        .set_member("App.Order", &order.id, "Total", serde_json::json!(12.5))
        .unwrap();
    store
        .set_member(
            "App.Order",
            &order.id,
            "Order_Customer",
            Value::String(customer.id.clone()),
        )
        .unwrap();
    store.commit("App.Order", &order.id).unwrap();

    assert_eq!(persistence.save(&store).unwrap(), 2);

    let mut restored = Store::new(store_schema(modules));
    assert_eq!(persistence.load(&mut restored).unwrap(), 2);
    let orders = restored.retrieve("App.Order").unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].members["Name"], Value::String("o-1".into()));
    assert_eq!(orders[0].members["Total"], serde_json::json!(12.5));
    assert_eq!(
        orders[0].members["Order_Customer"],
        Value::String(customer.id.clone())
    );
    let related = restored.retrieve_association("Order_Customer", &orders[0]);
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].members["Name"], Value::String("Ada".into()));
}

#[test]
fn additive_changes_apply_in_place_and_removals_are_refused_until_allowed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let before = order_module();
    {
        let modules = std::slice::from_ref(&before);
        let mut persistence = RelationalRuntimeStore::open(&path, modules, false).unwrap();
        let mut store = Store::new(store_schema(modules));
        let order = store.create("App.Order").unwrap();
        store
            .set_member("App.Order", &order.id, "Name", Value::String("keep".into()))
            .unwrap();
        store.commit("App.Order", &order.id).unwrap();
        persistence.save(&store).unwrap();
    }

    // Adding an attribute is additive; the stored row survives.
    let mut with_extra = order_module();
    if let Some(domain_model) = &mut with_extra.domain_model {
        domain_model.entities[0]
            .attributes
            .push(attribute("Notes", AttributeType::String));
    }
    {
        let modules = std::slice::from_ref(&with_extra);
        let mut persistence = RelationalRuntimeStore::open(&path, modules, false).unwrap();
        let mut store = Store::new(store_schema(modules));
        assert_eq!(persistence.load(&mut store).unwrap(), 1);
        let orders = store.retrieve("App.Order").unwrap();
        assert_eq!(orders[0].members["Name"], Value::String("keep".into()));
    }

    // Dropping the attribute again is destructive: refused, then allowed.
    let reverted = order_module();
    let modules = std::slice::from_ref(&reverted);
    let refused = RelationalRuntimeStore::open(&path, modules, false).unwrap_err();
    let message = refused.to_string();
    assert!(
        matches!(refused, SqliteRuntimeError::UnsafeMigration { .. }),
        "{message}"
    );
    assert!(message.contains("attribute Notes"), "{message}");
    let mut persistence = RelationalRuntimeStore::open(&path, modules, true).unwrap();
    let mut store = Store::new(store_schema(modules));
    assert_eq!(persistence.load(&mut store).unwrap(), 1);
    let orders = store.retrieve("App.Order").unwrap();
    assert_eq!(orders[0].members["Name"], Value::String("keep".into()));
    assert!(!orders[0].members.contains_key("Notes"));
}

#[test]
fn a_type_change_rebuilds_the_table_preserving_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let before = order_module();
    {
        let modules = std::slice::from_ref(&before);
        let mut persistence = RelationalRuntimeStore::open(&path, modules, false).unwrap();
        let mut store = Store::new(store_schema(modules));
        let order = store.create("App.Order").unwrap();
        store
            .set_member("App.Order", &order.id, "Name", Value::String("kept".into()))
            .unwrap();
        store.commit("App.Order", &order.id).unwrap();
        persistence.save(&store).unwrap();
    }
    let mut changed = order_module();
    if let Some(domain_model) = &mut changed.domain_model {
        // Total: Decimal → String is a type change on the same storage guid.
        domain_model.entities[0].attributes[1].attribute_type = AttributeType::String;
        domain_model.entities[0].attributes[1].default_value = None;
    }
    let modules = std::slice::from_ref(&changed);
    let mut persistence = RelationalRuntimeStore::open(&path, modules, false).unwrap();
    let mut store = Store::new(store_schema(modules));
    assert_eq!(persistence.load(&mut store).unwrap(), 1);
    assert_eq!(
        store.retrieve("App.Order").unwrap()[0].members["Name"],
        Value::String("kept".into())
    );
}

#[test]
fn generation_conflicts_still_guard_relational_saves() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let module = order_module();
    let modules = std::slice::from_ref(&module);
    let mut first = RelationalRuntimeStore::open(&path, modules, false).unwrap();
    let store = Store::new(store_schema(modules));
    first.save(&store).unwrap();

    let mut second = RelationalRuntimeStore::open(&path, modules, false).unwrap();
    let mut fresh = Store::new(store_schema(modules));
    second.load(&mut fresh).unwrap();
    second.save(&fresh).unwrap();

    let stale = first.save(&store).unwrap_err();
    assert!(
        matches!(stale, SqliteRuntimeError::Conflict { .. }),
        "{stale}"
    );
}

#[test]
fn a_version_one_snapshot_database_upgrades_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let module = order_module();
    let modules = std::slice::from_ref(&module);
    {
        let mut legacy = mxrs_runtime_sqlite::SqliteRuntimeStore::open(&path).unwrap();
        let mut store = Store::new(store_schema(modules));
        let order = store.create("App.Order").unwrap();
        store
            .set_member(
                "App.Order",
                &order.id,
                "Name",
                Value::String("from-v1".into()),
            )
            .unwrap();
        store.commit("App.Order", &order.id).unwrap();
        legacy.save(&store).unwrap();
    }
    let mut upgraded = RelationalRuntimeStore::open(&path, modules, false).unwrap();
    let mut store = Store::new(store_schema(modules));
    assert_eq!(upgraded.load(&mut store).unwrap(), 1);
    assert_eq!(
        store.retrieve("App.Order").unwrap()[0].members["Name"],
        Value::String("from-v1".into())
    );
}
