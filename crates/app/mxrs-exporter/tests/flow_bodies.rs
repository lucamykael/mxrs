//! Import actually reconstructs supported bodies; edits use the generated Rust.
use mxrs_bson::{Bson, Document};
use mxrs_ir::flow::FlowReturnType as Ty;
use mxrs_ir::{Activity, FlowParameterDecl, MicroflowCallMapping, MicroflowDecl};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("xtask/Cargo.toml").exists())
        .unwrap()
        .into()
}

fn fixture() -> mxrs_ir::ProjectDecl {
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Calls", |m| {
        m.entity("Record", |_| {});
    });
    let mut project = builder.build();
    for (name, ty) in [
        ("Echo", Ty::String),
        ("Object", Ty::Object("Calls.Record".into())),
    ] {
        let mut flow = MicroflowDecl::new(name);
        flow.parameters
            .push(FlowParameterDecl::new("input", ty.clone()));
        flow.return_expression = Some("$input".into());
        flow.return_type = Some(ty);
        project.modules[0].microflows.push(flow);
    }
    let mut list = MicroflowDecl::new("List");
    list.activities.push(Activity::CreateList {
        variable: "records".into(),
        entity: "Calls.Record".into(),
    });
    list.return_type = Some(Ty::List("Calls.Record".into()));
    list.return_expression = Some("$records".into());
    project.modules[0].microflows.push(list);
    let mut caller = MicroflowDecl::new("Caller");
    caller.parameters.push(FlowParameterDecl::new(
        "object",
        Ty::Object("Calls.Record".into()),
    ));
    for (target, variable, ty, value) in [
        ("Echo", "text", Ty::String, Some("'before'")),
        (
            "Object",
            "record",
            Ty::Object("Calls.Record".into()),
            Some("$object"),
        ),
        ("List", "records", Ty::List("Calls.Record".into()), None),
    ] {
        caller.activities.push(Activity::CallMicroflow {
            name: format!("Calls.{target}"),
            result_variable: Some(variable.into()),
            result_type: Some(ty.clone()),
            use_return: true,
            mappings: value
                .map(|v| {
                    vec![MicroflowCallMapping {
                        parameter: "input".into(),
                        value: v.into(),
                        value_type: Some(ty),
                    }]
                })
                .unwrap_or_default(),
        });
    }
    caller.activities.push(Activity::Commit {
        variable: "record".into(),
    });
    caller.activities.push(Activity::DeleteObject {
        variable: "record".into(),
    });
    caller.activities.push(Activity::CallMicroflow {
        name: "Calls.Echo".into(),
        result_variable: None,
        result_type: None,
        use_return: false,
        mappings: vec![MicroflowCallMapping {
            parameter: "input".into(),
            value: "$text".into(),
            value_type: Some(Ty::String),
        }],
    });
    caller.return_type = Some(Ty::String);
    caller.return_expression = Some("$text".into());
    project.modules[0].microflows.push(caller);
    let mut branch = MicroflowDecl::new("Branch");
    branch.activities.push(Activity::Decision {
        condition: "true".into(),
        true_branch: vec![],
        false_branch: vec![],
    });
    project.modules[0].microflows.push(branch);
    let mut client = MicroflowDecl::new("Client");
    client
        .parameters
        .push(FlowParameterDecl::new("input", Ty::Boolean));
    client.return_type = Some(Ty::Boolean);
    client.return_expression = Some("$input".into());
    project.modules[0].nanoflows.push(client);
    project
}

fn flows(path: &Path) -> BTreeMap<String, (String, String, Vec<u8>, Document)> {
    let mpr = mxrs_mpr::MprFile::open(path, true).unwrap();
    mpr.all_units()
        .unwrap()
        .into_iter()
        .filter_map(|unit| {
            let doc = mpr.parse_contents(&unit).unwrap();
            if !matches!(
                doc.get_str("$Type").ok(),
                Some("Microflows$Microflow" | "Microflows$Nanoflow")
            ) {
                return None;
            }
            Some((
                doc.get_str("Name").unwrap().into(),
                (
                    unit.unit_id.clone(),
                    unit.container_id.clone(),
                    mpr.content_bytes(&unit).unwrap().unwrap(),
                    doc,
                ),
            ))
        })
        .collect()
}

fn customize(path: &Path, name: &str, edit: impl FnOnce(&mut Document)) {
    let mut mpr = mxrs_mpr::MprFile::open(path, false).unwrap();
    let mut flow = flows(path).remove(name).unwrap();
    edit(&mut flow.3);
    mpr.update_unit(&flow.0, flow.3).unwrap();
}

fn run(generated: &Path, output: &Path) {
    let result = Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(generated.join("Cargo.toml"))
        .arg("--")
        .arg(output)
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace().join("target")),
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!String::from_utf8_lossy(&result.stderr).contains("warning:"));
}

