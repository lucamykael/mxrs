//! Parameter contracts through real MPR persistence, including native updates.
use std::path::Path;

use mxrs_bson::{Bson, Document, build_array, doc, extract_id, parse_array};
use mxrs_dsl::ProjectBuilder;
use mxrs_expr::*;
use mxrs_ir::flow::FlowReturnType;
use mxrs_ir::{Activity, FlowParameterDecl, MicroflowCallMapping, MicroflowDecl, ProjectDecl};
use mxrs_mpr::MprFile;

fn signature(value_type: FlowReturnType) -> ProjectDecl {
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.entity("Order", |_| {});
        module.microflow("Target", |_| {});
        module.microflow("Caller", |_| {});
    });
    let mut project = builder.build();
    project.modules[0].microflows[0]
        .parameters
        .push(FlowParameterDecl::new("value", value_type));
    project
}

fn call(parameter: &str, value_type: Option<FlowReturnType>) -> Activity {
    Activity::CallMicroflow {
        name: "Sales.Target".into(),
        result_variable: None,
        result_type: None,
        use_return: false,
        mappings: vec![MicroflowCallMapping {
            parameter: parameter.into(),
            value: "$input".into(),
            value_type,
        }],
    }
}

fn flow(path: &Path, name: &str) -> Document {
    let mpr = MprFile::open(path, true).unwrap();
    mpr.all_units()
        .unwrap()
        .iter()
        .map(|unit| mpr.parse_contents(unit).unwrap())
        .find(|doc| doc.get_str("Name").ok() == Some(name))
        .unwrap()
}

fn parameters(doc: &Document) -> Vec<Document> {
    mxrs_model::Microflow::from_bson(doc).parameters
}

fn id(doc: &Document) -> String {
    extract_id(doc.get("$ID").unwrap()).unwrap()
}

fn mutate_flow(path: &Path, name: &str, edit: impl FnOnce(&mut Document)) {
    let mut mpr = MprFile::open(path, false).unwrap();
    let unit = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find(|unit| mpr.parse_contents(unit).unwrap().get_str("Name").ok() == Some(name))
        .unwrap();
    let mut doc = mpr.parse_contents(&unit).unwrap();
    edit(&mut doc);
    mpr.update_unit(&unit.unit_id, doc).unwrap();
}

fn set_parameters(doc: &mut Document, parameters: Vec<Document>) {
    doc.get_document_mut("MicroflowParameterCollection")
        .unwrap()
        .insert(
            "Parameters",
            build_array(parameters.into_iter().map(Bson::Document).collect(), 3),
        );
}

#[test]
fn scalar_builders_persist_native_types_defaults_and_nonvoid_return() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Scalar.mpr");
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.microflow("Scalars", |f| {
            f.parameter::<MxString>("text", |p| {
                p.documentation("Caption")
                    .required(true)
                    .default_value(string("O'Reilly"));
            });
            f.parameter::<MxBool>("flag", |_| {});
            f.parameter::<MxInteger>("small", |_| {});
            let large = f.parameter::<MxLong>("large", |_| {});
            f.parameter::<MxFloat>("ratio", |_| {});
            f.parameter::<MxDecimal>("amount", |_| {});
            f.parameter::<MxDateTime>("stamp", |_| {});
            f.parameter::<MxBinary>("data", |_| {});
            f.return_value(large);
        });
        module.nanoflow("Client", |f| {
            let text = f.parameter::<MxString>("text", |_| {});
            f.return_value(text);
        });
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();
    let document = flow(&path, "Scalars");
    let params = parameters(&document);
    let kinds: Vec<_> = params
        .iter()
        .map(|p| {
            p.get_document("VariableType")
                .unwrap()
                .get_str("$Type")
                .unwrap()
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "DataTypes$StringType",
            "DataTypes$BooleanType",
            "DataTypes$IntegerType",
            "DataTypes$IntegerType",
            "DataTypes$FloatType",
            "DataTypes$DecimalType",
            "DataTypes$DateTimeType",
            "DataTypes$BinaryType"
        ]
    );
    assert_eq!(params[0].get_str("DefaultValue").unwrap(), "'O''Reilly'");
    assert_eq!(params[0].get_str("Documentation").unwrap(), "Caption");
    assert!(params[0].get_bool("IsRequired").unwrap());
    assert!(!params[1].get_bool("IsRequired").unwrap());
    assert_eq!(params[1].get_str("DefaultValue").unwrap(), "");
    assert_eq!(
        document
            .get_document("MicroflowReturnType")
            .unwrap()
            .get_str("$Type")
            .unwrap(),
        "DataTypes$IntegerType"
    );
    let model = mxrs_model::Microflow::from_bson(&document);
    assert!(
        model
            .objects
            .iter()
            .any(|o| o.get_str("ReturnValue").ok() == Some("$large"))
    );
    let client = flow(&path, "Client");
    assert_eq!(client.get_str("$Type").unwrap(), "Microflows$Nanoflow");
    assert_eq!(parameters(&client).len(), 1);
    for p in params {
        assert_eq!(p.keys().next().unwrap(), "$ID");
        assert_ne!(id(&p), id(p.get_document("VariableType").unwrap()));
    }
}

