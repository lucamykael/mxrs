//! Typed call results through persistence, preflight and imported signatures.
use std::path::Path;

use mxrs_bson::{Document, extract_id};
use mxrs_dsl::{CallArgument, ProjectBuilder};
use mxrs_expr::*;
use mxrs_ir::flow::FlowReturnType;
use mxrs_ir::{Activity, EntityMarker, MicroflowMarker, MicroflowRef, ProjectDecl};
use mxrs_mpr::MprFile;

struct Target;
impl MicroflowMarker for Target {
    const MODULE: &'static str = "Calls";
    const NAME: &'static str = "Target";
}
struct Record;
impl EntityMarker for Record {
    const MODULE: &'static str = "Calls";
    const NAME: &'static str = "Record";
}

fn declaration<T: FlowResultType>() -> ProjectDecl {
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Calls", |module| {
        module.entity("Record", |_| {});
        module.microflow("Target", |flow| {
            let input = flow.parameter::<T>("input", |_| {});
            flow.return_value(input);
        });
        module.microflow("Caller", |flow| {
            let input = flow.parameter::<T>("input", |_| {});
            let result = flow.call_microflow_result::<T>(
                MicroflowRef::<Target>::new(),
                "result",
                vec![CallArgument::new("input", input)],
            );
            flow.return_value(result);
        });
    });
    project.build()
}

fn document(mpr: &MprFile, name: &str) -> Document {
    mpr.all_units()
        .unwrap()
        .iter()
        .map(|unit| mpr.parse_contents(unit).unwrap())
        .find(|doc| doc.get_str("Name").ok() == Some(name))
        .unwrap()
}

fn id(doc: &Document) -> String {
    extract_id(doc.get("$ID").unwrap()).unwrap()
}

fn caller_return(path: &Path) -> Document {
    let mpr = MprFile::open(path, true).unwrap();
    document(&mpr, "Caller")
        .get_document("MicroflowReturnType")
        .unwrap()
        .clone()
}

fn assert_type<T: FlowResultType>(native: &str, entity: Option<&str>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Result.mpr");
    let project = declaration::<T>();
    mxrs_writer::write_project(&path, &project).unwrap();
    let returned = caller_return(&path);
    assert_eq!(returned.get_str("$Type").unwrap(), native);
    assert_eq!(returned.get_str("Entity").ok(), entity);
    let mpr = MprFile::open(&path, true).unwrap();
    let caller = mxrs_model::Microflow::from_bson(&document(&mpr, "Caller"));
    let action = caller
        .objects
        .iter()
        .filter_map(|o| o.get_document("Action").ok())
        .find(|action| action.get_str("$Type").ok() == Some("Microflows$MicroflowCallAction"))
        .unwrap();
    assert!(action.get_bool("UseReturnVariable").unwrap());
    assert_eq!(action.get_str("ResultVariableName").unwrap(), "result");
    assert_eq!(
        action
            .get_document("MicroflowCall")
            .unwrap()
            .get_str("Microflow")
            .unwrap(),
        "Calls.Target"
    );
    assert!(
        caller
            .objects
            .iter()
            .any(|o| o.get_str("ReturnValue").ok() == Some("$result"))
    );
}

#[test]
fn scalar_result_tags_persist_and_feed_a_typed_return() {
    assert_type::<MxString>("DataTypes$StringType", None);
    assert_type::<MxBool>("DataTypes$BooleanType", None);
    assert_type::<MxInteger>("DataTypes$IntegerType", None);
    assert_type::<MxLong>("DataTypes$IntegerType", None);
    assert_type::<MxFloat>("DataTypes$FloatType", None);
    assert_type::<MxDecimal>("DataTypes$DecimalType", None);
    assert_type::<MxDateTime>("DataTypes$DateTimeType", None);
    assert_type::<MxBinary>("DataTypes$BinaryType", None);
}

#[test]
fn object_and_list_result_tags_retain_the_entity() {
    assert_type::<MxObject<Record>>("DataTypes$ObjectType", Some("Calls.Record"));
    assert_type::<MxList<Record>>("DataTypes$ListType", Some("Calls.Record"));
}