#[test]
fn decompiled_bodies_are_portable_editable_and_preserve_native_identity_and_layout() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let original = source_dir.join("Original.mpr");
    let generated = dir.path().join("generated");
    let rebuilt = dir.path().join("Rebuilt.mpr");
    mxrs_writer::write_project(&original, &fixture()).unwrap();
    customize(&original, "Caller", |doc| {
        doc.insert("FutureHeader", "kept");
        let objects = doc
            .get_document_mut("ObjectCollection")
            .unwrap()
            .get_array_mut("Objects")
            .unwrap();
        objects[1..].reverse();
        for object in objects.iter_mut().filter_map(Bson::as_document_mut) {
            object.insert("RelativeMiddlePoint", "777;333");
            object.insert("Caption", "Native caption");
        }
        doc.get_array_mut("Flows").unwrap()[1..].reverse();
    });
    let before = flows(&original);
    let report = mxrs_exporter::verify_editable_document_round_trip(&original).unwrap();
    assert!(report.passed, "{:?}", report.failures);
    assert_eq!(report.candidate_units, 5);
    mxrs_exporter::import_cargo_project(&original, &generated, Some(&workspace())).unwrap();
    let editable = generated.join("src/application/microflows/mod.rs");
    let source = std::fs::read_to_string(&editable).unwrap();
    for token in [
        "call_microflow_result",
        "flow.commit(",
        "flow.delete_object(",
        "flow.create_list(",
        "flow.return_value(",
        "mxrs::string(\"before\")",
    ] {
        assert!(source.contains(token), "{token}\n{source}");
    }
    assert!(!source.contains("\"Branch\""));
    for forbidden in ["$ID", "Bson", "Expr::new", ".activities.push(", "777;333"] {
        assert!(!source.contains(forbidden));
    }
    let lib = generated.join("src/lib.rs");
    std::fs::write(
        &lib,
        format!(
            "#![deny(warnings)]\n{}",
            std::fs::read_to_string(&lib).unwrap()
        ),
    )
    .unwrap();
    std::fs::remove_dir_all(source_dir).unwrap();
    run(&generated, &rebuilt);
    assert_eq!(flows(&rebuilt), before);
    let edited = source.replace("mxrs::string(\"before\")", "mxrs::string(\"after\")");
    assert_ne!(edited, source);
    std::fs::write(&editable, edited).unwrap();
    run(&generated, &rebuilt);
    let after = flows(&rebuilt);
    for (name, old) in &before {
        let new = &after[name];
        assert_eq!((&new.0, &new.1), (&old.0, &old.1));
        if name != "Caller" {
            assert_eq!(new, old);
            continue;
        }
        let mut expected = old.3.clone();
        let objects = expected
            .get_document_mut("ObjectCollection")
            .unwrap()
            .get_array_mut("Objects")
            .unwrap();
        for object in objects.iter_mut().filter_map(Bson::as_document_mut) {
            let Ok(call) = object
                .get_document_mut("Action")
                .and_then(|a| a.get_document_mut("MicroflowCall"))
            else {
                continue;
            };
            for mapping in call
                .get_array_mut("ParameterMappings")
                .unwrap()
                .iter_mut()
                .filter_map(Bson::as_document_mut)
            {
                if mapping.get_str("Argument").ok() == Some("'before'") {
                    mapping.insert("Argument", "'after'");
                }
            }
        }
        assert_eq!(new.3, expected, "only the edited argument may change");
    }
}

#[test]
fn unsupported_graphs_and_action_options_are_reported_as_preserved() {
    for mutation in [
        "future",
        "refresh",
        "rollback",
        "expression",
        "duplicate-edge",
        "missing-node",
        "void-capture",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Unsupported.mpr");
        mxrs_writer::write_project(&path, &fixture()).unwrap();
        customize(&path, "Caller", |doc| match mutation {
            "duplicate-edge" => {
                let edges = doc.get_array_mut("Flows").unwrap();
                edges.push(edges[1].clone());
            }
            "missing-node" => {
                doc.get_document_mut("ObjectCollection")
                    .unwrap()
                    .get_array_mut("Objects")
                    .unwrap()
                    .remove(2);
            }
            "void-capture" => {
                doc.get_document_mut("MicroflowReturnType")
                    .unwrap()
                    .insert("$Type", "DataTypes$VoidType");
            }
            _ => {
                let objects = doc
                    .get_document_mut("ObjectCollection")
                    .unwrap()
                    .get_array_mut("Objects")
                    .unwrap();
                let action = objects
                    .iter_mut()
                    .filter_map(Bson::as_document_mut)
                    .find_map(|o| o.get_document_mut("Action").ok())
                    .unwrap();
                match mutation {
                    "future" => {
                        action.insert("FutureBehavior", true);
                    }
                    "refresh" => {
                        action.insert("RefreshInClient", true);
                    }
                    "rollback" => {
                        action.insert("ErrorHandlingType", "CustomWithoutRollBack");
                    }
                    _ => {
                        action
                            .get_document_mut("MicroflowCall")
                            .unwrap()
                            .get_array_mut("ParameterMappings")
                            .unwrap()[1]
                            .as_document_mut()
                            .unwrap()
                            .insert("Argument", "toString([%CurrentDateTime%])");
                    }
                }
            }
        });
        let report = mxrs_exporter::audit_portability(&path).unwrap();
        let family = report
            .families
            .iter()
            .find(|f| f.native_type == "Microflows$Microflow")
            .unwrap();
        assert_eq!(
            (family.partial, family.preserved),
            (3, 2),
            "{mutation}: {family:?}"
        );
        let generated = dir.path().join("generated");
        mxrs_exporter::import_cargo_project(&path, &generated, Some(&workspace())).unwrap();
        let source =
            std::fs::read_to_string(generated.join("src/application/microflows/mod.rs")).unwrap();
        assert!(!source.contains("\"Caller\""), "{mutation}");
    }
}

