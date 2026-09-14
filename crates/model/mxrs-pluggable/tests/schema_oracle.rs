//! Structural oracle: decodes every `CustomWidgets$CustomWidgetType` schema
//! found in an explicitly supplied, unversioned `.mpr` corpus.
//!
//! Run manually with:
//! ```text
//! MXRS_ACCEPTANCE_PROJECTS=/path/a.mpr:/path/b.mpr cargo test -p mxrs-pluggable \
//!     --test schema_oracle -- --ignored --nocapture
//! ```
//! The variable is a platform path list. Explicit `--ignored` execution
//! requires every input and fails if any is missing. Default runs remain
//! ignored. A
//! `CustomWidgets$CustomWidgetType` document is fully self-describing
//! (see `mpr_codec`'s module doc — no external catalog/manifest needed to
//! decode one), so any decode failure here is a real schema-decode bug,
//! not a "no real example seen yet" gap.

use mxrs_bson::{Bson, Document};
use mxrs_mpr::MprFile;
use mxrs_pluggable::decode_widget_type;

fn collect_widget_types(mpr: &MprFile) -> Vec<Document> {
    fn visit(value: &Bson, found: &mut Vec<Document>) {
        match value {
            Bson::Document(document) => {
                if document.get_str("$Type").ok() == Some("CustomWidgets$CustomWidgetType") {
                    found.push(document.clone());
                }
                for (_, child) in document {
                    visit(child, found);
                }
            }
            Bson::Array(items) => {
                for item in items {
                    visit(item, found);
                }
            }
            _ => {}
        }
    }

    let mut found = Vec::new();
    for unit in mpr.all_units().expect("read every corpus unit") {
        let document = mpr
            .parse_contents(&unit)
            .unwrap_or_else(|error| panic!("cannot parse corpus unit {}: {error}", unit.unit_id));
        visit(&Bson::Document(document), &mut found);
    }
    found
}

fn run_against(path: &std::path::Path, label: &str) {
    let mpr = MprFile::open(path, true)
        .unwrap_or_else(|error| panic!("[{label}] cannot open corpus {}: {error}", path.display()));
    let widget_types = collect_widget_types(&mpr);
    assert!(
        !widget_types.is_empty(),
        "[{label}] found zero CustomWidgets$CustomWidgetType documents — expected at least one \
         (Mendix 11's own Data Grid/Gallery/ComboBox are pluggable-under-the-hood on any real page)"
    );

    let mut errors = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for (index, document) in widget_types.iter().enumerate() {
        match decode_widget_type(document, &format!("$[{index}]")) {
            Ok((widget_type, _context)) => {
                ids.insert(widget_type.id);
            }
            Err(error) => errors.push(format!("[{index}]: {error}")),
        }
    }

    println!(
        "[{label}] {} widget-type document(s), {} distinct widget id(s), {} decode error(s)",
        widget_types.len(),
        ids.len(),
        errors.len()
    );
    for id in &ids {
        println!("[{label}]   {id}");
    }
    assert!(
        errors.is_empty(),
        "[{label}] {} schema decode error(s):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

fn corpus_projects() -> Vec<std::path::PathBuf> {
    let value = std::env::var_os("MXRS_ACCEPTANCE_PROJECTS")
        .expect("set MXRS_ACCEPTANCE_PROJECTS before explicitly running this ignored test");
    let paths = std::env::split_paths(&value).collect::<Vec<_>>();
    assert!(
        !paths.is_empty(),
        "MXRS_ACCEPTANCE_PROJECTS contains no paths"
    );
    for path in &paths {
        assert!(
            path.is_file(),
            "acceptance input is not a file: {}",
            path.display()
        );
    }
    paths
}

#[test]
#[ignore = "requires MXRS_ACCEPTANCE_PROJECTS with an authorized local corpus"]
fn decodes_every_configured_pluggable_widget_schema() {
    for (index, project) in corpus_projects().iter().enumerate() {
        run_against(project, &format!("case-{}", index + 1));
    }
}