fn reject_without_mutation(edit: impl FnOnce(&mut ProjectDecl), expected: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Before.mpr");
    let mut project = declaration::<MxString>();
    mxrs_writer::write_project(&path, &project).unwrap();
    let before = std::fs::read(&path).unwrap();
    edit(&mut project);
    let error = mxrs_writer::synchronize_project(&path, &project).unwrap_err();
    assert!(error.to_string().contains(expected), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let fresh = dir.path().join("Rejected.mpr");
    assert!(mxrs_writer::write_project(&fresh, &project).is_err());
    assert!(!fresh.exists());
}

#[test]
fn void_and_incompatible_target_returns_fail_before_writing() {
    reject_without_mutation(
        |p| {
            p.modules[0].microflows[0].return_type = None;
            p.modules[0].microflows[0].return_expression = None;
        },
        "void flow",
    );
    reject_without_mutation(
        |p| {
            if let Activity::CallMicroflow { result_type, .. } =
                &mut p.modules[0].microflows[1].activities[0]
            {
                *result_type = Some(FlowReturnType::Boolean);
            }
        },
        "target returns String",
    );
}

#[test]
fn unchecked_and_inconsistent_capture_flags_fail_before_writing() {
    reject_without_mutation(
        |p| {
            if let Activity::CallMicroflow { result_type, .. } =
                &mut p.modules[0].microflows[1].activities[0]
            {
                *result_type = None;
            }
        },
        "requires a checked type",
    );
    reject_without_mutation(
        |p| {
            if let Activity::CallMicroflow {
                result_variable, ..
            } = &mut p.modules[0].microflows[1].activities[0]
            {
                *result_variable = None;
            }
        },
        "requires a variable name",
    );
    reject_without_mutation(
        |p| {
            if let Activity::CallMicroflow { use_return, .. } =
                &mut p.modules[0].microflows[1].activities[0]
            {
                *use_return = false;
            }
        },
        "discarded call",
    );
}

#[test]
fn result_names_cannot_be_invalid_or_collide_with_a_parameter_or_local() {
    for name in ["", "bad.name", "input"] {
        reject_without_mutation(
            |p| {
                if let Activity::CallMicroflow {
                    result_variable, ..
                } = &mut p.modules[0].microflows[1].activities[0]
                {
                    *result_variable = Some(name.into());
                }
            },
            if name == "input" {
                "duplicate flow variable"
            } else {
                "invalid variable identifier"
            },
        );
    }
    reject_without_mutation(
        |p| {
            let duplicate = p.modules[0].microflows[1].activities[0].clone();
            p.modules[0].microflows[1].activities.push(duplicate);
        },
        "duplicate flow variable",
    );
    reject_without_mutation(
        |p| {
            p.modules[0].microflows[1].activities.insert(
                0,
                Activity::CreateList {
                    variable: "result".into(),
                    entity: "Calls.Record".into(),
                },
            );
        },
        "duplicate flow variable",
    );
}

#[test]
fn result_validation_reaches_nested_branches_and_rescue() {
    for location in ["true", "false", "while", "list", "rescue"] {
        reject_without_mutation(
            |p| {
                let caller = &mut p.modules[0].microflows[1];
                let mut bad = caller.activities.remove(0);
                if let Activity::CallMicroflow { result_type, .. } = &mut bad {
                    *result_type = Some(FlowReturnType::Boolean);
                }
                match location {
                    "true" => caller.activities.push(Activity::Decision {
                        condition: "true".into(),
                        true_branch: vec![bad],
                        false_branch: vec![],
                    }),
                    "false" => caller.activities.push(Activity::Decision {
                        condition: "true".into(),
                        true_branch: vec![],
                        false_branch: vec![bad],
                    }),
                    "while" => caller.activities.push(Activity::WhileLoop {
                        condition: "true".into(),
                        activities: vec![bad],
                    }),
                    "list" => caller.activities.push(Activity::LoopOver {
                        list_variable: "records".into(),
                        iterator: "record".into(),
                        activities: vec![bad],
                    }),
                    _ => caller.rescue_activities.push(bad),
                }
            },
            "target returns String",
        );
    }
}

#[test]
fn sibling_branch_results_have_independent_names() {
    let mut project = declaration::<MxString>();
    let caller = &mut project.modules[0].microflows[1];
    let capture = caller.activities.remove(0);
    caller.activities.push(Activity::Decision {
        condition: "true".into(),
        true_branch: vec![capture.clone()],
        false_branch: vec![capture],
    });
    caller.return_expression = None;
    caller.return_type = None;
    let dir = tempfile::tempdir().unwrap();
    mxrs_writer::write_project(dir.path().join("Branches.mpr"), &project).unwrap();
}

#[test]
fn native_integer_return_accepts_long_and_keeps_target_exact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Native.mpr");
    let project = declaration::<MxLong>();
    mxrs_writer::write_project(&path, &project).unwrap();
    let mut mpr = MprFile::open(&path, false).unwrap();
    let before = document(&mpr, "Target");
    let module = mpr.units_by_containment("Modules").unwrap().remove(0);
    mxrs_writer::documents::synchronize_microflows(
        &mut mpr,
        &module.unit_id,
        &[project.modules[0].microflows[1].clone()],
    )
    .unwrap();
    assert_eq!(document(&mpr, "Target"), before);
}

#[test]
fn unsupported_native_return_is_rejected_only_when_captured() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Future.mpr");
    let project = declaration::<MxString>();
    mxrs_writer::write_project(&path, &project).unwrap();
    let mut mpr = MprFile::open(&path, false).unwrap();
    let mut target = document(&mpr, "Target");
    target
        .get_document_mut("MicroflowReturnType")
        .unwrap()
        .insert("$Type", "DataTypes$FutureType");
    mpr.update_unit(&id(&target), target.clone()).unwrap();
    let module = mpr.units_by_containment("Modules").unwrap().remove(0);
    let mut caller = project.modules[0].microflows[1].clone();
    let before = document(&mpr, "Caller");
    let error = mxrs_writer::documents::synchronize_microflows(
        &mut mpr,
        &module.unit_id,
        &[caller.clone()],
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported native return signature"),
        "{error}"
    );
    assert_eq!(document(&mpr, "Caller"), before);
    if let Activity::CallMicroflow {
        result_variable,
        result_type,
        use_return,
        ..
    } = &mut caller.activities[0]
    {
        *result_variable = None;
        *result_type = None;
        *use_return = false;
    }
    caller.return_expression = None;
    caller.return_type = None;
    mxrs_writer::documents::synchronize_microflows(&mut mpr, &module.unit_id, &[caller]).unwrap();
    assert_eq!(document(&mpr, "Target"), target);
}

#[test]
fn different_object_entities_and_list_cardinality_are_not_interchangeable() {
    for expected in [
        FlowReturnType::List("Calls.Record".into()),
        FlowReturnType::Object("Calls.Other".into()),
    ] {
        let mut project = declaration::<MxObject<Record>>();
        project.modules[0]
            .entities
            .push(mxrs_ir::EntityDecl::new("Other"));
        if let Activity::CallMicroflow { result_type, .. } =
            &mut project.modules[0].microflows[1].activities[0]
        {
            *result_type = Some(expected);
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(
            mxrs_writer::write_project(dir.path().join("Wrong.mpr"), &project)
                .unwrap_err()
                .to_string()
                .contains("target returns Object")
        );
    }
}
