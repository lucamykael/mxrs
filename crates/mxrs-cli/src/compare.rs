//! Structural project comparison — ports the snapshot-and-diff shape of
//! mxrb's `Mxrb::Compare` (`compare.rb`), narrowed to what `mxrs-model`
//! already reads: the flat unit list, and per-module entities,
//! associations, and microflows/nanoflows. Not yet ported:
//! `security_summary` (needs a `Security$ProjectSecurity` reader),
//! `design_asset_summary` (filesystem asset hashing), and page/menu
//! summaries (mxrs-model reads `Page`/`Menu` already, but their summary
//! shape isn't wired up here yet — narrow the gap incrementally).
//!
//! Diffing reuses `compare.rb`'s two key insights directly:
//! - Named collections (anything shaped `[{ "name": ..., ... }, ...]`) diff
//!   by name, not by position — an item present on both sides under the
//!   same name is compared field-by-field; an unmatched item is reported as
//!   wholesale `added`/`removed` (with a same-content-but-renamed pass in
//!   between, so a rename reads as one change instead of a spurious
//!   remove+add pair).
//! - A microflow's `objects`/`flows` are graph-shaped, not list-shaped —
//!   comparing them positionally (or by their real `$ID`s, which are new
//!   every regeneration) would make every equivalent flow look different.
//!   `assign_flow_ids` (ported below) walks the graph from its
//!   `StartEvent`(s) and assigns each object a position-in-traversal index;
//!   [`normalize_flow_value`] then drops volatile presentation-only fields
//!   (`$ID`, `X`, `Y`, ...) and rewrites any field whose value is a known
//!   object's `$ID` into `{"object": <index>}`, so two structurally
//!   equivalent flows compare equal regardless of their real ids or
//!   declaration order.
//!
//!   **Narrowed vs. `compare.rb`**: outgoing edges out of a decision node
//!   are ordered here by `(is_error_handler, destination's declared index)`
//!   rather than mxrb's full `(is_error_handler, normalized_case_values)` —
//!   porting case-value normalization (`CaseValues`/`NewCaseValue`
//!   handling) faithfully is more machinery than a CLI skeleton needs right
//!   now. This still gives a deterministic, declaration-order-independent
//!   traversal; it's only decision-branch *labels* that aren't used to
//!   break ties.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use mxrs_bson::{Bson, Document};
use mxrs_model::{Association, Entity, Microflow, Module, Project};
use serde_json::{json, Value};

pub fn snapshot(path: impl AsRef<Path>) -> mxrs_model::Result<Value> {
    let project = Project::open(&path, true)?;
    let mut modules = project.modules()?;
    modules.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(json!({
        "project": { "mendix_version": project.mendix_version()? },
        "units": unit_summary(&project),
        "modules": modules.iter().map(module_summary).collect::<Vec<_>>(),
    }))
}

fn unit_summary(project: &Project) -> Vec<Value> {
    let mut summary: Vec<Value> = project
        .all_units()
        .unwrap_or_default()
        .into_iter()
        .filter(|u| u.unit_id != u.container_id)
        .filter_map(|u| {
            let doc = project.mpr().parse_contents(&u).ok()?;
            let ty = doc.get_str("$Type").unwrap_or_default().to_string();
            let name = doc
                .get_str("Name")
                .or_else(|_| doc.get_str("name"))
                .unwrap_or_default()
                .to_string();
            Some(json!({ "containment": u.containment_name, "type": ty, "name": name }))
        })
        .collect();
    summary.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v["containment"].as_str().unwrap_or_default().to_string(),
                v["type"].as_str().unwrap_or_default().to_string(),
                v["name"].as_str().unwrap_or_default().to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    summary
}

