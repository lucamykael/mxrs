//! Compiles an `mxrs_ir::Activity` list into the low-level Mendix microflow
//! BSON graph (`StartEvent`/`ActionActivity`/`SequenceFlow`/...) that
//! `mxrs_model::Microflow` persists. Ports the relevant slice of
//! `Writer#build_microflow_graph`/`#process_activity`/`#process_decision`/
//! `#build_activity`/`#activity_action_doc` — scoped to this pass's activity
//! subset (create/change/delete object, commit, call microflow, two-branch
//! decision; see `mxrs-ir`'s `flow` module doc for what's deferred).
//! Targets Mendix 11.x only, so the major-version branches in the Ruby
//! source (e.g. `SequenceFlow`'s pre-v10 Bezier-vector shape, the
//! `ArgumentModel`/`Queue` fields only present on majors 6-10/8-9) are
//! dropped rather than ported.

use mxrs_bson::{doc, Bson, Document};
use mxrs_ir::flow::{Activity, Member};

pub fn build_microflow_graph(
    activities: &[Activity],
    return_expression: Option<&str>,
) -> (Vec<Document>, Vec<Document>) {
    let mut objects = Vec::new();
    let mut flows = Vec::new();

    let start_id = uuid::Uuid::new_v4().to_string();
    objects.push(flow_object_doc(
        &start_id,
        "Microflows$StartEvent",
        50,
        100,
        "20;20",
    ));

    let mut prev_id = Some(start_id);
    let mut x = 190;
    for activity in activities {
        let (next_id, next_x) = process_activity(
            activity,
            prev_id.as_deref(),
            &mut objects,
            &mut flows,
            x,
            100,
        );
        prev_id = next_id;
        x = next_x;
    }

    if let Some(prev) = prev_id {
        let end_id = uuid::Uuid::new_v4().to_string();
        let mut end_doc = flow_object_doc(&end_id, "Microflows$EndEvent", x, 100, "20;20");
        end_doc.insert("Documentation", "");
        end_doc.insert("ReturnValue", return_expression.unwrap_or(""));
        objects.push(end_doc);
        flows.push(sequence_flow_doc(&prev, &end_id, None));
    }

    (objects, flows)
}

struct BranchResult {
    first: Option<String>,
    last: Option<String>,
}

fn process_activity(
    activity: &Activity,
    prev_id: Option<&str>,
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    x: i32,
    y: i32,
) -> (Option<String>, i32) {
    if let Activity::Decision {
        condition,
        true_branch,
        false_branch,
    } = activity
    {
        return process_decision(
            condition,
            true_branch,
            false_branch,
            prev_id,
            objects,
            flows,
            x,
            y,
        );
    }

    let act_id = uuid::Uuid::new_v4().to_string();
    objects.push(build_activity(activity, &act_id, x, y));
    if let Some(prev) = prev_id {
        flows.push(sequence_flow_doc(prev, &act_id, None));
    }
    (Some(act_id), x + 140)
}

#[allow(clippy::too_many_arguments)]
fn process_decision(
    condition: &str,
    true_branch: &[Activity],
    false_branch: &[Activity],
    prev_id: Option<&str>,
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    x: i32,
    y: i32,
) -> (Option<String>, i32) {
    let split_id = uuid::Uuid::new_v4().to_string();
    let mut split_doc = flow_object_doc(&split_id, "Microflows$ExclusiveSplit", x, y, "90;60");
    split_doc.insert("SplitCondition", split_condition_doc(condition));
    split_doc.insert("Caption", condition);
    split_doc.insert("ErrorHandlingType", "Rollback");
    split_doc.insert("Documentation", "");
    objects.push(split_doc);
    if let Some(prev) = prev_id {
        flows.push(sequence_flow_doc(prev, &split_id, None));
    }

    let branch_width = true_branch.len().max(false_branch.len()).max(1) as i32;
    let x_branch = x + 140;
    let x_merge = x + 140 * (branch_width + 1);

    let true_result =
        process_decision_branch(true_branch, &split_id, "true", objects, flows, x_branch, y);
    let false_result = process_decision_branch(
        false_branch,
        &split_id,
        "false",
        objects,
        flows,
        x_branch,
        y + 150,
    );

    let merge_id = uuid::Uuid::new_v4().to_string();
    objects.push(flow_object_doc(
        &merge_id,
        "Microflows$ExclusiveMerge",
        x_merge,
        y,
        "40;40",
    ));
    for (result, case_value) in [(&true_result, "true"), (&false_result, "false")] {
        match &result.first {
            None => flows.push(decision_flow_doc(&split_id, &merge_id, case_value)),
            Some(_) => flows.push(sequence_flow_doc(
                result
                    .last
                    .as_deref()
                    .expect("non-empty branch has a last id"),
                &merge_id,
                None,
            )),
        }
    }
    (Some(merge_id), x_merge + 140)
}

