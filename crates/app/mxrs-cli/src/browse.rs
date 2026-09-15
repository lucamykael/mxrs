//! Raw project/unit browsing — ports `bin/mxrb`'s `inspect`/`dump-unit`/
//! `sql`/`modules` commands. `bin/mxrb`'s `inspect` is renamed [`units`]
//! here: `mxrs inspect` already means something more useful in this crate
//! (a structural entity/association/microflow snapshot via `compare`'s
//! machinery — see `inspect.rs`), while `bin/mxrb inspect` is a raw
//! unit/type listing. Same command, same name would collide with a
//! different, more valuable meaning already taken.

use std::path::Path;

use mxrs_model::Project;
use mxrs_mpr::{Result as MprResult, SqlResult};

pub struct UnitsReport {
    pub project_name: Option<String>,
    pub mendix_version: Option<String>,
    pub tables: Vec<String>,
    pub unit_types: Vec<String>,
    pub units: Vec<UnitSummary>,
}

pub struct UnitSummary {
    pub unit_id: String,
    pub type_name: String,
    pub container_id: String,
    pub containment_name: String,
}

pub fn units(path: impl AsRef<Path>) -> mxrs_model::Result<UnitsReport> {
    let path = path.as_ref();
    let project = Project::open(path, true)?;
    let all_units = project.all_units()?;
    let mut units = Vec::with_capacity(all_units.len());
    let mut unit_types = std::collections::BTreeSet::new();
    let mut project_name = None;
    for unit in all_units {
        let doc = project.mpr().parse_contents(&unit)?;
        if unit.unit_id == unit.container_id && project_name.is_none() {
            project_name = Some(
                doc.get_str("Name")
                    .or_else(|_| doc.get_str("name"))
                    .map(str::to_owned)
                    .unwrap_or_else(|_| {
                        path.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    }),
            );
        }
        let type_name = doc.get_str("$Type").ok();
        if let Some(name) = type_name {
            unit_types.insert(name.to_owned());
        }
        units.push(UnitSummary {
            unit_id: unit.unit_id,
            type_name: type_name.unwrap_or_default().to_owned(),
            container_id: unit.container_id,
            containment_name: unit.containment_name,
        });
    }
    Ok(UnitsReport {
        project_name,
        mendix_version: project.mendix_version()?,
        tables: project.mpr().tables()?,
        unit_types: unit_types.into_iter().collect(),
        units,
    })
}

pub struct UnitDump {
    pub unit_id: String,
    pub container_id: String,
    pub containment_name: String,
    pub type_name: String,
    pub contents_hash: Option<String>,
    pub bytes: Option<Vec<u8>>,
}

pub fn dump_unit(path: impl AsRef<Path>, unit_id: &str) -> mxrs_model::Result<Option<UnitDump>> {
    let project = Project::open(path, true)?;
    let Some(unit) = project.mpr().unit(unit_id)? else {
        return Ok(None);
    };
    let bytes = project.mpr().content_bytes(&unit)?;
    // Decode the same bytes we report. An unreadable/corrupt unit must not
    // appear to be a successful dump with an unknown type.
    let type_name = match bytes.as_deref() {
        Some(bytes) if !bytes.is_empty() => mxrs_bson::parse(bytes)?
            .get_str("$Type")
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    };
    Ok(Some(UnitDump {
        unit_id: unit.unit_id,
        container_id: unit.container_id,
        containment_name: unit.containment_name,
        type_name,
        contents_hash: unit.contents_hash,
        bytes,
    }))
}

/// A hex+ASCII dump in the classic `xxd`/`hexdump -C` layout — mirrors
/// `bin/mxrb`'s own hand-rolled 16-bytes-per-row format.
pub fn format_hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (i, row) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = row
            .iter()
            .map(|&b| {
                if (32..=126).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        out.push_str(&format!(
            "  {:04x}  {:<48}  {}\n",
            i * 16,
            hex.join(" "),
            ascii
        ));
    }
    out
}

