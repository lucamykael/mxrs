//! Compiles an `mxrs_ir::Activity` list into the low-level Mendix microflow
//! BSON graph (`StartEvent`/`ActionActivity`/`SequenceFlow`/...) that
//! `mxrs_model::Microflow` persists. Ports the relevant slice of
//! `Writer#build_microflow_graph`/`#process_activity`/`#process_decision`/
//! `#build_activity`/`#activity_action_doc` — scoped to this pass's activity
//! subset (create/change/delete object, commit, call microflow, two-branch
//! decision, functional-test retrieval/count/log/return activities; see
//! `mxrs-ir`'s `flow` module doc for what's deferred).
//! Targets Mendix 11.x only, so the major-version branches in the Ruby
//! source (e.g. `SequenceFlow`'s pre-v10 Bezier-vector shape, the
//! `ArgumentModel`/`Queue` fields only present on majors 6-10/8-9) are
//! dropped rather than ported.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document, doc};
use mxrs_ir::flow::{Activity, FlowReturnType, Member};

pub fn return_type_document(return_type: Option<&FlowReturnType>) -> Option<Document> {
    let return_type = return_type?;
    let id = uuid::Uuid::new_v4().to_string();
    let document = match return_type {
        FlowReturnType::String => doc! { "$ID": id, "$Type": "DataTypes$StringType" },
        // Flow Integer is the native 64-bit integer type for both attribute tags.
        FlowReturnType::Integer | FlowReturnType::Long => {
            doc! { "$ID": id, "$Type": "DataTypes$IntegerType" }
        }
        FlowReturnType::Float => doc! { "$ID": id, "$Type": "DataTypes$FloatType" },
        FlowReturnType::Decimal => doc! { "$ID": id, "$Type": "DataTypes$DecimalType" },
        FlowReturnType::Boolean => doc! { "$ID": id, "$Type": "DataTypes$BooleanType" },
        FlowReturnType::DateTime => doc! { "$ID": id, "$Type": "DataTypes$DateTimeType" },
        FlowReturnType::Binary => doc! { "$ID": id, "$Type": "DataTypes$BinaryType" },
        FlowReturnType::Object(entity) => {
            doc! { "$ID": id, "$Type": "DataTypes$ObjectType", "Entity": entity.clone() }
        }
        FlowReturnType::List(entity) => {
            doc! { "$ID": id, "$Type": "DataTypes$ListType", "Entity": entity.clone() }
        }
        FlowReturnType::Enumeration(enumeration) => {
            doc! { "$ID": id, "$Type": "DataTypes$EnumerationType", "Enumeration": enumeration.clone() }
        }
    };
    Some(document)
}

/// A stated action document as the model stores it: every nested document
/// gets an identity of its own, and every list the marker it was declared
/// with.
pub(crate) fn native_document(document: &mxrs_ir::NativeDocument) -> Document {
    let mut lowered = doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": document.ty.clone(),
    };
    for (key, value) in &document.fields {
        lowered.insert(key.clone(), native_value(value));
    }
    lowered
}

fn native_value(value: &mxrs_ir::NativeValue) -> Bson {
    use mxrs_ir::NativeValue;
    match value {
        NativeValue::Null => Bson::Null,
        NativeValue::Bool(value) => Bson::Boolean(*value),
        NativeValue::Int32(value) => Bson::Int32(*value),
        NativeValue::Int64(value) => Bson::Int64(*value),
        NativeValue::Text(value) => Bson::String(value.clone()),
        NativeValue::Document(document) => Bson::Document(native_document(document)),
        NativeValue::List(marker, items) => Bson::Array(mxrs_bson::build_array(
            items.iter().map(native_value).collect(),
            *marker,
        )),
    }
}

