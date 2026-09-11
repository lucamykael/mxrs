//! `xtask noise-audit` — a mechanically-checked gate for the eloquence axis
//! of the mxrs plan (D2 in `decisions/mxrs-rust-rewrite-plan.md` in this
//! project's ai-memory): mxrs's authoring surface must never grow an escape
//! hatch into raw GUIDs/hashes/BSON, the way mxrb's `native_unit`/
//! `deep_structure`/`bson_binary`/`native_fragment` do. Ports the *intent*
//! of `lib/mxrb/public_source_audit.rb` (which measures exactly this kind
//! of leak in exported Ruby) as a hard gate rather than a report: this
//! command exits non-zero the moment either check below finds anything,
//! and is meant to run in CI on every commit, not just at a milestone.
//!
//! Two independent checks, both required to be clean:
//!
//! 1. **API surface** — `crates/mxrs-dsl`, `crates/mxrs-ir`,
//!    `crates/mxrs-macros` source is grepped for [`BANNED_IDENTIFIERS`].
//!    These are exactly mxrb's own opaque-API/unit-identity name lists
//!    (`OPAQUE_API_NAMES`/`UNIT_IDENTITY_NAMES` in `public_source_audit.rb`)
//!    minus the names that don't apply to mxrs's shape (`TypePointer`,
//!    `native_fragments`/`native_units`/`native_widgets`/`native_documents`
//!    collapse to their singular forms here since mxrs has no plural-path
//!    sidecar-loading variant to separately name). A hit here means someone
//!    added one of mxrb's escape hatches to the human-facing authoring API
//!    — the exact regression D2 exists to prevent.
//! 2. **Generated output** — `mxrs_exporter::export_project` is run against
//!    every committed `xtask/fixtures/*` fixture, and the resulting Rust
//!    source text is scanned for a UUID, a SHA-1/SHA-256 hex digest, or a
//!    literal `"$ID"`/`"$Type"` string — the concrete artifacts a human
//!    would actually have to read/edit if the exporter ever started leaking
//!    storage internals into its output.
//!
//! Deliberately **not** scanned: crates below the authoring surface
//! (`mxrs-model`, `mxrs-writer`, `mxrs-mpr`, the compiler crates, …) —
//! those necessarily work with real GUIDs/BSON internally (that's their
//! job), and `unit_id`/`container_id` are legitimate field names there.
//! D2 constrains what a human writes and reads, not how mxrs represents
//! `.mpr` data internally.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// mxrb's `OPAQUE_API_NAMES` + `UNIT_IDENTITY_NAMES`, adapted to mxrs's own
/// shape (no plural sidecar-loading variants to name separately, no
/// `TypePointer` equivalent yet). If a `pub fn`/`pub struct` in the
/// authoring crates below ever needs one of these words, that is D2 being
/// reversed, not a false positive — flag it to the user per the repo's
/// rule #8 (a locked decision is a decision) rather than silencing this
/// check.
const BANNED_IDENTIFIERS: &[&str] = &[
    "native_unit",
    "native_fragment",
    "native_widget",
    "native_document",
    "deep_structure",
    "form_structure",
    "bson_binary",
    "unit_id",
    "container_id",
    "mendix_id",
];

/// Crates that make up mxrs's human-facing authoring surface — the only
/// place [`BANNED_IDENTIFIERS`] is disallowed. See this module's doc
/// comment for why the rest of the workspace is out of scope.
const AUTHORING_CRATES: &[&str] = &["mxrs-dsl", "mxrs-ir", "mxrs-macros"];

pub struct Finding {
    pub category: &'static str,
    pub location: String,
    pub line: usize,
    pub excerpt: String,
}

pub fn noise_audit(workspace_root: &Path) -> Result<(), String> {
    let mut findings = Vec::new();
    findings.extend(audit_api_surface(workspace_root)?);
    findings.extend(audit_generated_fixtures(workspace_root)?);

    if findings.is_empty() {
        println!("[xtask] noise-audit: 0 finding(s)");
        println!("[xtask] PASS");
        return Ok(());
    }

    let mut categories: BTreeSet<&str> = BTreeSet::new();
    for finding in &findings {
        categories.insert(finding.category);
        println!(
            "  [{}] {}:{}: {}",
            finding.category, finding.location, finding.line, finding.excerpt
        );
    }
    println!(
        "[xtask] noise-audit: {} finding(s) across {} categor{}",
        findings.len(),
        categories.len(),
        if categories.len() == 1 { "y" } else { "ies" }
    );
    Err("noise-audit found opacity in mxrs's authoring surface or generated output (see above) — see D2 in decisions/mxrs-rust-rewrite-plan.md".to_string())
}

