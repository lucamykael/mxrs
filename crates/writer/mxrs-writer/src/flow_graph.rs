//! Structured graph traversal and identity-preserving synchronization. Storage
//! order is independent of execution order; ambiguous graphs are rejected.
use std::collections::{HashMap, HashSet};

use mxrs_bson::{Bson, Document, extract_id};

mod structured;
pub use structured::{Case, Node, structured_nodes};

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

/// Retains identities, layout and metadata for matching structured bodies.
/// Structural edits use normal lowering.
pub(crate) fn merge(previous: &Document, fresh: &Document) -> Option<Document> {
    let old_nodes = structured_nodes(previous)?;
    let new_nodes = structured_nodes(fresh)?;
    let mut pairs = Vec::new();
    pair_nodes(&old_nodes, &new_nodes, &mut pairs)?;
    let mapping: HashMap<_, _> = pairs
        .iter()
        .map(|(old, new)| Some((id(new)?, id(old)?)))
        .collect::<Option<_>>()?;
    if edge_keys(previous, None)? != edge_keys(fresh, Some(&mapping))? {
        return None;
    }
    let mut replacements = HashMap::new();
    for (old, new) in pairs {
        let mut merged = old.clone();
        for field in ["Action", "SplitCondition", "LoopSource"] {
            if let Some(value) = new.get(field) {
                let prior = old.get(field)?;
                if prior.as_document()?.get_str("$Type").ok()?
                    != value.as_document()?.get_str("$Type").ok()?
                {
                    return None;
                }
                merged.insert(field, merge_value(prior, value));
            }
        }
        // What a node is declared to do, where the declaration says it on
        // the node itself rather than in a document of its own.
        for field in ["ReturnValue", "SplitVariableName"] {
            if let Some(value) = new.get(field) {
                merged.insert(field, value.clone());
            }
        }
        // A loop's answer to failing is the node's; a split's is always the
        // default, and an action's is part of its action document.
        if new.get_str("$Type").ok() == Some("Microflows$LoopedActivity")
            && let Some(value) = new.get("ErrorHandlingType")
        {
            merged.insert("ErrorHandlingType", value.clone());
        }
        // Whether the node runs is the declaration's to say. A model that
        // never stored the flag keeps not storing it.
        let disabled = new.get_bool("Disabled").unwrap_or(false);
        if disabled || old.contains_key("Disabled") {
            merged.insert("Disabled", disabled);
        }
        replacements.insert(id(old)?, merged);
    }
    let mut collection = previous.get_document("ObjectCollection").ok()?.clone();
    replace_nodes(&mut collection, &mut replacements)?;
    Some(collection)
}

