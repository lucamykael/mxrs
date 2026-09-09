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
    assert!(
        !output.status.success(),
        "expected a compile failure for an unknown attribute kind"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("blob"),
        "expected the diagnostic to name the bad kind, got:\n{stderr}"
    );
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
    assert!(
        !output.status.success(),
        "expected a compile failure for an unknown association type"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Bogus"),
        "expected the diagnostic to name the bad association type, got:\n{stderr}"
    );
}

/// The actual point of wiring `mxrs-dsl` to require `Ref<M>`: a target
/// naming a marker type that was never declared fails at `cargo build`,
/// not at `mxrs-writer`'s runtime `UnknownAssociationTarget`.
#[test]
fn an_association_target_naming_an_undeclared_marker_type_fails_to_compile() {
    let output = try_compile(
        r#"
        mod markers {
            pub struct Customer;
            impl mxrs_ir::EntityMarker for Customer {
                const MODULE: &'static str = "Sales";
                const NAME: &'static str = "Customer";
            }
        }

        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order {
                        association Order_Customer -> markers::Ghost as Reference;
                    }
                }
            }
        }
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for an undeclared marker type"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Ghost"),
        "expected the diagnostic to name the unresolved marker, got:\n{stderr}"
    );
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
    assert!(
        !output.status.success(),
        "expected a compile failure for a missing semicolon"
    );
}

#[test]
fn a_duplicate_documentation_field_fails_to_compile() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order {
                        documentation "first";
                        documentation "second";
                    }
                }
            }
        }
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for a duplicate `documentation` field"
    );
}

#[test]
fn a_field_missing_mx_attribute_fails_to_compile() {
    let output = try_compile(
        r#"
        #[derive(mxrs_macros::MxEntity)]
        struct Order {
            number: String,
        }
        pub fn make() {}
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for a field with no #[mx_attribute(...)]"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mx_attribute"),
        "expected the diagnostic to mention the missing attribute, got:\n{stderr}"
    );
}

#[test]
fn an_unknown_mx_attribute_kind_fails_to_compile() {
    let output = try_compile(
        r#"
        #[derive(mxrs_macros::MxEntity)]
        struct Order {
            #[mx_attribute(kind = "blob")]
            number: String,
        }
        pub fn make() {}
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for an unknown mx_attribute kind"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("blob"),
        "expected the diagnostic to name the bad kind, got:\n{stderr}"
    );
}

#[test]
fn deriving_mx_entity_on_an_enum_fails_to_compile() {
    let output = try_compile(
        r#"
        #[derive(mxrs_macros::MxEntity)]
        enum Order {
            Draft,
        }
        pub fn make() {}
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for deriving MxEntity on an enum"
    );
}

#[test]
fn a_microflow_missing_a_return_keyword_fails_to_compile() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    microflow ACT_Broken {
                        "42";
                    }
                }
            }
        }
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for a microflow body missing `return`"
    );
}

#[test]
fn an_if_without_an_else_branch_fails_to_compile() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    microflow ACT_Broken {
                        if "$order/Total > 0" {
                            commit order;
                        }
                        return "$order";
                    }
                }
            }
        }
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for an `if` with no `else` branch"
    );
}

#[test]
fn an_unknown_microflow_statement_fails_with_a_message_naming_it() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    microflow ACT_Broken {
                        rollback order;
                        return "$order";
                    }
                }
            }
        }
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for an unknown microflow statement"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown microflow statement `rollback`"),
        "expected the error to name the bad keyword, got: {stderr}"
    );
}