fn audit_api_surface(workspace_root: &Path) -> Result<Vec<Finding>, String> {
    let mut findings = Vec::new();
    for crate_name in AUTHORING_CRATES {
        let src_dir = workspace_root.join("crates").join(crate_name).join("src");
        if !src_dir.is_dir() {
            continue;
        }
        for path in rust_files(&src_dir)? {
            let relative = path
                .strip_prefix(workspace_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            for (identifier, line, excerpt) in scan_identifiers(&text) {
                findings.push(Finding {
                    category: identifier,
                    location: relative.clone(),
                    line,
                    excerpt,
                });
            }
        }
    }
    Ok(findings)
}

fn audit_generated_fixtures(workspace_root: &Path) -> Result<Vec<Finding>, String> {
    let mut findings = Vec::new();
    let fixtures_dir = workspace_root.join("xtask").join("fixtures");
    let Ok(entries) = std::fs::read_dir(&fixtures_dir) else {
        return Ok(findings);
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let fixture_dir = entry.path();
        if !fixture_dir.is_dir() {
            continue;
        }
        let Some(mpr_path) = find_mpr_file(&fixture_dir) else {
            continue;
        };
        let label = format!(
            "generated::{}",
            fixture_dir.file_name().unwrap().to_string_lossy()
        );
        let source = match mxrs_exporter::export_project(&mpr_path) {
            Ok(source) => source,
            // A fail-closed refusal (unsupported feature in the fixture) is
            // not itself a noise-audit finding — export_project's own
            // RoundTripGap reporting covers that separately.
            Err(_) => continue,
        };
        for (identifier, line, excerpt) in scan_identifiers(&source) {
            findings.push(Finding {
                category: identifier,
                location: label.clone(),
                line,
                excerpt,
            });
        }
        for (category, line, excerpt) in scan_opaque_text(&source) {
            findings.push(Finding {
                category,
                location: label.clone(),
                line,
                excerpt,
            });
        }
    }
    Ok(findings)
}

/// Matches [`BANNED_IDENTIFIERS`] as whole words (so `container_id` doesn't
/// also fire on an unrelated `container_id_prefix`), line by line rather
/// than via a single multi-line regex — keeps line numbers trivial to
/// compute and avoids pulling in a regex crate for a handful of literal
/// words.
fn scan_identifiers(text: &str) -> Vec<(&'static str, usize, String)> {
    let mut hits = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        for identifier in BANNED_IDENTIFIERS {
            if contains_word(line, identifier) {
                hits.push((*identifier, idx + 1, line.trim().to_string()));
            }
        }
    }
    hits
}

fn contains_word(line: &str, word: &str) -> bool {
    let is_ident_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut search_from = 0;
    while let Some(offset) = line[search_from..].find(word) {
        let start = search_from + offset;
        let end = start + word.len();
        let before_ok = start == 0 || !is_ident_char(line.as_bytes()[start - 1] as char);
        let after_ok = end == line.len() || !is_ident_char(line.as_bytes()[end] as char);
        if before_ok && after_ok {
            return true;
        }
        search_from = start + 1;
    }
    false
}

/// Catches the concrete artifacts a human would have to read if generated
/// output ever leaked storage internals: a UUID, a SHA-1/SHA-256 hex
/// digest, or a literal `$ID`/`$Type` BSON key — the same three text
/// patterns `public_source_audit.rb`'s `TEXT_PATTERNS` flags (its
/// `sidecar_reference` pattern has no mxrs equivalent yet — the sidecar
/// design in D2 isn't built, so there is no `.mxrs/` path convention to
/// check for).
fn scan_opaque_text(text: &str) -> Vec<(&'static str, usize, String)> {
    let mut hits = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if let Some(excerpt) = find_uuid(line) {
            hits.push(("uuid", idx + 1, excerpt));
        }
        if let Some(excerpt) = find_hex_digest(line) {
            hits.push(("digest", idx + 1, excerpt));
        }
        if line.contains("\"$ID\"") || line.contains("\"$Type\"") {
            hits.push(("storage_schema", idx + 1, line.trim().to_string()));
        }
    }
    hits
}

/// `8-4-4-4-12` hex groups joined by `-`, case-insensitive — a Mendix
/// MS-GUID string form, not a Rust-specific pattern, so this is
/// hand-rolled rather than pulled in via `uuid::Uuid::parse_str` scanning
/// (which needs the candidate substring pre-isolated anyway).
fn find_uuid(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let groups = [8, 4, 4, 4, 12];
    'outer: for start in 0..bytes.len() {
        let mut pos = start;
        for (i, &group_len) in groups.iter().enumerate() {
            if pos + group_len > bytes.len()
                || !bytes[pos..pos + group_len]
                    .iter()
                    .all(|b| b.is_ascii_hexdigit())
            {
                continue 'outer;
            }
            pos += group_len;
            let is_last = i == groups.len() - 1;
            if !is_last {
                if pos >= bytes.len() || bytes[pos] != b'-' {
                    continue 'outer;
                }
                pos += 1;
            }
        }
        let word_boundary_before =
            start == 0 || !(bytes[start - 1] as char).is_ascii_alphanumeric();
        let word_boundary_after =
            pos >= bytes.len() || !(bytes[pos] as char).is_ascii_alphanumeric();
        if word_boundary_before && word_boundary_after {
            return Some(line.trim().to_string());
        }
    }
    None
}

/// A bare 40- or 64-hex-char run (SHA-1 or SHA-256), word-bounded.
fn find_hex_digest(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut run_start = None;
    for (i, &b) in bytes.iter().enumerate() {
        if (b as char).is_ascii_hexdigit() {
            if run_start.is_none() {
                run_start = Some(i);
            }
        } else if let Some(start) = run_start.take()
            && matches!(i - start, 40 | 64)
        {
            return Some(line.trim().to_string());
        }
    }
    if let Some(start) = run_start
        && matches!(bytes.len() - start, 40 | 64)
    {
        return Some(line.trim().to_string());
    }
    None
}

fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            out.extend(rust_files(&path)?);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    Ok(out)
}

fn find_mpr_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("mpr"))
}