pub fn sql(path: impl AsRef<Path>, query: &str) -> MprResult<SqlResult> {
    let mpr = mxrs_mpr::MprFile::open(path, true)?;
    let result = mpr.raw_query(query)?;
    if result.columns.is_empty() {
        return Err(mxrs_mpr::MprError::EmptyQuery);
    }
    Ok(result)
}

/// Unambiguous, lossless SQL cells: JSON-quoted UTF-8 text and hexadecimal
/// SQLite blobs. Invalid UTF-8 TEXT retains its type and every original byte.
pub fn format_sql_cell(cell: &mxrs_mpr::SqlCell) -> String {
    use mxrs_mpr::SqlCell;
    match cell {
        SqlCell::Null => "NULL".into(),
        SqlCell::Integer(value) => value.to_string(),
        SqlCell::Real(value) => format!("{value:?}"),
        SqlCell::Text(value) => serde_json::to_string(value).expect("text is serializable"),
        SqlCell::Blob(bytes) | SqlCell::InvalidText(bytes) => {
            let prefix = if matches!(cell, SqlCell::InvalidText(_)) {
                "TEXT "
            } else {
                ""
            };
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("{prefix}X'{hex}'")
        }
    }
}

pub fn list_modules(path: impl AsRef<Path>) -> mxrs_model::Result<Vec<String>> {
    let project = Project::open(path, true)?;
    let mut names: Vec<String> = project
        .modules()?
        .into_iter()
        .filter_map(|m| m.name)
        .collect();
    names.sort();
    Ok(names)
}

/// The complete public `mxrb modules` record. Nanoflows and rules are not
/// microflows; pages and microflows include documents in nested folders.
#[derive(Debug, serde::Serialize)]
pub struct ModuleSummary {
    pub name: Option<String>,
    pub entities: usize,
    pub pages: usize,
    pub microflows: usize,
}

pub fn module_summaries(path: impl AsRef<Path>) -> mxrs_model::Result<Vec<ModuleSummary>> {
    let project = Project::open(path, true)?;
    project.modules().map(|modules| {
        modules
            .into_iter()
            .map(|module| ModuleSummary {
                entities: module.entities().len(),
                pages: module.pages.len(),
                microflows: module.microflows.len(),
                name: module.name,
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(path: &std::path::Path) {
        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |m| {
            m.entity("Order", |e| {
                e.string("Number");
            });
        });
        project.module("CRM", |_m| {});
        mxrs_writer::write_project(path, &project.build()).unwrap();
    }

    #[test]
    fn units_reports_project_metadata_and_type_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        write_fixture(&path);

        let report = units(&path).unwrap();
        assert_eq!(report.mendix_version.as_deref(), Some("11.12.1"));
        assert!(!report.tables.is_empty());
        assert!(report.unit_types.contains(&"Projects$Module".to_string()));
        assert!(!report.units.is_empty());
    }

    #[test]
    fn dump_unit_returns_none_for_an_unknown_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        write_fixture(&path);
        let dump = dump_unit(&path, "00000000-0000-0000-0000-000000000000").unwrap();
        assert!(dump.is_none());
    }

    #[test]
    fn dump_unit_returns_the_matching_unit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        write_fixture(&path);
        let report = units(&path).unwrap();
        let first = &report.units[0];
        let dump = dump_unit(&path, &first.unit_id).unwrap().unwrap();
        assert_eq!(dump.unit_id, first.unit_id);
        assert_eq!(dump.type_name, first.type_name);
    }

    #[test]
    fn sql_runs_a_read_query_against_the_raw_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        write_fixture(&path);
        let result = sql(&path, "SELECT COUNT(*) FROM Unit").unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn list_modules_returns_sorted_module_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        write_fixture(&path);
        assert_eq!(
            list_modules(&path).unwrap(),
            vec!["CRM".to_string(), "Sales".to_string()]
        );
    }

    #[test]
    fn format_hex_dump_lays_out_16_bytes_per_row() {
        let dump = format_hex_dump(b"Hello, mxrs!");
        assert!(dump.contains("0000"));
        assert!(dump.contains("Hello, mxrs!"));
    }
}
