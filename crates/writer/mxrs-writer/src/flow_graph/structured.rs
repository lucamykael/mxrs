use super::{documents, id, unique_ids};
use mxrs_bson::{Bson, Document, extract_id};
use std::collections::{HashMap, HashSet};

/// Execution structure, independent of the physical order of native objects
/// and of how the model draws the points where paths join.
#[derive(Debug)]
pub enum Node<'a> {
    Simple(&'a Document),
    /// A split on a boolean expression.
    Decision {
        split: &'a Document,
        yes: Vec<Node<'a>>,
        no: Vec<Node<'a>>,
    },
    /// A split with a branch per case: enumeration values on an exclusive
    /// split, entities on an inheritance split.
    Switch {
        split: &'a Document,
        cases: Vec<Case<'a>>,
    },
    Loop {
        node: &'a Document,
        body: Vec<Node<'a>>,
    },
    /// An action or a loop with an error handler of its own. A handler that
    /// does not end continues with whatever follows the node.
    Handled {
        node: Box<Node<'a>>,
        handler: Vec<Node<'a>>,
    },
    /// A point other paths jump to: what follows is the node of this
    /// identity, which those paths reach from somewhere else.
    Label(String),
    /// The end of a path that carries on at the [`Node::Label`] of this
    /// identity.
    Jump(String),
}

/// One branch of a [`Node::Switch`], in the order the model stores its edges.
#[derive(Debug)]
pub struct Case<'a> {
    pub values: Vec<String>,
    pub body: Vec<Node<'a>>,
}

/// An edge between two objects that do something. Merges do nothing: an edge
/// into one is the edge to wherever the merge leads, so two drawings of the
/// same control flow have the same edges.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Edge {
    /// `None` when the path ends at a merge nothing leaves: inside a loop,
    /// the end of the iteration.
    pub to: Option<String>,
    pub cases: Vec<String>,
    pub inheritance: bool,
    pub error: bool,
}

pub(super) struct Edges {
    pub outgoing: HashMap<String, Vec<Edge>>,
    /// Merges and annotations: objects the structure does not include.
    passive: HashSet<String>,
    incoming: HashMap<String, usize>,
}

impl Edges {
    /// The nodes some path comes back to: the targets of the edges that
    /// close a cycle, found depth first in the order the edges are read.
    fn returned_to(&self) -> HashSet<String> {
        let mut targets = HashSet::new();
        let mut done = HashSet::new();
        let mut roots: Vec<&String> = self
            .outgoing
            .keys()
            .filter(|key| self.incoming(key) == 0)
            .collect();
        roots.sort();
        for root in roots {
            // Each entry: a node on the current path and the edge to try next.
            let mut path: Vec<(&str, usize)> = vec![(root.as_str(), 0)];
            let mut on_path: HashSet<&str> = HashSet::from([root.as_str()]);
            while let Some((node, next)) = path.last_mut() {
                let node: &str = node;
                match self.from(node).get(*next) {
                    None => {
                        on_path.remove(node);
                        done.insert(node.to_string());
                        path.pop();
                    }
                    Some(edge) => {
                        *next += 1;
                        let Some(to) = edge.to.as_deref() else {
                            continue;
                        };
                        if on_path.contains(to) {
                            targets.insert(to.to_string());
                        } else if !done.contains(to) {
                            on_path.insert(to);
                            path.push((to, 0));
                        }
                    }
                }
            }
        }
        targets
    }

