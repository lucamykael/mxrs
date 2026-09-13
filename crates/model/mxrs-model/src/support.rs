//! Small helpers for reading Mendix BSON documents whose field names vary by
//! casing across Mendix/Studio Pro versions (mxrb's decode methods try
//! several key spellings per field — these mirror that without repeating the
//! `doc["X"] || doc["x"]` chain at every call site).

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

pub fn get_i32_any(doc: &Document, keys: &[&str]) -> Option<i32> {
    match get_any(doc, keys)? {
        Bson::Int32(i) => Some(*i),
        Bson::Int64(i) => i32::try_from(*i).ok(),
        Bson::Double(d) => Some(*d as i32),
        _ => None,
    }
}

pub fn get_doc_any(doc: &Document, keys: &[&str]) -> Option<Document> {
    match get_any(doc, keys)? {
        Bson::Document(d) => Some(d.clone()),
        _ => None,
    }
}

pub fn get_id_any(doc: &Document, keys: &[&str]) -> Option<String> {
    mxrs_bson::extract_id(get_any(doc, keys)?)
}

/// Fetches a Mendix-style marked array field and returns its items (marker
/// stripped), defaulting to empty when the field is absent or not an array.
pub fn items_any(doc: &Document, keys: &[&str]) -> Vec<Bson> {
    match get_any(doc, keys) {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        _ => mxrs_bson::parse_array(None).items,
    }
}

pub fn docs_any(doc: &Document, keys: &[&str]) -> Vec<Document> {
    items_any(doc, keys)
        .into_iter()
        .filter_map(|b| match b {
            Bson::Document(d) => Some(d),
            _ => None,
        })
        .collect()
}