fn module_summary(module: &Module) -> Value {
    let mut entities: Vec<&Entity> = module.entities().iter().collect();
    entities.sort_by(|a, b| a.name.cmp(&b.name));
    let mut associations: Vec<&Association> = module.associations();
    associations.sort_by(|a, b| a.name.cmp(&b.name));
    let mut microflows: Vec<&Microflow> = module.microflows.iter().collect();
    microflows.sort_by(|a, b| a.name.cmp(&b.name));
    let mut nanoflows: Vec<&Microflow> = module.nanoflows.iter().collect();
    nanoflows.sort_by(|a, b| a.name.cmp(&b.name));

    json!({
        "name": module.name,
        "entities": entities.iter().map(|e| entity_summary(e)).collect::<Vec<_>>(),
        "associations": associations.iter().map(|a| association_summary(a)).collect::<Vec<_>>(),
        "microflows": microflows.iter().map(|f| flow_summary(f)).collect::<Vec<_>>(),
        "nanoflows": nanoflows.iter().map(|f| flow_summary(f)).collect::<Vec<_>>(),
    })
}

fn entity_summary(entity: &Entity) -> Value {
    let mut attributes = entity.attributes.clone();
    attributes.sort_by(|a, b| a.name.cmp(&b.name));
    json!({
        "name": entity.name,
        "documentation": entity.documentation,
        "persistable": entity.persistable,
        "attributes": attributes.iter().map(|a| json!({
            "name": a.name,
            "documentation": a.documentation,
            "type": format!("{:?}", a.attribute_type),
            "default": a.default_value,
        })).collect::<Vec<_>>(),
    })
}

fn association_summary(association: &Association) -> Value {
    json!({
        "name": association.name,
        "type": format!("{:?}", association.association_type),
        "owner": format!("{:?}", association.owner),
        "storage_format": format!("{:?}", association.storage_format),
        "documentation": association.documentation,
    })
}

fn flow_summary(flow: &Microflow) -> Value {
    let mut ids: HashMap<String, usize> = HashMap::new();
    assign_flow_ids(&flow.objects, &flow.flows, &mut ids);

    let mut objects: Vec<&Document> = flow.objects.iter().collect();
    objects.sort_by_key(|o| {
        flow_id(o)
            .and_then(|id| ids.get(&id).copied())
            .unwrap_or(ids.len())
    });

    let mut normalized_flows: Vec<Value> = flow
        .flows
        .iter()
        .map(|edge| {
            json!({
                "origin": edge.get("OriginPointer").map(|v| normalize_flow_value(v, &ids)).unwrap_or(Value::Null),
                "destination": edge.get("DestinationPointer").map(|v| normalize_flow_value(v, &ids)).unwrap_or(Value::Null),
                "error_handler": edge.get_bool("IsErrorHandler").unwrap_or(false),
            })
        })
        .collect();
    normalized_flows.sort_by_key(|v| v.to_string());

    json!({
        "name": flow.name,
        "return_type": flow.return_type,
        "parameters": flow.parameters.iter().map(|p| normalize_flow_value(&Bson::Document(p.clone()), &ids)).collect::<Vec<_>>(),
        "objects": objects.iter().map(|o| normalize_flow_value(&Bson::Document((*o).clone()), &ids)).collect::<Vec<_>>(),
        "flows": normalized_flows,
    })
}

fn flow_id(doc: &Document) -> Option<String> {
    doc.get("$ID").and_then(mxrs_bson::extract_id)
}

