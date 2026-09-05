//! Content-addressed storage for native model fragments that have no
//! concise typed DSL representation yet.
//!
//! Ports `Mxrb::NativeFragmentStore` from `lib/mxrb/native_fragment_store.rb`.
//! Deliberately **not** ported: `NativeFragmentAccess`/`NativeFragmentEvaluation`
//! and the `Thread.current`-bound `current`/`with` accessors — those exist in
//! Ruby only to make a store ambiently reachable while `instance_eval`-ing a
//! DSL script. mxrs has no script interpreter (the DSL is compiled Rust
//! code), so callers just hold a `&NativeFragmentStore` directly; there's no
//! ambient-context problem to solve.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use mxrs_bson::{Bson, Document};

use crate::error::{FragmentStoreError, Result};

/// Optional constraints/edits applied by [`NativeFragmentStore::fetch`].
#[derive(Debug, Default, Clone)]
pub struct FetchOptions {
    /// `$Type` values that must appear somewhere in the fragment (at any
    /// nesting depth), or the fetch fails closed.
    pub types: Vec<String>,
    /// String values under a key ending in `url`/`uri`/`endpoint`/`host`
    /// (case-insensitive) that must appear somewhere in the fragment.
    pub hints: Vec<String>,
    /// Scalar-to-scalar replacements applied to existing top-level fields
    /// after validation. Overriding a missing key, a non-scalar field, or
    /// supplying a non-scalar value is an error.
    pub overrides: BTreeMap<String, Bson>,
}

pub struct NativeFragmentStore {
    root: PathBuf,
    write_lock: Mutex<()>,
}

impl NativeFragmentStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            root: std::path::absolute(root.as_ref())?,
            write_lock: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Serializes and stores `document`, returning its SHA-256 hex digest.
    /// Storing the same content twice is a no-op past the first write.
    pub fn put(&self, document: &Document) -> Result<String> {
        let payload = mxrs_bson::serialize(document)?;
        let digest = sha256_hex(&payload);
        let path = self.fragment_path(&digest)?;

        let _guard = self.write_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.persist(&path, &payload, &digest)?;
        Ok(digest)
    }

    /// Reads back a fragment by digest, verifying its content hash and the
    /// caller's declared `types`/`hints`, then applies `overrides`.
    pub fn fetch(&self, digest: &str, options: &FetchOptions) -> Result<Document> {
        let path = self.fragment_path(digest)?;
        if !path.is_file() {
            return Err(FragmentStoreError::NotFound(digest.to_string()));
        }

        let payload = fs::read(&path)?;
        verify_payload(&payload, digest)?;
        let mut document = mxrs_bson::parse(&payload)?;

        validate_types(&document, &options.types, digest)?;
        validate_hints(&document, &options.hints, digest)?;
        apply_overrides(&mut document, &options.overrides, digest)?;
        Ok(document)
    }

    fn fragment_path(&self, digest: &str) -> Result<PathBuf> {
        if !is_valid_digest(digest) {
            return Err(FragmentStoreError::InvalidDigest(digest.to_string()));
        }
        Ok(self.root.join(format!("{digest}.bson")))
    }

    /// Content-addressed write: if `path` already exists, verify it matches
    /// rather than overwriting (a digest collision with different content
    /// would mean SHA-256 broke, so this is a correctness fail-safe, not an
    /// optimization).
    fn persist(&self, path: &Path, payload: &[u8], digest: &str) -> Result<()> {
        if path.is_file() {
            let existing = fs::read(path)?;
            return verify_payload(&existing, digest);
        }

        fs::create_dir_all(&self.root)?;
        let mut temp_name = path.as_os_str().to_owned();
        temp_name.push(format!(".tmp-{}", std::process::id()));
        let temp_path = PathBuf::from(temp_name);
        fs::write(&temp_path, payload)?;
        fs::rename(&temp_path, path)?;
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn is_valid_digest(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

fn verify_payload(payload: &[u8], digest: &str) -> Result<()> {
    let actual = sha256_hex(payload);
    if actual == digest {
        Ok(())
    } else {
        Err(FragmentStoreError::DigestMismatch { expected: digest.to_string(), actual })
    }
}

fn validate_types(document: &Document, expected: &[String], digest: &str) -> Result<()> {
    if expected.is_empty() {
        return Ok(());
    }
    let found = collect_types(document);
    let missing: Vec<String> = expected.iter().filter(|t| !found.contains(*t)).cloned().collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(FragmentStoreError::MissingTypes { digest: digest.to_string(), missing })
    }
}

fn collect_types(document: &Document) -> std::collections::HashSet<String> {
    let mut found = std::collections::HashSet::new();
    collect_types_from_doc(document, &mut found);
    found
}

fn collect_types_from_doc(doc: &Document, found: &mut std::collections::HashSet<String>) {
    if let Ok(t) = doc.get_str("$Type") {
        if !t.is_empty() {
            found.insert(t.to_string());
        }
    }
    for (_, value) in doc {
        collect_types_from_value(value, found);
    }
}

fn collect_types_from_value(value: &Bson, found: &mut std::collections::HashSet<String>) {
    match value {
        Bson::Document(doc) => collect_types_from_doc(doc, found),
        Bson::Array(items) => items.iter().for_each(|item| collect_types_from_value(item, found)),
        _ => {}
    }
}

fn validate_hints(document: &Document, expected: &[String], digest: &str) -> Result<()> {
    if expected.is_empty() {
        return Ok(());
    }
    let found = collect_hints(document);
    let missing: Vec<String> = expected.iter().filter(|h| !found.contains(*h)).cloned().collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(FragmentStoreError::MissingHints { digest: digest.to_string(), missing })
    }
}

