//! Structural integrity checks over a `.mpr` file's SQLite/BSON contents —
//! ports the storage-format-invariant checks from mxrb's
//! `Integrity::Validator` (root uniqueness, dangling containers, duplicate
//! `UnitID`s, `ContentsHash`/`$ID` integrity, nested-`$ID` well-formedness,
//! orphan/missing v2 `.mxunit` content files).
//!
//! Also rejects containment cycles that otherwise satisfy root uniqueness and
//! parent existence. This remains storage integrity, not Studio Pro's complete
//! model consistency checker. Deliberately narrower than mxrb's validator:
//! - No v1 legacy content-`$ID`-mismatch allowance — mxrs's locked MVP
//!   scope targets v2/`.mxunit` storage only (see
//!   `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory), so
//!   every identity mismatch is an error, never a downgraded warning.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use mxrs_bson::{Bson, Document};
use mxrs_mpr::{MprFile, RawUnit, Result, SqlCell, StorageFormat};

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
    validate_tables(&mpr, &mut report)?;
    if !report.is_valid() {
        report.errors.sort();
        return Ok(report);
    }
    let units = mpr.all_units()?;

    validate_root(&units, &mut report);
    validate_unit_ids(&units, &mut report);
    validate_containment_cycles(&units, &mut report);
    validate_contents(&mpr, &units, &mut report);
    validate_v2_files(&mpr, &units, &mut report)?;

    report.errors.sort();
    report.warnings.sort();
    Ok(report)
}

fn validate_tables(mpr: &MprFile, report: &mut ValidateReport) -> Result<()> {
    if !mpr.tables()?.iter().any(|name| name == "_MetaData") {
        report.error("missing _MetaData table");
    }
    let columns = mpr.raw_query("PRAGMA table_info(Unit)")?;
    for required in ["UnitID", "ContainerID", "ContainmentName", "ContentsHash"] {
        if !columns
            .rows
            .iter()
            .any(|row| matches!(row.get(1), Some(SqlCell::Text(name)) if name == required))
        {
            report.error(format!("missing Unit.{required} column"));
        }
    }
    Ok(())
}

fn validate_containment_cycles(units: &[RawUnit], report: &mut ValidateReport) {
    let parents: HashMap<_, _> = units
        .iter()
        .map(|unit| (unit.unit_id.as_str(), unit.container_id.as_str()))
        .collect();
    let mut complete = HashSet::new();
    for unit in units {
        let mut path = Vec::new();
        let mut positions = HashMap::new();
        let mut current = unit.unit_id.as_str();
        while !complete.contains(current) {
            if let Some(&index) = positions.get(current) {
                let mut cycle = path[index..].to_vec();
                cycle.sort_unstable();
                report.error(format!(
                    "containment cycle among units {}",
                    cycle.join(", ")
                ));
                break;
            }
            let Some(&parent) = parents.get(current) else {
                break;
            };
            if parent == current {
                complete.insert(current);
                break;
            }
            positions.insert(current, path.len());
            path.push(current);
            current = parent;
        }
        complete.extend(path);
    }
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
        collect_nested_ids(
            &Bson::Document(doc),
            &mut nested_counts,
            &mut misordered,
            &u.unit_id,
            report,
        );
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
    unit_id: &str,
    report: &mut ValidateReport,
) {
    match value {
        Bson::Document(d) => {
            if d.get_str("$Type").ok() == Some("DomainModels$Attribute") {
                validate_attribute_default(unit_id, d, report);
            }
            if let Some(id) = d.get("$ID").and_then(mxrs_bson::extract_id) {
                *counts.entry(id.clone()).or_insert(0) += 1;
                if d.keys().next().map(String::as_str) != Some("$ID") {
                    misordered.push(id);
                }
            }
            for v in d.values() {
                collect_nested_ids(v, counts, misordered, unit_id, report);
            }
        }
        Bson::Array(items) => {
            for v in items {
                collect_nested_ids(v, counts, misordered, unit_id, report);
            }
        }
        _ => {}
    }
}

