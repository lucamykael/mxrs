//! Proves `export_project`'s output is genuinely valid, compilable Rust —
//! not just a string that happens to contain the right substrings (which
//! is all `src/lib.rs`'s own unit tests check). Spins up a real throwaway
//! `cargo build`, same rationale and pattern as `mxrs-macros`'
//! `tests/diagnostics.rs`: this is what "the round trip actually works"
//! means for generated source, and pinning a full expected-output snapshot
//! would rot on every formatting or field-ordering change.

use std::path::{Path, PathBuf};
use std::process::Output;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn try_compile(body: &str) -> Output {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let unique: String = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let name = format!("exporter-fixture-{unique}");
    let cargo_toml = format!(
        "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nmxrs-macros = {{ path = {:?} }}\nmxrs-ir = {{ path = {:?} }}\nmxrs-dsl = {{ path = {:?} }}\n",
        workspace_root().join("crates/mxrs-macros"),
        workspace_root().join("crates/mxrs-ir"),
        workspace_root().join("crates/mxrs-dsl"),
    );
    std::fs::write(dir.path().join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), body).unwrap();

    std::process::Command::new(env!("CARGO"))
        .arg("build")
        .arg("--offline")
        .current_dir(dir.path())
        .env("CARGO_TARGET_DIR", workspace_root().join("target"))
        .output()
        .expect("failed to invoke cargo for the fixture crate")
}

#[test]
fn exported_source_compiles_as_a_standalone_crate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.persistable(true);
            e.string("Number").default_value = Some("A-0".to_string());
            e.decimal("Total");
            e.association(
                "Order_Customer",
                mxrs_ir::Ref::<markers::Customer>::new(),
                mxrs_ir::AssociationType::Reference,
            );
        });
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();

    let source = mxrs_exporter::export_project(&path).unwrap();
    let output = try_compile(&source);
    assert!(
        output.status.success(),
        "generated source failed to compile:\n{}\n---source---\n{source}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[allow(dead_code, non_snake_case)]
mod markers {
    pub struct Customer;
    impl mxrs_ir::EntityMarker for Customer {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "Customer";
    }
}
