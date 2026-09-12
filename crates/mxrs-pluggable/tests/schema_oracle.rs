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
//! Unlike the flow compiler's acceptance test, this one *does* assert: a
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
    for unit in mpr.all_units().unwrap_or_default() {
        if let Ok(document) = mpr.parse_contents(&unit) {
            visit(&Bson::Document(document), &mut found);
        }
    }
    found
}

fn run_against(path: &str, label: &str) {
    let Ok(mpr) = MprFile::open(path, true) else {
        eprintln!("[{label}] could not open {path}");
        return;
    };
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
#[ignore]
fn decodes_every_real_pluggable_widget_schema() {
    let Ok(base) = std::env::var("MXRS_ACCEPTANCE_DIR") else {
        eprintln!(
            "[schema_oracle] skipped: MXRS_ACCEPTANCE_DIR is not set (see this file's module doc \
             for the expected layout)"
        );
        return;
    };
    run_against(&format!("{base}/qrqc-ruby/build/eQRQC.mpr"), "QRQC");
    run_against(&format!("{base}/spc-ruby/build/JEMScc-SPC.mpr"), "SPC");
}