#[test]
fn scalar_object_and_list_parameters_and_canonical_literals_rebuild_without_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("Types.mpr");
    let generated = dir.path().join("generated");
    let rebuilt = dir.path().join("Rebuilt.mpr");
    let mut project = fixture();
    project.modules[0].microflows.clear();
    project.modules[0].nanoflows.clear();
    for (index, ty) in [
        Ty::String,
        Ty::Integer,
        Ty::Long,
        Ty::Boolean,
        Ty::Float,
        Ty::Decimal,
        Ty::DateTime,
        Ty::Binary,
        Ty::Object("Calls.Record".into()),
        Ty::List("Calls.Record".into()),
    ]
    .into_iter()
    .enumerate()
    {
        let mut flow = MicroflowDecl::new(format!("Parameter{index}"));
        let mut parameter = FlowParameterDecl::new("type", ty.clone());
        parameter.documentation = "Typed input".into();
        parameter.required = true;
        flow.parameters.push(parameter);
        flow.return_type = Some(ty);
        flow.return_expression = Some("$type".into());
        project.modules[0].microflows.push(flow);
    }
    for (index, ty, value) in [
        (0, Ty::String, "'It''s ready'"),
        (1, Ty::Boolean, "true"),
        (2, Ty::Long, "-9223372036854775808"),
        (3, Ty::Float, "1.5"),
        (4, Ty::Decimal, "10.0"),
    ] {
        let mut flow = MicroflowDecl::new(format!("Literal{index}"));
        flow.return_type = Some(ty);
        flow.return_expression = Some(value.into());
        project.modules[0].microflows.push(flow);
    }
    mxrs_writer::write_project(&original, &project).unwrap();
    let before = flows(&original);
    mxrs_exporter::import_cargo_project(&original, &generated, Some(&workspace())).unwrap();
    let source =
        std::fs::read_to_string(generated.join("src/application/microflows/mod.rs")).unwrap();
    for index in 0..10 {
        assert!(source.contains(&format!("\"Parameter{index}\"")));
    }
    for index in 0..5 {
        assert!(source.contains(&format!("\"Literal{index}\"")));
    }
    run(&generated, &rebuilt);
    assert_eq!(flows(&rebuilt), before);
}

#[test]
fn malformed_target_parameters_keep_both_target_and_caller_out_of_the_projection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Malformed.mpr");
    mxrs_writer::write_project(&path, &fixture()).unwrap();
    customize(&path, "Echo", |doc| {
        doc.get_document_mut("MicroflowParameterCollection")
            .unwrap()
            .get_array_mut("Parameters")
            .unwrap()
            .push(Bson::String("malformed".into()));
    });
    let generated = dir.path().join("generated");
    mxrs_exporter::import_cargo_project(&path, &generated, Some(&workspace())).unwrap();
    let source =
        std::fs::read_to_string(generated.join("src/application/microflows/mod.rs")).unwrap();
    assert!(!source.contains("\"Caller\""));
    assert!(!source.contains("\"Echo\""));
    assert!(source.contains("\"Object\""));
}

#[test]
fn duplicate_flow_names_do_not_choose_an_arbitrary_call_signature() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Duplicate.mpr");
    mxrs_writer::write_project(&path, &fixture()).unwrap();
    let echo = flows(&path).remove("Echo").unwrap();
    let mut duplicate = echo.3;
    let id = uuid::Uuid::new_v4().to_string();
    duplicate.insert("$ID", id.clone());
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mpr.insert_unit(&echo.1, "Documents", duplicate, Some(&id))
        .unwrap();
    drop(mpr);
    let generated = dir.path().join("generated");
    mxrs_exporter::import_cargo_project(&path, &generated, Some(&workspace())).unwrap();
    let source =
        std::fs::read_to_string(generated.join("src/application/microflows/mod.rs")).unwrap();
    assert!(!source.contains("\"Echo\""));
    assert!(!source.contains("\"Caller\""));
}