#[test]
fn fresh_parameter_identities_are_deterministic_and_kind_specific() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let mut project = signature(FlowReturnType::String);
    let client = project.modules[0].microflows[0].clone();
    project.modules[0].nanoflows.push(client);
    for directory in [&first, &second] {
        mxrs_writer::write_project(directory.path().join("Same.mpr"), &project).unwrap();
    }
    let before = flow(&first.path().join("Same.mpr"), "Target");
    let after = flow(&second.path().join("Same.mpr"), "Target");
    assert_eq!(id(&parameters(&before)[0]), id(&parameters(&after)[0]));
    assert_eq!(
        id(parameters(&before)[0].get_document("VariableType").unwrap()),
        id(parameters(&after)[0].get_document("VariableType").unwrap())
    );
    let model = mxrs_model::Project::open(first.path().join("Same.mpr"), true).unwrap();
    let modules = model.modules().unwrap();
    assert_ne!(
        id(&modules[0]
            .microflows
            .iter()
            .find(|f| f.name.as_deref() == Some("Target"))
            .unwrap()
            .parameters[0]),
        id(&modules[0].nanoflows[0].parameters[0])
    );
}

#[test]
fn sync_preserves_folder_parameter_type_and_collection_ids_and_future_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Sync.mpr");
    let mut project = signature(FlowReturnType::Object("Sales.Order".into()));
    mxrs_writer::write_project(&path, &project).unwrap();
    mutate_flow(&path, "Target", |doc| {
        doc.insert("FutureHeader", "header");
        doc.insert("ApplyEntityAccess", true);
        let mut params = parameters(doc);
        params[0].insert("FutureParameter", "parameter");
        params[0].insert("RelativeMiddlePoint", "420;120");
        params[0]
            .get_document_mut("VariableType")
            .unwrap()
            .insert("FutureType", "type");
        set_parameters(doc, params);
        doc.get_document_mut("MicroflowParameterCollection")
            .unwrap()
            .insert("FutureCollection", "collection");
    });
    let before = flow(&path, "Target");
    let mut mpr = MprFile::open(&path, false).unwrap();
    let module = mpr.units_by_containment("Modules").unwrap().remove(0);
    let folder = mpr
        .insert_unit(
            &module.unit_id,
            "Folders",
            doc! { "$Type": "Projects$Folder", "Name": "Application" },
            None,
        )
        .unwrap();
    mpr.relocate_unit(&id(&before), &folder, "Documents")
        .unwrap();
    drop(mpr);
    let param = &mut project.modules[0].microflows[0].parameters[0];
    param.value_type = FlowReturnType::String;
    param.documentation = "Edited".into();
    param.default_value = Some("'new'".into());
    param.required = true;
    mxrs_writer::synchronize_project(&path, &project).unwrap();
    let after = flow(&path, "Target");
    assert_eq!(id(&before), id(&after));
    assert_eq!(id(&parameters(&before)[0]), id(&parameters(&after)[0]));
    let old_type = parameters(&before)[0]
        .get_document("VariableType")
        .unwrap()
        .clone();
    let params = parameters(&after);
    let new_type = params[0].get_document("VariableType").unwrap();
    assert_eq!(id(&old_type), id(new_type));
    assert!(!new_type.contains_key("Entity"));
    assert_eq!(new_type.get_str("FutureType").unwrap(), "type");
    assert_eq!(params[0].get_str("FutureParameter").unwrap(), "parameter");
    assert_eq!(params[0].get_str("RelativeMiddlePoint").unwrap(), "420;120");
    assert_eq!(params[0].get_str("Documentation").unwrap(), "Edited");
    assert_eq!(params[0].get_str("DefaultValue").unwrap(), "'new'");
    assert!(params[0].get_bool("IsRequired").unwrap());
    let collection = after.get_document("MicroflowParameterCollection").unwrap();
    assert_eq!(
        id(before.get_document("MicroflowParameterCollection").unwrap()),
        id(collection)
    );
    assert_eq!(
        collection.get_str("FutureCollection").unwrap(),
        "collection"
    );
    assert_eq!(after.get_str("FutureHeader").unwrap(), "header");
    assert!(after.get_bool("ApplyEntityAccess").unwrap());
    let mpr = MprFile::open(&path, true).unwrap();
    assert_eq!(mpr.unit(&id(&after)).unwrap().unwrap().container_id, folder);
    assert_eq!(
        mpr.all_units()
            .unwrap()
            .iter()
            .filter(|u| mpr.parse_contents(u).unwrap().get_str("Name").ok() == Some("Target"))
            .count(),
        1
    );
}

