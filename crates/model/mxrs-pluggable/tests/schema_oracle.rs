//! Structural oracle: decodes every real `CustomWidgets$CustomWidgetType`
//! schema found inside real production `.mpr` files. Committed as an
//! `#[ignore]`d test parameterized by `MXRS_ACCEPTANCE_DIR`, same
//! convention as `mxrs-compiler-flow`'s `acceptance_qrqc_spc.rs` — the real
//! customer `.mpr` files themselves are never committed.
//!
//! Run manually with:
//! ```text
//! MXRS_ACCEPTANCE_DIR=/path/to/dir cargo test -p mxrs-pluggable \
//!     --test schema_oracle -- --ignored --nocapture
//! ```
//! Expected directory layout under `MXRS_ACCEPTANCE_DIR`:
//! ```text
//! qrqc-ruby/build/eQRQC.mpr
//! spc-ruby/build/JEMScc-SPC.mpr
//! ```
//! Explicit `--ignored` execution requires the corpus and fails if it is
//! missing; default test runs remain ignored. A
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

fn run_against(path: &str, label: &str) {
    let mpr = MprFile::open(path, true)
        .unwrap_or_else(|error| panic!("[{label}] cannot open private corpus {path}: {error}"));
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

#[test]
#[ignore = "requires MXRS_ACCEPTANCE_DIR with QRQC/SPC .mpr and mprcontents"]
fn decodes_every_real_pluggable_widget_schema() {
    let base = std::env::var("MXRS_ACCEPTANCE_DIR")
        .expect("set MXRS_ACCEPTANCE_DIR to the private QRQC/SPC corpus root before explicitly running this ignored test; see this file's directory layout");
    run_against(&format!("{base}/qrqc-ruby/build/eQRQC.mpr"), "QRQC");
    run_against(&format!("{base}/spc-ruby/build/JEMScc-SPC.mpr"), "SPC");
}