/// Mirrors `Integrity::Validator#validate_attribute_default`, including native
/// field casing and arbitrarily long positive decimal strings (not a bounded
/// integer parse). Checking raw BSON avoids model defaults hiding corruption.
fn validate_attribute_default(unit_id: &str, attribute: &Document, report: &mut ValidateReport) {
    let Some(Bson::Document(kind)) =
        get_truthy_any(attribute, &["NewType", "Type", "newType", "type"])
    else {
        return;
    };
    if kind.get_str("$Type").ok() != Some("DomainModels$AutoNumberAttributeType") {
        return;
    }
    let default = match get_truthy_any(attribute, &["Value", "value"]) {
        Some(Bson::Document(value)) => get_truthy_any(value, &["DefaultValue", "defaultValue"]),
        _ => None,
    };
    let valid = match default {
        Some(Bson::String(value)) => {
            value
                .as_bytes()
                .first()
                .is_some_and(|first| matches!(first, b'1'..=b'9'))
                && value.bytes().all(|byte| byte.is_ascii_digit())
        }
        Some(Bson::Int32(value)) => *value > 0,
        Some(Bson::Int64(value)) => *value > 0,
        _ => false,
    };
    if !valid {
        let name = get_truthy_any(attribute, &["Name", "name"])
            .and_then(Bson::as_str)
            .unwrap_or("<unnamed>");
        report.error(format!(
            "unit {unit_id} AutoNumber attribute {name} must have a default value of 1 or higher"
        ));
    }
}

