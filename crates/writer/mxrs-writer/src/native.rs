//! Documents stated whole — a flow's action, a page — lowered into the
//! model: every nested document gets an identity, every list its marker,
//! and every pointer the identity of the document it names.

use std::collections::HashMap;

use mxrs_bson::{Binary, BinarySubtype, Bson, Document};
use mxrs_ir::{NativeDocument, NativeValue};

use crate::Result;
use mxrs_identity::{ArtifactKind, ProjectIdentity};

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn assign(
    document: &NativeDocument,
    path: &str,
    identity: &mut dyn FnMut(&str) -> String,
    ids: &mut HashMap<String, String>,
) {
    ids.insert(path.to_string(), identity(path));
    for (key, value) in &document.fields {
        let nested = join(path, key);
        match value {
            NativeValue::Document(document) => assign(document, &nested, identity, ids),
            NativeValue::List(_, items) => {
                for (index, item) in items.iter().enumerate() {
                    if let NativeValue::Document(document) = item {
                        assign(document, &format!("{nested}[{index}]"), identity, ids);
                    }
                }
            }
            _ => {}
        }
    }
}

fn blob(id: &str) -> Bson {
    match mxrs_bson::uuid_to_blob(id) {
        Ok(bytes) => Bson::Binary(Binary {
            subtype: BinarySubtype::Generic,
            bytes: bytes.to_vec(),
        }),
        Err(_) => Bson::String(id.to_string()),
    }
}

fn build(
    document: &NativeDocument,
    path: &str,
    ids: &HashMap<String, String>,
    stored: bool,
) -> Document {
    let id = &ids[path];
    let mut lowered = Document::new();
    lowered.insert(
        "$ID",
        if stored {
            blob(id)
        } else {
            Bson::String(id.clone())
        },
    );
    lowered.insert("$Type", document.ty.clone());
    for (key, value) in &document.fields {
        lowered.insert(
            key.clone(),
            build_value(value, &join(path, key), ids, stored),
        );
    }
    lowered
}

fn build_value(
    value: &NativeValue,
    path: &str,
    ids: &HashMap<String, String>,
    stored: bool,
) -> Bson {
    match value {
        NativeValue::Null => Bson::Null,
        NativeValue::Bool(value) => Bson::Boolean(*value),
        NativeValue::Int32(value) => Bson::Int32(*value),
        NativeValue::Int64(value) => Bson::Int64(*value),
        NativeValue::Text(value) => Bson::String(value.clone()),
        NativeValue::Document(document) => Bson::Document(build(document, path, ids, stored)),
        NativeValue::List(marker, items) => Bson::Array(mxrs_bson::build_array(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| build_value(item, &format!("{path}[{index}]"), ids, stored))
                .collect(),
            *marker,
        )),
        // A pointer at nothing the document holds points at nothing.
        NativeValue::Pointer(target) => ids.get(target).map_or(Bson::Null, |id| blob(id)),
        NativeValue::Identity(id) => blob(id),
    }
}

/// `document` as the model stores it, each nested document identified by
/// what `identity` gives for where it is. `stored` writes identities the
/// way a stored unit holds them rather than as text.
pub(crate) fn lower(
    document: &NativeDocument,
    identity: &mut dyn FnMut(&str) -> String,
    stored: bool,
) -> Document {
    let mut ids = HashMap::new();
    assign(document, "", identity, &mut ids);
    build(document, "", &ids, stored)
}

fn id_of(document: &Document) -> Option<String> {
    document.get("$ID").and_then(mxrs_bson::extract_id)
}

/// What tells a list's item apart from the others: its type and its name.
fn named(document: &Document) -> Option<(&str, &str)> {
    let name = document
        .get_str("Name")
        .ok()
        .filter(|name| !name.is_empty())?;
    Some((document.get_str("$Type").unwrap_or_default(), name))
}

const WIDGET_PROPERTY: &str = "CustomWidgets$WidgetProperty";
const WIDGET_OBJECT: &str = "CustomWidgets$WidgetObject";

fn documents(items: &[Bson]) -> Vec<&Document> {
    items.iter().filter_map(Bson::as_document).collect()
}

fn pointer(document: &Document) -> Option<String> {
    document.get("TypePointer").and_then(mxrs_bson::extract_id)
}

/// Lists of a widget object's properties, waiting for the types they point
/// at to be paired: a property stands for the stored one of the same type.
type Waiting<'a> = Vec<(Vec<&'a Document>, Vec<&'a Document>)>;

