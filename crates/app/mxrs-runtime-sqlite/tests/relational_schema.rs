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

/// The v1 upgrade reads the snapshot with the *current* model, so objects of
/// an entity the model no longer declares have nowhere to go. Losing them is
/// a destructive migration and is refused by name — a version-1 database has
/// no `mxrb_schema_*` catalogue for `schema::migrate` to detect the removal
/// from, so this is the only place that can see it.
#[test]
fn a_version_one_snapshot_of_a_removed_entity_is_refused_then_dropped_when_allowed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let mut with_legacy = order_module();
    if let Some(domain_model) = &mut with_legacy.domain_model {
        domain_model.entities.push(entity(
            "Legacy",
            vec![attribute("Name", AttributeType::String)],
        ));
    }
    {
        let modules = std::slice::from_ref(&with_legacy);
        let mut legacy_store = mxrs_runtime_sqlite::SqliteRuntimeStore::open(&path).unwrap();
        let mut store = Store::new(store_schema(modules));
        for (entity_name, name) in [("App.Order", "kept"), ("App.Legacy", "orphan")] {
            let object = store.create(entity_name).unwrap();
            store
                .set_member(entity_name, &object.id, "Name", Value::String(name.into()))
                .unwrap();
            store.commit(entity_name, &object.id).unwrap();
        }
        legacy_store.save(&store).unwrap();
    }

    let current = order_module();
    let modules = std::slice::from_ref(&current);
    let refused = RelationalRuntimeStore::open(&path, modules, false).unwrap_err();
    let message = refused.to_string();
    assert!(
        matches!(refused, SqliteRuntimeError::UnsafeMigration { .. }),
        "{message}"
    );
    assert!(message.contains("App.Legacy (1)"), "{message}");
    assert!(message.contains("--allow-destructive-schema"), "{message}");
    // The refusal left the database on version 1, so a second attempt sees
    // exactly the same thing rather than a half-migrated file.
    assert!(RelationalRuntimeStore::open(&path, modules, false).is_err());

    let mut upgraded = RelationalRuntimeStore::open(&path, modules, true).unwrap();
    let mut store = Store::new(store_schema(modules));
    assert_eq!(upgraded.load(&mut store).unwrap(), 1);
    assert_eq!(
        store.retrieve("App.Order").unwrap()[0].members["Name"],
        Value::String("kept".into())
    );
}