    fn from(&self, key: &str) -> &[Edge] {
        self.outgoing.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    fn incoming(&self, key: &str) -> usize {
        self.incoming.get(key).copied().unwrap_or(0)
    }
}

struct RawEdge {
    from: String,
    to: String,
    cases: Vec<String>,
    inheritance: bool,
    error: bool,
}

/// Reads the sequence flows of a microflow document with merges resolved
/// away. Annotations and the lines that attach them are drawing, not flow.
/// Cross-container edges, merges that branch and merge cycles have no answer.
pub(super) fn read_edges(doc: &Document) -> Option<Edges> {
    if !unique_ids(&Bson::Document(doc.clone()), &mut HashSet::new()) {
        return None;
    }
    let collection = doc.get_document("ObjectCollection").ok()?;
    if doc.contains_key("Flows") && collection.contains_key("Flows") {
        return None;
    }
    let mut owners = HashMap::new();
    let mut kinds = HashMap::new();
    collect_owners(collection, None, &mut owners, &mut kinds, 0)?;
    let mut raw = Vec::new();
    for edge in documents(doc.get("Flows").or_else(|| collection.get("Flows"))?)? {
        id(edge)?;
        let from = extract_id(edge.get("OriginPointer")?)?;
        let to = extract_id(edge.get("DestinationPointer")?)?;
        match edge.get_str("$Type").ok()? {
            "Microflows$AnnotationFlow" => {
                owners.get(&from)?;
                owners.get(&to)?;
                continue;
            }
            "Microflows$SequenceFlow" => {}
            _ => return None,
        }
        let error = match edge.get("IsErrorHandler") {
            None | Some(Bson::Boolean(false)) => false,
            Some(Bson::Boolean(true)) => true,
            _ => return None,
        };
        if owners.get(&from)? != owners.get(&to)? {
            return None;
        }
        let mut cases = Vec::new();
        let mut enumeration = false;
        let mut inheritance = false;
        if let Some(value) = edge.get("CaseValues") {
            for case in documents(value)? {
                id(case)?;
                if case
                    .keys()
                    .any(|key| !matches!(key.as_str(), "$ID" | "$Type" | "Value"))
                {
                    return None;
                }
                match case.get_str("$Type").ok()? {
                    "Microflows$NoCase" => {}
                    "Microflows$EnumerationCase" => {
                        enumeration = true;
                        cases.push(case.get_str("Value").ok()?.to_string());
                    }
                    "Microflows$InheritanceCase" => {
                        inheritance = true;
                        cases.push(case.get_str("Value").ok()?.to_string());
                    }
                    _ => return None,
                }
            }
        }
        if enumeration && inheritance {
            return None;
        }
        raw.push(RawEdge {
            from,
            to,
            cases,
            inheritance,
            error,
        });
    }
    let is = |key: &str, kind: &str| kinds.get(key).map(String::as_str) == Some(kind);
    let merge = |key: &str| is(key, "Microflows$ExclusiveMerge");
    // What leaves a merge is one plain edge, or nothing.
    let mut leaving: HashMap<&str, &str> = HashMap::new();
    let mut entered = HashSet::new();
    for edge in &raw {
        if is(&edge.from, "Microflows$Annotation") || is(&edge.to, "Microflows$Annotation") {
            return None;
        }
        if merge(&edge.to) {
            entered.insert(edge.to.as_str());
        }
        if merge(&edge.from)
            && (edge.error
                || !edge.cases.is_empty()
                || leaving.insert(&edge.from, &edge.to).is_some())
        {
            return None;
        }
    }
    let mut passive = HashSet::new();
    for (key, kind) in &kinds {
        match kind.as_str() {
            "Microflows$Annotation" => {
                passive.insert(key.clone());
            }
            "Microflows$ExclusiveMerge" => {
                // A merge nothing reaches is a stray object, not a drawing
                // of anything.
                if !entered.contains(key.as_str()) {
                    return None;
                }
                passive.insert(key.clone());
            }
            _ => {}
        }
    }
    let resolve = |start: &str| -> Option<Option<String>> {
        let mut cursor = start;
        let mut steps = 0;
        while merge(cursor) {
            steps += 1;
            if steps > kinds.len() {
                return None;
            }
            match leaving.get(cursor) {
                Some(next) => cursor = next,
                None => return Some(None),
            }
        }
        Some(Some(cursor.to_string()))
    };
    let mut outgoing: HashMap<String, Vec<Edge>> = HashMap::new();
    let mut incoming: HashMap<String, usize> = HashMap::new();
    for edge in &raw {
        if merge(&edge.from) {
            continue;
        }
        let to = resolve(&edge.to)?;
        if let Some(to) = &to {
            *incoming.entry(to.clone()).or_default() += 1;
        }
        outgoing.entry(edge.from.clone()).or_default().push(Edge {
            to,
            cases: edge.cases.clone(),
            inheritance: edge.inheritance,
            error: edge.error,
        });
    }
    // The order a model happens to store its edges in says nothing: a
    // handler is read before what follows its activity, the branch taken
    // when a condition holds before the other, and cases by their values.
    for edges in outgoing.values_mut() {
        edges.sort_by(|a, b| {
            let key = |edge: &Edge| {
                (
                    !edge.error,
                    edge.cases != ["true"],
                    edge.cases.clone(),
                    edge.to.clone(),
                )
            };
            key(a).cmp(&key(b))
        });
    }
    Some(Edges {
        outgoing,
        passive,
        incoming,
    })
}

/// Reads a flow as nested blocks: sequences, splits whose branches end or
/// rejoin at one point, loops, and activities with an error handler that
/// ends or rejoins right after them.
///
/// A graph that is not built that way is still read, with the points its
/// paths converge on named: first the ones a path returns to — a retry —
/// and, where that is not enough, every one. What reaches such a point from
/// elsewhere is a [`Node::Jump`] to its [`Node::Label`]. Disconnected
/// objects and edges that cross a loop's boundary have no reading.
pub fn structured_nodes(doc: &Document) -> Option<Vec<Node<'_>>> {
    let edges = read_edges(doc)?;
    let collection = doc.get_document("ObjectCollection").ok()?;
    let returns = edges.returned_to();
    let converging: HashSet<String> = edges
        .incoming
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(key, _)| key.clone())
        .collect();
    [HashSet::new(), returns, converging]
        .iter()
        .find_map(|labels| parse_collection(collection, &edges, labels, false, 0))
}

