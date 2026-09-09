//! Tiny casing-tolerant BSON field readers, same rationale as
//! `mxrs-model`'s own (private) `support` module: raw sub-documents this
//! crate reads (an entity's `MaybeGeneralization`, a validation rule, an
//! association's `DeleteBehavior`, `Security$ProjectSecurity`) may carry
//! either PascalCase (real Studio Pro output) or lowercase (mxrs-writer's
//! own editor-DSL round-trip) keys depending on where they came from.
//! Duplicated here rather than depending on `mxrs-model::support` because
//! that module is crate-private — same precedent as `mxrs-cli::compare`'s
//! own small `Security$ProjectSecurity` reader.

use mxrs_bson::{Bson, Document};

pub fn get_any<'a>(doc: &'a Document, keys: &[&str]) -> Option<&'a Bson> {
    keys.iter().find_map(|k| doc.get(*k))
}

pub fn get_str_any(doc: &Document, keys: &[&str]) -> Option<String> {
    match get_any(doc, keys)? {
        Bson::String(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn get_bool_any(doc: &Document, keys: &[&str]) -> Option<bool> {
    match get_any(doc, keys)? {
        Bson::Boolean(b) => Some(*b),
        _ => None,
    }
}

pub fn get_doc_any(doc: &Document, keys: &[&str]) -> Option<Document> {
    match get_any(doc, keys)? {
        Bson::Document(d) => Some(d.clone()),
        _ => None,
    }
}

/// Dedups while keeping first-occurrence order — mirrors Ruby's `#uniq`,
/// used for `AllowedUserRoles` (a role reachable via two different module
/// roles should only be listed once).
pub fn stable_dedup(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

/// Reads a Mendix-array-encoded field (the `$ID: [1,2], items: [...]`
/// convention `mxrs-bson::parse_array` already understands).
pub fn array_items(doc: &Document, keys: &[&str]) -> Vec<Bson> {
    match get_any(doc, keys) {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        _ => Vec::new(),
    }
}

pub fn array_docs(doc: &Document, keys: &[&str]) -> Vec<Document> {
    array_items(doc, keys)
        .into_iter()
        .filter_map(|b| match b {
            Bson::Document(d) => Some(d),
            _ => None,
        })
        .collect()
}

pub fn string_list(doc: &Document, keys: &[&str]) -> Vec<String> {
    array_items(doc, keys)
        .into_iter()
        .filter_map(|b| match b {
            Bson::String(s) => Some(s),
            _ => None,
        })
        .collect()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