/// Pairs each stored node with the rebuilt node that stands for it. Merges
/// are not paired: they do nothing, a model may draw one join as several or
/// several as one, and the edges are compared with them resolved away.
fn pair_nodes<'a>(
    old: &[Node<'a>],
    new: &[Node<'a>],
    pairs: &mut Vec<(&'a Document, &'a Document)>,
) -> Option<()> {
    if old.len() != new.len() {
        return None;
    }
    for (old, new) in old.iter().zip(new) {
        match (old, new) {
            (Node::Simple(a), Node::Simple(b))
                if a.get_str("$Type").ok()? == b.get_str("$Type").ok()? =>
            {
                pairs.push((a, b))
            }
            (Node::Loop { node: a, body: ab }, Node::Loop { node: b, body: bb }) => {
                pairs.push((a, b));
                pair_nodes(ab, bb, pairs)?;
            }
            (
                Node::Decision {
                    split: a,
                    yes: ay,
                    no: an,
                },
                Node::Decision {
                    split: b,
                    yes: by,
                    no: bn,
                },
            ) => {
                pairs.push((a, b));
                pair_nodes(ay, by, pairs)?;
                pair_nodes(an, bn, pairs)?;
            }
            (
                Node::Switch {
                    split: a,
                    cases: ac,
                },
                Node::Switch {
                    split: b,
                    cases: bc,
                },
            ) if a.get_str("$Type").ok()? == b.get_str("$Type").ok()? && ac.len() == bc.len() => {
                pairs.push((a, b));
                // A case is the values that select it, in whatever order the
                // branches are written.
                for case in ac {
                    let mut values: Vec<_> = case.values.iter().collect();
                    values.sort();
                    let other = bc.iter().find(|other| {
                        let mut theirs: Vec<_> = other.values.iter().collect();
                        theirs.sort();
                        theirs == values
                    })?;
                    pair_nodes(&case.body, &other.body, pairs)?;
                }
            }
            (
                Node::Handled {
                    node: a,
                    handler: ah,
                },
                Node::Handled {
                    node: b,
                    handler: bh,
                },
            ) => {
                pair_nodes(std::slice::from_ref(a), std::slice::from_ref(b), pairs)?;
                pair_nodes(ah, bh, pairs)?;
            }
            // Which point a label names and a jump goes to is in the edges,
            // compared once every node is paired.
            (Node::Label(_), Node::Label(_)) | (Node::Jump(_), Node::Jump(_)) => {}
            _ => return None,
        }
    }
    Some(())
}

/// One edge: where it leaves, where it arrives, the case values that select
/// it, and whether it is the edge a failure follows.
type EdgeKey = (String, String, Vec<String>, bool);

/// The edges of a graph between the objects that do something, in the
/// identities `mapping` translates them to. A path that ends at a merge
/// nothing leaves ends where a path with no edge at all does.
fn edge_keys(doc: &Document, mapping: Option<&HashMap<String, String>>) -> Option<Vec<EdgeKey>> {
    let translate = |key: &str| match mapping {
        Some(mapping) => mapping.get(key).cloned(),
        None => Some(key.to_string()),
    };
    let mut keys = Vec::new();
    for (from, edges) in &structured::read_edges(doc)?.outgoing {
        for edge in edges {
            let Some(to) = &edge.to else {
                continue;
            };
            let mut cases = edge.cases.clone();
            cases.sort();
            keys.push((translate(from)?, translate(to)?, cases, edge.error));
        }
    }
    keys.sort();
    Some(keys)
}

fn replace_nodes(
    collection: &mut Document,
    replacements: &mut HashMap<String, Document>,
) -> Option<()> {
    for value in collection.get_array_mut("Objects").ok()? {
        if let Some(key) = value.as_document().and_then(id)
            && let Some(mut replacement) = replacements.remove(&key)
        {
            if let Ok(inner) = replacement.get_document_mut("ObjectCollection") {
                replace_nodes(inner, replacements)?;
            }
            *value = Bson::Document(replacement);
        }
    }
    Some(())
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
                    if let Some(key) = named_item(value) {
                        return old
                            .iter()
                            .find(|prior| named_item(prior).as_ref() == Some(&key))
                            .map(|prior| merge_value(prior, value))
                            .unwrap_or_else(|| value.clone());
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

fn named_item(value: &Bson) -> Option<(String, String)> {
    let doc = value.as_document()?;
    Some(match doc.get_str("$Type").ok()? {
        "Microflows$ChangeActionItem" => (
            doc.get_str("Attribute").ok()?.into(),
            doc.get_str("Association").ok()?.into(),
        ),
        "Microflows$MicroflowCallParameterMapping" => {
            (doc.get_str("Parameter").ok()?.into(), String::new())
        }
        _ => return None,
    })
}

/// Attests that the writer can project this declaration without changing its
/// existing structured body, signature or documentation on an unedited rebuild.
pub fn preserves_body(previous: &Document, declaration: &mxrs_ir::MicroflowDecl) -> bool {
    if !same_parameters(previous, declaration)
        || previous.get_str("Documentation").ok() != Some(declaration.documentation.as_str())
    {
        return false;
    }
    let (objects, flows) = crate::flow_compiler::build_flow_graph(
        &declaration.activities,
        &declaration.rescue_activities,
        declaration.return_expression.as_deref(),
        previous.get_str("$Type").ok() == Some("Microflows$Nanoflow"),
    );
    let fresh = mxrs_bson::doc! {
        "ObjectCollection": { "Objects": mxrs_bson::build_array(objects.into_iter().map(Bson::Document).collect(),3) },
        "Flows":mxrs_bson::build_array(flows.into_iter().map(Bson::Document).collect(),3),
    };
    merge(previous, &fresh).as_ref() == previous.get_document("ObjectCollection").ok()
}

/// Where an unedited rebuild of `previous` from `declaration` would first
/// differ from what is stored, for diagnostics: `None` when
/// [`preserves_body`] holds.
pub fn rebuild_difference(
    previous: &Document,
    declaration: &mxrs_ir::MicroflowDecl,
) -> Option<String> {
    if !same_parameters(previous, declaration) {
        return Some("its parameters differ".to_string());
    }
    if previous.get_str("Documentation").ok() != Some(declaration.documentation.as_str()) {
        return Some("its documentation differs".to_string());
    }
    let (objects, flows) = crate::flow_compiler::build_flow_graph(
        &declaration.activities,
        &declaration.rescue_activities,
        declaration.return_expression.as_deref(),
        previous.get_str("$Type").ok() == Some("Microflows$Nanoflow"),
    );
    let fresh = mxrs_bson::doc! {
        "ObjectCollection": { "Objects": mxrs_bson::build_array(objects.into_iter().map(Bson::Document).collect(),3) },
        "Flows":mxrs_bson::build_array(flows.into_iter().map(Bson::Document).collect(),3),
    };
    let Some(merged) = merge(previous, &fresh) else {
        let reason = match (structured_nodes(previous), structured_nodes(&fresh)) {
            (None, _) => "the stored graph is not structured",
            (_, None) => "the rebuilt graph is not structured",
            (Some(old), Some(new)) => {
                if pair_nodes(&old, &new, &mut Vec::new()).is_none() {
                    "its nodes do not pair up with the rebuilt graph"
                } else {
                    "its edges differ from the rebuilt graph"
                }
            }
        };
        return Some(reason.to_string());
    };
    let stored = previous.get_document("ObjectCollection").ok()?;
    first_difference(
        &Bson::Document(stored.clone()),
        &Bson::Document(merged),
        "ObjectCollection",
    )
}

fn first_difference(stored: &Bson, rebuilt: &Bson, path: &str) -> Option<String> {
    match (stored, rebuilt) {
        (Bson::Document(stored), Bson::Document(rebuilt)) => {
            let kind = stored.get_str("$Type").unwrap_or_default();
            for (key, value) in stored {
                let here = format!("{path}.{key}");
                match rebuilt.get(key) {
                    None => return Some(format!("{here} ({kind}) is missing after the rebuild")),
                    Some(other) => {
                        if let Some(difference) = first_difference(value, other, &here) {
                            return Some(difference);
                        }
                    }
                }
            }
            rebuilt
                .keys()
                .find(|key| !stored.contains_key(key.as_str()))
                .map(|key| format!("{path}.{key} ({kind}) appears after the rebuild"))
        }
        (Bson::Array(stored), Bson::Array(rebuilt)) => {
            if stored.len() != rebuilt.len() {
                return Some(format!(
                    "{path} holds {} items, {} after the rebuild",
                    stored.len(),
                    rebuilt.len()
                ));
            }
            stored
                .iter()
                .zip(rebuilt)
                .enumerate()
                .find_map(|(index, (left, right))| {
                    first_difference(left, right, &format!("{path}[{index}]"))
                })
        }
        (stored, rebuilt) if stored == rebuilt => None,
        (stored, rebuilt) => Some(format!("{path}: {stored:?} becomes {rebuilt:?}")),
    }
}

/// Compatibility entry point for callers that require a linear body.
pub fn preserves_linear_body(previous: &Document, declaration: &mxrs_ir::MicroflowDecl) -> bool {
    linear_nodes(previous).is_some() && preserves_body(previous, declaration)
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
                            && prior.get("Enumeration") == ty.get("Enumeration")
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