fn collect_owners(
    collection: &Document,
    owner: Option<&str>,
    owners: &mut HashMap<String, Option<String>>,
    kinds: &mut HashMap<String, String>,
    depth: usize,
) -> Option<()> {
    if depth > 128 {
        return None;
    }
    for node in documents(collection.get("Objects")?)? {
        let kind = node.get_str("$Type").ok()?;
        if kind == "Microflows$MicroflowParameter" && depth == 0 {
            continue;
        }
        let key = id(node)?;
        if owners
            .insert(key.clone(), owner.map(str::to_string))
            .is_some()
        {
            return None;
        }
        kinds.insert(key.clone(), kind.to_string());
        if kind == "Microflows$LoopedActivity" {
            let inner = node.get_document("ObjectCollection").ok()?;
            // The supported native format stores all connections at flow level.
            if inner.contains_key("Flows") {
                return None;
            }
            collect_owners(inner, Some(&key), owners, kinds, depth + 1)?;
        }
    }
    Some(())
}

fn parse_collection<'a>(
    collection: &'a Document,
    edges: &Edges,
    labels: &HashSet<String>,
    in_loop: bool,
    depth: usize,
) -> Option<Vec<Node<'a>>> {
    if depth > 128 {
        return None;
    }
    let mut nodes = HashMap::new();
    for node in documents(collection.get("Objects")?)? {
        if !in_loop && node.get_str("$Type").ok() == Some("Microflows$MicroflowParameter") {
            continue;
        }
        let key = id(node)?;
        if !edges.passive.contains(&key) {
            nodes.insert(key, node);
        }
    }
    if nodes.is_empty() {
        return in_loop.then(Vec::new);
    }
    let roots: Vec<_> = nodes
        .keys()
        .filter(|key| edges.incoming(key) == 0)
        .cloned()
        .collect();
    let [root] = roots.as_slice() else {
        return None;
    };
    if !in_loop && nodes.get(root)?.get_str("$Type").ok()? != "Microflows$StartEvent" {
        return None;
    }
    let mut walk = Walk {
        nodes,
        edges,
        labels,
        seen: HashSet::new(),
        in_loop,
    };
    let (body, stop) = walk.block(Some(root.clone()), depth + 1)?;
    match stop {
        Stop::Ends => {}
        Stop::Join(None, _) if in_loop => {}
        Stop::Join(..) => return None,
    }
    (walk.seen.len() == walk.nodes.len()).then_some(body)
}