/// Ports `Mxrb::Compare::Comparator#assign_flow_ids`: assigns every object
/// (recursing into nested `ObjectCollection`s, e.g. inside a decision's
/// branches) a position-in-traversal index, walked breadth-first from the
/// flow's `StartEvent`(s) plus any object with no incoming edge, then any
/// still-unreached object in declaration order (disconnected components).
fn assign_flow_ids(objects: &[Document], flows: &[Document], ids: &mut HashMap<String, usize>) {
    let local_ids: HashSet<String> = objects.iter().filter_map(flow_id).collect();
    let local_flows: Vec<&Document> = flows
        .iter()
        .filter(|e| {
            let origin = e.get("OriginPointer").and_then(mxrs_bson::extract_id);
            let dest = e.get("DestinationPointer").and_then(mxrs_bson::extract_id);
            origin.is_some_and(|o| local_ids.contains(&o))
                && dest.is_some_and(|d| local_ids.contains(&d))
        })
        .collect();

    let mut outgoing: HashMap<String, Vec<&Document>> = HashMap::new();
    let mut has_incoming: HashSet<String> = HashSet::new();
    for e in &local_flows {
        if let Some(origin) = e.get("OriginPointer").and_then(mxrs_bson::extract_id) {
            outgoing.entry(origin).or_default().push(e);
        }
        if let Some(dest) = e.get("DestinationPointer").and_then(mxrs_bson::extract_id) {
            has_incoming.insert(dest);
        }
    }
    let object_index: HashMap<String, usize> = objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| flow_id(o).map(|id| (id, i)))
        .collect();

    let mut root_ids: HashSet<String> = HashSet::new();
    let mut roots: Vec<&Document> = Vec::new();
    for o in objects {
        if o.get_str("$Type").ok() == Some("Microflows$StartEvent") {
            if let Some(id) = flow_id(o) {
                if root_ids.insert(id) {
                    roots.push(o);
                }
            }
        }
    }
    for o in objects {
        if let Some(id) = flow_id(o) {
            if !has_incoming.contains(&id) && !root_ids.contains(&id) {
                root_ids.insert(id);
                roots.push(o);
            }
        }
    }

    let mut queue: VecDeque<&Document> = roots.into_iter().collect();
    while let Some(object) = queue.pop_front() {
        let Some(id) = flow_id(object) else { continue };
        if ids.contains_key(&id) {
            continue;
        }
        ids.insert(id.clone(), ids.len());
        recurse_into_nested(object, flows, ids);

        let mut edges: Vec<&&Document> = outgoing.get(&id).into_iter().flatten().collect();
        edges.sort_by_key(|e| {
            let is_error = e.get_bool("IsErrorHandler").unwrap_or(false);
            let dest_index = e
                .get("DestinationPointer")
                .and_then(mxrs_bson::extract_id)
                .and_then(|d| object_index.get(&d).copied())
                .unwrap_or(usize::MAX);
            (is_error, dest_index)
        });
        for edge in edges {
            if let Some(dest_id) = edge
                .get("DestinationPointer")
                .and_then(mxrs_bson::extract_id)
            {
                if let Some(target) = objects
                    .iter()
                    .find(|o| flow_id(o).as_deref() == Some(dest_id.as_str()))
                {
                    queue.push_back(target);
                }
            }
        }
    }

    for object in objects {
        let Some(id) = flow_id(object) else { continue };
        if ids.contains_key(&id) {
            continue;
        }
        ids.insert(id.clone(), ids.len());
        recurse_into_nested(object, flows, ids);
    }
}

fn recurse_into_nested(object: &Document, flows: &[Document], ids: &mut HashMap<String, usize>) {
    let Ok(oc) = object.get_document("ObjectCollection") else {
        return;
    };
    let Ok(nested_arr) = oc.get_array("Objects") else {
        return;
    };
    let nested: Vec<Document> = mxrs_bson::parse_array(Some(nested_arr))
        .items
        .into_iter()
        .filter_map(|b| match b {
            Bson::Document(d) => Some(d),
            _ => None,
        })
        .collect();
    assign_flow_ids(&nested, flows, ids);
}

const DROPPED_FLOW_KEYS: &[&str] = &[
    "$ID",
    "X",
    "Y",
    "RelativeMiddlePoint",
    "Size",
    "OriginBezierVector",
    "DestinationBezierVector",
    "OriginConnectionIndex",
    "DestinationConnectionIndex",
    "Line",
];

