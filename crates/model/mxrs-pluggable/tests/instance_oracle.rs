//! Structural oracle for pluggable-widget *instances* (not just their
//! schema — see `schema_oracle.rs` for that) in an explicitly supplied,
//! unversioned `.mpr` corpus.
//!
//! Run manually with:
//! ```text
//! MXRS_ACCEPTANCE_PROJECTS=/path/a.mpr:/path/b.mpr cargo test -p mxrs-pluggable \
//!     --test instance_oracle -- --ignored --nocapture
//! ```
//!
//! A pluggable value can contain a native Forms element, so this gate also
//! exercises the real `mxrs-forms` integration. Every instance must decode,
//! encode, and decode again with semantic equality.

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
    for unit in mpr.all_units().expect("read every corpus unit") {
        let document = mpr
            .parse_contents(&unit)
            .unwrap_or_else(|error| panic!("cannot parse corpus unit {}: {error}", unit.unit_id));
        visit(&Bson::Document(document), &mut found);
    }
    found
}

// The editor calls these ClientTemplates, but 11.12.1 stores Texts$Text.
// Semantic equality alone would miss regenerated translation IDs/order.
fn translated_placeholder_bytes(document: &Document) -> BTreeMap<String, Vec<u8>> {
    fn visit(value: &Bson, path: &str, found: &mut BTreeMap<String, Vec<u8>>) {
        match value {
            Bson::Document(document) => {
                for (key, child) in document {
                    let path = format!("{path}.{key}");
                    if key == "Placeholder"
                        && let Bson::Document(text) = child
                        && text.get_str("$Type").ok() == Some("Texts$Text")
                    {
                        found.insert(
                            path.clone(),
                            mxrs_bson::serialize(text).expect("serialize corpus placeholder"),
                        );
                    }
                    visit(child, &path, found);
                }
            }
            Bson::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    visit(item, &format!("{path}[{index}]"), found);
                }
            }
            _ => {}
        }
    }
    let mut found = BTreeMap::new();
    visit(&Bson::Document(document.clone()), "$", &mut found);
    found
}

fn run_against(path: &std::path::Path, label: &str) -> Vec<String> {
    let mpr = MprFile::open(path, true)
        .unwrap_or_else(|error| panic!("[{label}] cannot open corpus {}: {error}", path.display()));
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
    let mut embedded_failure_details: BTreeMap<String, usize> = BTreeMap::new();
    let mut unexpected_errors = Vec::new();
    let mut exact_placeholders = 0usize;

    for (index, instance) in instances.iter().enumerate() {
        let item_path = format!("$[{index}]");
        match decode_widget(instance, &item_path, &forms_codec) {
            Ok(widget) => {
                full_success += 1;
                match encode_widget(&widget, &item_path, &forms_codec).and_then(|encoded| {
                    let original = translated_placeholder_bytes(instance);
                    if original == translated_placeholder_bytes(&encoded) {
                        exact_placeholders += original.len();
                    } else {
                        unexpected_errors.push(format!(
                            "{item_path}: translated placeholder bytes, identities or order changed"
                        ));
                    }
                    decode_widget(&encoded, &item_path, &forms_codec)
                }) {
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
            Err(PluggableError::EmbeddedDecodeFailed { path, source }) => {
                embedded_decode_failures += 1;
                *embedded_failure_details
                    .entry(format!("{path}: {source}"))
                    .or_default() += 1;
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
    println!(
        "[{label}] {exact_placeholders} nested translated placeholder(s) byte-identical after encoding"
    );
    for (kind, count) in &blocked_by_kind {
        println!("[{label}]   blocked by {kind}: {count}");
    }
    for (detail, count) in &embedded_failure_details {
        println!("[{label}]   embedded failure ({count}x): {detail}");
    }
    let mut failures = Vec::new();
    if exact_placeholders == 0 {
        failures.push(format!(
            "[{label}] no translated placeholder byte comparisons exercised"
        ));
    }
    if !blocked_by_kind.is_empty() {
        failures.push(format!(
            "[{label}] pluggable value kinds still need Forms integration: {blocked_by_kind:#?}"
        ));
    }
    if !embedded_failure_details.is_empty() {
        failures.push(format!(
            "[{label}] embedded Forms values failed decoding: {embedded_failure_details:#?}"
        ));
    }
    if !unexpected_errors.is_empty() {
        failures.push(format!(
            "[{label}] unexpected round-trip errors: {}",
            unexpected_errors.join("\n")
        ));
    }
    if full_success != instances.len() {
        failures.push(format!(
            "[{label}] {full_success}/{} widget instances decoded",
            instances.len()
        ));
    }
    failures
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
fn decodes_every_configured_pluggable_widget_instance() {
    let mut failures = Vec::new();
    for (index, project) in corpus_projects().iter().enumerate() {
        failures.extend(run_against(project, &format!("case-{}", index + 1)));
    }
    assert!(
        failures.is_empty(),
        "acceptance corpus instance parity is incomplete:\n{}",
        failures.join("\n")
    );
}
