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
//! Unlike `schema_oracle.rs`, this one does **not** assert zero decode
//! errors: a pluggable value can contain a native Forms element whose own
//! type is not implemented yet. Every successfully decoded widget is also
//! encoded and decoded again, and the test asserts semantic equality.

use std::collections::BTreeMap;
use std::rc::Rc;

use mxrs_bson::{Bson, Document};
use mxrs_mpr::MprFile;
use mxrs_pluggable::{PluggableError, decode_widget, encode_widget};

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

    // A real `mxrs-forms` codec, not a stand-in — this is what lets
    // `TextTemplate`/`Action`/`Icon` actually decode via
    // `EmbeddedFormsDecoder` instead of only ever hitting
    // `NeedsFormsIntegration`, so the numbers this test prints reflect
    // real end-to-end capability.
    let catalog = Rc::new(
        mxrs_forms::Catalog::for_version("11.12.1").expect("embedded 11.12.1 schema must load"),
    );
    let forms_codec = mxrs_forms::MprCodec::new(catalog);

    let mut full_success = 0usize;
    let mut blocked_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut embedded_decode_failures = 0usize;
    let mut unexpected_errors = Vec::new();

    for (index, instance) in instances.iter().enumerate() {
        let item_path = format!("$[{index}]");
        match decode_widget(instance, &item_path, &forms_codec) {
            Ok(widget) => {
                full_success += 1;
                match encode_widget(&widget, &item_path, &forms_codec)
                    .and_then(|encoded| decode_widget(&encoded, &item_path, &forms_codec))
                {
                    Ok(round_tripped) if round_tripped == widget => {}
                    Ok(_) => unexpected_errors.push(format!(
                        "{item_path} ({}): semantic value changed after encode/decode",
                        widget.widget_type.id
                    )),
                    Err(error) => unexpected_errors.push(format!(
                        "{item_path} ({}): round-trip failed: {error}",
                        widget.widget_type.id
                    )),
                }
            }
            Err(PluggableError::NeedsFormsIntegration { kind, .. }) => {
                *blocked_by_kind.entry(kind).or_default() += 1;
            }
            // A real TextTemplate/Action/Icon element exists but hits a
            // `mxrs-forms` gap of its own (e.g. an unsupported native
            // widget nested inside it) — a legitimate, separately-named
            // failure mode, not this crate's own gap and not a panic.
            Err(PluggableError::EmbeddedDecodeFailed { .. }) => {
                embedded_decode_failures += 1;
            }
            Err(other) => unexpected_errors.push(format!("{item_path}: {other}")),
        }
    }

    println!(
        "[{label}] {} widget instance(s): {} fully decoded, {} blocked on a not-yet-integrated \
         value kind, {} blocked on an mxrs-forms decode failure inside an embedded element, {} \
         unexpected error(s)",
        instances.len(),
        full_success,
        blocked_by_kind.values().sum::<usize>(),
        embedded_decode_failures,
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
