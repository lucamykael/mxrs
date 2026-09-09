//! Structural integrity checks over a `.mpr` file's SQLite/BSON contents —
//! ports the storage-format-invariant checks from mxrb's
//! `Integrity::Validator` (root uniqueness, dangling containers, duplicate
//! `UnitID`s, `ContentsHash`/`$ID` integrity, nested-`$ID` well-formedness,
//! orphan/missing v2 `.mxunit` content files).
//!
//! Deliberately narrower than mxrb's validator:
//! - No SQLite table/column-shape checks (`Unit`/`_MetaData` tables,
//!   `ContentsHash` column, ...) — mxrs always creates its own known schema
//!   via `mxrs-schema`, so that check only matters for arbitrary
//!   externally-authored files, which mxrb (but not yet mxrs) has to
//!   tolerate.
//! - No v1 legacy content-`$ID`-mismatch allowance — mxrs's locked MVP
//!   scope targets v2/`.mxunit` storage only (see
//!   `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory), so
//!   every identity mismatch is an error, never a downgraded warning.
//! - No `DomainModels$Attribute` `AutoNumber`-default-value semantic check
//!   yet (mxrb's `validate_mendix_semantics`) — narrow enough to add later
//!   without restructuring this pass.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use mxrs_bson::Bson;
use mxrs_mpr::{MprFile, RawUnit, Result, StorageFormat};

