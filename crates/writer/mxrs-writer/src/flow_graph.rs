//! Linear graph traversal and identity-preserving synchronization. Storage
//! order is independent of execution order; ambiguous graphs are rejected.
use std::collections::{HashMap, HashSet};

use mxrs_bson::{Bson, Document, extract_id};

pub fn documents(value: &Bson) -> Option<Vec<&Document>> {
    let array = value.as_array()?;
    let offset = usize::from(matches!(
        array.first(),
        Some(Bson::Int32(1..=3) | Bson::Int64(1..=3))
    ));
    array[offset..].iter().map(Bson::as_document).collect()
}

fn id(doc: &Document) -> Option<String> {
    extract_id(doc.get("$ID")?)
}

/// Returns executable nodes in edge order, including the start and end.
/// Parameters may be embedded in the object collection in older projects.
pub fn linear_nodes(doc: &Document) -> Option<Vec<&Document>> {
    if !unique_ids(&Bson::Document(doc.clone()), &mut HashSet::new()) {
        return None;
    }
    let collection = doc.get_document("ObjectCollection").ok()?;
    let objects = documents(collection.get("Objects")?)?;
    let mut nodes = HashMap::new();
    let mut start = None;
    for object in objects {
        if object.get_str("$Type").ok()? == "Microflows$MicroflowParameter" {
            continue;
        }
        let key = id(object)?;
        if object.get_str("$Type").ok()? == "Microflows$StartEvent"
            && start.replace(key.clone()).is_some()
        {
            return None;
        }
        if nodes.insert(key, object).is_some() {
            return None;
        }
    }
    let edges = documents(doc.get("Flows").or_else(|| collection.get("Flows"))?)?;
    if edges.len() + 1 != nodes.len() {
        return None;
    }
    let mut next = HashMap::new();
    let mut destinations = HashSet::new();
    let mut edge_ids = HashSet::new();
    for edge in edges {
        if edge.get_str("$Type").ok()? != "Microflows$SequenceFlow"
            || !matches!(
                edge.get("IsErrorHandler"),
                None | Some(Bson::Boolean(false))
            )
        {
            return None;
        }
        if let Some(cases) = edge.get("CaseValues")
            && documents(cases)?
                .iter()
                .any(|case| case.get_str("$Type").ok() != Some("Microflows$NoCase"))
        {
            return None;
        }
        let origin = extract_id(edge.get("OriginPointer")?)?;
        let destination = extract_id(edge.get("DestinationPointer")?)?;
        if !nodes.contains_key(&origin)
            || !nodes.contains_key(&destination)
            || !destinations.insert(destination.clone())
            || !edge_ids.insert(id(edge)?)
            || next.insert(origin, destination).is_some()
        {
            return None;
        }
    }
    let mut cursor = start?;
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    loop {
        if !seen.insert(cursor.clone()) {
            return None;
        }
        let node = *nodes.get(&cursor)?;
        ordered.push(node);
        match next.get(&cursor) {
            Some(to) => cursor = to.clone(),
            None => break,
        }
    }
    if seen.len() != nodes.len() || ordered.last()?.get_str("$Type").ok()? != "Microflows$EndEvent"
    {
        return None;
    }
    if ordered
        .iter()
        .skip(1)
        .take(ordered.len().saturating_sub(2))
        .any(|node| node.get_str("$Type").ok() != Some("Microflows$ActionActivity"))
    {
        return None;
    }
    Some(ordered)
}

fn unique_ids(value: &Bson, seen: &mut HashSet<String>) -> bool {
    match value {
        Bson::Document(doc) => {
            if let Some(value) = doc.get("$ID") {
                let Some(id) = extract_id(value) else {
                    return false;
                };
                if !seen.insert(id) {
                    return false;
                }
            }
            doc.values().all(|value| unique_ids(value, seen))
        }
        Bson::Array(values) => values.iter().all(|value| unique_ids(value, seen)),
        _ => true,
    }
}