#[test]
fn legacy_graph_parameters_move_to_collection_without_identity_loss() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Legacy.mpr");
    let mut project = signature(FlowReturnType::String);
    mxrs_writer::write_project(&path, &project).unwrap();
    let before = parameters(&flow(&path, "Target"));
    mutate_flow(&path, "Target", |doc| {
        let params = parameters(doc);
        doc.remove("MicroflowParameterCollection");
        let objects = doc.get_document_mut("ObjectCollection").unwrap();
        let mut list = parse_array(objects.get_array("Objects").ok().map(Vec::as_slice)).items;
        list.extend(params.into_iter().map(Bson::Document));
        objects.insert("Objects", build_array(list, 3));
    });
    project.modules[0].microflows[0].parameters[0].documentation = "Updated".into();
    mxrs_writer::synchronize_project(&path, &project).unwrap();
    let after = flow(&path, "Target");
    assert_eq!(id(&before[0]), id(&parameters(&after)[0]));
    assert_eq!(
        parameters(&after)[0].get_str("Documentation").unwrap(),
        "Updated"
    );
    let objects = parse_array(
        after
            .get_document("ObjectCollection")
            .unwrap()
            .get_array("Objects")
            .ok()
            .map(Vec::as_slice),
    );
    assert!(!objects.items.iter().any(|v| {
        v.as_document()
            .is_some_and(|d| d.get_str("$Type").ok() == Some("Microflows$MicroflowParameter"))
    }));
}

#[test]
fn invalid_signatures_fail_before_creating_output() {
    let cases = [
        ("", FlowReturnType::String, "identifier"),
        ("bad.name", FlowReturnType::String, "identifier"),
        ("1value", FlowReturnType::String, "identifier"),
        (
            "value",
            FlowReturnType::Object("Sales.Missing".into()),
            "unknown flow value entity",
        ),
        (
            "value",
            FlowReturnType::List("Sales.Missing".into()),
            "unknown flow value entity",
        ),
    ];
    for (name, kind, message) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Invalid.mpr");
        let mut project = signature(kind);
        project.modules[0].microflows[0].parameters[0].name = name.into();
        let error = mxrs_writer::write_project(&path, &project).unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
        assert!(!path.exists());
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Duplicate.mpr");
    let mut project = signature(FlowReturnType::String);
    project.modules[0].microflows[0]
        .parameters
        .push(FlowParameterDecl::new("value", FlowReturnType::String));
    assert!(
        mxrs_writer::write_project(&path, &project)
            .unwrap_err()
            .to_string()
            .contains("duplicate parameter")
    );
    assert!(!path.exists());
}

