use std::collections::{BTreeMap, BTreeSet};

use divan::Bencher;
use mxrs_runtime::{Runtime, SecurityContext, SecurityPolicy, Store, StoreSchema};
use serde_json::{Value, json};

fn main() {
    divan::main();
}

fn schema() -> StoreSchema {
    StoreSchema::default().entity(
        "Sales.Order",
        BTreeMap::from([("Status".to_string(), json!("New"))]),
        false,
    )
}

fn store_with_committed_objects(count: usize) -> Store {
    let mut store = Store::new(schema());
    for _ in 0..count {
        let object = store.create("Sales.Order").unwrap();
        store.commit("Sales.Order", &object.id).unwrap();
    }
    store
}

#[divan::bench(consts = [0, 100, 1_000])]
fn create_set_commit_transaction<const N: usize>(bencher: Bencher) {
    bencher
        .with_inputs(|| store_with_committed_objects(N))
        .bench_refs(|store| {
            store
                .transaction(|store| {
                    let order = store.create("Sales.Order")?;
                    store.set_member("Sales.Order", &order.id, "Status", json!("Paid"))?;
                    store.commit("Sales.Order", &order.id)
                })
                .unwrap()
        });
}

#[divan::bench(consts = [0, 100, 1_000])]
fn authorized_action_dispatch<const N: usize>(bencher: Bencher) {
    let context = SecurityContext {
        user: Some("benchmark-user".into()),
        module_roles: BTreeSet::from(["Sales.User".into()]),
        ..SecurityContext::default()
    };
    let arguments = json!({ "id": 42 });
    bencher
        .with_inputs(|| {
            let security = SecurityPolicy {
                enabled: true,
                documents: BTreeMap::from([(
                    "Sales.ACT_Echo".into(),
                    BTreeSet::from(["Sales.User".into()]),
                )]),
                ..SecurityPolicy::default()
            };
            let mut runtime = Runtime::new(store_with_committed_objects(N), security);
            runtime.register_action("Sales.ACT_Echo", |_store: &mut Store, arguments: &Value| {
                Ok(arguments.clone())
            });
            runtime
        })
        .bench_refs(|runtime| {
            runtime
                .invoke(
                    "Sales.ACT_Echo",
                    divan::black_box(&arguments),
                    divan::black_box(&context),
                )
                .unwrap()
        });
}