#[allow(clippy::too_many_arguments)]
fn process_decision_branch(
    activities: &[Activity],
    split_id: &str,
    case_value: &str,
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    mut x: i32,
    y: i32,
) -> BranchResult {
    let mut first: Option<String> = None;
    let mut previous: Option<String> = None;
    for activity in activities {
        let before = objects.len();
        let (next_id, next_x) =
            process_activity(activity, previous.as_deref(), objects, flows, x, y);
        let created_first = objects[before].get_str("$ID").ok().map(str::to_string);
        if first.is_none() {
            first = created_first.clone();
        }
        if previous.is_none() {
            flows.push(decision_flow_doc(
                split_id,
                created_first
                    .as_deref()
                    .expect("activity always assigns an $ID"),
                case_value,
            ));
        }
        previous = next_id;
        x = next_x;
    }
    BranchResult {
        first,
        last: previous,
    }
}

fn split_condition_doc(condition: &str) -> Document {
    doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$ExpressionSplitCondition", "Expression": condition }
}

fn decision_flow_doc(split_id: &str, to_id: &str, case_value: &str) -> Document {
    sequence_flow_doc(split_id, to_id, Some(case_value))
}

fn sequence_flow_doc(from_id: &str, to_id: &str, case_value: Option<&str>) -> Document {
    let mut case_doc = doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": if case_value.is_some() { "Microflows$EnumerationCase" } else { "Microflows$NoCase" },
    };
    if let Some(v) = case_value {
        case_doc.insert("Value", v);
    }
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$SequenceFlow",
        "OriginPointer": from_id, "DestinationPointer": to_id,
        "OriginConnectionIndex": 1, "DestinationConnectionIndex": 3,
        "IsErrorHandler": false,
        "Line": doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$BezierCurve",
            "OriginControlVector": "0;0", "DestinationControlVector": "0;0",
        },
        "CaseValues": mxrs_bson::build_array(vec![Bson::Document(case_doc)], 2),
    }
}

fn flow_object_doc(id: &str, ty: &str, x: i32, y: i32, size: &str) -> Document {
    doc! { "$ID": id, "$Type": ty, "RelativeMiddlePoint": format!("{x};{y}"), "Size": size }
}

fn build_activity(activity: &Activity, id: &str, x: i32, y: i32) -> Document {
    let mut doc = flow_object_doc(id, "Microflows$ActionActivity", x, y, "120;60");
    doc.insert("Documentation", "");
    doc.insert("AutoGenerateCaption", true);
    doc.insert("BackgroundColor", "Default");
    doc.insert("Caption", "Activity");
    doc.insert("Action", activity_action_doc(activity));
    doc
}

fn activity_action_doc(activity: &Activity) -> Document {
    match activity {
        Activity::CreateObject {
            variable,
            entity,
            members,
            commit,
        } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$CreateChangeAction",
            "Commit": if *commit { "Yes" } else { "No" },
            "Entity": entity.clone(),
            "ErrorHandlingType": "Rollback",
            "Items": mxrs_bson::build_array(members.iter().map(|m| Bson::Document(change_action_item_doc(m, entity))).collect(), 2),
            "RefreshInClient": false,
            "VariableName": variable.clone(),
        },
        Activity::ChangeObject {
            variable,
            entity,
            members,
            commit,
        } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$ChangeAction",
            "ChangeVariableName": variable.clone(),
            "Commit": if *commit { "Yes" } else { "No" },
            "ErrorHandlingType": "Rollback",
            "Items": mxrs_bson::build_array(members.iter().map(|m| Bson::Document(change_action_item_doc(m, entity))).collect(), 2),
            "RefreshInClient": false,
        },
        Activity::Commit { variable } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$CommitAction",
            "CommitVariableName": variable.clone(),
            "ErrorHandlingType": "Rollback",
            "RefreshInClient": false,
            "WithEvents": true,
        },
        Activity::DeleteObject { variable } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$DeleteAction",
            "DeleteVariableName": variable.clone(),
            "ErrorHandlingType": "Rollback",
            "RefreshInClient": false,
        },
        Activity::CallMicroflow {
            name,
            result_variable,
            use_return,
            mappings,
        } => {
            let mapping_docs: Vec<Bson> = mappings
                .iter()
                .map(|m| {
                    let parameter =
                        if m.parameter.contains('.') { m.parameter.clone() } else { format!("{name}.{}", m.parameter) };
                    Bson::Document(doc! {
                        "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$MicroflowCallParameterMapping",
                        "Parameter": parameter, "Argument": m.value.clone(),
                    })
                })
                .collect();
            doc! {
                "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$MicroflowCallAction",
                "ErrorHandlingType": "Rollback",
                "UseReturnVariable": *use_return,
                "ResultVariableName": result_variable.clone().unwrap_or_default(),
                "MicroflowCall": doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$MicroflowCall",
                    "Microflow": name.clone(),
                    "ParameterMappings": mxrs_bson::build_array(mapping_docs, 2),
                },
            }
        }
        Activity::Decision { .. } => {
            unreachable!("Decision is handled by process_activity, never reaches build_activity")
        }
    }
}