/// Retains node/edge identities, layout and native metadata when the new body
/// has the same linear activity kinds. Structural edits use normal lowering.
pub(crate) fn merge(previous: &Document, fresh: &Document) -> Option<Document> {
    let old_nodes = linear_nodes(previous)?;
    let new_nodes = linear_nodes(fresh)?;
    if old_nodes.len() != new_nodes.len() {
        return None;
    }
    let mut replacements = HashMap::new();
    for (old, new) in old_nodes.into_iter().zip(new_nodes) {
        if old.get_str("$Type").ok()? != new.get_str("$Type").ok()? {
            return None;
        }
        let mut merged = old.clone();
        if let Ok(action) = new.get_document("Action") {
            let prior = old.get_document("Action").ok()?;
            if prior.get_str("$Type").ok()? != action.get_str("$Type").ok()? {
                return None;
            }
            merged.insert(
                "Action",
                merge_value(
                    &Bson::Document(prior.clone()),
                    &Bson::Document(action.clone()),
                ),
            );
        }
        if let Some(value) = new.get("ReturnValue") {
            merged.insert("ReturnValue", value.clone());
        }
        replacements.insert(id(old)?, merged);
    }
    let mut collection = previous.get_document("ObjectCollection").ok()?.clone();
    let mut objects = collection.get_array("Objects").ok()?.clone();
    for value in &mut objects {
        if let Some(key) = value.as_document().and_then(id)
            && let Some(replacement) = replacements.remove(&key)
        {
            *value = Bson::Document(replacement);
        }
    }
    collection.insert("Objects", objects);
    Some(collection)
}

fn merge_value(old: &Bson, new: &Bson) -> Bson {
    match (old, new) {
        (Bson::Document(old), Bson::Document(new))
            if old.get_str("$Type").ok() == new.get_str("$Type").ok() =>
        {
            let mut merged = old.clone();
            for (key, value) in new {
                if key == "$ID" {
                    continue;
                }
                merged.insert(
                    key,
                    old.get(key)
                        .map(|prior| merge_value(prior, value))
                        .unwrap_or_else(|| value.clone()),
                );
            }
            Bson::Document(merged)
        }
        (Bson::Array(old), Bson::Array(new)) => Bson::Array(
            new.iter()
                .enumerate()
                .map(|(i, value)| {
                    if i == 0
                        && matches!(old.first(), Some(Bson::Int32(_) | Bson::Int64(_)))
                        && matches!(value, Bson::Int32(_) | Bson::Int64(_))
                    {
                        return old[0].clone();
                    }
                    old.get(i)
                        .map(|prior| merge_value(prior, value))
                        .unwrap_or_else(|| value.clone())
                })
                .collect(),
        ),
        _ => new.clone(),
    }
}

/// Attests that the writer can project this declaration without changing its
/// existing linear body, signature or documentation on an unedited rebuild.
pub fn preserves_linear_body(previous: &Document, declaration: &mxrs_ir::MicroflowDecl) -> bool {
    if !same_parameters(previous, declaration)
        || previous.get_str("Documentation").ok() != Some(declaration.documentation.as_str())
    {
        return false;
    }
    let (objects, flows) = crate::flow_compiler::build_microflow_graph(
        &declaration.activities,
        &declaration.rescue_activities,
        declaration.return_expression.as_deref(),
    );
    let fresh = mxrs_bson::doc! {
        "ObjectCollection": { "Objects": mxrs_bson::build_array(objects.into_iter().map(Bson::Document).collect(),3) },
        "Flows":mxrs_bson::build_array(flows.into_iter().map(Bson::Document).collect(),3),
    };
    merge(previous, &fresh).as_ref() == previous.get_document("ObjectCollection").ok()
}

pub(crate) fn same_parameters(previous: &Document, decl: &mxrs_ir::MicroflowDecl) -> bool {
    let old = mxrs_model::Microflow::from_bson(previous);
    old.parameters.len() == decl.parameters.len()
        && old
            .parameters
            .iter()
            .zip(&decl.parameters)
            .all(|(old, new)| {
                let Some(ty) = crate::flow_compiler::return_type_document(Some(&new.value_type))
                else {
                    return false;
                };
                old.get_str("Name").ok() == Some(new.name.as_str())
                    && old.get_str("Documentation").unwrap_or_default() == new.documentation
                    && old.get_str("DefaultValue").unwrap_or_default()
                        == new.default_value.as_deref().unwrap_or_default()
                    && old.get_bool("IsRequired").unwrap_or(false) == new.required
                    && old.get_document("VariableType").ok().is_some_and(|prior| {
                        prior.get("$Type") == ty.get("$Type")
                            && prior.get("Entity") == ty.get("Entity")
                    })
            })
}

pub(crate) fn merge_return_type(previous: &Document, fresh: &Document) -> Bson {
    let new = fresh.get("MicroflowReturnType").expect("fresh return type");
    let Some(old) = previous.get("MicroflowReturnType") else {
        return new.clone();
    };
    if let (Some(old), Some(new_doc)) = (old.as_document(), new.as_document())
        && old.get("$Type") == new_doc.get("$Type")
        && old.get("Entity") == new_doc.get("Entity")
    {
        return Bson::Document(old.clone());
    }
    new.clone()
}
