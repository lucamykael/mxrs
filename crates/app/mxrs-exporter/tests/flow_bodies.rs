//! Import actually reconstructs supported bodies; edits use the generated Rust.
use mxrs_bson::{Bson, Document};
use mxrs_ir::flow::FlowReturnType as Ty;
use mxrs_ir::{Activity, FlowParameterDecl, MicroflowCallMapping, MicroflowDecl};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

/// Every module's microflow services. The authored tree is layer-first, so
/// `application/services/` holds one folder per Mendix module.
fn service_files(generated: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let services = generated.join("src/application/services");
    if !services.is_dir() {
        return paths;
    }
    for module in std::fs::read_dir(services).unwrap() {
        let module = module.unwrap().path();
        if !module.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(module).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

fn flow_sources(generated: &Path) -> String {
    service_files(generated)
        .into_iter()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The file declaring `flow_name`: a flow is named by what it does, so
/// `Structured` lives in `structured_service.rs`.
fn flow_source_path(generated: &Path, flow_name: &str) -> PathBuf {
    let file = format!("{}_service.rs", flow_name.to_lowercase());
    service_files(generated)
        .into_iter()
        .find(|path| path.file_name().is_some_and(|name| name == file.as_str()))
        .unwrap_or_else(|| panic!("generated source for flow {flow_name:?} not found"))
}

fn structured_fixture() -> mxrs_ir::ProjectDecl {
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Calls", |m| {
        m.entity("Record", |e| {
            e.string("Name");
            e.boolean("Active");
        });
    });
    let mut project = builder.build();
    let mut flow = MicroflowDecl::new("Structured");
    flow.parameters
        .push(FlowParameterDecl::new("flag", Ty::Boolean));
    flow.parameters.push(FlowParameterDecl::new(
        "records",
        Ty::List("Calls.Record".into()),
    ));
    flow.activities.push(Activity::Decision {
        condition: "$flag".into(),
        true_branch: vec![Activity::LoopOver {
            list_variable: "records".into(),
            iterator: "record".into(),
            activities: vec![
                Activity::WhileLoop {
                    condition: "$record/Active".into(),
                    activities: vec![Activity::Decision {
                        condition: "true".into(),
                        true_branch: vec![Activity::BreakLoop],
                        false_branch: vec![Activity::ContinueLoop],
                    }],
                },
                Activity::Decision {
                    condition: "false".into(),
                    true_branch: vec![Activity::BreakLoop],
                    false_branch: vec![Activity::ChangeObject {
                        variable: "record".into(),
                        entity: "Calls.Record".into(),
                        commit: false,
                        members: vec![mxrs_ir::Member::attribute("Name", "'before'")],
                    }],
                },
                Activity::ContinueLoop,
            ],
        }],
        false_branch: vec![Activity::LoopOver {
            list_variable: "records".into(),
            iterator: "record".into(),
            activities: vec![],
        }],
    });
    flow.activities.push(Activity::WhileLoop {
        condition: "false".into(),
        activities: vec![],
    });
    flow.return_type = Some(Ty::Boolean);
    flow.return_expression = Some("$flag".into());
    project.modules[0].microflows.push(flow.clone());
    flow.name = "ClientStructured".into();
    project.modules[0].nanoflows.push(flow);
    project
}

fn visit_documents(doc: &mut Document, edit: &mut impl FnMut(&mut Document)) {
    edit(doc);
    for (_, value) in doc.iter_mut() {
        match value {
            Bson::Document(child) => visit_documents(child, edit),
            Bson::Array(items) => {
                for child in items.iter_mut().filter_map(Bson::as_document_mut) {
                    visit_documents(child, edit);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn nested_decisions_and_loops_rebuild_exactly_and_edit_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let path = source_dir.join("Structured.mpr");
    let generated = dir.path().join("generated");
    let rebuilt = dir.path().join("Rebuilt.mpr");
    mxrs_writer::write_project(&path, &structured_fixture()).unwrap();
    for name in ["Structured", "ClientStructured"] {
        customize(&path, name, |doc| {
            visit_documents(doc, &mut |node| {
                if node.contains_key("RelativeMiddlePoint") {
                    node.insert("RelativeMiddlePoint", "731;492");
                    node.insert("Caption", "Native caption");
                }
                if let Ok(objects) = node.get_array_mut("Objects") {
                    objects[1..].reverse();
                }
            });
            doc.get_array_mut("Flows").unwrap()[1..].reverse();
        });
    }
    let before = flows(&path);
    let report = mxrs_exporter::verify_editable_document_round_trip(&path).unwrap();
    assert!(report.passed, "{:?}", report.failures);
    assert_eq!(report.candidate_units, 2);
    mxrs_exporter::import_cargo_project(&path, &generated, Some(&workspace())).unwrap();
    let editable = flow_source_path(&generated, "Structured");
    let source = std::fs::read_to_string(&editable).unwrap();
    for token in [
        "flow.decision(",
        "flow.loop_over(",
        "flow.while_loop(",
        "flow.break_loop()",
        "flow.continue_loop()",
        "Record::active()",
    ] {
        assert!(source.contains(token), "{token}\n{source}");
    }
    for forbidden in ["Expr::new", "$ID", "Bson", ".activities.push("] {
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
    // Plain literals: rustfmt may split `string("before")` across lines in
    // the fmt-cleaned project, so edit the string itself.
    let edited = source
        .replace("\"before\"", "\"after\"")
        .replace("&value_flag,", "boolean(false),");
    assert_ne!(source, edited);
    std::fs::write(&editable, edited).unwrap();
    run(&generated, &rebuilt);
    let after = flows(&rebuilt);
    assert_eq!(after["ClientStructured"], before["ClientStructured"]);
    let mut expected = before["Structured"].3.clone();
    visit_documents(&mut expected, &mut |doc| {
        if doc.get_str("Value").ok() == Some("'before'") {
            doc.insert("Value", "'after'");
        }
        if doc.get_str("Expression").ok() == Some("$flag") {
            doc.insert("Expression", "false");
        }
    });
    assert_eq!(after["Structured"].3, expected);
    assert_eq!(after["Structured"].0, before["Structured"].0);
    assert_eq!(after["Structured"].1, before["Structured"].1);
}

#[test]
fn unsupported_control_semantics_and_out_of_scope_variables_stay_preserved() {
    for mutation in [
        "comparison",
        "non-boolean",
        "condition-option",
        "error-handler",
        "loop-option",
        "unknown-loop",
        "wrong-list",
        "iterator-collision",
        "iterator-leak",
        "branch-return",
        "break-outside",
        "branch-local-leak",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Unsupported.mpr");
        let mut project = structured_fixture();
        let flow = &mut project.modules[0].microflows[0];
        match mutation {
            "iterator-leak" => {
                flow.return_type = Some(Ty::Object("Calls.Record".into()));
                flow.return_expression = Some("$record".into());
            }
            "break-outside" => {
                flow.activities.push(Activity::BreakLoop);
            }
            "branch-return" => {
                let Activity::Decision { true_branch, .. } = &mut flow.activities[0] else {
                    unreachable!()
                };
                *true_branch = vec![Activity::ReturnValue {
                    expression: "$flag".into(),
                }];
            }
            "branch-local-leak" => {
                let Activity::Decision { true_branch, .. } = &mut flow.activities[0] else {
                    unreachable!()
                };
                true_branch.push(Activity::CreateList {
                    variable: "local".into(),
                    entity: "Calls.Record".into(),
                });
                flow.return_type = Some(Ty::List("Calls.Record".into()));
                flow.return_expression = Some("$local".into());
            }
            _ => {}
        }
        // Raw graph lowering lets malformed native scope reach the importer;
        // normal authored preflight may correctly reject these declarations.
        mxrs_writer::write_project(&path, &structured_fixture()).unwrap();
        customize(&path, "Structured", |doc| {
            if matches!(
                mutation,
                "iterator-leak" | "break-outside" | "branch-return" | "branch-local-leak"
            ) {
                let (objects, edges) = mxrs_writer::flow_compiler::build_microflow_graph(
                    &flow.activities,
                    &[],
                    flow.return_expression.as_deref(),
                );
                doc.get_document_mut("ObjectCollection").unwrap().insert(
                    "Objects",
                    mxrs_bson::build_array(objects.into_iter().map(Bson::Document).collect(), 2),
                );
                doc.insert(
                    "Flows",
                    mxrs_bson::build_array(edges.into_iter().map(Bson::Document).collect(), 2),
                );
                doc.insert(
                    "MicroflowReturnType",
                    mxrs_writer::flow_compiler::return_type_document(flow.return_type.as_ref())
                        .unwrap(),
                );
            }
            visit_documents(doc, &mut |node| match node.get_str("$Type").ok() {
                Some("Microflows$ExpressionSplitCondition") => match mutation {
                    "comparison" => {
                        node.insert("Expression", "$flag = true");
                    }
                    "non-boolean" => {
                        node.insert("Expression", "$records");
                    }
                    "condition-option" => {
                        node.insert("FutureBehavior", true);
                    }
                    _ => {}
                },
                Some("Microflows$ExclusiveSplit") if mutation == "error-handler" => {
                    node.insert("ErrorHandlingType", "CustomWithoutRollBack");
                }
                Some("Microflows$IterableList") => match mutation {
                    "loop-option" => {
                        node.insert("FutureBehavior", true);
                    }
                    "unknown-loop" => {
                        node.insert("$Type", "Microflows$FutureLoop");
                    }
                    "wrong-list" => {
                        node.insert("ListVariableName", "flag");
                    }
                    "iterator-collision" => {
                        node.insert("VariableName", "flag");
                    }
                    _ => {}
                },
                _ => {}
            });
        });
        let report = mxrs_exporter::audit_portability(&path).unwrap();
        let family = report
            .families
            .iter()
            .find(|f| f.native_type == "Microflows$Microflow")
            .unwrap();
        assert_eq!((family.partial, family.preserved), (0, 1), "{mutation}");
        let generated = dir.path().join("generated");
        mxrs_exporter::import_cargo_project(&path, &generated, Some(&workspace())).unwrap();
        assert!(
            !flow_sources(&generated).contains("pub fn structured("),
            "{mutation}"
        );
    }
}

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
    assert_eq!(report.candidate_units, 6);
    mxrs_exporter::import_cargo_project(&original, &generated, Some(&workspace())).unwrap();
    let editable = flow_source_path(&generated, "Caller");
    let editable_source = std::fs::read_to_string(&editable).unwrap();
    let source = flow_sources(&generated);
    for token in [
        "call_microflow_result",
        "flow.commit(",
        "flow.delete_object(",
        "flow.create_list(",
        "flow.return_value(",
        "string(\"before\")",
    ] {
        assert!(source.contains(token), "{token}\n{source}");
    }
    assert!(source.contains("pub fn branch("), "{source}");
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
    let edited = editable_source.replace("string(\"before\")", "string(\"after\")");
    assert_ne!(edited, editable_source);
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
            (4, 1),
            "{mutation}: {family:?}"
        );
        let generated = dir.path().join("generated");
        mxrs_exporter::import_cargo_project(&path, &generated, Some(&workspace())).unwrap();
        let source = flow_sources(&generated);
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
    let source = flow_sources(&generated);
    for index in 0..10 {
        assert!(source.contains(&format!("pub fn parameter{index}(")));
    }
    for index in 0..5 {
        assert!(source.contains(&format!("pub fn literal{index}(")));
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
    let source = flow_sources(&generated);
    // Neither is declared in Rust; both stay nameable from the imported model.
    assert!(!source.contains("pub fn caller("), "{source}");
    assert!(!source.contains("pub fn echo("), "{source}");
    assert!(source.contains("microflow Caller;"), "{source}");
    assert!(source.contains("microflow Echo;"), "{source}");
    assert!(source.contains("pub fn object("), "{source}");
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
    let source = flow_sources(&generated);
    assert!(!source.contains("pub fn echo("), "{source}");
    assert!(!source.contains("pub fn caller("), "{source}");
}

fn member_fixture() -> mxrs_ir::ProjectDecl {
    use mxrs_ir::Member;
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Calls", |m| {
        m.entity("Record", |e| {
            e.string("Name");
            e.integer("Count");
            e.long("Serial");
            e.float("Weight");
            e.decimal("Amount");
            e.boolean("Active");
            e.datetime("When");
            e.binary("Data");
        });
    });
    let mut project = builder.build();
    let mut flow = MicroflowDecl::new("Create");
    flow.parameters = vec![
        FlowParameterDecl::new("source", Ty::Object("Calls.Record".into())),
        FlowParameterDecl::new("date", Ty::DateTime),
        FlowParameterDecl::new("data", Ty::Binary),
    ];
    flow.activities.push(Activity::CreateObject {
        variable: "record".into(),
        entity: "Calls.Record".into(),
        commit: false,
        members: vec![
            Member::attribute("Name", "'before'"),
            Member::attribute("Count", "1"),
            Member::attribute("Serial", "$source/Count"),
            Member::attribute("Weight", "1.5"),
            Member::attribute("Amount", "10.0"),
            Member::attribute("Active", "true"),
            Member::attribute("When", "$date"),
            Member::attribute("Data", "$data"),
        ],
    });
    flow.activities.push(Activity::ChangeObject {
        variable: "record".into(),
        entity: "Calls.Record".into(),
        commit: true,
        members: [
            "Name", "Count", "Serial", "Weight", "Amount", "Active", "When", "Data",
        ]
        .into_iter()
        .map(|name| Member::attribute(name, format!("$source/{name}")))
        .collect(),
    });
    flow.return_type = Some(Ty::Object("Calls.Record".into()));
    flow.return_expression = Some("$record".into());
    let mut client = flow.clone();
    client.name = "ClientCreate".into();
    project.modules[0].nanoflows.push(client);
    project.modules[0].microflows.push(flow);
    let mut echo = MicroflowDecl::new("Echo");
    echo.parameters
        .push(FlowParameterDecl::new("input", Ty::Long));
    echo.return_type = Some(Ty::Long);
    echo.return_expression = Some("$input".into());
    project.modules[0].microflows.push(echo);
    for (name, call) in [("Read", false), ("Call", true)] {
        let mut flow = MicroflowDecl::new(name);
        flow.parameters.push(FlowParameterDecl::new(
            "record",
            Ty::Object("Calls.Record".into()),
        ));
        flow.return_type = Some(Ty::Long);
        flow.return_expression = Some("$record/Count".into());
        if call {
            flow.activities.push(Activity::CallMicroflow {
                name: "Calls.Echo".into(),
                result_variable: Some("result".into()),
                result_type: Some(Ty::Long),
                use_return: true,
                mappings: vec![MicroflowCallMapping {
                    parameter: "input".into(),
                    value: "$record/Count".into(),
                    value_type: Some(Ty::Long),
                }],
            });
            flow.return_expression = Some("$result".into());
        }
        project.modules[0].microflows.push(flow);
    }
    project
}

#[test]
fn create_change_and_member_reads_are_generated_typed_and_edit_without_other_native_changes() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let original = source_dir.join("Members.mpr");
    let generated = dir.path().join("generated");
    let rebuilt = dir.path().join("Rebuilt.mpr");
    mxrs_writer::write_project(&original, &member_fixture()).unwrap();
    let before = flows(&original);
    let report = mxrs_exporter::verify_editable_document_round_trip(&original).unwrap();
    assert!(report.passed, "{:?}", report.failures);
    assert_eq!(report.candidate_units, 5);
    mxrs_exporter::import_cargo_project(&original, &generated, Some(&workspace())).unwrap();
    let editable = flow_source_path(&generated, "Create");
    let source = std::fs::read_to_string(&editable).unwrap();
    for needle in [
        "flow.create_object(",
        "flow.change_object(",
        "Record::name().set(",
        ".get(Record::count()).into_long()",
    ] {
        assert!(
            source
                .split_whitespace()
                .collect::<String>()
                .contains(needle),
            "{needle}\n{source}"
        );
    }
    // Every attribute is named through the struct that declares it; there
    // is no marker file to keep in step.
    assert!(!generated.join("src/domain/markers").exists());
    let record =
        std::fs::read_to_string(generated.join("src/domain/entities/calls/record.rs")).unwrap();
    for field in [
        "name", "count", "serial", "weight", "amount", "active", "when", "data",
    ] {
        assert!(
            record.contains(&format!("pub {field}: ")),
            "{field}\n{record}"
        );
    }
    std::fs::remove_dir_all(source_dir).unwrap();
    run(&generated, &rebuilt);
    assert_eq!(flows(&rebuilt), before);
    let invalid = source.replace("integer(1)", "boolean(true)");
    assert_ne!(invalid, source);
    std::fs::write(&editable, invalid).unwrap();
    let bytes = std::fs::read(&rebuilt).unwrap();
    let rejected = Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(generated.join("Cargo.toml"))
        .arg("--")
        .arg(&rebuilt)
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace().join("target")),
        )
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        error.contains("IntoExpr") && error.contains("MxInteger"),
        "{error}"
    );
    assert_eq!(std::fs::read(&rebuilt).unwrap(), bytes);
    let edited = source.replace("string(\"before\")", "string(\"after\")");
    assert_ne!(edited, source);
    std::fs::write(&editable, edited).unwrap();
    run(&generated, &rebuilt);
    let after = flows(&rebuilt);
    for (name, old) in &before {
        let new = &after[name];
        assert_eq!((&old.0, &old.1), (&new.0, &new.1));
        if name != "Create" {
            assert_eq!(old, new);
            continue;
        }
        let mut expected = old.3.clone();
        for object in expected
            .get_document_mut("ObjectCollection")
            .unwrap()
            .get_array_mut("Objects")
            .unwrap()
            .iter_mut()
            .filter_map(Bson::as_document_mut)
        {
            let Ok(action) = object.get_document_mut("Action") else {
                continue;
            };
            if action.get_str("$Type").ok() != Some("Microflows$CreateChangeAction") {
                continue;
            }
            for item in action
                .get_array_mut("Items")
                .unwrap()
                .iter_mut()
                .filter_map(Bson::as_document_mut)
            {
                if item.get_str("Value").ok() == Some("'before'") {
                    item.insert("Value", "'after'");
                }
            }
        }
        assert_eq!(new.3, expected);
    }
}