fn change_action_item_doc(member: &Member, entity: &str) -> Document {
    let association = member
        .association
        .as_deref()
        .map(|a| qualified_association_identifier(a, entity))
        .unwrap_or_default();
    let attribute = member
        .attribute
        .as_deref()
        .map(|a| qualified_attribute_identifier(a, entity))
        .unwrap_or_default();
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$ChangeActionItem",
        "Association": association,
        "Attribute": attribute,
        "Type": "Set",
        "Value": member.value.clone(),
        "ValueModel": no_expression_doc(),
    }
}

fn no_expression_doc() -> Document {
    doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Expressions$NoExpression" }
}

/// Qualifies a bare attribute name (e.g. `"Number"`) against its owning
/// entity's qualified name (e.g. `"Sales.Order"`) into `"Sales.Order.Number"`.
/// Simplified relative to `Writer#qualified_attribute_identifier`: this
/// crate's DSL always supplies an already-qualified `entity`, so the
/// "infer the entity from context" branches Ruby needs don't apply here.
fn qualified_attribute_identifier(attribute: &str, entity: &str) -> String {
    let value = attribute.replace('/', ".");
    if value.split('.').count() >= 3 {
        return value;
    }
    if value.contains('.') {
        return value;
    }
    format!("{entity}.{value}")
}

/// Qualifies a bare association name against its owning entity's *module*
/// (not the full entity name — associations are named at module scope).
fn qualified_association_identifier(association: &str, entity: &str) -> String {
    let value = association.replace('/', ".");
    if value.contains('.') {
        return value;
    }
    let module = entity.split('.').next().unwrap_or(entity);
    format!("{module}.{value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_activity_wires_start_activity_end() {
        let activities = vec![Activity::Commit {
            variable: "order".into(),
        }];
        let (objects, flows) = build_microflow_graph(&activities, Some("$order"));
        assert_eq!(objects.len(), 3); // StartEvent, ActionActivity, EndEvent
        assert_eq!(flows.len(), 2);
        assert_eq!(
            objects[0].get_str("$Type").unwrap(),
            "Microflows$StartEvent"
        );
        assert_eq!(objects[2].get_str("$Type").unwrap(), "Microflows$EndEvent");
        assert_eq!(objects[2].get_str("ReturnValue").unwrap(), "$order");
    }

    #[test]
    fn create_object_action_carries_qualified_member_attribute() {
        let activities = vec![Activity::CreateObject {
            variable: "order".into(),
            entity: "Sales.Order".into(),
            members: vec![Member::attribute("Number", "'A-1'")],
            commit: true,
        }];
        let (objects, _) = build_microflow_graph(&activities, None);
        let action = objects[1].get_document("Action").unwrap();
        assert_eq!(
            action.get_str("$Type").unwrap(),
            "Microflows$CreateChangeAction"
        );
        assert_eq!(action.get_str("Commit").unwrap(), "Yes");
        let items =
            mxrs_bson::parse_array(action.get_array("Items").ok().map(|a| a.as_slice())).items;
        let item = items[0].as_document().unwrap();
        assert_eq!(item.get_str("Attribute").unwrap(), "Sales.Order.Number");
        assert_eq!(item.get_str("Value").unwrap(), "'A-1'");
    }

    #[test]
    fn decision_with_empty_else_branch_connects_split_directly_to_merge() {
        let activities = vec![Activity::Decision {
            condition: "$order/Total > 0".into(),
            true_branch: vec![Activity::Commit {
                variable: "order".into(),
            }],
            false_branch: vec![],
        }];
        let (objects, flows) = build_microflow_graph(&activities, None);
        let types: Vec<&str> = objects
            .iter()
            .map(|o| o.get_str("$Type").unwrap())
            .collect();
        assert!(types.contains(&"Microflows$ExclusiveSplit"));
        assert!(types.contains(&"Microflows$ExclusiveMerge"));
        // false branch has no activity, so its flow goes straight from the
        // split to the merge with an EnumerationCase("false").
        let direct_case_flow = flows.iter().find(|f| {
            f.get_str("$Type").unwrap() == "Microflows$SequenceFlow"
                && mxrs_bson::parse_array(f.get_array("CaseValues").ok().map(|a| a.as_slice()))
                    .items
                    .iter()
                    .any(|c| c.as_document().and_then(|d| d.get_str("Value").ok()) == Some("false"))
        });
        assert!(direct_case_flow.is_some());
    }
}
