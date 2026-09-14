use super::*;
use serde_json::json;

fn schema() -> StoreSchema {
    StoreSchema::default()
        .entity(
            "Sales.Order",
            BTreeMap::from([("Status".into(), json!("New"))]),
            false,
        )
        .entity("Sales.Customer", BTreeMap::new(), false)
        .entity("Session.Filter", BTreeMap::new(), true)
}

fn object(entity: &str, id: &str) -> ObjectValue {
    ObjectValue {
        entity: entity.into(),
        id: id.into(),
        members: BTreeMap::from([("Status".into(), json!("New"))]),
    }
}

#[test]
fn lifecycle_boundaries_match_the_executed_mxrb_native_store_oracle() {
    let mut store = Store::new(schema());
    let order = store.create("Sales.Order").unwrap();
    store.commit("Sales.Order", &order.id).unwrap();
    let transient = store.create("Session.Filter").unwrap();
    store
        .set_member("Session.Filter", &transient.id, "Value", json!("saved"))
        .unwrap();
    store.commit("Session.Filter", &transient.id).unwrap();
    store
        .set_member("Session.Filter", &transient.id, "Value", json!("dirty"))
        .unwrap();
    store.rollback("Session.Filter", &transient.id).unwrap();
    let transient_rollback = store
        .find("Session.Filter", &transient.id)
        .unwrap()
        .unwrap()
        .members["Value"]
        .clone();
    let failed: Result<()> = store.transaction(|store| {
        store.set_member("Sales.Order", &order.id, "Status", json!("Outer"))?;
        store.commit("Sales.Order", &order.id)?;
        let inner: Result<()> = store.transaction(|store| {
            store.set_member("Sales.Order", &order.id, "Status", json!("Inner"))?;
            store.commit("Sales.Order", &order.id)?;
            Err(RuntimeError::Transaction("inner".into()))
        });
        assert!(inner.is_err());
        assert_eq!(
            store.find("Sales.Order", &order.id)?.unwrap().members["Status"],
            "Outer"
        );
        Err(RuntimeError::Transaction("outer".into()))
    });
    assert!(failed.is_err());
    store.create("Sales.Order").unwrap();
    store
        .set_member("Sales.Order", &order.id, "Status", json!("Uncommitted"))
        .unwrap();
    store.transaction(|_| Ok(())).unwrap();
    assert_eq!(
        json!({
            "count": store.retrieve("Sales.Order").unwrap().len(),
            "status": store.find("Sales.Order", &order.id).unwrap().unwrap().members["Status"],
            "transient_rollback": transient_rollback,
            "transient_count": store.retrieve("Session.Filter").unwrap().len(),
        }),
        json!({"count":1,"status":"New","transient_rollback":"saved","transient_count":1})
    );
}

#[test]
fn panicking_transactions_restore_records_commits_deletions_and_nested_member_values() {
    let mut store = Store::new(schema());
    store
        .restore_persistent([object("Sales.Order", "existing")])
        .unwrap();
    store
        .set_member("Sales.Order", "existing", "Nested", json!({"items":[1,2]}))
        .unwrap();
    store.commit("Sales.Order", "existing").unwrap();
    let transient = store.create("Session.Filter").unwrap();
    let before_records = store.retrieve("Sales.Order").unwrap();
    let before_persistent = store.persistent_objects();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _: Result<()> = store.transaction(|store| {
            store.set_member("Sales.Order", "existing", "Nested", json!({"items":[3]}))?;
            store.commit("Sales.Order", "existing")?;
            store.delete("Session.Filter", &transient.id)?;
            let new = store.create("Sales.Customer")?;
            store.commit("Sales.Customer", &new.id)?;
            std::panic::panic_any("original panic payload");
        });
    }))
    .unwrap_err();
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"original panic payload")
    );
    assert_eq!(store.retrieve("Sales.Order").unwrap(), before_records);
    assert_eq!(store.persistent_objects(), before_persistent);
    assert!(store.retrieve("Sales.Customer").unwrap().is_empty());
    assert_eq!(
        store.find("Session.Filter", &transient.id).unwrap(),
        Some(transient)
    );
}