#[derive(Debug, Default, Clone)]
pub struct ValidateReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidateReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    fn warning(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

pub fn validate(path: impl AsRef<Path>) -> Result<ValidateReport> {
    let mpr = MprFile::open(path, true)?;
    let mut report = ValidateReport::default();
    let units = mpr.all_units()?;

    validate_root(&units, &mut report);
    validate_unit_ids(&units, &mut report);
    validate_contents(&mpr, &units, &mut report);
    validate_v2_files(&mpr, &units, &mut report)?;

    Ok(report)
}

fn validate_root(units: &[RawUnit], report: &mut ValidateReport) {
    let roots = units.iter().filter(|u| u.unit_id == u.container_id).count();
    if roots != 1 {
        report.error(format!("expected exactly one root unit, found {roots}"));
    }
}

fn validate_unit_ids(units: &[RawUnit], report: &mut ValidateReport) {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for u in units {
        if u.unit_id.is_empty() {
            report.error("blank UnitID");
        }
        *counts.entry(u.unit_id.as_str()).or_insert(0) += 1;
    }
    for (id, count) in &counts {
        if *count > 1 {
            report.error(format!("duplicate UnitID {id}"));
        }
    }

    let id_set: HashSet<&str> = units.iter().map(|u| u.unit_id.as_str()).collect();
    for u in units {
        if u.container_id == u.unit_id {
            continue;
        }
        if !id_set.contains(u.container_id.as_str()) {
            report.error(format!(
                "unit {} references missing container {}",
                u.unit_id, u.container_id
            ));
        }
    }
}

fn validate_contents(mpr: &MprFile, units: &[RawUnit], report: &mut ValidateReport) {
    for u in units {
        let bytes = match mpr.content_bytes(u) {
            Ok(Some(b)) if !b.is_empty() => b,
            Ok(_) => {
                report.error(format!("unit {} has no content bytes", u.unit_id));
                continue;
            }
            Err(e) => {
                report.error(format!("unit {} content read failed: {e}", u.unit_id));
                continue;
            }
        };

        let expected = u.contents_hash.as_deref().unwrap_or_default();
        let actual = mxrs_bson::contents_hash(&bytes);
        if expected != actual {
            report.error(format!("unit {} ContentsHash mismatch", u.unit_id));
        }

        let doc = match mpr.parse_contents(u) {
            Ok(d) => d,
            Err(e) => {
                report.error(format!("unit {} content parse failed: {e}", u.unit_id));
                continue;
            }
        };

        match doc.get_str("$Type") {
            Ok(t) if !t.is_empty() => {}
            _ => report.error(format!("unit {} missing $Type", u.unit_id)),
        }

        let doc_id = doc.get("$ID").and_then(mxrs_bson::extract_id);
        match doc_id {
            None => report.error(format!("unit {} missing $ID", u.unit_id)),
            Some(id) if id != u.unit_id => {
                report.error(format!("unit {} content $ID mismatch {id}", u.unit_id));
            }
            _ => {}
        }

        let mut nested_counts: HashMap<String, u32> = HashMap::new();
        let mut misordered = Vec::new();
        collect_nested_ids(&Bson::Document(doc), &mut nested_counts, &mut misordered);
        for (id, count) in &nested_counts {
            if *count > 1 {
                report.error(format!(
                    "unit {} contains duplicate nested $ID {id}",
                    u.unit_id
                ));
            }
        }
        for id in misordered {
            report.error(format!(
                "unit {} storage object {id} does not begin with $ID",
                u.unit_id
            ));
        }
    }
}

/// Mirrors `Integrity::Validator#collect_nested_ids`: every nested document
/// carrying a `$ID` is counted (duplicates are a corruption signal — ids
/// must be unique within a unit's content tree), and flagged if `$ID` isn't
/// serialized as the document's first key (Mendix's storage convention).
fn collect_nested_ids(
    value: &Bson,
    counts: &mut HashMap<String, u32>,
    misordered: &mut Vec<String>,
) {
    match value {
        Bson::Document(d) => {
            if let Some(id) = d.get("$ID").and_then(mxrs_bson::extract_id) {
                *counts.entry(id.clone()).or_insert(0) += 1;
                if d.keys().next().map(String::as_str) != Some("$ID") {
                    misordered.push(id);
                }
            }
            for v in d.values() {
                collect_nested_ids(v, counts, misordered);
            }
        }
        Bson::Array(items) => {
            for v in items {
                collect_nested_ids(v, counts, misordered);
            }
        }
        _ => {}
    }
}

fn validate_v2_files(mpr: &MprFile, units: &[RawUnit], report: &mut ValidateReport) -> Result<()> {
    if mpr.format() != StorageFormat::V2 {
        return Ok(());
    }

    let expected: HashSet<_> = units.iter().filter_map(|u| mpr.content_path(u)).collect();
    let actual: HashSet<_> = mpr.content_files()?.into_iter().collect();
    for path in expected.difference(&actual) {
        report.error(format!("missing mxunit file {}", path.display()));
    }
    for path in actual.difference(&expected) {
        report.warning(format!("orphan mxunit file {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freshly_written_project_validates_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Clean.mpr");

        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |m| {
            m.entity("Order", |e| {
                e.string("Number");
            });
        });
        mxrs_writer::write_project(&path, &project.build()).unwrap();

        let report = validate(&path).unwrap();
        assert!(report.is_valid(), "unexpected errors: {:?}", report.errors);
    }

    #[test]
    fn a_dangling_container_reference_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Dangling.mpr");

        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |m| {
            m.entity("Order", |_e| {});
        });
        mxrs_writer::write_project(&path, &project.build()).unwrap();

        {
            let mut mpr = MprFile::open(&path, false).unwrap();
            let module_unit = mpr
                .units_by_containment("Modules")
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            let ghost_container = uuid::Uuid::new_v4().to_string();
            mpr.relocate_unit(&module_unit.unit_id, &ghost_container, "Modules")
                .unwrap();
        }

        let report = validate(&path).unwrap();
        assert!(!report.is_valid());
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("missing container")),
            "{:?}",
            report.errors
        );
    }

    #[test]
    fn a_tampered_content_file_is_a_hash_mismatch_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Tampered.mpr");

        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |m| {
            m.entity("Order", |_e| {});
        });
        mxrs_writer::write_project(&path, &project.build()).unwrap();

        // v2 storage keeps content in a separate `.mxunit` file named after
        // its hash; overwriting that file directly (bypassing `update_unit`,
        // which would recompute a matching hash) reproduces a corrupted
        // on-disk file whose bytes no longer match the `Unit` row's
        // recorded `ContentsHash`.
        let mpr = MprFile::open(&path, true).unwrap();
        let unit = mpr
            .units_by_containment("Modules")
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let content_path = mpr
            .content_path(&unit)
            .expect("v2 storage has a content path");
        drop(mpr);
        std::fs::write(&content_path, b"not valid bson content at all").unwrap();

        let report = validate(&path).unwrap();
        assert!(!report.is_valid());
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("ContentsHash mismatch")),
            "{:?}",
            report.errors
        );
    }
}