/// Lowers the body of a microflow or, with `nanoflow`, of a nanoflow. The
/// two differ in what an activity does about failing when nothing says
/// otherwise: a microflow rolls back, a nanoflow — which has no transaction
/// to roll back — aborts.
pub fn build_flow_graph(
    activities: &[Activity],
    rescue_activities: &[Activity],
    return_expression: Option<&str>,
    nanoflow: bool,
) -> (Vec<Document>, Vec<Document>) {
    let (mut objects, flows) =
        build_microflow_graph(activities, rescue_activities, return_expression);
    if nanoflow {
        for object in &mut objects {
            abort_on_error(object);
        }
    }
    (objects, flows)
}

fn abort_on_error(document: &mut Document) {
    if document.get_str("ErrorHandlingType").ok() == Some("Rollback") {
        document.insert("ErrorHandlingType", "Abort");
    }
    for (_, value) in document.iter_mut() {
        match value {
            Bson::Document(inner) => abort_on_error(inner),
            Bson::Array(items) => {
                for item in items {
                    if let Bson::Document(inner) = item {
                        abort_on_error(inner);
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn build_microflow_graph(
    activities: &[Activity],
    rescue_activities: &[Activity],
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
    let mut rescue_origin = None;
    for (index, activity) in activities.iter().enumerate() {
        // Once a path has ended only a label, which jumps reach, starts
        // another one.
        if prev_id.is_none() && !matches!(activity, Activity::Label(_)) {
            continue;
        }
        let error_handling = if !rescue_activities.is_empty() && index + 1 == activities.len() {
            "CustomWithoutRollBack"
        } else {
            "Rollback"
        };
        let (next_id, next_x) = process_activity(
            activity,
            prev_id.as_deref(),
            &mut objects,
            &mut flows,
            x,
            100,
            error_handling,
        );
        if index + 1 == activities.len() {
            rescue_origin = next_id.clone();
        }
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

    if let Some(origin) = rescue_origin {
        build_rescue_branch(
            &origin,
            rescue_activities,
            &mut objects,
            &mut flows,
            190,
            250,
        );
    }

    // A jump is lowered before anything knows which merge its label became:
    // its edge points at the label's name until every merge exists.
    let mut labels = HashMap::new();
    for object in &mut objects {
        collect_labels(object, &mut labels);
    }
    for flow in &mut flows {
        if let Some(name) = flow
            .get_str("DestinationPointer")
            .ok()
            .and_then(|pointer| pointer.strip_prefix(LABEL))
            && let Some(merge) = labels.get(name)
        {
            flow.insert("DestinationPointer", merge.as_str());
        }
    }

    (objects, flows)
}

/// What a jump's edge points at until its label's merge is known, followed
/// by the label's name.
const LABEL: &str = "label:";

fn collect_labels(object: &mut Document, labels: &mut HashMap<String, String>) {
    if let Some(Bson::String(name)) = object.remove("$Label")
        && let Ok(id) = object.get_str("$ID")
    {
        labels.insert(name, id.to_string());
    }
    if let Ok(collection) = object.get_document_mut("ObjectCollection")
        && let Ok(inner) = collection.get_array_mut("Objects")
    {
        for item in inner {
            if let Bson::Document(inner) = item {
                collect_labels(inner, labels);
            }
        }
    }
}

struct BranchResult {
    first: Option<String>,
    last: Option<String>,
    terminal: bool,
}

fn process_activity(
    activity: &Activity,
    prev_id: Option<&str>,
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    x: i32,
    y: i32,
    error_handling: &str,
) -> (Option<String>, i32) {
    if let Activity::Disabled(inner) = activity {
        let first = objects.len();
        let result = process_activity(inner, prev_id, objects, flows, x, y, error_handling);
        // The model disables one node; what an activity lowers to first is
        // the node that stands for it.
        if let Some(node) = objects.get_mut(first) {
            node.insert("Disabled", true);
        }
        return result;
    }
    if let Activity::OnError {
        handling,
        activity: inner,
        handler,
    } = activity
    {
        return process_handled(*handling, inner, handler, prev_id, objects, flows, x, y);
    }
    match activity {
        Activity::Decision {
            condition,
            true_branch,
            false_branch,
        } => {
            let mut split = flow_object_doc("", "Microflows$ExclusiveSplit", x, y, "90;60");
            split.insert("SplitCondition", split_condition_doc(condition));
            split.insert("Caption", condition.as_str());
            split.insert("ErrorHandlingType", "Rollback");
            split.insert("Documentation", "");
            let branches = [
                (vec!["true".to_string()], true_branch.as_slice()),
                (vec!["false".to_string()], false_branch.as_slice()),
            ];
            return process_split(
                split,
                "Microflows$EnumerationCase",
                &branches,
                prev_id,
                objects,
                flows,
                x,
                y,
            );
        }
        Activity::RuleDecision {
            rule,
            arguments,
            true_branch,
            false_branch,
        } => {
            let branches = [
                (vec!["true".to_string()], true_branch.as_slice()),
                (vec!["false".to_string()], false_branch.as_slice()),
            ];
            return process_split(
                rule_split(rule, arguments, x, y),
                "Microflows$EnumerationCase",
                &branches,
                prev_id,
                objects,
                flows,
                x,
                y,
            );
        }
        Activity::Label(name) => {
            let merge_id = uuid::Uuid::new_v4().to_string();
            let mut merge = flow_object_doc(&merge_id, "Microflows$ExclusiveMerge", x, y, "40;40");
            merge.insert("$Label", name.as_str());
            objects.push(merge);
            if let Some(prev) = prev_id {
                flows.push(sequence_flow_doc(prev, &merge_id, None));
            }
            return (Some(merge_id), x + 80);
        }
        Activity::Jump(name) => {
            if let Some(prev) = prev_id {
                flows.push(sequence_flow_doc(prev, &format!("{LABEL}{name}"), None));
            }
            return (None, x);
        }
        Activity::RuleSwitch {
            rule,
            arguments,
            cases,
        } => {
            let branches: Vec<_> = cases
                .iter()
                .map(|case| (case.values.clone(), case.activities.as_slice()))
                .collect();
            return process_split(
                rule_split(rule, arguments, x, y),
                "Microflows$EnumerationCase",
                &branches,
                prev_id,
                objects,
                flows,
                x,
                y,
            );
        }
        Activity::Switch { expression, cases } => {
            let mut split = flow_object_doc("", "Microflows$ExclusiveSplit", x, y, "90;60");
            split.insert("SplitCondition", split_condition_doc(expression));
            split.insert("Caption", expression.as_str());
            split.insert("ErrorHandlingType", "Rollback");
            split.insert("Documentation", "");
            let branches: Vec<_> = cases
                .iter()
                .map(|case| (case.values.clone(), case.activities.as_slice()))
                .collect();
            return process_split(
                split,
                "Microflows$EnumerationCase",
                &branches,
                prev_id,
                objects,
                flows,
                x,
                y,
            );
        }
        Activity::TypeSwitch { variable, cases } => {
            let mut split = flow_object_doc("", "Microflows$InheritanceSplit", x, y, "90;60");
            split.insert("SplitVariableName", variable.as_str());
            split.insert("Caption", "");
            split.insert("Documentation", "");
            let branches: Vec<_> = cases
                .iter()
                .map(|case| (case.values.clone(), case.activities.as_slice()))
                .collect();
            return process_split(
                split,
                "Microflows$InheritanceCase",
                &branches,
                prev_id,
                objects,
                flows,
                x,
                y,
            );
        }
        _ => {}
    }

    match activity {
        Activity::LoopOver {
            list_variable,
            iterator,
            activities,
        } => {
            return process_loop(
                LoopSource::Iterable(list_variable, iterator),
                activities,
                prev_id,
                objects,
                flows,
                x,
                y,
            );
        }
        Activity::WhileLoop {
            condition,
            activities,
        } => {
            return process_loop(
                LoopSource::While(condition),
                activities,
                prev_id,
                objects,
                flows,
                x,
                y,
            );
        }
        _ => {}
    }

    let act_id = uuid::Uuid::new_v4().to_string();
    let terminal_type = match activity {
        Activity::BreakLoop => Some(("Microflows$BreakEvent", None)),
        Activity::RaiseError => Some(("Microflows$ErrorEvent", None)),
        Activity::ContinueLoop => Some(("Microflows$ContinueEvent", None)),
        Activity::ReturnValue { expression } => {
            Some(("Microflows$EndEvent", Some(expression.as_str())))
        }
        _ => None,
    };
    if let Some((ty, expression)) = terminal_type {
        let mut object = flow_object_doc(&act_id, ty, x, y, "20;20");
        if let Some(expression) = expression {
            object.insert("Documentation", "");
            object.insert("ReturnValue", expression);
        }
        objects.push(object);
    } else {
        objects.push(build_activity(activity, &act_id, x, y, error_handling));
    }
    if let Some(prev) = prev_id {
        flows.push(sequence_flow_doc(prev, &act_id, None));
    }
    if terminal_type.is_some() {
        (None, x + 140)
    } else {
        (Some(act_id), x + 140)
    }
}

/// Lowers an activity with an error handler of its own: the activity, the
/// handler hanging off it by an error edge, and — when the handler does not
/// end the flow — a merge after the activity where the two paths join.
#[allow(clippy::too_many_arguments)]
fn process_handled(
    handling: mxrs_ir::ErrorHandling,
    activity: &Activity,
    handler: &[Activity],
    prev_id: Option<&str>,
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    x: i32,
    y: i32,
) -> (Option<String>, i32) {
    let first = objects.len();
    let (next, next_x) = process_activity(
        activity,
        prev_id,
        objects,
        flows,
        x,
        y,
        handling.native_name(),
    );
    // An action says how it fails in its action document, a loop on its node.
    if let Some(node) = objects.get_mut(first) {
        match node.get_document_mut("Action") {
            Ok(action) => {
                action.insert("ErrorHandlingType", handling.native_name());
            }
            Err(_) => {
                node.insert("ErrorHandlingType", handling.native_name());
            }
        }
    }
    let Some(origin) = next else {
        return (None, next_x);
    };
    if handling == mxrs_ir::ErrorHandling::Continue {
        return (Some(origin), next_x);
    }
    let link = |to: &str| flow_doc(&origin, to, &[], true);
    let branch = process_branch(handler, &link, objects, flows, x, y + 150);
    if branch.terminal {
        return (Some(origin), next_x);
    }
    let merge_id = uuid::Uuid::new_v4().to_string();
    objects.push(flow_object_doc(
        &merge_id,
        "Microflows$ExclusiveMerge",
        next_x,
        y,
        "40;40",
    ));
    flows.push(sequence_flow_doc(&origin, &merge_id, None));
    match branch.last.as_deref() {
        Some(last) => flows.push(sequence_flow_doc(last, &merge_id, None)),
        None => flows.push(link(&merge_id)),
    }
    (Some(merge_id), next_x + 140)
}

/// The split a rule decides: the rule and what it is called with.
fn rule_split(rule: &str, arguments: &[(String, String)], x: i32, y: i32) -> Document {
    let mappings = arguments
        .iter()
        .map(|(parameter, argument)| {
            Bson::Document(doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Microflows$RuleCallParameterMapping",
                "Parameter": parameter.as_str(),
                "Argument": argument.as_str(),
            })
        })
        .collect();
    let mut split = flow_object_doc("", "Microflows$ExclusiveSplit", x, y, "90;60");
    split.insert(
        "SplitCondition",
        doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Microflows$RuleSplitCondition",
            "RuleCall": doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Microflows$RuleCall",
                "Microflow": rule,
                "ParameterMappings": mxrs_bson::build_array(mappings, 2),
            },
        },
    );
    split.insert("Caption", rule);
    split.insert("ErrorHandlingType", "Rollback");
    split.insert("Documentation", "");
    split
}

/// Lowers a split and its branches: one edge per branch carrying the case
/// values that select it, and a merge where the branches that do not end
/// the flow join.
#[allow(clippy::too_many_arguments)]
fn process_split(
    mut split: Document,
    case_type: &str,
    branches: &[(Vec<String>, &[Activity])],
    prev_id: Option<&str>,
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    x: i32,
    y: i32,
) -> (Option<String>, i32) {
    let split_id = uuid::Uuid::new_v4().to_string();
    split.insert("$ID", split_id.as_str());
    objects.push(split);
    if let Some(prev) = prev_id {
        flows.push(sequence_flow_doc(prev, &split_id, None));
    }

    let branch_width = branches
        .iter()
        .map(|(_, activities)| activities.len())
        .max()
        .unwrap_or(0)
        .max(1) as i32;
    let x_branch = x + 140;
    let x_merge = x + 140 * (branch_width + 1);

    let link = |values: &[String], to: &str| {
        let cases: Vec<_> = values
            .iter()
            .map(|value| (case_type, value.as_str()))
            .collect();
        flow_doc(&split_id, to, &cases, false)
    };
    let mut results = Vec::new();
    for (index, (values, activities)) in branches.iter().enumerate() {
        results.push(process_branch(
            activities,
            &|to: &str| link(values, to),
            objects,
            flows,
            x_branch,
            y + 150 * index as i32,
        ));
    }
    if results.iter().all(|result| result.terminal) && !results.is_empty() {
        return (None, x_merge + 140);
    }
    let merge_id = uuid::Uuid::new_v4().to_string();
    objects.push(flow_object_doc(
        &merge_id,
        "Microflows$ExclusiveMerge",
        x_merge,
        y,
        "40;40",
    ));
    for (result, (values, _)) in results.iter().zip(branches) {
        match &result.first {
            None => flows.push(link(values, &merge_id)),
            Some(_) if !result.terminal => flows.push(sequence_flow_doc(
                result
                    .last
                    .as_deref()
                    .expect("non-terminal branch has a last id"),
                &merge_id,
                None,
            )),
            Some(_) => {}
        }
    }
    (Some(merge_id), x_merge + 140)
}

/// Lowers the activities of one branch; `link` draws the edge that enters
/// it, to whatever its first activity lowers to.
fn process_branch(
    activities: &[Activity],
    link: &dyn Fn(&str) -> Document,
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    mut x: i32,
    y: i32,
) -> BranchResult {
    let mut first: Option<String> = None;
    let mut previous: Option<String> = None;
    let mut terminal = false;
    for activity in activities {
        if terminal && !matches!(activity, Activity::Label(_)) {
            continue;
        }
        // A jump lowers to an edge and nothing else: one that opens the
        // branch is the branch's own entering edge, pointed at the label.
        if let Activity::Jump(name) = activity
            && previous.is_none()
        {
            let target = format!("{LABEL}{name}");
            flows.push(link(&target));
            first = Some(target);
            terminal = true;
            break;
        }
        let before = objects.len();
        let (next_id, next_x) = process_activity(
            activity,
            previous.as_deref(),
            objects,
            flows,
            x,
            y,
            "Rollback",
        );
        let created_first = objects
            .get(before)
            .and_then(|object| object.get_str("$ID").ok())
            .map(str::to_string);
        // Only the branch's first activity is entered from the split; a
        // label after an ended path is reached by its jumps alone.
        if first.is_none()
            && let Some(created) = created_first.as_deref()
        {
            flows.push(link(created));
            first = created_first.clone();
        }
        previous = next_id;
        terminal = previous.is_none();
        x = next_x;
    }
    BranchResult {
        first,
        last: previous,
        terminal,
    }
}

enum LoopSource<'a> {
    Iterable(&'a str, &'a str),
    While(&'a str),
}

#[allow(clippy::too_many_arguments)]
fn process_loop(
    source: LoopSource<'_>,
    activities: &[Activity],
    prev_id: Option<&str>,
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    x: i32,
    y: i32,
) -> (Option<String>, i32) {
    let loop_id = uuid::Uuid::new_v4().to_string();
    let mut inner_objects = Vec::new();
    let mut inner_previous: Option<String> = None;
    let mut inner_x = 50;
    for activity in activities {
        if !inner_objects.is_empty()
            && inner_previous.is_none()
            && !matches!(activity, Activity::Label(_))
        {
            continue;
        }
        let (next, next_x) = process_activity(
            activity,
            inner_previous.as_deref(),
            &mut inner_objects,
            flows,
            inner_x,
            100,
            "Rollback",
        );
        inner_previous = next;
        inner_x = next_x;
    }
    let source_doc = match source {
        LoopSource::Iterable(list, iterator) => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Microflows$IterableList",
            "ListVariableName": list,
            "VariableName": iterator,
        },
        LoopSource::While(condition) => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Microflows$WhileLoopCondition",
            "WhileExpression": condition,
        },
    };
    let mut loop_doc = flow_object_doc(&loop_id, "Microflows$LoopedActivity", x, y, "300;200");
    loop_doc.insert("ErrorHandlingType", "Rollback");
    loop_doc.insert("LoopSource", source_doc);
    loop_doc.insert(
        "ObjectCollection",
        doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Microflows$MicroflowObjectCollection",
            "Objects": mxrs_bson::build_array(inner_objects.into_iter().map(Bson::Document).collect(), 2),
        },
    );
    objects.push(loop_doc);
    if let Some(previous) = prev_id {
        flows.push(sequence_flow_doc(previous, &loop_id, None));
    }
    (Some(loop_id), x + 140)
}

fn build_rescue_branch(
    origin_id: &str,
    activities: &[Activity],
    objects: &mut Vec<Document>,
    flows: &mut Vec<Document>,
    mut x: i32,
    y: i32,
) {
    let mut previous: Option<String> = None;
    let mut started = false;
    for activity in activities {
        if started && previous.is_none() && !matches!(activity, Activity::Label(_)) {
            continue;
        }
        let before = objects.len();
        let (next, next_x) = process_activity(
            activity,
            previous.as_deref(),
            objects,
            flows,
            x,
            y,
            "Rollback",
        );
        let created = objects[before]
            .get_str("$ID")
            .expect("rescue activity assigns an id")
            .to_string();
        if !started {
            let mut error_flow = sequence_flow_doc(origin_id, &created, None);
            error_flow.insert("IsErrorHandler", true);
            flows.push(error_flow);
        }
        started = true;
        previous = next;
        x = next_x;
    }
    if let Some(previous) = previous {
        let end_id = uuid::Uuid::new_v4().to_string();
        let mut end = flow_object_doc(&end_id, "Microflows$EndEvent", x, y, "20;20");
        end.insert("Documentation", "");
        end.insert("ReturnValue", "");
        objects.push(end);
        flows.push(sequence_flow_doc(&previous, &end_id, None));
    }
}

fn split_condition_doc(condition: &str) -> Document {
    doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$ExpressionSplitCondition", "Expression": condition }
}

fn sequence_flow_doc(from_id: &str, to_id: &str, case_value: Option<&str>) -> Document {
    match case_value {
        Some(value) => flow_doc(
            from_id,
            to_id,
            &[("Microflows$EnumerationCase", value)],
            false,
        ),
        None => flow_doc(from_id, to_id, &[], false),
    }
}

/// One sequence flow. `cases` are the values that select it out of a split,
/// each with the case type that carries it; `error` makes it the edge an
/// activity's failure follows.
fn flow_doc(from_id: &str, to_id: &str, cases: &[(&str, &str)], error: bool) -> Document {
    let case_docs: Vec<Bson> = if cases.is_empty() {
        vec![Bson::Document(doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Microflows$NoCase",
        })]
    } else {
        cases
            .iter()
            .map(|(ty, value)| {
                Bson::Document(doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": *ty,
                    "Value": *value,
                })
            })
            .collect()
    };
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$SequenceFlow",
        "OriginPointer": from_id, "DestinationPointer": to_id,
        "OriginConnectionIndex": 1, "DestinationConnectionIndex": 3,
        "IsErrorHandler": error,
        "Line": doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$BezierCurve",
            "OriginControlVector": "0;0", "DestinationControlVector": "0;0",
        },
        "CaseValues": mxrs_bson::build_array(case_docs, 2),
    }
}

