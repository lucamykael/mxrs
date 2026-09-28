//! The port marshalling working against a real flow engine: a project
//! authored with the DSL, written to an `.mpr`, reopened as a model, and
//! called through `FlowEngine` with `PortValue`-converted arguments.

use mxrs::ports::{FlowEngine, PortValue, Variables};
use mxrs::prelude::*;
use mxrs::{Store, StoreSchema};

#[test]
fn port_values_drive_a_real_engine_call() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Ports.mpr");
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("App", |module| {
        module.microflow("Echo", |flow| {
            let input = flow.parameter::<MxString>("input", |_| {});
            flow.return_value(input);
        });
    });
    mxrs::write_project(&path, &builder.build()).unwrap();

    let project = mxrs_model::Project::open(&path, true).unwrap();
    let engine = FlowEngine::from_modules(&project.modules().unwrap());
    let mut store = Store::new(StoreSchema::default());
    let mut arguments = Variables::new();
    arguments.insert("input".to_string(), MxString::to_flow("olá".into()));

    let (result, _) = engine
        .call(&mut store, "App.Echo", arguments, None)
        .unwrap();
    assert_eq!(MxString::from_flow(result).unwrap(), "olá");
}