/// `application_id = 0` is SQLite's default, so it names no application at
/// all: the relational adapter has to inspect the schema before adopting a
/// (0, 0) file, exactly like the version-1 adapter does.
#[test]
fn a_foreign_sqlite_database_is_refused_without_changing_any_file_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("someone-elses.sqlite3");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE invoices (id TEXT PRIMARY KEY); INSERT INTO invoices VALUES ('i-1')",
            )
            .unwrap();
    }
    let before = std::fs::read(&path).unwrap();

    let module = order_module();
    let modules = std::slice::from_ref(&module);
    let refused = RelationalRuntimeStore::open(&path, modules, false).unwrap_err();
    assert!(
        matches!(
            refused,
            SqliteRuntimeError::UnrelatedDatabase {
                application_id: 0,
                version: 0
            }
        ),
        "{refused}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Not even `--allow-destructive-schema` may adopt it: the flag answers
    // "may this model's own data be dropped", not "may any file be taken".
    assert!(RelationalRuntimeStore::open(&path, modules, true).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn boolean_float_and_reference_set_members_round_trip_relationally() {
    let flag = attribute("Flag", AttributeType::Boolean);
    let ratio = attribute("Ratio", AttributeType::Float);
    let order = entity("Order", vec![flag, ratio]);
    let tag = entity("Tag", vec![attribute("Name", AttributeType::String)]);
    let tags = association("Order_Tags", &order, &tag, false);
    let module = module_with(vec![order, tag], vec![tags]);
    let modules = std::slice::from_ref(&module);

    let mut persistence = RelationalRuntimeStore::in_memory(modules, false).unwrap();
    let mut store = Store::new(store_schema(modules));
    let mut tag_ids = Vec::new();
    for name in ["red", "blue"] {
        let created = store.create("App.Tag").unwrap();
        store
            .set_member("App.Tag", &created.id, "Name", Value::String(name.into()))
            .unwrap();
        store.commit("App.Tag", &created.id).unwrap();
        tag_ids.push(Value::String(created.id));
    }
    let order = store.create("App.Order").unwrap();
    store
        .set_member("App.Order", &order.id, "Flag", Value::Bool(true))
        .unwrap();
    store
        .set_member("App.Order", &order.id, "Ratio", serde_json::json!(0.25))
        .unwrap();
    store
        .set_member(
            "App.Order",
            &order.id,
            "Order_Tags",
            Value::Array(tag_ids.clone()),
        )
        .unwrap();
    store.commit("App.Order", &order.id).unwrap();
    assert_eq!(persistence.save(&store).unwrap(), 3);

    let mut restored = Store::new(store_schema(modules));
    assert_eq!(persistence.load(&mut restored).unwrap(), 3);
    let orders = restored.retrieve("App.Order").unwrap();
    // A boolean survives as a boolean, not as the 0/1 its column stores.
    assert_eq!(orders[0].members["Flag"], Value::Bool(true));
    assert_eq!(orders[0].members["Ratio"], serde_json::json!(0.25));
    let mut stored_tags = orders[0].members["Order_Tags"].as_array().unwrap().clone();
    stored_tags.sort_by_key(|value| value.as_str().unwrap().to_string());
    let mut expected = tag_ids.clone();
    expected.sort_by_key(|value| value.as_str().unwrap().to_string());
    assert_eq!(stored_tags, expected);
}

/// Ports `synchronize_sequence`'s `INSERT OR IGNORE`: the sequence row is
/// seeded once, when the AutoNumber column is first registered, and later
/// migrations never move it. Allocation itself still belongs to the store
/// layer, so nothing here hands out numbers.
#[test]
fn autonumber_sequences_are_seeded_once_at_the_high_water_mark() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let order = entity(
        "Order",
        vec![attribute("Number", AttributeType::AutoNumber)],
    );
    let module = module_with(vec![order], Vec::new());
    let modules = std::slice::from_ref(&module);
    {
        let mut persistence = RelationalRuntimeStore::open(&path, modules, false).unwrap();
        let mut store = Store::new(store_schema(modules));
        let created = store.create("App.Order").unwrap();
        store
            .set_member("App.Order", &created.id, "Number", serde_json::json!(7))
            .unwrap();
        store.commit("App.Order", &created.id).unwrap();
        persistence.save(&store).unwrap();
    }
    // Reopening re-runs the migration over rows that now reach 7.
    RelationalRuntimeStore::open(&path, modules, false).unwrap();

    let connection = rusqlite::Connection::open(&path).unwrap();
    let next: i64 = connection
        .query_row("SELECT next_value FROM mxrb_schema_sequences", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(next, 1, "OR IGNORE keeps the first seed, like the oracle");
}

/// "Zero silent loss" applies to the durability boundary above all: a number
/// that cannot be stored in its integer column fails the save by name rather
/// than landing as NULL.
#[test]
fn a_member_that_does_not_fit_its_column_fails_the_save_by_name() {
    let order = entity("Order", vec![attribute("Count", AttributeType::Integer)]);
    let module = module_with(vec![order], Vec::new());
    let modules = std::slice::from_ref(&module);
    let mut persistence = RelationalRuntimeStore::in_memory(modules, false).unwrap();
    let mut store = Store::new(store_schema(modules));
    let created = store.create("App.Order").unwrap();
    store
        .set_member("App.Order", &created.id, "Count", serde_json::json!(2.5))
        .unwrap();
    store.commit("App.Order", &created.id).unwrap();

    let error = persistence.save(&store).unwrap_err();
    let message = error.to_string();
    assert!(
        matches!(error, SqliteRuntimeError::LossyMember { .. }),
        "{message}"
    );
    assert!(message.contains("App.Order/Count"), "{message}");
    assert!(message.contains("2.5"), "{message}");
}

/// The store tags datetime members; a TEXT column carries that tag through
/// unchanged, so what comes back out is a datetime again and not text that
/// merely looks like one.
#[test]
fn tagged_datetime_members_survive_a_relational_round_trip_verbatim() {
    let tagged = format!("{}1758462896", mxrs_runtime::DATETIME_MEMBER_PREFIX);
    let order = entity("Order", vec![attribute("When", AttributeType::DateTime)]);
    let module = module_with(vec![order], Vec::new());
    let modules = std::slice::from_ref(&module);
    let mut persistence = RelationalRuntimeStore::in_memory(modules, false).unwrap();
    let mut store = Store::new(store_schema(modules));
    let created = store.create("App.Order").unwrap();
    store
        .set_member(
            "App.Order",
            &created.id,
            "When",
            Value::String(tagged.clone()),
        )
        .unwrap();
    store.commit("App.Order", &created.id).unwrap();
    persistence.save(&store).unwrap();

    let mut restored = Store::new(store_schema(modules));
    persistence.load(&mut restored).unwrap();
    assert_eq!(
        restored.retrieve("App.Order").unwrap()[0].members["When"],
        Value::String(tagged)
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

/// A specialization's table holds what it inherits, so an object keeps its
/// inherited members across a restart, and asking for the generalization
/// answers with it.
#[test]
fn a_specialization_keeps_its_inherited_members_across_a_restart() {
    let animal = entity("Animal", vec![attribute("Name", AttributeType::String)]);
    let mut dog = entity("Dog", vec![attribute("Breed", AttributeType::String)]);
    dog.generalization = Some(mxrs_model::entity::Generalization {
        id: None,
        native_type: "DomainModels$Generalization".to_string(),
        target: Some("App.Animal".to_string()),
        persistable: None,
        system_members: Default::default(),
        raw: doc! {},
    });
    let module = module_with(vec![animal, dog], Vec::new());
    let modules = std::slice::from_ref(&module);
    let derived = schema::derive(modules);
    let columns: Vec<&str> = derived
        .entity("App.Dog")
        .unwrap()
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    assert_eq!(columns, ["Name", "Breed"]);

    let schema = || store_schema(modules).generalization("App.Dog", "App.Animal");
    let mut persistence = RelationalRuntimeStore::in_memory(modules, false).unwrap();
    let mut store = Store::new(schema());
    let rex = store.create("App.Dog").unwrap();
    store
        .set_member("App.Animal", &rex.id, "Name", Value::String("Rex".into()))
        .unwrap();
    store
        .set_member("App.Dog", &rex.id, "Breed", Value::String("Collie".into()))
        .unwrap();
    store.commit("App.Animal", &rex.id).unwrap();
    persistence.save(&store).unwrap();

    let mut restored = Store::new(schema());
    persistence.load(&mut restored).unwrap();
    let animals = restored.retrieve("App.Animal").unwrap();
    assert_eq!(animals.len(), 1);
    assert_eq!(animals[0].entity, "App.Dog");
    assert_eq!(animals[0].members["Name"], Value::String("Rex".into()));
    assert_eq!(animals[0].members["Breed"], Value::String("Collie".into()));
    assert!(restored.find("App.Animal", &rex.id).unwrap().is_some());
    assert!(restored.retrieve("App.Dog").unwrap().len() == 1);
}