#[test]
fn nested_success_is_still_rolled_back_when_the_outer_transaction_fails() {
    let mut store = Store::new(schema());
    store
        .restore_persistent([object("Sales.Order", "existing")])
        .unwrap();
    let before = store.persistent_objects();
    let failure: Result<()> = store.transaction(|store| {
        store.transaction(|store| {
            store.delete("Sales.Order", "existing")?;
            let customer = store.create("Sales.Customer")?;
            store.commit("Sales.Customer", &customer.id)?;
            Ok(())
        })?;
        Err(RuntimeError::Transaction("outer abort".into()))
    });
    assert!(failure.is_err());
    assert_eq!(store.persistent_objects(), before);
    assert_eq!(store.retrieve("Sales.Order").unwrap(), before);
    assert!(store.retrieve("Sales.Customer").unwrap().is_empty());
}

#[test]
fn a_caught_inner_panic_restores_the_outer_unit_of_work_and_can_be_retried() {
    let mut store = Store::new(schema());
    store
        .transaction(|store| {
            let order = store.create("Sales.Order")?;
            let panic = catch_unwind(AssertUnwindSafe(|| {
                let _: Result<()> = store.transaction(|store| {
                    store.delete("Sales.Order", &order.id)?;
                    panic!("inner abort");
                });
            }));
            assert!(panic.is_err());
            assert_eq!(store.find("Sales.Order", &order.id)?, Some(order.clone()));
            store.commit("Sales.Order", &order.id)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.persistent_objects().len(), 1);
}

#[test]
fn read_only_transactions_preserve_shared_roots_without_cloning_object_payloads() {
    let mut store = Store::new(schema());
    store
        .restore_persistent((0..1000).map(|index| object("Sales.Order", &index.to_string())))
        .unwrap();
    let snapshot = store.clone();
    store
        .transaction(|store| {
            assert_eq!(store.find("Sales.Order", "500")?.unwrap().id, "500");
            Ok(())
        })
        .unwrap();
    assert!(Arc::ptr_eq(
        &store.schema.entities,
        &snapshot.schema.entities
    ));
    assert!(Arc::ptr_eq(&store.records, &snapshot.records));
    assert!(Arc::ptr_eq(&store.committed, &snapshot.committed));
    assert!(Arc::ptr_eq(&store.dirty, &snapshot.dirty));
    store
        .set_member("Sales.Order", "500", "Status", json!("Changed"))
        .unwrap();
    assert!(Arc::ptr_eq(
        &store.records["Sales.Order"]["499"],
        &snapshot.records["Sales.Order"]["499"]
    ));
    assert!(!Arc::ptr_eq(
        &store.records["Sales.Order"]["500"],
        &snapshot.records["Sales.Order"]["500"]
    ));
    assert_eq!(
        snapshot
            .find("Sales.Order", "500")
            .unwrap()
            .unwrap()
            .members["Status"],
        "New"
    );
}

#[test]
fn detached_retrieved_objects_and_store_clones_cannot_mutate_each_others_values() {
    let mut store = Store::new(schema());
    let mut first = store.create("Sales.Order").unwrap();
    first.members.insert("Status".into(), json!("detached"));
    assert_eq!(
        store
            .find("Sales.Order", &first.id)
            .unwrap()
            .unwrap()
            .members["Status"],
        "New"
    );
    let mut clone = store.clone();
    clone
        .set_member("Sales.Order", &first.id, "Status", json!("clone only"))
        .unwrap();
    clone.commit("Sales.Order", &first.id).unwrap();
    assert!(store.persistent_objects().is_empty());
    assert_eq!(
        store
            .find("Sales.Order", &first.id)
            .unwrap()
            .unwrap()
            .members["Status"],
        "New"
    );
    store.rollback("Sales.Order", &first.id).unwrap();
    assert!(store.find("Sales.Order", &first.id).unwrap().is_none());
    assert_eq!(
        clone
            .find("Sales.Order", &first.id)
            .unwrap()
            .unwrap()
            .members["Status"],
        "clone only"
    );
}

#[test]
fn mutations_of_unknown_entities_and_objects_fail_without_inventing_records() {
    let mut store = Store::new(schema());
    for entity in ["Sales.Missing", "Sales.Order"] {
        let expected = if entity == "Sales.Missing" {
            RuntimeError::UnknownEntity(entity.into())
        } else {
            RuntimeError::UnknownObject {
                entity: entity.into(),
                id: "absent".into(),
            }
        };
        assert_eq!(store.commit(entity, "absent").unwrap_err(), expected);
        assert_eq!(store.rollback(entity, "absent").unwrap_err(), expected);
        assert_eq!(store.delete(entity, "absent").unwrap_err(), expected);
        assert_eq!(
            store
                .set_member(entity, "absent", "Status", Value::Null)
                .unwrap_err(),
            expected
        );
    }
    assert!(matches!(
        store.create("Sales.Missing"),
        Err(RuntimeError::UnknownEntity(_))
    ));
    assert!(matches!(
        store.retrieve("Sales.Missing"),
        Err(RuntimeError::UnknownEntity(_))
    ));
    assert!(matches!(
        store.find("Sales.Missing", "absent"),
        Err(RuntimeError::UnknownEntity(_))
    ));
    assert!(store.retrieve("Sales.Order").unwrap().is_empty());
    assert!(store.find("Sales.Order", "absent").unwrap().is_none());
    let order = store.create("Sales.Order").unwrap();
    assert!(matches!(
        store.set_member("Sales.Order", "absent", "Status", Value::Null),
        Err(RuntimeError::UnknownObject { .. })
    ));
    assert_eq!(store.delete("Sales.Order", &order.id).unwrap(), order);
    assert!(store.persistent_objects().is_empty());
}

#[test]
fn malformed_schema_and_member_names_are_rejected_without_inferring_json_types() {
    for name in ["", "   ", "bad\nname", "bad\0name"] {
        let mut invalid_entity =
            Store::new(StoreSchema::default().entity(name, BTreeMap::new(), false));
        assert!(matches!(
            invalid_entity.create(name),
            Err(RuntimeError::Transaction(_))
        ));
        let mut invalid_member = Store::new(StoreSchema::default().entity(
            "Entity",
            BTreeMap::from([(name.into(), Value::Null)]),
            false,
        ));
        assert!(matches!(
            invalid_member.create("Entity"),
            Err(RuntimeError::Transaction(_))
        ));
        let mut store = Store::new(schema());
        let order = store.create("Sales.Order").unwrap();
        assert!(matches!(
            store.set_member("Sales.Order", &order.id, name, Value::Null),
            Err(RuntimeError::Transaction(_))
        ));
        assert_eq!(store.find("Sales.Order", &order.id).unwrap(), Some(order));
    }
    let mut store = Store::new(schema());
    let order = store.create("Sales.Order").unwrap();
    for value in [
        Value::Null,
        json!(true),
        json!(42),
        json!(1.5),
        json!("text"),
        json!([]),
        json!({"nested":"value"}),
    ] {
        store
            .set_member("Sales.Order", &order.id, "Status", value.clone())
            .unwrap();
        assert_eq!(
            store
                .find("Sales.Order", &order.id)
                .unwrap()
                .unwrap()
                .members["Status"],
            value
        );
    }
}

#[test]
fn restoration_is_atomic_and_rejects_invalid_identity_or_member_shapes() {
    let mut store = Store::new(schema());
    store
        .restore_persistent([object("Sales.Order", "existing")])
        .unwrap();
    let before = store.persistent_objects();
    let transient = store.create("Session.Filter").unwrap();
    let mut bad_member = object("Sales.Order", "bad-member");
    bad_member.members.insert("\0".into(), Value::Null);
    for invalid in [
        object("Unknown", "x"),
        object("Session.Filter", "x"),
        object("Sales.Order", ""),
        object("Sales.Order", " \t"),
        object("Sales.Order", "bad\n"),
        bad_member,
        object("Sales.Order", &transient.id),
    ] {
        assert!(matches!(
            store.restore_persistent([object("Sales.Customer", "valid-before-error"), invalid]),
            Err(RuntimeError::InvalidPersistence(_))
        ));
        assert_eq!(store.persistent_objects(), before);
        assert_eq!(
            store.find("Session.Filter", &transient.id).unwrap(),
            Some(transient.clone())
        );
    }
    let mut invalid_schema = Store::new(StoreSchema::default().entity("", BTreeMap::new(), false));
    assert!(matches!(
        invalid_schema.restore_persistent([object("", "id"), object("", "second-id")]),
        Err(RuntimeError::InvalidPersistence(_))
    ));
    for second_entity in ["Sales.Order", "Sales.Customer"] {
        assert!(matches!(
            store.restore_persistent([
                object("Sales.Order", "duplicate"),
                object(second_entity, "duplicate")
            ]),
            Err(RuntimeError::InvalidPersistence(_))
        ));
        assert_eq!(store.persistent_objects(), before);
    }
}

#[test]
fn persistence_replacement_keeps_transient_values_and_their_local_rollback_snapshots() {
    let mut store = Store::new(schema());
    store
        .restore_persistent([object("Sales.Order", "old")])
        .unwrap();
    let transient = store.create("Session.Filter").unwrap();
    store
        .set_member("Session.Filter", &transient.id, "Value", json!("saved"))
        .unwrap();
    store.commit("Session.Filter", &transient.id).unwrap();
    store
        .set_member("Session.Filter", &transient.id, "Value", json!("dirty"))
        .unwrap();
    store
        .restore_persistent([object("Sales.Order", "z"), object("Sales.Customer", "a")])
        .unwrap();
    assert!(store.find("Sales.Order", "old").unwrap().is_none());
    assert_eq!(
        store
            .persistent_objects()
            .iter()
            .map(|object| object.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert_eq!(
        store
            .find("Session.Filter", &transient.id)
            .unwrap()
            .unwrap()
            .members["Value"],
        "dirty"
    );
    store.rollback("Session.Filter", &transient.id).unwrap();
    assert_eq!(
        store
            .find("Session.Filter", &transient.id)
            .unwrap()
            .unwrap()
            .members["Value"],
        "saved"
    );
    store.restore_persistent([]).unwrap();
    assert!(store.persistent_objects().is_empty());
    assert_eq!(store.retrieve("Session.Filter").unwrap().len(), 1);
    store.delete("Session.Filter", &transient.id).unwrap();
    assert!(store.retrieve("Session.Filter").unwrap().is_empty());
}

#[test]
fn uncommitted_transient_rollbacks_remove_the_object_and_committed_edits_survive_boundaries() {
    let mut store = Store::new(schema());
    let transient = store.create("Session.Filter").unwrap();
    store.rollback("Session.Filter", &transient.id).unwrap();
    assert!(
        store
            .find("Session.Filter", &transient.id)
            .unwrap()
            .is_none()
    );
    let order = store.create("Sales.Order").unwrap();
    store
        .transaction(|store| {
            store.set_member("Sales.Order", &order.id, "Status", json!("Paid"))?;
            store.commit("Sales.Order", &order.id)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store
            .find("Sales.Order", &order.id)
            .unwrap()
            .unwrap()
            .members["Status"],
        "Paid"
    );
    store
        .transaction(|store| {
            store.delete("Sales.Order", &order.id)?;
            Ok(())
        })
        .unwrap();
    assert!(store.persistent_objects().is_empty());
}

#[test]
fn association_resolution_handles_short_names_mixed_values_and_deterministic_order() {
    let mut store = Store::new(schema());
    let mut start = object("Sales.Order", "start");
    start
        .members
        .insert("Related".into(), json!(["z", "a", "z", false, null]));
    let mut inverse = object("Sales.Order", "inverse");
    inverse.members.insert("Related".into(), json!("start"));
    let mut irrelevant = object("Sales.Order", "irrelevant");
    irrelevant.members.insert("Related".into(), json!(42));
    store
        .restore_persistent([
            start.clone(),
            object("Sales.Customer", "z"),
            object("Sales.Customer", "a"),
            inverse,
            irrelevant,
        ])
        .unwrap();
    assert_eq!(
        store
            .retrieve_association("Sales.Related", &start)
            .iter()
            .map(|object| object.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "inverse", "z"]
    );
    assert!(store.retrieve_association("Missing", &start).is_empty());
    start.members.insert("Sales.Related".into(), json!("z"));
    assert_eq!(
        store
            .retrieve_association("Sales.Related", &start)
            .iter()
            .map(|object| object.id.as_str())
            .collect::<Vec<_>>(),
        ["inverse", "z"]
    );
}