#[test]
fn invalid_members_unsupported_options_and_narrowing_stay_preserved() {
    for mutation in [
        "unknown-member",
        "wrong-owner",
        "wrong-type",
        "overflow",
        "self-reference",
        "duplicate-member",
        "association",
        "operation",
        "refresh",
        "commit",
        "long-to-integer",
        "list-read",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Unsupported.mpr");
        mxrs_writer::write_project(&path, &member_fixture()).unwrap();
        customize(&path, "Create", |doc| {
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
            if mutation == "refresh" {
                action.insert("RefreshInClient", true);
                return;
            }
            if mutation == "commit" {
                action.insert("Commit", "YesWithoutEvents");
                return;
            }
            let items = action.get_array_mut("Items").unwrap();
            if mutation == "duplicate-member" {
                items.push(items[1].clone());
                return;
            }
            let index = if matches!(mutation, "overflow" | "long-to-integer") {
                2
            } else {
                1
            };
            let item = items[index].as_document_mut().unwrap();
            match mutation {
                "unknown-member" => {
                    item.insert("Attribute", "Calls.Record.Missing");
                }
                "wrong-owner" => {
                    item.insert("Attribute", "Calls.Other.Name");
                }
                "wrong-type" => {
                    item.insert("Value", "true");
                }
                "overflow" => {
                    item.insert("Value", "2147483648");
                }
                "self-reference" => {
                    item.insert("Value", "$record/Name");
                }
                "association" => {
                    item.insert("Association", "Calls.Record_Other");
                }
                "operation" => {
                    item.insert("Type", "Add");
                }
                "long-to-integer" => {
                    item.insert("Value", "$source/Serial");
                }
                _ => {
                    item.insert("Value", "$source/Name/Other");
                }
            }
        });
        let generated = dir.path().join("generated");
        mxrs_exporter::import_cargo_project(&path, &generated, Some(&workspace())).unwrap();
        let source = flow_sources(&generated);
        assert!(!source.contains("\"Create\""), "{mutation}");
        let report = mxrs_exporter::audit_portability(&path).unwrap();
        let family = report
            .families
            .iter()
            .find(|f| f.native_type == "Microflows$Microflow")
            .unwrap();
        assert_eq!((family.partial, family.preserved), (3, 1), "{mutation}");
    }
}

