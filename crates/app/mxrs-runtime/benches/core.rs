use std::collections::BTreeMap;

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

#[divan::bench]
fn create_set_commit_transaction(bencher: Bencher) {
    bencher
        .with_inputs(|| Store::new(schema()))
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

#[divan::bench]
fn authorized_action_dispatch(bencher: Bencher) {
    bencher
        .with_inputs(|| {
            let mut runtime = Runtime::new(Store::new(schema()), SecurityPolicy::default());
            runtime.register_action("Sales.ACT_Echo", |_store: &mut Store, arguments: &Value| {
                Ok(arguments.clone())
            });
            runtime
        })
        .bench_refs(|runtime| {
            runtime
                .invoke(
                    "Sales.ACT_Echo",
                    divan::black_box(&json!({ "id": 42 })),
                    &SecurityContext::default(),
                )
                .unwrap()
        });
}
