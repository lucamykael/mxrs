//! Single-file structural summary — exposes `compare::snapshot` (already
//! built to give `compare` both sides of a diff) as a standalone `mxrs
//! inspect` command: a birds-eye view of one project without needing a
//! second file to diff against. No new model-reading logic here, only a
//! new front end onto machinery `compare` already has — same "don't
//! duplicate what already exists" rule the rest of this crate follows.

use std::path::Path;

use serde_json::Value;

pub fn inspect(path: impl AsRef<Path>) -> mxrs_model::Result<Value> {
    crate::compare::snapshot(path)
}

/// A short human-readable rollup of an [`inspect`] snapshot — per-module
/// entity/association/microflow/page/menu counts, not the full nested
/// structure (`--json` is for that).
pub fn summarize(snapshot: &Value) -> String {
    let mendix_version = snapshot["project"]["mendix_version"]
        .as_str()
        .unwrap_or("?");
    let units = snapshot["units"].as_array().map(Vec::len).unwrap_or(0);
    let modules = snapshot["modules"].as_array().cloned().unwrap_or_default();

    let mut out = format!(
        "[mxrs] Mendix {mendix_version} — {units} unit(s), {} module(s)\n",
        modules.len()
    );
    for module in &modules {
        let name = module["name"].as_str().unwrap_or("?");
        let count = |key: &str| module[key].as_array().map(Vec::len).unwrap_or(0);
        out.push_str(&format!(
            "  {name}: {} entity(ies), {} association(s), {} microflow(s), {} page(s), {} menu(s)\n",
            count("entities"),
            count("associations"),
            count("microflows"),
            count("pages"),
            count("menus"),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_module_and_unit_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |m| {
            m.entity("Order", |e| {
                e.string("Number");
            });
            m.entity("Customer", |_e| {});
            m.microflow("ACT_Do", |f| {
                f.return_value("1");
            });
        });
        mxrs_writer::write_project(&path, &project.build()).unwrap();

        let snapshot = inspect(&path).unwrap();
        let text = summarize(&snapshot);
        assert!(text.contains("Sales: 2 entity(ies), 0 association(s), 1 microflow(s)"));
    }

    #[test]
    fn matches_the_project_side_of_a_compare_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |m| {
            m.entity("Order", |_e| {});
        });
        mxrs_writer::write_project(&path, &project.build()).unwrap();

        let inspected = inspect(&path).unwrap();
        let compared = crate::compare::snapshot(&path).unwrap();
        assert_eq!(inspected, compared);
    }
}
