//! Tiny BSON field readers plus Mendix's leading-array-marker convention,
//! duplicated per this workspace's own precedent (see
//! `mxrs-compiler-domain`/`mxrs-compiler-flow`'s own `support.rs` — a
//! shared crate for a handful of ~5-line readers isn't worth the extra
//! workspace dependency two sibling crates would otherwise need only for
//! this). `bson::Document`'s own fallible `get_str`/`get_document`/etc.
//! accessors are used directly at call sites for typed reads; the helpers
//! here cover what those don't: null-safe raw-value fetch, marker-array
//! stripping/rebuilding, and mxrb's `ModelValues#plain_value` deep-strip.

use mxrs_bson::{Bson, Document};

/// `source[field]` — a raw value, defaulting to `Bson::Null` for a missing
/// key (Ruby's `Hash#[]` returns `nil` the same way).
pub fn get(doc: &Document, key: &str) -> Bson {
    doc.get(key).cloned().unwrap_or(Bson::Null)
}

/// `.to_s` — used only on the handful of fields mxrb defensively stringifies
/// (qualified-name reference fields that are ordinarily already strings).
pub fn to_s(value: &Bson) -> String {
    match value {
        Bson::Null => String::new(),
        Bson::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `array(value)` — strips the leading Mendix array-marker integer.
pub fn array_items(doc: &Document, key: &str) -> Vec<Bson> {
    match doc.get(key) {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        _ => Vec::new(),
    }
}

/// Mendix's leading-array-marker convention re-applied to compiled output.
/// The exact marker integer isn't independently verifiable without a real
/// Mendix Runtime to diff Phase 5 output against — `3` matches
/// `mxrs_bson::parse_array`'s own default for a freshly-built array, the
/// same simplification `mxrs-compiler-flow` already makes at several of its
/// own output sites (see e.g. `node.rs`'s `ParameterMappings` default).
pub fn build_array(items: Vec<Bson>) -> Bson {
    Bson::Array(mxrs_bson::build_array(items, 3))
}

/// `plain_array(value) = value ? array(value).map { plain_value } : []` —
/// note the two different empty-array shapes: a genuinely absent/null field
/// defaults to a bare `Bson::Array(vec![])` (nothing to preserve), while a
/// present-but-empty field still goes through `build_array` (matches
/// mxrb's own truthiness check being on the raw field, not on whether the
/// stripped result ends up empty).
pub fn plain_array_field(doc: &Document, key: &str) -> Bson {
    match doc.get(key) {
        None | Some(Bson::Null) => Bson::Array(vec![]),
        Some(_) => build_array(array_items(doc, key).into_iter().map(plain_value).collect()),
    }
}

/// `plain_document(value) = value ? plain_value(value) : nil`.
pub fn plain_document_field(doc: &Document, key: &str) -> Bson {
    match doc.get(key) {
        None | Some(Bson::Null) => Bson::Null,
        Some(v) => plain_value(v.clone()),
    }
}

/// Removes Mendix's leading array markers recursively — mirrors mxrb's
/// `ModelValues#plain_value`.
pub fn plain_value(value: Bson) -> Bson {
    match value {
        Bson::Document(document) => Bson::Document(
            document
                .into_iter()
                .map(|(key, value)| (key, plain_value(value)))
                .collect(),
        ),
        Bson::Array(items) => build_array(
            mxrs_bson::parse_array(Some(&items))
                .items
                .into_iter()
                .map(plain_value)
                .collect(),
        ),
        other => other,
    }
}