fn flow_object_doc(id: &str, ty: &str, x: i32, y: i32, size: &str) -> Document {
    doc! { "$ID": id, "$Type": ty, "RelativeMiddlePoint": format!("{x};{y}"), "Size": size }
}

fn build_activity(activity: &Activity, id: &str, x: i32, y: i32, error_handling: &str) -> Document {
    let mut doc = flow_object_doc(id, "Microflows$ActionActivity", x, y, "120;60");
    doc.insert("Documentation", "");
    doc.insert("AutoGenerateCaption", true);
    doc.insert("BackgroundColor", "Default");
    doc.insert("Caption", "Activity");
    doc.insert("Action", activity_action_doc(activity, error_handling));
    doc
}

fn activity_action_doc(activity: &Activity, error_handling: &str) -> Document {
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
            "ErrorHandlingType": error_handling,
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
            "ErrorHandlingType": error_handling,
            "Items": mxrs_bson::build_array(members.iter().map(|m| Bson::Document(change_action_item_doc(m, entity))).collect(), 2),
            "RefreshInClient": false,
        },
        Activity::Commit { variable } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$CommitAction",
            "CommitVariableName": variable.clone(),
            "ErrorHandlingType": error_handling,
            "RefreshInClient": false,
            "WithEvents": true,
        },
        Activity::DeleteObject { variable } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$DeleteAction",
            "DeleteVariableName": variable.clone(),
            "ErrorHandlingType": error_handling,
            "RefreshInClient": false,
        },
        Activity::CreateList { variable, entity } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$CreateListAction",
            "Entity": entity.clone(),
            "ErrorHandlingType": error_handling,
            "VariableName": variable.clone(),
        },
        Activity::CallMicroflow {
            name,
            result_variable,
            use_return,
            mappings,
            ..
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
                "ErrorHandlingType": error_handling,
                "UseReturnVariable": *use_return,
                "ResultVariableName": result_variable.clone().unwrap_or_default(),
                "MicroflowCall": doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$MicroflowCall",
                    "Microflow": name.clone(),
                    "ParameterMappings": mxrs_bson::build_array(mapping_docs, 2),
                },
            }
        }
        Activity::RetrieveObjects {
            entity,
            variable,
            xpath,
        } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$RetrieveAction",
            "ErrorHandlingType": error_handling,
            "ResultVariableName": variable.clone(),
            "RetrieveSource": doc! {
                "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$DatabaseRetrieveSource",
                "Entity": entity.clone(),
                "NewSortings": doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$SortingsList",
                    "Sortings": mxrs_bson::build_array(vec![], 2),
                },
                "Range": doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$ConstantRange",
                    "SingleObject": false,
                },
                "XpathConstraint": xpath.clone().unwrap_or_default(),
            },
        },
        Activity::AggregateCount {
            list_variable,
            output_variable,
        } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$AggregateAction",
            "AggregateFunction": "Count",
            "AggregateVariableName": list_variable.clone(),
            "Attribute": "",
            "ErrorHandlingType": error_handling,
            "VariableName": output_variable.clone(),
        },
        Activity::LogMessage {
            message,
            level,
            node,
        } => doc! {
            "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$LogMessageAction",
            "ErrorHandlingType": error_handling,
            "IncludeLatestStackTrace": false,
            "Level": level.native_name(),
            "MessageTemplate": doc! {
                "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$StringTemplate",
                "Parameters": mxrs_bson::build_array(vec![], 2),
                "Text": message.clone(),
            },
            "Node": node.clone(),
        },
        // The document states its own error handling along with everything
        // else the action is.
        Activity::Action(document) => native_document(document),
        Activity::Disabled(_)
        | Activity::OnError { .. }
        | Activity::RaiseError
        | Activity::RuleDecision { .. }
        | Activity::RuleSwitch { .. }
        | Activity::Label(_)
        | Activity::Jump(_)
        | Activity::Switch { .. }
        | Activity::TypeSwitch { .. } => {
            unreachable!("lowered by process_activity before build_activity")
        }
        Activity::Decision { .. }
        | Activity::LoopOver { .. }
        | Activity::WhileLoop { .. }
        | Activity::BreakLoop
        | Activity::ContinueLoop => {
            unreachable!("control-flow activities are handled before build_activity")
        }
        Activity::ReturnValue { .. } => {
            unreachable!("return activities are handled before build_activity")
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

    fn assert_storage_ids_first(value: &Bson) {
        match value {
            Bson::Document(document) => {
                if document.contains_key("$Type") {
                    assert_eq!(document.keys().next().map(String::as_str), Some("$ID"));
                }
                for value in document.values() {
                    assert_storage_ids_first(value);
                }
            }
            Bson::Array(values) => {
                for value in values {
                    assert_storage_ids_first(value);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn every_typed_return_document_starts_with_a_storage_id() {
        for return_type in [
            FlowReturnType::String,
            FlowReturnType::Integer,
            FlowReturnType::Long,
            FlowReturnType::Float,
            FlowReturnType::Decimal,
            FlowReturnType::Boolean,
            FlowReturnType::DateTime,
            FlowReturnType::Binary,
            FlowReturnType::Object("Sales.Order".into()),
            FlowReturnType::List("Sales.Order".into()),
        ] {
            let document = return_type_document(Some(&return_type)).unwrap();
            assert_storage_ids_first(&Bson::Document(document));
        }
    }

    #[test]
    fn a_single_activity_wires_start_activity_end() {
        let activities = vec![Activity::Commit {
            variable: "order".into(),
        }];
        let (objects, flows) = build_microflow_graph(&activities, &[], Some("$order"));
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
        let (objects, _) = build_microflow_graph(&activities, &[], None);
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
        let (objects, flows) = build_microflow_graph(&activities, &[], None);
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

    #[test]
    fn functional_actions_have_native_shapes_and_rescue_can_return_false() {
        let activities = vec![
            Activity::RetrieveObjects {
                entity: "Sales.Order".into(),
                variable: "items".into(),
                xpath: Some("[Number = 'A-1']".into()),
            },
            Activity::AggregateCount {
                list_variable: "items".into(),
                output_variable: "count".into(),
            },
            Activity::LogMessage {
                message: "done".into(),
                level: mxrs_ir::flow::LogLevel::Info,
                node: "'MXRS_TEST'".into(),
            },
        ];
        let rescue = vec![Activity::ReturnValue {
            expression: "false".into(),
        }];
        let (objects, flows) = build_microflow_graph(&activities, &rescue, Some("$count = 1"));
        let actions = objects
            .iter()
            .filter_map(|object| object.get_document("Action").ok())
            .collect::<Vec<_>>();
        assert_eq!(
            actions[0]
                .get_document("RetrieveSource")
                .unwrap()
                .get_str("XpathConstraint")
                .unwrap(),
            "[Number = 'A-1']"
        );
        assert_eq!(actions[1].get_str("AggregateFunction").unwrap(), "Count");
        assert_eq!(actions[2].get_str("Level").unwrap(), "Info");
        assert!(objects.iter().any(|object| {
            object.get_str("$Type").ok() == Some("Microflows$EndEvent")
                && object.get_str("ReturnValue").ok() == Some("false")
        }));
        assert!(
            flows
                .iter()
                .any(|flow| flow.get_bool("IsErrorHandler").ok() == Some(true))
        );
        for document in objects.iter().chain(&flows) {
            assert_storage_ids_first(&Bson::Document(document.clone()));
        }
    }
}
