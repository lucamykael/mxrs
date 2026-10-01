//! Editable callers consume imported signatures without the original MPR.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mxrs_bson::Document;
use mxrs_ir::flow::FlowReturnType;
use mxrs_ir::{FlowParameterDecl, MicroflowDecl};

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("xtask/Cargo.toml").is_file())
        .unwrap()
        .to_path_buf()
}

fn fixture() -> mxrs_ir::ProjectDecl {
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Calls", |module| {
        module.entity("Record", |_| {});
    });
    let mut project = builder.build();
    for (name, value_type) in [
        ("Text", FlowReturnType::String),
        ("Object", FlowReturnType::Object("Calls.Record".into())),
        ("List", FlowReturnType::List("Calls.Record".into())),
    ] {
        let mut flow = MicroflowDecl::new(name);
        flow.parameters
            .push(FlowParameterDecl::new("input", value_type.clone()));
        flow.return_type = Some(value_type);
        flow.return_expression = Some("$input".into());
        project.modules[0].microflows.push(flow);
    }
    project
}

/// A hand-written service in the shape the importer generates: the flow is
/// the function that builds it, the entity is named by its struct, and the
/// flows it calls — which this file does not declare — are named with
/// `imported!`.
const CALLERS: &str = r#"
#![deny(warnings)]
use mxrs::prelude::*;

use crate::domain::entities::calls::record::Record;

mxrs::imported! {
    module = "Calls";
    microflow Text;
    microflow Object;
    microflow List;
}

#[microflow(module = "Calls")]
pub fn caller(flow: &mut FlowBuilder) {
    let object = flow.object_parameter("object", Ref::<Record>::new(), |_| {});
    let list = flow.list_parameter("list", Ref::<Record>::new(), |_| {});
    let text = flow.call_microflow_result::<MxString>(
        MicroflowRef::<Text>::new(), "text",
        vec![CallArgument::new("input", string("before"))]);
    let record = flow.call_microflow_result::<MxObject<Record>>(
        MicroflowRef::<Object>::new(), "record",
        vec![CallArgument::new("input", object)]);
    flow.commit(&record);
    let records = flow.call_microflow_result::<MxList<Record>>(
        MicroflowRef::<List>::new(), "records",
        vec![CallArgument::new("input", list)]);
    flow.loop_over(&records, "item", |flow, item| { flow.commit(&item); });
    let result = flow.call_microflow_result::<MxString>(
        MicroflowRef::<Text>::new(), "result",
        vec![CallArgument::new("input", text)]);
    flow.return_value(result);
}
"#;

fn run(generated: &Path, output: &Path) -> Output {
    Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(generated.join("Cargo.toml"))
        .arg("--")
        .arg(output)
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace_root().join("target")),
        )
        .output()
        .unwrap()
}

fn flows(path: &Path) -> Vec<(String, String, Vec<u8>, Document)> {
    let mpr = mxrs_mpr::MprFile::open(path, true).unwrap();
    let mut flows: Vec<_> = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .filter_map(|unit| {
            let document = mpr.parse_contents(&unit).unwrap();
            (document.get_str("$Type").ok() == Some("Microflows$Microflow")).then(|| {
                let bytes = mpr.content_bytes(&unit).unwrap().unwrap();
                (unit.unit_id, unit.container_id, bytes, document)
            })
        })
        .collect();
    flows.sort_by(|a, b| a.0.cmp(&b.0));
    flows
}

#[test]
fn imported_callees_survive_typed_caller_edits_and_transactional_rejections() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let source = source_dir.join("Original.mpr");
    let generated = dir.path().join("generated");
    let output = dir.path().join("Rebuilt.mpr");
    mxrs_writer::write_project(&source, &fixture()).unwrap();
    let before = flows(&source);
    assert_eq!(before.len(), 3);
    mxrs_exporter::import_cargo_project(&source, &generated, Some(&workspace_root())).unwrap();
    std::fs::remove_dir_all(&source_dir).unwrap();
    // Reuse a registered per-flow module so this edit replaces the typed Text
    // overlay while adding Caller; the imported Text document remains preserved.
    let editable = generated.join("src/application/services/calls/text_service.rs");
    std::fs::write(&editable, CALLERS).unwrap();
    let built = run(&generated, &output);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let first = flows(&output);
    let caller = first
        .iter()
        .find(|f| f.3.get_str("Name").ok() == Some("Caller"))
        .unwrap();
    let parsed = mxrs_model::Microflow::from_bson(&caller.3);
    let names: Vec<_> = parsed
        .objects
        .iter()
        .filter_map(|o| o.get_document("Action").ok())
        .filter(|a| a.get_bool("UseReturnVariable").ok() == Some(true))
        .map(|a| a.get_str("ResultVariableName").unwrap())
        .collect();
    assert_eq!(names, ["text", "record", "records", "result"]);
    assert!(
        parsed
            .objects
            .iter()
            .any(|o| o.get_str("ReturnValue").ok() == Some("$result"))
    );
    assert_eq!(
        first
            .iter()
            .filter(|f| f.0 != caller.0)
            .cloned()
            .collect::<Vec<_>>(),
        before
    );

    // The same valid Rust type-checks, but the imported callee has a string
    // return. Preflight must reject it before replacing an existing output.
    std::fs::write(
        &editable,
        CALLERS.replace(
            "call_microflow_result::<MxString>(",
            "call_microflow_result::<mxrs::MxBool>(",
        ),
    )
    .unwrap();
    let bytes = std::fs::read(&output).unwrap();
    let rejected = run(&generated, &output);
    assert!(!rejected.status.success());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("target returns String"), "{error}");
    assert_eq!(std::fs::read(&output).unwrap(), bytes);

    std::fs::write(&editable, CALLERS.replace("\"before\"", "\"after\"")).unwrap();
    let edited = run(&generated, &output);
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );
    let after = flows(&output);
    let changed = after
        .iter()
        .find(|f| f.3.get_str("Name").ok() == Some("Caller"))
        .unwrap();
    assert_eq!(changed.0, caller.0);
    assert_eq!(changed.1, caller.1);
    assert_ne!(changed.3, caller.3);
    assert_eq!(
        after
            .into_iter()
            .filter(|f| f.0 != caller.0)
            .collect::<Vec<_>>(),
        before
    );
}