fn collect_hints(document: &Document) -> std::collections::HashSet<String> {
    let mut found = std::collections::HashSet::new();
    collect_hints_from_doc(document, &mut found);
    found
}

fn collect_hints_from_doc(doc: &Document, found: &mut std::collections::HashSet<String>) {
    for (key, child) in doc {
        if looks_like_hint_key(key) {
            if let Bson::String(s) = child {
                if !s.is_empty() && s.len() <= 512 {
                    found.insert(s.clone());
                }
            }
        }
        collect_hints_from_value(child, found);
    }
}

fn collect_hints_from_value(value: &Bson, found: &mut std::collections::HashSet<String>) {
    match value {
        Bson::Document(doc) => collect_hints_from_doc(doc, found),
        Bson::Array(items) => items.iter().for_each(|item| collect_hints_from_value(item, found)),
        _ => {}
    }
}

fn looks_like_hint_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    ["url", "uri", "endpoint", "host"].iter().any(|suffix| lower.ends_with(suffix))
}

fn apply_overrides(document: &mut Document, overrides: &BTreeMap<String, Bson>, digest: &str) -> Result<()> {
    for (key, value) in overrides {
        let existing_is_scalar = document.get(key).is_some_and(is_scalar);
        if !existing_is_scalar || !is_scalar(value) {
            return Err(FragmentStoreError::InvalidOverride { digest: digest.to_string(), key: key.clone() });
        }
        document.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn is_scalar(value: &Bson) -> bool {
    matches!(
        value,
        Bson::Null
            | Bson::String(_)
            | Bson::Int32(_)
            | Bson::Int64(_)
            | Bson::Double(_)
            | Bson::Decimal128(_)
            | Bson::Boolean(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    fn store() -> (tempfile::TempDir, NativeFragmentStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = NativeFragmentStore::new(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn put_then_fetch_round_trips() {
        let (_dir, store) = store();
        let document = doc! { "$Type": "Widgets$Custom", "Name": "A" };
        let digest = store.put(&document).unwrap();
        assert_eq!(digest.len(), 64);
        let fetched = store.fetch(&digest, &FetchOptions::default()).unwrap();
        assert_eq!(fetched, document);
    }

    #[test]
    fn put_is_idempotent_for_identical_content() {
        let (_dir, store) = store();
        let document = doc! { "Name": "A" };
        let first = store.put(&document).unwrap();
        let second = store.put(&document).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn fetch_rejects_malformed_digest() {
        let (_dir, store) = store();
        let result = store.fetch("not-a-digest", &FetchOptions::default());
        assert!(matches!(result, Err(FragmentStoreError::InvalidDigest(_))));
        // Right length but uppercase — DIGEST_PATTERN requires lowercase hex.
        let result = store.fetch(&"A".repeat(64), &FetchOptions::default());
        assert!(matches!(result, Err(FragmentStoreError::InvalidDigest(_))));
    }

    #[test]
    fn fetch_rejects_nonexistent_digest() {
        let (_dir, store) = store();
        let result = store.fetch(&"0".repeat(64), &FetchOptions::default());
        assert!(matches!(result, Err(FragmentStoreError::NotFound(_))));
    }

    #[test]
    fn fetch_fails_closed_on_tampered_content() {
        let (dir, store) = store();
        let digest = store.put(&doc! { "Name": "A" }).unwrap();
        let path = dir.path().join(format!("{digest}.bson"));
        std::fs::write(&path, b"tampered bytes").unwrap();

        let result = store.fetch(&digest, &FetchOptions::default());
        assert!(matches!(result, Err(FragmentStoreError::DigestMismatch { .. })));
    }

    #[test]
    fn fetch_validates_declared_types_including_nested() {
        let (_dir, store) = store();
        let document = doc! { "$Type": "Outer$Type", "Child": { "$Type": "Inner$Type" } };
        let digest = store.put(&document).unwrap();

        let ok = FetchOptions { types: vec!["Outer$Type".into(), "Inner$Type".into()], ..Default::default() };
        assert!(store.fetch(&digest, &ok).is_ok());

        let missing = FetchOptions { types: vec!["Nonexistent$Type".into()], ..Default::default() };
        let result = store.fetch(&digest, &missing);
        assert!(matches!(result, Err(FragmentStoreError::MissingTypes { .. })));
    }

    #[test]
    fn fetch_validates_declared_hints_by_key_suffix_including_nested() {
        let (_dir, store) = store();
        let document = doc! {
            "Name": "A",
            "Settings": { "ServiceEndpoint": "https://example.test/api" }
        };
        let digest = store.put(&document).unwrap();

        let ok = FetchOptions { hints: vec!["https://example.test/api".into()], ..Default::default() };
        assert!(store.fetch(&digest, &ok).is_ok());

        let missing = FetchOptions { hints: vec!["https://not-there.test".into()], ..Default::default() };
        assert!(matches!(store.fetch(&digest, &missing), Err(FragmentStoreError::MissingHints { .. })));
    }

    #[test]
    fn fetch_applies_scalar_overrides_to_existing_fields() {
        let (_dir, store) = store();
        let digest = store.put(&doc! { "Name": "A", "Total": 1_i32 }).unwrap();

        let mut overrides = BTreeMap::new();
        overrides.insert("Name".to_string(), Bson::String("B".to_string()));
        let options = FetchOptions { overrides, ..Default::default() };

        let result = store.fetch(&digest, &options).unwrap();
        assert_eq!(result.get_str("Name").unwrap(), "B");
        assert_eq!(result.get_i32("Total").unwrap(), 1);
    }

    #[test]
    fn fetch_rejects_override_of_missing_key() {
        let (_dir, store) = store();
        let digest = store.put(&doc! { "Name": "A" }).unwrap();

        let mut overrides = BTreeMap::new();
        overrides.insert("Nonexistent".to_string(), Bson::String("x".to_string()));
        let options = FetchOptions { overrides, ..Default::default() };

        assert!(matches!(store.fetch(&digest, &options), Err(FragmentStoreError::InvalidOverride { .. })));
    }

    #[test]
    fn fetch_rejects_override_of_non_scalar_field() {
        let (_dir, store) = store();
        let digest = store.put(&doc! { "Child": { "Name": "A" } }).unwrap();

        let mut overrides = BTreeMap::new();
        overrides.insert("Child".to_string(), Bson::String("x".to_string()));
        let options = FetchOptions { overrides, ..Default::default() };

        assert!(matches!(store.fetch(&digest, &options), Err(FragmentStoreError::InvalidOverride { .. })));
    }

    #[test]
    fn fetch_rejects_non_scalar_override_value() {
        let (_dir, store) = store();
        let digest = store.put(&doc! { "Name": "A" }).unwrap();

        let mut overrides = BTreeMap::new();
        overrides.insert("Name".to_string(), Bson::Array(vec![Bson::String("x".to_string())]));
        let options = FetchOptions { overrides, ..Default::default() };

        assert!(matches!(store.fetch(&digest, &options), Err(FragmentStoreError::InvalidOverride { .. })));
    }
}