/// Native legacy aliases use Ruby's `a || b` precedence, so null and false
/// cannot hide a later spelling. Other malformed values remain visible.
fn get_truthy_any<'a>(document: &'a Document, keys: &[&str]) -> Option<&'a Bson> {
    keys.iter()
        .filter_map(|key| document.get(*key))
        .find(|value| !matches!(value, Bson::Null | Bson::Boolean(false)))
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
    fn null_and_false_legacy_aliases_cannot_hide_native_autonumber_validation() {
        for absent in [Bson::Null, Bson::Boolean(false)] {
            for (default, valid) in [("0", false), ("1", true)] {
                let attribute = mxrs_bson::doc! {
                    "NewType": absent.clone(), "Type": { "$Type": "DomainModels$AutoNumberAttributeType" },
                    "Value": absent.clone(), "value": { "DefaultValue": absent.clone(), "defaultValue": default },
                    "Name": absent.clone(), "name": "Sequence"
                };
                let mut report = ValidateReport::default();
                validate_attribute_default("unit", &attribute, &mut report);
                assert_eq!(report.is_valid(), valid, "{attribute:?}");
                if !valid {
                    assert!(report.errors[0].contains("attribute Sequence"));
                }
            }
        }
    }

    fn raw_unit(id: &str, parent: &str) -> RawUnit {
        RawUnit {
            unit_id: id.into(),
            container_id: parent.into(),
            containment_name: String::new(),
            contents_hash: None,
            contents: None,
        }
    }

    #[test]
    fn containment_cycles_are_reported_once_without_recursion_or_quadratic_walks() {
        let mut units = vec![
            raw_unit("root", "root"),
            raw_unit("a", "b"),
            raw_unit("b", "a"),
            raw_unit("child", "a"),
            raw_unit("orphan", "missing"),
        ];
        let mut parent = "root".to_string();
        for number in 0..20_000 {
            let id = format!("chain-{number}");
            units.push(raw_unit(&id, &parent));
            parent = id;
        }
        units.reverse();
        let mut report = ValidateReport::default();
        validate_containment_cycles(&units, &mut report);
        assert_eq!(report.errors, ["containment cycle among units a, b"]);
    }

    #[test]
    fn blank_duplicate_dangling_and_multiple_root_ids_remain_distinct_errors() {
        let units = [
            raw_unit("", ""),
            raw_unit("root", "root"),
            raw_unit("root", "missing"),
        ];
        let mut report = ValidateReport::default();
        validate_root(&units, &mut report);
        validate_unit_ids(&units, &mut report);
        report.errors.sort();
        assert_eq!(
            report.errors,
            [
                "blank UnitID",
                "duplicate UnitID root",
                "expected exactly one root unit, found 2",
                "unit root references missing container missing"
            ]
        );
    }

    #[test]
    fn external_sqlite_schema_gaps_are_reported_before_querying_model_rows() {
        for sql in [
            "DROP TABLE _MetaData",
            "ALTER TABLE Unit DROP COLUMN ContentsHash",
            "ALTER TABLE Unit DROP COLUMN ContainmentName",
            "ALTER TABLE Unit DROP COLUMN ContainerID",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("Broken.mpr");
            let mpr = MprFile::create(&path, "11.12.1", "schema").unwrap();
            mpr.raw_query(sql).unwrap();
            drop(mpr);
            let report = validate(&path).unwrap();
            assert_eq!(report.errors.len(), 1, "{sql}: {:?}", report.errors);
            assert!(report.errors[0].starts_with("missing "));
        }
    }

    #[test]
    fn autonumber_defaults_follow_native_casing_and_positive_decimal_rules() {
        for type_key in ["NewType", "Type", "newType", "type"] {
            for (value_key, default_key, name_key) in [
                ("Value", "DefaultValue", "Name"),
                ("value", "defaultValue", "name"),
            ] {
                for default in [
                    Bson::String("1".into()),
                    Bson::String("999999999999999999999999999999".into()),
                    Bson::Int32(1),
                    Bson::Int64(i64::MAX),
                    Bson::String("".into()),
                    Bson::String("0".into()),
                    Bson::String("01".into()),
                    Bson::String("-1".into()),
                    Bson::String("1\n".into()),
                    Bson::String("١".into()),
                    Bson::Int32(0),
                    Bson::Int64(-1),
                    Bson::Double(1.0),
                    Bson::Null,
                ] {
                    let valid = matches!(&default, Bson::String(value) if value == "1" || value.starts_with("999"))
                        || matches!(&default, Bson::Int32(1) | Bson::Int64(i64::MAX));
                    let mut attribute = mxrs_bson::doc! { "$Type": "DomainModels$Attribute" };
                    attribute.insert(
                        type_key,
                        mxrs_bson::doc! { "$Type": "DomainModels$AutoNumberAttributeType" },
                    );
                    attribute.insert(name_key, "Sequence");
                    let mut value = Document::new();
                    value.insert(default_key, default);
                    attribute.insert(value_key, value);
                    let mut report = ValidateReport::default();
                    validate_attribute_default("unit", &attribute, &mut report);
                    assert_eq!(
                        report.is_valid(),
                        valid,
                        "{attribute:?}: {:?}",
                        report.errors
                    );
                }
            }
        }
        for attribute in [
            mxrs_bson::doc! {},
            mxrs_bson::doc! { "NewType": { "$Type": "DomainModels$StringAttributeType" } },
        ] {
            let mut report = ValidateReport::default();
            validate_attribute_default("unit", &attribute, &mut report);
            assert!(report.is_valid());
        }
        for value in [Bson::Null, Bson::Document(Document::new())] {
            let attribute = mxrs_bson::doc! { "Type": { "$Type": "DomainModels$AutoNumberAttributeType" }, "Value": value };
            let mut report = ValidateReport::default();
            validate_attribute_default("unit", &attribute, &mut report);
            assert!(report.errors[0].contains("<unnamed>"));
        }
    }

    #[test]
    fn nested_native_autonumber_and_identity_errors_are_not_hidden_by_model_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Broken.mpr");
        let mut mpr = MprFile::create(&path, "11.12.1", "schema").unwrap();
        let root = mpr.root_unit().unwrap().unwrap();
        let mut document = mpr.parse_contents(&root).unwrap();
        let nested_id = uuid::Uuid::new_v4().to_string();
        let attribute = mxrs_bson::doc! { "$ID": &nested_id, "$Type": "DomainModels$Attribute", "Name": "Sequence", "NewType": { "$Type": "DomainModels$AutoNumberAttributeType" }, "Value": { "DefaultValue": "0" } };
        document.insert(
            "Items",
            vec![Bson::Document(attribute.clone()), Bson::Document(attribute)],
        );
        mpr.update_unit(&root.unit_id, document).unwrap();
        drop(mpr);
        let report = validate(&path).unwrap();
        assert_eq!(
            report
                .errors
                .iter()
                .filter(|error| error.contains("AutoNumber attribute Sequence"))
                .count(),
            2
        );
        assert_eq!(
            report
                .errors
                .iter()
                .filter(|error| error.contains("duplicate nested $ID"))
                .count(),
            1
        );
        let mut counts = HashMap::new();
        let mut misordered = Vec::new();
        let mut report = ValidateReport::default();
        collect_nested_ids(
            &Bson::Document(mxrs_bson::doc! { "Name": "BadOrder", "$ID": &nested_id }),
            &mut counts,
            &mut misordered,
            "unit",
            &mut report,
        );
        assert_eq!(misordered, [nested_id]);
    }

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
