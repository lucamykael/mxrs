use super::{documents, id, unique_ids};
use mxrs_bson::{Bson, Document, extract_id};
use std::collections::{HashMap, HashSet};

/// Execution structure, independent of the physical order of native objects.
#[derive(Debug)]
pub enum Node<'a> {
    Simple(&'a Document),
    Decision {
        split: &'a Document,
        yes: Vec<Node<'a>>,
        no: Vec<Node<'a>>,
        merge: Option<&'a Document>,
    },
    Loop {
        node: &'a Document,
        body: Vec<Node<'a>>,
    },
}

struct Edge {
    to: String,
    case: Option<bool>,
}

struct Graph<'a> {
    nodes: HashMap<String, &'a Document>,
    edges: HashMap<String, Vec<Edge>>,
    seen: HashSet<String>,
}

/// Accepts nested, single-entry structured blocks. Error paths, cross-container
/// edges, cycles, shared branches and disconnected objects are rejected.
pub fn structured_nodes(doc: &Document) -> Option<Vec<Node<'_>>> {
    if !unique_ids(&Bson::Document(doc.clone()), &mut HashSet::new()) {
        return None;
    }
    let collection = doc.get_document("ObjectCollection").ok()?;
    if doc.contains_key("Flows") && collection.contains_key("Flows") {
        return None;
    }
    let mut owners = HashMap::new();
    collect_owners(collection, None, &mut owners, 0)?;
    let edges = documents(doc.get("Flows").or_else(|| collection.get("Flows"))?)?;
    let mut outgoing: HashMap<String, Vec<Edge>> = HashMap::new();
    for edge in edges {
        id(edge)?;
        if edge.get_str("$Type").ok()? != "Microflows$SequenceFlow"
            || !matches!(
                edge.get("IsErrorHandler"),
                None | Some(Bson::Boolean(false))
            )
        {
            return None;
        }
        let from = extract_id(edge.get("OriginPointer")?)?;
        let to = extract_id(edge.get("DestinationPointer")?)?;
        if owners.get(&from)? != owners.get(&to)? {
            return None;
        }
        let case = match edge.get("CaseValues") {
            None => None,
            Some(value) => match documents(value)?.as_slice() {
                [] => None,
                [case] => {
                    id(case)?;
                    if case
                        .keys()
                        .any(|key| !matches!(key.as_str(), "$ID" | "$Type" | "Value"))
                    {
                        return None;
                    }
                    match case.get_str("$Type").ok()? {
                        "Microflows$NoCase" => None,
                        "Microflows$EnumerationCase" => Some(match case.get_str("Value").ok()? {
                            "true" => true,
                            "false" => false,
                            _ => return None,
                        }),
                        _ => return None,
                    }
                }
                _ => return None,
            },
        };
        outgoing.entry(from).or_default().push(Edge { to, case });
    }
    parse_collection(collection, &outgoing, false, 0)
}

fn collect_owners(
    collection: &Document,
    owner: Option<&str>,
    owners: &mut HashMap<String, Option<String>>,
    depth: usize,
) -> Option<()> {
    if depth > 128 {
        return None;
    }
    for node in documents(collection.get("Objects")?)? {
        if node.get_str("$Type").ok()? == "Microflows$MicroflowParameter" && depth == 0 {
            continue;
        }
        let key = id(node)?;
        if owners
            .insert(key.clone(), owner.map(str::to_string))
            .is_some()
        {
            return None;
        }
        if node.get_str("$Type").ok()? == "Microflows$LoopedActivity" {
            let inner = node.get_document("ObjectCollection").ok()?;
            // The supported native format stores all connections at flow level.
            if inner.contains_key("Flows") {
                return None;
            }
            collect_owners(inner, Some(&key), owners, depth + 1)?;
        }
    }
    Some(())
}

