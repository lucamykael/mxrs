//! Proves bad `project! {}` input fails to compile with a message pointing
//! at the actual mistake (M8.4's "span-accurate diagnostics" spirit — full
//! `proc-macro2::Span`-driven diagnostics are a later refinement, but
//! `syn::Error`'s built-in span reporting already gets most of the way
//! there for free). Spins up real throwaway `cargo build` invocations
//! rather than `trybuild`'s `.stderr` snapshots, same rationale as
//! `mxrs-typegen`'s `tests/reference_checking.rs`: pinning exact rustc/syn
//! diagnostic text rots across compiler and dependency versions.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

fn try_compile(body: &str) -> Output {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let unique: String =
        dir.path().file_name().unwrap().to_string_lossy().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let name = format!("macros-diagnostics-fixture-{unique}");
    let cargo_toml = format!(
        "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nmxrs-macros = {{ path = {:?} }}\nmxrs-dsl = {{ path = {:?} }}\nmxrs-model = {{ path = {:?} }}\nmxrs-ir = {{ path = {:?} }}\n",
        workspace_root().join("crates/mxrs-macros"),
        workspace_root().join("crates/mxrs-dsl"),
        workspace_root().join("crates/mxrs-model"),
        workspace_root().join("crates/mxrs-ir"),
    );
    std::fs::write(dir.path().join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), body).unwrap();

    Command::new(env!("CARGO"))
        .arg("build")
        .current_dir(dir.path())
        .env("CARGO_TARGET_DIR", workspace_root().join("target"))
        .output()
        .expect("failed to invoke cargo for the fixture crate")
}

#[test]
fn an_unknown_attribute_kind_fails_with_a_message_naming_it() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order {
                        blob Attachment;
                    }
                }
            }
        }
        "#,
    );
    assert!(!output.status.success(), "expected a compile failure for an unknown attribute kind");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("blob"), "expected the diagnostic to name the bad kind, got:\n{stderr}");
}

#[test]
fn an_unknown_association_type_fails_with_a_message_naming_it() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Customer {}
                    entity Order {
                        association Order_Customer -> Customer as Bogus;
                    }
                }
            }
        }
        "#,
    );
    assert!(!output.status.success(), "expected a compile failure for an unknown association type");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Bogus"), "expected the diagnostic to name the bad association type, got:\n{stderr}");
}

#[test]
fn a_missing_semicolon_fails_to_compile() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order {
                        string Number
                    }
                }
            }
        }
        "#,
    );
    assert!(!output.status.success(), "expected a compile failure for a missing semicolon");
}