fn normalize_flow_value(value: &Bson, ids: &HashMap<String, usize>) -> Value {
    if let Some(id) = mxrs_bson::extract_id(value) {
        if let Some(idx) = ids.get(&id) {
            return json!({ "object": idx });
        }
    }

    match value {
        Bson::Document(d) => {
            let mut map = serde_json::Map::new();
            for (k, v) in d {
                if DROPPED_FLOW_KEYS.contains(&k.as_str()) {
                    continue;
                }
                if k.ends_with("Model") {
                    if let Bson::Document(inner) = v {
                        if inner.get_str("$Type").ok() == Some("Expressions$NoExpression") {
                            continue;
                        }
                    }
                }
                map.insert(k.clone(), normalize_flow_value(v, ids));
            }
            Value::Object(map)
        }
        Bson::Array(items) => {
            Value::Array(items.iter().map(|v| normalize_flow_value(v, ids)).collect())
        }
        Bson::String(s) => Value::String(s.clone()),
        Bson::Boolean(b) => Value::Bool(*b),
        Bson::Int32(i) => json!(i),
        Bson::Int64(i) => json!(i),
        Bson::Double(d) => json!(d),
        Bson::Null => Value::Null,
        other => Value::String(format!("{other:?}")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub operation: Operation,
    pub path: Vec<String>,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

impl Change {
    pub fn format(&self) -> String {
        format!(
            "{}: {} != {}",
            self.path.join("."),
            self.before
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_else(|| "nil".into()),
            self.after
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_else(|| "nil".into()),
        )
    }
}

#[derive(Debug, Clone)]
pub struct CompareResult {
    pub changes: Vec<Change>,
}

impl CompareResult {
    pub fn is_identical(&self) -> bool {
        self.changes.is_empty()
    }
}

pub fn compare(
    left: impl AsRef<Path>,
    right: impl AsRef<Path>,
) -> mxrs_model::Result<CompareResult> {
    let left_snapshot = snapshot(left)?;
    let right_snapshot = snapshot(right)?;
    Ok(CompareResult {
        changes: diff(&left_snapshot, &right_snapshot),
    })
}

pub fn diff(left: &Value, right: &Value) -> Vec<Change> {
    let mut path = Vec::new();
    diff_values(left, right, &mut path)
}

fn diff_values(left: &Value, right: &Value, path: &mut Vec<String>) -> Vec<Change> {
    if left == right {
        return vec![];
    }

    match (left, right) {
        (Value::Object(l), Value::Object(r)) => {
            let mut keys: Vec<&String> = l.keys().chain(r.keys()).collect();
            keys.sort();
            keys.dedup();
            keys.into_iter()
                .flat_map(|k| {
                    path.push(k.clone());
                    let result = diff_values(
                        l.get(k).unwrap_or(&Value::Null),
                        r.get(k).unwrap_or(&Value::Null),
                        path,
                    );
                    path.pop();
                    result
                })
                .collect()
        }
        (Value::Array(l), Value::Array(r)) if named_array(l) && named_array(r) => {
            diff_named_arrays(l, r, path)
        }
        (Value::Array(l), Value::Array(r)) => {
            let max = l.len().max(r.len());
            (0..max)
                .flat_map(|i| {
                    path.push(i.to_string());
                    let result = diff_values(
                        l.get(i).unwrap_or(&Value::Null),
                        r.get(i).unwrap_or(&Value::Null),
                        path,
                    );
                    path.pop();
                    result
                })
                .collect()
        }
        _ => vec![Change {
            operation: change_operation(left, right),
            path: path.clone(),
            before: (!left.is_null()).then(|| left.clone()),
            after: (!right.is_null()).then(|| right.clone()),
        }],
    }
}

fn change_operation(left: &Value, right: &Value) -> Operation {
    if left.is_null() {
        Operation::Added
    } else if right.is_null() {
        Operation::Removed
    } else {
        Operation::Changed
    }
}

fn named_array(items: &[Value]) -> bool {
    if !items
        .iter()
        .all(|v| matches!(v, Value::Object(m) if m.contains_key("name")))
    {
        return false;
    }
    let mut names: Vec<String> = items.iter().map(name_of).collect();
    let len = names.len();
    names.sort();
    names.dedup();
    names.len() == len
}

fn name_of(v: &Value) -> String {
    v.get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn same_except_name(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => {
            let mut am2 = am.clone();
            am2.remove("name");
            let mut bm2 = bm.clone();
            bm2.remove("name");
            am2 == bm2
        }
        _ => a == b,
    }
}

/// Ports `Mxrb::Compare::Comparator#diff_named_arrays`: matches items by
/// `"name"` rather than position, with a same-content-except-name pass so a
/// rename reports as one change instead of a spurious remove+add pair.
fn diff_named_arrays(left: &[Value], right: &[Value], path: &mut Vec<String>) -> Vec<Change> {
    let mut left_by_name: Vec<(String, Value)> =
        left.iter().map(|v| (name_of(v), v.clone())).collect();
    let mut right_by_name: Vec<(String, Value)> =
        right.iter().map(|v| (name_of(v), v.clone())).collect();
    let mut changes = Vec::new();

    let mut common_names: Vec<String> = left_by_name
        .iter()
        .filter(|(n, _)| right_by_name.iter().any(|(rn, _)| rn == n))
        .map(|(n, _)| n.clone())
        .collect();
    common_names.sort();
    for name in &common_names {
        let l = left_by_name
            .iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
            .clone();
        let r = right_by_name
            .iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
            .clone();
        path.push(name.clone());
        changes.extend(diff_values(&l, &r, path));
        path.pop();
    }
    left_by_name.retain(|(n, _)| !common_names.contains(n));
    right_by_name.retain(|(n, _)| !common_names.contains(n));

    let left_only_snapshot = left_by_name.clone();
    for (name, value) in &left_only_snapshot {
        if let Some(pos) = right_by_name
            .iter()
            .position(|(_, rv)| same_except_name(value, rv))
        {
            let (rname, rvalue) = right_by_name.remove(pos);
            path.push(format!("{name} -> {rname}"));
            changes.extend(diff_values(value, &rvalue, path));
            path.pop();
            left_by_name.retain(|(n, _)| n != name);
        }
    }

    left_by_name.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, value) in left_by_name {
        path.push(name);
        changes.extend(diff_values(&value, &Value::Null, path));
        path.pop();
    }
    right_by_name.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, value) in right_by_name {
        path.push(name);
        changes.extend(diff_values(&Value::Null, &value, path));
        path.pop();
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(path: &std::path::Path, configure: impl FnOnce(&mut mxrs_dsl::ModuleBuilder)) {
        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", configure);
        mxrs_writer::write_project(path, &project.build()).unwrap();
    }

    #[test]
    fn an_identical_project_compares_clean() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        write_fixture(&left, |m| {
            m.entity("Order", |e| {
                e.string("Number");
            });
        });
        write_fixture(&right, |m| {
            m.entity("Order", |e| {
                e.string("Number");
            });
        });

        let result = compare(&left, &right).unwrap();
        assert!(
            result.is_identical(),
            "unexpected changes: {:?}",
            result.changes
        );
    }

    #[test]
    fn a_changed_attribute_default_is_reported_by_name_not_position() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        write_fixture(&left, |m| {
            m.entity("Order", |e| {
                e.string("Number").default_value = Some("A-0".into());
            });
        });
        write_fixture(&right, |m| {
            m.entity("Order", |e| {
                e.string("Number").default_value = Some("A-1".into());
            });
        });

        let result = compare(&left, &right).unwrap();
        assert!(!result.is_identical());
        let change = result
            .changes
            .iter()
            .find(|c| c.path.contains(&"default".to_string()))
            .unwrap();
        assert_eq!(change.operation, Operation::Changed);
        assert!(change.path.contains(&"Number".to_string()));
    }

    #[test]
    fn an_added_entity_is_reported_as_added() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        write_fixture(&left, |m| {
            m.entity("Order", |_e| {});
        });
        write_fixture(&right, |m| {
            m.entity("Order", |_e| {});
            m.entity("Customer", |_e| {});
        });

        let result = compare(&left, &right).unwrap();
        assert!(result
            .changes
            .iter()
            .any(|c| c.operation == Operation::Added && c.path.contains(&"Customer".to_string())));
    }

    #[test]
    fn equivalent_microflows_compare_equal_despite_fresh_ids() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        let mk_flow = |m: &mut mxrs_dsl::ModuleBuilder| {
            m.microflow("ACT_Do", |f| {
                f.return_value("1");
            });
        };
        let mut left_project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        left_project.module("Sales", |m| {
            m.entity("Order", |_e| {});
            mk_flow(m);
        });
        mxrs_writer::write_project(&left, &left_project.build()).unwrap();
        let mut right_project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        right_project.module("Sales", |m| {
            m.entity("Order", |_e| {});
            mk_flow(m);
        });
        mxrs_writer::write_project(&right, &right_project.build()).unwrap();

        let result = compare(&left, &right).unwrap();
        assert!(
            result.is_identical(),
            "unexpected changes: {:?}",
            result.changes
        );
    }
}