#[test]
fn missing_extra_duplicate_unknown_and_mistyped_calls_fail_before_mutation() {
    let valid = call("value", Some(FlowReturnType::String));
    let mut missing = valid.clone();
    if let Activity::CallMicroflow { mappings, .. } = &mut missing {
        mappings.clear();
    }
    let mut duplicate = valid.clone();
    if let Activity::CallMicroflow { mappings, .. } = &mut duplicate {
        mappings.push(mappings[0].clone());
    }
    let mut unknown = valid.clone();
    if let Activity::CallMicroflow { name, .. } = &mut unknown {
        *name = "Sales.Missing".into();
    }
    for (activity, expected) in [
        (missing, "missing argument"),
        (duplicate, "duplicate argument"),
        (unknown, "unknown microflow target"),
        (
            call("extra", Some(FlowReturnType::String)),
            "unknown argument",
        ),
        (
            call("value", Some(FlowReturnType::Boolean)),
            "expects String",
        ),
        (call("value", None), "no checked expression type"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Calls.mpr");
        let mut project = signature(FlowReturnType::String);
        mxrs_writer::write_project(&path, &project).unwrap();
        let original = std::fs::read(&path).unwrap();
        project.modules[0].microflows[1].activities.push(activity);
        let error = mxrs_writer::synchronize_project(&path, &project).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let fresh = dir.path().join("Rejected.mpr");
        assert!(mxrs_writer::write_project(&fresh, &project).is_err());
        assert!(!fresh.exists());
    }
}

#[test]
fn calls_are_checked_in_both_decision_branches_loops_and_rescue() {
    let bad = call("value", Some(FlowReturnType::Boolean));
    for activity in [
        Activity::Decision {
            condition: "true".into(),
            true_branch: vec![bad.clone()],
            false_branch: vec![],
        },
        Activity::Decision {
            condition: "true".into(),
            true_branch: vec![],
            false_branch: vec![bad.clone()],
        },
        Activity::LoopOver {
            list_variable: "items".into(),
            iterator: "item".into(),
            activities: vec![bad.clone()],
        },
        Activity::WhileLoop {
            condition: "true".into(),
            activities: vec![bad.clone()],
        },
    ] {
        let mut project = signature(FlowReturnType::String);
        project.modules[0].microflows[1].activities.push(activity);
        let dir = tempfile::tempdir().unwrap();
        assert!(
            mxrs_writer::write_project(dir.path().join("Nested.mpr"), &project)
                .unwrap_err()
                .to_string()
                .contains("expects String")
        );
    }
    let mut project = signature(FlowReturnType::String);
    project.modules[0].microflows[1].rescue_activities.push(bad);
    let dir = tempfile::tempdir().unwrap();
    assert!(
        mxrs_writer::write_project(dir.path().join("Rescue.mpr"), &project)
            .unwrap_err()
            .to_string()
            .contains("expects String")
    );
}

#[test]
fn object_list_and_entity_mismatches_are_rejected() {
    for actual in [
        FlowReturnType::List("Sales.Order".into()),
        FlowReturnType::Object("Sales.Other".into()),
    ] {
        let mut project = signature(FlowReturnType::Object("Sales.Order".into()));
        project.modules[0].microflows[1]
            .activities
            .push(call("value", Some(actual)));
        let dir = tempfile::tempdir().unwrap();
        assert!(
            mxrs_writer::write_project(dir.path().join("Entity.mpr"), &project)
                .unwrap_err()
                .to_string()
                .contains("expects Object")
        );
    }
}

#[test]
fn standalone_sync_uses_native_signature_and_accepts_qualified_parameter_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Native.mpr");
    let project = signature(FlowReturnType::Integer);
    mxrs_writer::write_project(&path, &project).unwrap();
    let mut caller = MicroflowDecl::new("Caller");
    caller
        .activities
        .push(call("Sales.Target.value", Some(FlowReturnType::Long)));
    let mut mpr = MprFile::open(&path, false).unwrap();
    let module = mpr.units_by_containment("Modules").unwrap().remove(0);
    mxrs_writer::documents::synchronize_microflows(&mut mpr, &module.unit_id, &[caller.clone()])
        .unwrap();
    let before = mpr
        .parse_contents(&mpr.unit(&id(&flow(&path, "Caller"))).unwrap().unwrap())
        .unwrap();
    caller.activities = vec![call("value", Some(FlowReturnType::Boolean))];
    assert!(
        mxrs_writer::documents::synchronize_microflows(&mut mpr, &module.unit_id, &[caller])
            .unwrap_err()
            .to_string()
            .contains("expects Integer")
    );
    assert_eq!(
        mpr.parse_contents(&mpr.unit(&id(&before)).unwrap().unwrap())
            .unwrap(),
        before
    );
}