#[test]
fn readonly_and_unknown_attribute_shapes_do_not_produce_writable_members() {
    for shape in ["autonumber", "calculated", "unknown"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Attributes.mpr");
        let mut project = member_fixture();
        if shape == "autonumber" {
            project.modules[0].entities[0]
                .attributes
                .iter_mut()
                .find(|a| a.name == "Count")
                .unwrap()
                .attribute_type = mxrs_ir::AttributeType::AutoNumber;
        }
        mxrs_writer::write_project(&path, &project).unwrap();
        if shape != "autonumber" {
            let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
            let unit = mpr
                .all_units()
                .unwrap()
                .into_iter()
                .find(|u| {
                    mpr.parse_contents(u).unwrap().get_str("$Type").ok()
                        == Some("DomainModels$DomainModel")
                })
                .unwrap();
            let mut doc = mpr.parse_contents(&unit).unwrap();
            let entity = doc
                .get_array_mut("entities")
                .unwrap()
                .iter_mut()
                .filter_map(Bson::as_document_mut)
                .find(|e| e.get_str("name").ok() == Some("Record"))
                .unwrap();
            let attribute = entity
                .get_array_mut("attributes")
                .unwrap()
                .iter_mut()
                .filter_map(Bson::as_document_mut)
                .find(|a| a.get_str("name").ok() == Some("Count"))
                .unwrap();
            let field = if shape == "calculated" {
                "Value"
            } else {
                "Type"
            };
            let key = if attribute.contains_key(field) {
                field.to_string()
            } else {
                field.to_ascii_lowercase()
            };
            attribute.get_document_mut(&key).unwrap().insert(
                "$Type",
                if shape == "calculated" {
                    "DomainModels$CalculatedValue"
                } else {
                    "DomainModels$FutureAttributeType"
                },
            );
            mpr.update_unit(&unit.unit_id, doc).unwrap();
        }
        let report = mxrs_exporter::audit_portability(&path).unwrap();
        let family = report
            .families
            .iter()
            .find(|f| f.native_type == "Microflows$Microflow")
            .unwrap();
        assert_eq!(
            family.partial,
            if shape == "unknown" { 1 } else { 3 },
            "{shape}: {family:?}"
        );
        assert!(family.preserved >= 1);
    }
}