fn parse_collection<'a>(
    collection: &'a Document,
    outgoing: &HashMap<String, Vec<Edge>>,
    in_loop: bool,
    depth: usize,
) -> Option<Vec<Node<'a>>> {
    if depth > 128 {
        return None;
    }
    let nodes: HashMap<_, _> = documents(collection.get("Objects")?)?
        .into_iter()
        .filter(|node| {
            in_loop || node.get_str("$Type").ok() != Some("Microflows$MicroflowParameter")
        })
        .map(|node| Some((id(node)?, node)))
        .collect::<Option<_>>()?;
    let destinations: HashSet<_> = nodes
        .keys()
        .filter_map(|key| outgoing.get(key))
        .flatten()
        .map(|edge| edge.to.as_str())
        .collect();
    if nodes.is_empty() {
        return in_loop.then(Vec::new);
    }
    let roots: Vec<_> = nodes
        .keys()
        .filter(|key| !destinations.contains(key.as_str()))
        .cloned()
        .collect();
    let [root] = roots.as_slice() else {
        return None;
    };
    if !in_loop && nodes.get(root)?.get_str("$Type").ok()? != "Microflows$StartEvent" {
        return None;
    }
    let mut graph = Graph {
        nodes,
        edges: HashMap::new(),
        seen: HashSet::new(),
    };
    // Edges borrow no BSON; keep the recursive graph traversal local to this collection.
    for key in graph.nodes.keys() {
        if let Some(edges) = outgoing.get(key) {
            graph.edges.insert(
                key.clone(),
                edges
                    .iter()
                    .map(|e| Edge {
                        to: e.to.clone(),
                        case: e.case,
                    })
                    .collect(),
            );
        }
    }
    let (body, stop) = graph.block(root, outgoing, in_loop, depth + 1)?;
    if stop.is_some() || graph.seen.len() != graph.nodes.len() {
        return None;
    }
    if !in_loop
        && !matches!(body.last(), Some(Node::Simple(end)) if end.get_str("$Type").ok() == Some("Microflows$EndEvent"))
    {
        return None;
    }
    Some(body)
}

impl<'a> Graph<'a> {
    fn block(
        &mut self,
        start: &str,
        outgoing: &HashMap<String, Vec<Edge>>,
        in_loop: bool,
        depth: usize,
    ) -> Option<(Vec<Node<'a>>, Option<String>)> {
        if depth > 128 {
            return None;
        }
        let mut cursor = start.to_string();
        let mut result = Vec::new();
        loop {
            let node = *self.nodes.get(&cursor)?;
            let kind = node.get_str("$Type").ok()?;
            if kind == "Microflows$ExclusiveMerge" {
                return Some((result, Some(cursor)));
            }
            if !self.seen.insert(cursor.clone()) {
                return None;
            }
            match kind {
                "Microflows$ExclusiveSplit" => {
                    let edges = self.edges.get(&cursor)?;
                    if edges.len() != 2 {
                        return None;
                    }
                    let yes = edges.iter().find(|e| e.case == Some(true))?.to.clone();
                    let no = edges.iter().find(|e| e.case == Some(false))?.to.clone();
                    let (yes, yes_stop) = self.block(&yes, outgoing, in_loop, depth + 1)?;
                    let (no, no_stop) = self.block(&no, outgoing, in_loop, depth + 1)?;
                    let stop = match (yes_stop, no_stop) {
                        (Some(a), Some(b)) if a == b => Some(a),
                        (Some(a), None) | (None, Some(a)) => Some(a),
                        (None, None) => None,
                        _ => return None,
                    };
                    let merge = if let Some(key) = &stop {
                        if !self.seen.insert(key.clone()) {
                            return None;
                        }
                        Some(*self.nodes.get(key)?)
                    } else {
                        None
                    };
                    result.push(Node::Decision {
                        split: node,
                        yes,
                        no,
                        merge,
                    });
                    let Some(stop) = stop else {
                        return Some((result, None));
                    };
                    cursor = stop;
                }
                "Microflows$LoopedActivity" => {
                    let body = parse_collection(
                        node.get_document("ObjectCollection").ok()?,
                        outgoing,
                        true,
                        depth + 1,
                    )?;
                    result.push(Node::Loop { node, body });
                }
                "Microflows$BreakEvent" | "Microflows$ContinueEvent" if in_loop => {
                    if self.edges.contains_key(&cursor) {
                        return None;
                    }
                    result.push(Node::Simple(node));
                    return Some((result, None));
                }
                "Microflows$EndEvent" if !in_loop => {
                    if self.edges.contains_key(&cursor) {
                        return None;
                    }
                    result.push(Node::Simple(node));
                    return Some((result, None));
                }
                "Microflows$StartEvent" if !in_loop && self.seen.len() == 1 => {
                    result.push(Node::Simple(node))
                }
                "Microflows$ActionActivity" => result.push(Node::Simple(node)),
                _ => return None,
            }
            match self.edges.get(&cursor).map(Vec::as_slice) {
                Some([edge]) if edge.case.is_none() => cursor = edge.to.clone(),
                None if in_loop => return Some((result, None)),
                _ => return None,
            }
        }
    }
}
