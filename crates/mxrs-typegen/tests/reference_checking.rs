//! Proves Phase 4's literal done-definition (per
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory): "a
//! nonexistent/renamed attribute or entity reference fails `cargo build`".
//! This spins up a real, throwaway `cargo build` per case (pointed at the
//! workspace's own `target/` dir via `CARGO_TARGET_DIR` so it reuses
//! already-compiled `mxrs-ir` artifacts instead of rebuilding from scratch)
//! rather than using `trybuild`'s `.stderr`-snapshot comparison — snapshot
//! matching pins exact rustc diagnostic text, which drifts across compiler
//! versions; a plain compile-success/failure check is what the done-
//! definition actually asks for.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn sample_manifest() -> mxrs_typegen::Manifest {
    mxrs_typegen::Manifest {
        modules: vec![mxrs_typegen::ModuleManifest {
            name: "Sales".into(),
            entities: vec![mxrs_typegen::EntityManifest {
                name: "Order".into(),
                attributes: vec!["Number".into()],
            }],
        }],
    }
}

fn try_compile(body: &str) -> Output {
    let markers = mxrs_typegen::generate(&sample_manifest()).unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    // Each fixture gets a package name unique to its tempdir: several of
    // these builds run concurrently (cargo test's default threading), all
    // pointed at the *same* CARGO_TARGET_DIR to reuse already-compiled
    // mxrs-ir artifacts — reusing one fixed package name across them made
    // Cargo's fingerprint cache reuse a stale (wrong) build result across
    // runs, silently turning "should fail to compile" tests into false
    // passes.
    let unique: String = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let name = format!("typegen-refcheck-fixture-{unique}");
    let cargo_toml = format!(
        "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nmxrs-ir = {{ path = {:?} }}\n",
        workspace_root().join("crates/mxrs-ir")
    );
    std::fs::write(dir.path().join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        format!("{markers}\n\n{body}\n"),
    )
    .unwrap();

    Command::new(env!("CARGO"))
        .arg("build")
        .current_dir(dir.path())
        .env("CARGO_TARGET_DIR", workspace_root().join("target"))
        .output()
        .expect("failed to invoke cargo for the fixture crate")
}

#[test]
fn a_reference_to_a_known_entity_marker_compiles() {
    let output = try_compile(
        "fn wants_entity<M: mxrs_ir::EntityMarker>() {}\npub fn use_it() { wants_entity::<Sales::Order>(); }",
    );
    assert!(
        output.status.success(),
        "expected success, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_reference_to_an_unknown_entity_fails_to_compile() {
    let output = try_compile(
        "fn wants_entity<M: mxrs_ir::EntityMarker>() {}\npub fn use_it() { wants_entity::<Sales::Ghost>(); }",
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for an unknown entity marker, but it compiled"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Ghost"),
        "expected the diagnostic to mention the unknown name, got:\n{stderr}"
    );
}

#[test]
fn a_reference_to_a_known_attribute_marker_resolves_its_owning_entity() {
    let output = try_compile(
        "pub fn use_it() -> String { use mxrs_ir::{AttributeMarker, EntityMarker}; <Sales::Order_Number as AttributeMarker>::Entity::qualified_name() }",
    );
    assert!(
        output.status.success(),
        "expected success, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_reference_to_a_renamed_attribute_fails_to_compile() {
    let output = try_compile(
        "fn wants_attribute<M: mxrs_ir::AttributeMarker>() {}\npub fn use_it() { wants_attribute::<Sales::Order_Total>(); }",
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for a renamed/removed attribute marker, but it compiled"
    );
}
