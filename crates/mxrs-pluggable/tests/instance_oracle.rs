//! Structural oracle for pluggable-widget *instances* (not just their
//! schema — see `schema_oracle.rs` for that) in real production `.mpr`
//! files. Same `#[ignore]`d/`MXRS_ACCEPTANCE_DIR` convention as
//! `schema_oracle.rs` and `mxrs-compiler-flow`'s `acceptance_qrqc_spc.rs`.
//!
//! Run manually with:
//! ```text
//! MXRS_ACCEPTANCE_DIR=/path/to/dir cargo test -p mxrs-pluggable \
//!     --test instance_oracle -- --ignored --nocapture
//! ```
//!
//! Unlike `schema_oracle.rs`, this one does **not** assert zero errors —
//! `mpr_codec`'s module doc already documents, with real numbers, that a
//! whole-instance decode of a real Data Grid 2/Gallery/ComboBox still
//! usually needs kinds `mxrs-forms` doesn't expose yet
//! (`PluggableError::NeedsFormsIntegration`). What this test asserts
//! instead: every failure is *that specific, named* error — never a
//! panic, never a silently-wrong value — and it reports the exact
//! kind/success breakdown so the numbers in that doc comment stay honest
//! as the codebase changes.

use std::collections::BTreeMap;

use mxrs_bson::{Bson, Document};
use mxrs_mpr::MprFile;
use mxrs_pluggable::{PluggableError, decode_object, decode_widget_type};

fn collect_widget_instances(mpr: &MprFile) -> Vec<Document> {
    fn visit(value: &Bson, found: &mut Vec<Document>) {
        match value {
            Bson::Document(document) => {
                if document.get_str("$Type").ok() == Some("CustomWidgets$CustomWidget") {
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
    let instances = collect_widget_instances(&mpr);
    assert!(
        !instances.is_empty(),
        "[{label}] found zero CustomWidgets$CustomWidget instances — expected at least one"
    );

    let mut full_success = 0usize;
    let mut blocked_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut unexpected_errors = Vec::new();

    for (index, instance) in instances.iter().enumerate() {
        let item_path = format!("$[{index}]");
        let Ok(type_document) = instance.get_document("Type") else {
            unexpected_errors.push(format!("{item_path}: missing Type document"));
            continue;
        };
        let Ok((widget_type, context)) =
            decode_widget_type(type_document, &format!("{item_path}.Type"))
        else {
            // Already covered exhaustively by schema_oracle.rs (asserts
            // zero schema errors) — a schema failure here would mean the
            // two oracles disagree, worth its own unexpected-error bucket.
            unexpected_errors.push(format!("{item_path}: schema decode failed unexpectedly"));
            continue;
        };
        let Ok(object_document) = instance.get_document("Object") else {
            unexpected_errors.push(format!("{item_path}: missing Object document"));
            continue;
        };
        match decode_object(object_document, &context, &format!("{item_path}.Object")) {
            Ok(_) => full_success += 1,
            Err(PluggableError::NeedsFormsIntegration { kind, .. }) => {
                *blocked_by_kind.entry(kind).or_default() += 1;
            }
            Err(other) => {
                unexpected_errors.push(format!("{item_path} ({}): {other}", widget_type.id))
            }
        }
    }

    println!(
        "[{label}] {} widget instance(s): {} fully decoded, {} blocked on a not-yet-integrated \
         value kind, {} unexpected error(s)",
        instances.len(),
        full_success,
        blocked_by_kind.values().sum::<usize>(),
        unexpected_errors.len()
    );
    for (kind, count) in &blocked_by_kind {
        println!("[{label}]   blocked by {kind}: {count}");
    }
    assert!(
        unexpected_errors.is_empty(),
        "[{label}] {} unexpected (non-NeedsFormsIntegration) error(s):\n{}",
        unexpected_errors.len(),
        unexpected_errors.join("\n")
    );
}

#[test]
#[ignore]
fn decodes_self_contained_properties_of_every_real_pluggable_widget_instance() {
    let Ok(base) = std::env::var("MXRS_ACCEPTANCE_DIR") else {
        eprintln!(
            "[instance_oracle] skipped: MXRS_ACCEPTANCE_DIR is not set (see this file's module \
             doc for the expected layout)"
        );
        return;
    };
    run_against(&format!("{base}/qrqc-ruby/build/eQRQC.mpr"), "QRQC");
    run_against(&format!("{base}/spc-ruby/build/JEMScc-SPC.mpr"), "SPC");
}