#[test]
fn unsupported_native_signature_fails_closed_but_unrelated_flows_can_sync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Unknown.mpr");
    mxrs_writer::write_project(&path, &signature(FlowReturnType::String)).unwrap();
    mutate_flow(&path, "Target", |doc| {
        let mut params = parameters(doc);
        params[0]
            .get_document_mut("VariableType")
            .unwrap()
            .insert("$Type", "DataTypes$FutureType");
        set_parameters(doc, params);
    });
    let mut mpr = MprFile::open(&path, false).unwrap();
    let module = mpr.units_by_containment("Modules").unwrap().remove(0);
    mxrs_writer::documents::synchronize_microflows(
        &mut mpr,
        &module.unit_id,
        &[MicroflowDecl::new("Unrelated")],
    )
    .unwrap();
    let mut caller = MicroflowDecl::new("Caller");
    caller
        .activities
        .push(call("value", Some(FlowReturnType::String)));
    assert!(
        mxrs_writer::documents::synchronize_microflows(&mut mpr, &module.unit_id, &[caller])
            .unwrap_err()
            .to_string()
            .contains("unsupported native data type")
    );
}

#[test]
fn duplicate_native_parameters_reject_the_whole_project_before_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("DuplicateNative.mpr");
    let mut project = signature(FlowReturnType::String);
    mxrs_writer::write_project(&path, &project).unwrap();
    mutate_flow(&path, "Target", |doc| {
        let mut params = parameters(doc);
        params.push(params[0].clone());
        set_parameters(doc, params);
    });
    let before = std::fs::read(&path).unwrap();
    project.modules[0].entities[0].documentation = "Must not change".into();
    assert!(
        mxrs_writer::synchronize_project(&path, &project)
            .unwrap_err()
            .to_string()
            .contains("duplicate native parameter")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
#[should_panic(expected = "outer flow builder")]
fn branch_parameter_declarations_never_disappear_silently() {
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.microflow("Bad", |flow| {
            flow.decision(
                boolean(true),
                |branch| {
                    branch.parameter::<MxString>("lost", |_| {});
                },
                |_| {},
            );
        });
    });
}

#[test]
fn overwriting_an_unsupported_native_signature_is_rejected_before_domain_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Future.mpr");
    let mut project = signature(FlowReturnType::String);
    mxrs_writer::write_project(&path, &project).unwrap();
    mutate_flow(&path, "Target", |doc| {
        let mut params = parameters(doc);
        params[0]
            .get_document_mut("VariableType")
            .unwrap()
            .insert("$Type", "DataTypes$FutureType");
        set_parameters(doc, params);
    });
    let before = std::fs::read(&path).unwrap();
    project.modules[0].entities[0].documentation = "Must not change".into();
    assert!(
        mxrs_writer::synchronize_project(&path, &project)
            .unwrap_err()
            .to_string()
            .contains("unsupported native data type")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn duplicate_flow_declarations_are_rejected_before_creation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Duplicate.mpr");
    let mut project = signature(FlowReturnType::String);
    let duplicate = project.modules[0].microflows[0].clone();
    project.modules[0].microflows.push(duplicate);
    assert!(
        mxrs_writer::write_project(&path, &project)
            .unwrap_err()
            .to_string()
            .contains("duplicate flow declaration")
    );
    assert!(!path.exists());
}