/// Whether a branch is a guard: a handful of activities, one after the
/// other, and then the end of the path.
fn guard(body: &[Node<'_>]) -> bool {
    body.len() <= 4
        && body
            .iter()
            .all(|node| matches!(node, Node::Simple(_) | Node::Jump(_)))
}

/// How a block stops: every path through it ended on its own, or what is
/// left of it arrives — over `usize` edges — at a point other paths arrive
/// at too. `None` is the end of a loop's body.
enum Stop {
    Ends,
    Join(Option<String>, usize),
}

struct Walk<'a, 'e> {
    nodes: HashMap<String, &'a Document>,
    edges: &'e Edges,
    labels: &'e HashSet<String>,
    seen: HashSet<String>,
    in_loop: bool,
}

impl<'a> Walk<'a, '_> {
    /// Continues at `target` when every edge into it comes from the block
    /// that just closed there; otherwise the join belongs to something that
    /// encloses the block.
    fn close(&self, target: Option<String>, arrived: usize) -> Result<String, Stop> {
        match target {
            Some(target) if arrived == self.edges.incoming(&target) => Ok(target),
            target => Err(Stop::Join(target, arrived)),
        }
    }

    fn block(&mut self, start: Option<String>, depth: usize) -> Option<(Vec<Node<'a>>, Stop)> {
        if depth > 128 {
            return None;
        }
        let mut cursor = start;
        // Whether the node at the cursor is a join this block closed, and so
        // continues from rather than stops at.
        let mut closed = false;
        let mut result = Vec::new();
        loop {
            let Some(key) = cursor.take() else {
                return self.in_loop.then_some((result, Stop::Join(None, 1)));
            };
            if self.labels.contains(&key) {
                // The first path to get here goes on from here; the others
                // jump to where it did.
                if self.seen.contains(&key) {
                    result.push(Node::Jump(key));
                    return Some((result, Stop::Ends));
                }
                self.nodes.get(&key)?;
                result.push(Node::Label(key.clone()));
            } else if !closed && self.edges.incoming(&key) > 1 {
                return Some((result, Stop::Join(Some(key), 1)));
            }
            closed = false;
            let node = *self.nodes.get(&key)?;
            if !self.seen.insert(key.clone()) {
                return None;
            }
            let kind = node.get_str("$Type").ok()?;
            let edges = self.edges.from(&key);
            match kind {
                "Microflows$ExclusiveSplit" | "Microflows$InheritanceSplit" => {
                    let inheritance = kind == "Microflows$InheritanceSplit";
                    if edges.is_empty()
                        || edges.iter().any(|edge| {
                            edge.error || edge.cases.is_empty() || edge.inheritance != inheritance
                        })
                    {
                        return None;
                    }
                    let mut values = HashSet::new();
                    if !edges
                        .iter()
                        .flat_map(|edge| &edge.cases)
                        .all(|value| values.insert(value.as_str()))
                    {
                        return None;
                    }
                    let mut cases = Vec::new();
                    let mut join: Option<(Option<String>, usize)> = None;
                    let mut joining = Vec::new();
                    for edge in edges {
                        let (body, stop) = self.block(edge.to.clone(), depth + 1)?;
                        if let Stop::Join(target, arrived) = stop {
                            joining.push(cases.len());
                            match &mut join {
                                None => join = Some((target, arrived)),
                                Some((known, total)) if *known == target => *total += arrived,
                                Some(_) => return None,
                            }
                        }
                        cases.push(Case {
                            values: edge.cases.clone(),
                            body,
                        });
                    }
                    // A split whose other branches are guards — a few
                    // activities and an end — reads as a guard followed by
                    // the rest of the flow, not as the rest of the flow
                    // nested inside its last branch. Nothing in the graph
                    // tells the two apart; this is the one reading.
                    let continues = match &join {
                        // The one branch that runs off the end of a loop's
                        // body while the others end.
                        Some((None, _)) => match joining.as_slice() {
                            [index] if cases.len() > 1 => Some(*index),
                            _ => None,
                        },
                        Some(_) => None,
                        None => {
                            let mut long = cases
                                .iter()
                                .enumerate()
                                .filter(|(_, case)| !guard(&case.body))
                                .map(|(index, _)| index);
                            // Among guards only, a branch that does nothing
                            // but end is the flow ending, not a branch.
                            let mut bare = cases
                                .iter()
                                .enumerate()
                                .filter(|(_, case)| {
                                    matches!(case.body.as_slice(), [Node::Simple(end)]
                                        if end.get_str("$Type").ok() == Some("Microflows$EndEvent"))
                                })
                                .map(|(index, _)| index);
                            match (long.next(), long.next()) {
                                (Some(index), None) if cases.len() > 1 => Some(index),
                                (None, _) => match (bare.next(), bare.next()) {
                                    (Some(index), None) if cases.len() > 1 => Some(index),
                                    _ => None,
                                },
                                _ => None,
                            }
                        }
                    };
                    let rest = continues
                        .map(|index| std::mem::take(&mut cases[index].body))
                        .unwrap_or_default();
                    let boolean = !inheritance
                        && cases.len() == 2
                        && cases.iter().all(|case| case.values.len() == 1)
                        && values.contains("true")
                        && values.contains("false");
                    if boolean {
                        let (mut yes, mut no) = (Vec::new(), Vec::new());
                        for case in cases {
                            if case.values[0] == "true" {
                                yes = case.body;
                            } else {
                                no = case.body;
                            }
                        }
                        result.push(Node::Decision {
                            split: node,
                            yes,
                            no,
                        });
                    } else {
                        result.push(Node::Switch { split: node, cases });
                    }
                    result.extend(rest);
                    let Some((target, arrived)) = join else {
                        return Some((result, Stop::Ends));
                    };
                    match self.close(target, arrived) {
                        Ok(target) => {
                            cursor = Some(target);
                            closed = true;
                        }
                        Err(stop) => return Some((result, stop)),
                    }
                }
                "Microflows$LoopedActivity" | "Microflows$ActionActivity" => {
                    let inner = if kind == "Microflows$LoopedActivity" {
                        Node::Loop {
                            node,
                            body: parse_collection(
                                node.get_document("ObjectCollection").ok()?,
                                self.edges,
                                self.labels,
                                true,
                                depth + 1,
                            )?,
                        }
                    } else {
                        Node::Simple(node)
                    };
                    if edges.iter().any(|edge| !edge.cases.is_empty()) {
                        return None;
                    }
                    let mut plain = edges.iter().filter(|edge| !edge.error);
                    let mut failing = edges.iter().filter(|edge| edge.error);
                    let next = plain.next().and_then(|edge| edge.to.clone());
                    let handler = failing.next();
                    if plain.next().is_some() || failing.next().is_some() {
                        return None;
                    }
                    let Some(handler) = handler else {
                        result.push(inner);
                        cursor = next;
                        continue;
                    };
                    let (handler, stop) = self.block(handler.to.clone(), depth + 1)?;
                    result.push(Node::Handled {
                        node: Box::new(inner),
                        handler,
                    });
                    match stop {
                        Stop::Ends => cursor = next,
                        // A handler that carries on does so with what follows
                        // the activity, and nowhere else.
                        Stop::Join(target, _) if target != next => return None,
                        Stop::Join(target, arrived) => match self.close(target, arrived + 1) {
                            Ok(target) => {
                                cursor = Some(target);
                                closed = true;
                            }
                            Err(stop) => return Some((result, stop)),
                        },
                    }
                }
                "Microflows$BreakEvent" | "Microflows$ContinueEvent" if self.in_loop => {
                    if !edges.is_empty() {
                        return None;
                    }
                    result.push(Node::Simple(node));
                    return Some((result, Stop::Ends));
                }
                "Microflows$EndEvent" if !self.in_loop => {
                    if !edges.is_empty() {
                        return None;
                    }
                    result.push(Node::Simple(node));
                    return Some((result, Stop::Ends));
                }
                "Microflows$ErrorEvent" => {
                    if !edges.is_empty() {
                        return None;
                    }
                    result.push(Node::Simple(node));
                    return Some((result, Stop::Ends));
                }
                "Microflows$StartEvent" if !self.in_loop && self.seen.len() == 1 => {
                    let [edge] = edges else {
                        return None;
                    };
                    if edge.error || !edge.cases.is_empty() {
                        return None;
                    }
                    result.push(Node::Simple(node));
                    cursor = edge.to.clone();
                }
                _ => return None,
            }
        }
    }
}