/// Pairs each document of `fresh` with the one of `previous` it stands
/// for: the same type in the same place, or — among the items of a list —
/// the same type and name.
fn pair<'a>(
    previous: &'a Document,
    fresh: &'a Document,
    adopted: &mut HashMap<String, String>,
    waiting: &mut Waiting<'a>,
) {
    if previous.get_str("$Type").ok() != fresh.get_str("$Type").ok() {
        return;
    }
    if let (Some(old), Some(new)) = (id_of(previous), id_of(fresh)) {
        adopted.insert(new, old);
    }
    for (key, value) in fresh {
        match (previous.get(key), value) {
            (Some(Bson::Document(old)), Bson::Document(new)) => pair(old, new, adopted, waiting),
            (Some(Bson::Array(old)), Bson::Array(new)) => {
                let (old, new) = (documents(old), documents(new));
                if new
                    .first()
                    .is_some_and(|item| item.get_str("$Type").ok() == Some(WIDGET_PROPERTY))
                {
                    waiting.push((old, new));
                    continue;
                }
                let mut taken = vec![false; old.len()];
                let mut unnamed = Vec::new();
                for item in &new {
                    let found = named(item).and_then(|name| {
                        old.iter()
                            .enumerate()
                            .position(|(index, other)| !taken[index] && named(other) == Some(name))
                    });
                    match found {
                        Some(index) => {
                            taken[index] = true;
                            pair(old[index], item, adopted, waiting);
                        }
                        None => unnamed.push(*item),
                    }
                }
                // An item without a name stands for the stored one that says
                // the same, so taking one out of a list leaves the others
                // who they were.
                let mut changed = Vec::new();
                for item in unnamed {
                    let found = old.iter().enumerate().position(|(index, other)| {
                        !taken[index] && named(other).is_none() && says_the_same(other, item)
                    });
                    match found {
                        Some(index) => {
                            taken[index] = true;
                            pair(old[index], item, adopted, waiting);
                        }
                        None => changed.push(item),
                    }
                }
                // What is left stands for what is left, in order.
                let mut left = old
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !taken[*index])
                    .map(|(_, document)| *document);
                for item in changed {
                    let Some(other) = left.next() else {
                        break;
                    };
                    if named(other).is_none() || named(item).is_none() {
                        pair(other, item, adopted, waiting);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Whether two documents say the same, whatever their identities and
/// whatever they point at.
fn says_the_same(left: &Document, right: &Document) -> bool {
    fn same(left: &Bson, right: &Bson) -> bool {
        match (left, right) {
            (Bson::Document(left), Bson::Document(right)) => says_the_same(left, right),
            (Bson::Array(left), Bson::Array(right)) => {
                left.len() == right.len() && left.iter().zip(right).all(|(a, b)| same(a, b))
            }
            (Bson::Binary(_), Bson::Binary(_)) => true,
            // A number is the same number however wide it is stored.
            (Bson::Int32(left), Bson::Int64(right)) | (Bson::Int64(right), Bson::Int32(left)) => {
                i64::from(*left) == *right
            }
            _ => left == right,
        }
    }
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|((left_key, left), (right_key, right))| {
                left_key == right_key && (left_key == "$ID" || same(left, right))
            })
}

fn identities(value: &Bson, found: &mut std::collections::HashSet<String>) {
    match value {
        Bson::Document(document) => {
            if let Some(id) = id_of(document) {
                found.insert(id);
            }
            for value in document.values() {
                identities(value, found);
            }
        }
        Bson::Array(items) => {
            for item in items {
                identities(item, found);
            }
        }
        _ => {}
    }
}

fn adopt(value: &mut Bson, adopted: &HashMap<String, String>) {
    match value {
        Bson::Binary(_) => {
            if let Some(old) = mxrs_bson::extract_id(value).and_then(|id| adopted.get(&id)) {
                *value = blob(old);
            }
        }
        Bson::Document(document) => {
            for (_, value) in document.iter_mut() {
                adopt(value, adopted);
            }
        }
        Bson::Array(items) => {
            for item in items {
                adopt(item, adopted);
            }
        }
        _ => {}
    }
}

/// The order each stored widget object keeps its properties in, by the
/// object's identity.
fn stored_orders(value: &Bson, orders: &mut HashMap<String, Vec<String>>) {
    match value {
        Bson::Document(document) => {
            if document.get_str("$Type").ok() == Some(WIDGET_OBJECT)
                && let (Some(id), Ok(properties)) =
                    (id_of(document), document.get_array("Properties"))
            {
                orders.insert(
                    id,
                    documents(properties)
                        .into_iter()
                        .filter_map(id_of)
                        .collect(),
                );
            }
            for value in document.values() {
                stored_orders(value, orders);
            }
        }
        Bson::Array(items) => {
            for item in items {
                stored_orders(item, orders);
            }
        }
        _ => {}
    }
}

/// Puts the properties of each widget object in the order its stored
/// counterpart keeps them; one the stored object lacks goes last.
fn keep_orders(value: &mut Bson, orders: &HashMap<String, Vec<String>>) {
    match value {
        Bson::Document(document) => {
            if document.get_str("$Type").ok() == Some(WIDGET_OBJECT)
                && let Some(order) = id_of(document).and_then(|id| orders.get(&id))
                && let Ok(properties) = document.get_array_mut("Properties")
            {
                let marker: Vec<Bson> = properties
                    .iter()
                    .filter(|item| item.as_document().is_none())
                    .cloned()
                    .collect();
                let mut items: Vec<Bson> = properties
                    .iter()
                    .filter(|item| item.as_document().is_some())
                    .cloned()
                    .collect();
                items.sort_by_key(|item| {
                    item.as_document()
                        .and_then(id_of)
                        .and_then(|id| order.iter().position(|known| known == &id))
                        .unwrap_or(usize::MAX)
                });
                *properties = marker.into_iter().chain(items).collect();
            }
            for (_, value) in document.iter_mut() {
                keep_orders(value, orders);
            }
        }
        Bson::Array(items) => {
            for item in items {
                keep_orders(item, orders);
            }
        }
        _ => {}
    }
}

/// `fresh` with the identities of `previous` wherever a document of one
/// stands for a document of the other, so what points at a widget from
/// outside still finds it and an unchanged form is stored unchanged. A
/// widget's properties keep the order they are stored in, which says
/// nothing a declaration could.
pub(crate) fn keep_identities(previous: &Document, fresh: Document) -> Document {
    let mut adopted = HashMap::new();
    {
        let mut waiting = Waiting::new();
        pair(previous, &fresh, &mut adopted, &mut waiting);
        // A property stands for the stored property of its type, once the
        // types — a document apart — are paired.
        while let Some((old, new)) = waiting.pop() {
            for property in new {
                let stored = pointer(property)
                    .and_then(|ty| adopted.get(&ty).cloned())
                    .and_then(|ty| {
                        old.iter()
                            .find(|other| pointer(other).as_ref() == Some(&ty))
                    });
                if let Some(stored) = stored {
                    pair(stored, property, &mut adopted, &mut waiting);
                }
            }
        }
    }
    // A document that stands for no stored one keeps the identity it was
    // given, unless a stored document already has it: an earlier build
    // gave identities by place too, and places shift.
    let mut stored = std::collections::HashSet::new();
    identities(&Bson::Document(previous.clone()), &mut stored);
    let mut given = std::collections::HashSet::new();
    identities(&Bson::Document(fresh.clone()), &mut given);
    let mut unpaired: Vec<&String> = given
        .iter()
        .filter(|id| !adopted.contains_key(*id) && stored.contains(*id))
        .collect();
    unpaired.sort();
    let mut renamed = HashMap::new();
    for id in unpaired {
        let namespace = uuid::Uuid::parse_str(id).unwrap_or(uuid::Uuid::NAMESPACE_OID);
        let free = (0_u32..)
            .map(|attempt| {
                uuid::Uuid::new_v5(&namespace, format!("again {attempt}").as_bytes()).to_string()
            })
            .find(|candidate| !stored.contains(candidate) && !given.contains(candidate))
            .expect("an identity nothing has yet");
        renamed.insert(id.clone(), free);
    }
    adopted.extend(renamed);
    let mut kept = Bson::Document(fresh);
    adopt(&mut kept, &adopted);
    let mut orders = HashMap::new();
    stored_orders(&Bson::Document(previous.clone()), &mut orders);
    keep_orders(&mut kept, &orders);
    match kept {
        Bson::Document(mut kept) => {
            // The unit's own identity is the caller's to give.
            if let Some(id) = previous.get("$ID") {
                kept.insert("$ID", id.clone());
            }
            kept
        }
        _ => unreachable!("a document stays one"),
    }
}

/// Writes the pages, layouts and snippets a module states whole.
pub(crate) fn synchronize_forms_with_identity(
    mpr: &mut mxrs_mpr::MprFile,
    module_id: &str,
    module_name: &str,
    forms: &[mxrs_ir::FormDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    let mut existing: HashMap<String, HashMap<String, (String, Document)>> = HashMap::new();
    for form in forms {
        let kind = form.kind().to_string();
        if !existing.contains_key(&kind) {
            let found = crate::documents::existing_documents_by_name(mpr, module_id, &kind)?;
            existing.insert(kind.clone(), found);
        }
        let previous = existing[&kind].get(form.name());
        let id = previous.map_or_else(
            || {
                // A layout has its own kind; a page and a snippet of one
                // name are told apart by what they are.
                let artifact = match kind.as_str() {
                    "Forms$Layout" => ArtifactKind::Layout,
                    _ => ArtifactKind::Page,
                };
                let qualified = match kind.as_str() {
                    "Forms$Snippet" => format!("snippet:{module_name}.{}", form.name()),
                    _ => format!("{module_name}.{}", form.name()),
                };
                identity.artifact_id(artifact, &qualified)
            },
            |(id, _)| id.clone(),
        );
        let namespace = uuid::Uuid::parse_str(&id).unwrap_or(uuid::Uuid::NAMESPACE_OID);
        let fresh = lower(
            &form.document,
            &mut |path| {
                if path.is_empty() {
                    id.clone()
                } else {
                    uuid::Uuid::new_v5(&namespace, path.as_bytes()).to_string()
                }
            },
            true,
        );
        match previous {
            Some((_, stored)) => {
                let document = keep_identities(stored, fresh);
                // A form that states what is stored is left as it is stored.
                if &document != stored {
                    mpr.update_unit(&id, document)?;
                }
            }
            None => {
                mpr.insert_unit(module_id, "Documents", fresh, Some(&id))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(name: &str) -> NativeDocument {
        NativeDocument::new("Forms$DivContainer").with("Name", name)
    }

    fn page(widgets: Vec<NativeDocument>) -> NativeDocument {
        NativeDocument::new("Forms$TabControl")
            .with(
                "DefaultPagePointer",
                NativeValue::Pointer("Widgets[0]".into()),
            )
            .with(
                "Widgets",
                NativeValue::List(2, widgets.into_iter().map(NativeValue::from).collect()),
            )
    }

    fn lowered(document: &NativeDocument, seed: &str) -> Document {
        let namespace = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, seed.as_bytes());
        lower(
            document,
            &mut |path| uuid::Uuid::new_v5(&namespace, path.as_bytes()).to_string(),
            true,
        )
    }

    fn widgets(document: &Document) -> Vec<(String, String)> {
        document
            .get_array("Widgets")
            .unwrap()
            .iter()
            .filter_map(Bson::as_document)
            .map(|widget| {
                (
                    widget.get_str("Name").unwrap().to_string(),
                    id_of(widget).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn a_pointer_is_the_identity_of_the_document_it_names() {
        let document = lowered(&page(vec![widget("a"), widget("b")]), "one");
        assert_eq!(
            mxrs_bson::extract_id(document.get("DefaultPagePointer").unwrap()),
            Some(widgets(&document)[0].1.clone())
        );
    }

    #[test]
    fn a_document_stated_again_keeps_its_identities() {
        let stored = lowered(&page(vec![widget("a"), widget("b")]), "stored");
        // The same form, built anew, is the stored one.
        let same = lowered(&page(vec![widget("a"), widget("b")]), "fresh");
        assert_ne!(same, stored);
        assert_eq!(keep_identities(&stored, same), stored);
        // A widget put before the others leaves them who they were.
        let edited = keep_identities(
            &stored,
            lowered(
                &page(vec![widget("new"), widget("a"), widget("b")]),
                "fresh",
            ),
        );
        let (before, after) = (widgets(&stored), widgets(&edited));
        assert_eq!(after[1], before[0]);
        assert_eq!(after[2], before[1]);
        assert!(before.iter().all(|(_, id)| id != &after[0].1));
        // A document that stands for no stored one never takes the identity
        // of one: an earlier build gave identities by place too.
        let before_again = keep_identities(
            &edited,
            lowered(
                &page(vec![
                    widget("first"),
                    widget("new"),
                    widget("a"),
                    widget("b"),
                ]),
                "fresh",
            ),
        );
        let mut ids: Vec<String> = widgets(&before_again)
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        assert_eq!(widgets(&before_again)[1], after[0]);
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 4);
        // The pointer follows the document it names.
        assert_eq!(
            mxrs_bson::extract_id(edited.get("DefaultPagePointer").unwrap()),
            Some(after[0].1.clone())
        );
    }

    #[test]
    fn an_unnamed_item_taken_out_leaves_the_others_who_they_were() {
        let column = |caption: &str, inner: &str| {
            NativeDocument::new("Forms$Column")
                .with("Caption", caption)
                .with("Widgets", NativeValue::List(2, vec![widget(inner).into()]))
        };
        let grid = |columns: Vec<NativeDocument>| {
            NativeDocument::new("Forms$Grid").with(
                "Columns",
                NativeValue::List(2, columns.into_iter().map(NativeValue::from).collect()),
            )
        };
        let stored = lowered(
            &grid(vec![column("A", "a"), column("B", "b"), column("C", "c")]),
            "stored",
        );
        let edited = keep_identities(
            &stored,
            lowered(&grid(vec![column("B", "b"), column("C", "c")]), "fresh"),
        );
        let columns = |document: &Document| -> Vec<Document> {
            document
                .get_array("Columns")
                .unwrap()
                .iter()
                .filter_map(Bson::as_document)
                .cloned()
                .collect()
        };
        assert_eq!(columns(&edited), columns(&stored)[1..]);
    }
}
