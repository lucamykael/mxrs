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

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .expect("workspace root contains xtask/Cargo.toml")
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
        "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nmxrs-macros = {{ path = {:?} }}\nmxrs-dsl = {{ path = {:?} }}\nmxrs-expr = {{ path = {:?} }}\nmxrs-ir = {{ path = {:?} }}\n",
        workspace_root().join("crates/authoring/mxrs-macros"),
        workspace_root().join("crates/authoring/mxrs-dsl"),
        workspace_root().join("crates/authoring/mxrs-expr"),
        workspace_root().join("crates/authoring/mxrs-ir"),
    );
    std::fs::write(dir.path().join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), body).unwrap();

    Command::new(env!("CARGO"))
        .arg("build")
        .arg("--offline")
        .current_dir(dir.path())
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace_root().join("target")),
        )
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
fn entity_parent_and_lifecycle_references_are_marker_checked() {
    for (body, missing) in [
        (
            r#"
            pub fn make() -> mxrs_ir::ProjectDecl {
                mxrs_macros::project! {
                    "11.12.1",
                    module Sales {
                        entity Order { generalizes Sales::MissingParent; }
                    }
                }
            }
            "#,
            "MissingParent",
        ),
        (
            r#"
            pub fn make() -> mxrs_ir::ProjectDecl {
                mxrs_macros::project! {
                    "11.12.1",
                    module Sales {
                        entity Order { before_commit Sales::MissingFlow; }
                    }
                }
            }
            "#,
            "MissingFlow",
        ),
    ] {
        let output = try_compile(body);
        assert!(!output.status.success(), "{missing} unexpectedly compiled");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(missing), "unexpected diagnostic:\n{stderr}");
    }
}

#[test]
fn an_unknown_indexed_system_member_fails_at_the_declaration() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order {
                        index { system InventedTimestamp; }
                    }
                }
            }
        }
        "#,
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown indexed system member"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn an_unknown_access_right_fails_at_the_declaration() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order {
                        string Number;
                        access_rule ["User"] {
                            attribute Number Admin;
                        }
                    }
                }
            }
        }
        "#,
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Admin"), "unexpected diagnostic:\n{stderr}");
    assert!(
        stderr.contains("member rights"),
        "unexpected diagnostic:\n{stderr}"
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
fn an_uninferable_field_type_fails_to_compile() {
    let output = try_compile(
        r#"
        #[derive(mxrs_macros::MxEntity)]
        struct Order {
            unsupported: (String, String),
        }
        pub fn make() {}
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for a field whose type cannot be inferred"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot infer a Mendix attribute type"),
        "expected the diagnostic to explain type inference, got:\n{stderr}"
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
fn a_reference_target_must_be_an_entity_marker() {
    let output = try_compile(
        r#"
        struct NotAnEntity;

        #[derive(mxrs_macros::MxEntity)]
        #[mxrs(module = "Sales")]
        struct Order {
            customer: mxrs_ir::Reference<NotAnEntity>,
        }
        pub fn make() {}
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a compile failure for a Reference<T> whose target is not an entity"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a Mendix entity marker")
            || stderr.contains("EntityMarker is not implemented"),
        "expected the diagnostic to identify the invalid entity target, got:\n{stderr}"
    );
}

#[test]
fn a_reference_to_a_non_entity_type_fails_to_compile() {
    let output = try_compile(
        r#"
        struct NotAnEntity;

        #[derive(mxrs_macros::MxEntity)]
        #[mxrs(module = "Sales")]
        struct Order {
            customer: mxrs_ir::Reference<NotAnEntity>,
        }
        pub fn make() {}
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a typed association target without EntityMarker to fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Mendix entity marker") || stderr.contains("EntityMarker"),
        "expected the diagnostic to name the missing entity-marker contract, got:\n{stderr}"
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

#[test]
fn assigning_a_string_to_a_decimal_attribute_fails_to_compile() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order { decimal Total; }
                    microflow ACT_Broken {
                        create order = Sales::Order { Total = "not a decimal"; };
                        return order;
                    }
                }
            }
        }
        "#,
    );
    assert!(
        !output.status.success(),
        "expected a typed assignment failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("IntoExpr<MxDecimal>"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn using_an_unbound_flow_variable_fails_to_compile() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order {}
                    microflow ACT_Broken { commit missing; }
                }
            }
        }
        "#,
    );
    assert!(
        !output.status.success(),
        "expected an unbound variable failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn break_outside_a_loop_is_rejected_by_the_macro() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales { microflow ACT_Broken { break; } }
            }
        }
        "#,
    );
    assert!(
        !output.status.success(),
        "expected an invalid break failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("only valid inside a microflow loop"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn iterating_an_unbound_list_fails_at_the_list_name() {
    let output = try_compile(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {
            mxrs_macros::project! {
                "11.12.1",
                module Sales {
                    entity Order {}
                    microflow ACT_Broken {
                        for current in missing { commit current; }
                    }
                }
            }
        }
        "#,
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown list variable `missing`"),
        "unexpected diagnostic:\n{stderr}"
    );
}

fn parameter_diagnostic(module_body: &str, expected: &str) {
    let source = format!(
        "pub fn build() -> mxrs_ir::ProjectDecl {{ mxrs_macros::project! {{ \"11.12.1\", module Sales {{ {module_body} }} }} }}"
    );
    let output = try_compile(&source);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "invalid input compiled: {module_body}"
    );
    assert!(
        stderr.contains(expected),
        "expected {expected:?}, got {stderr}"
    );
}

#[test]
fn flow_parameter_names_and_options_have_explicit_diagnostics() {
    for (body, expected) in [
        (
            "microflow Target { parameter value: string; parameter value: string; }",
            "duplicate flow parameter",
        ),
        (
            "microflow Target { parameter value: enumeration; }",
            "unsupported flow parameter type",
        ),
        (
            "microflow Target { parameter value: string { required true; required false; } }",
            "duplicate parameter required",
        ),
        (
            "microflow Target { parameter value: string { documentation \"a\"; documentation \"b\"; } }",
            "duplicate parameter documentation",
        ),
        (
            "microflow Target { parameter value: string { default_value mxrs_expr::string(\"a\"); default_value mxrs_expr::string(\"b\"); } }",
            "duplicate parameter default",
        ),
        (
            "microflow Target { parameter value: string { mystery true; } }",
            "unknown flow parameter option",
        ),
        (
            "entity Order {} microflow Target { create order = Sales::Order {}; parameter value: string; }",
            "parameters must precede",
        ),
    ] {
        parameter_diagnostic(body, expected);
    }
}

#[test]
fn local_calls_require_each_declared_argument_exactly_once() {
    for (arguments, expected) in [
        ("", "missing argument `value`"),
        (
            "(extra: mxrs_expr::string(\"x\"))",
            "unknown parameter `extra`",
        ),
        (
            "(value: mxrs_expr::string(\"a\"), value: mxrs_expr::string(\"b\"))",
            "duplicate call argument",
        ),
    ] {
        parameter_diagnostic(
            &format!(
                "microflow Target {{ parameter value: string; }} microflow Caller {{ call Sales::Target {arguments}; }}"
            ),
            expected,
        );
    }
}

#[test]
fn local_call_arguments_and_parameter_defaults_are_rust_type_checked() {
    parameter_diagnostic(
        "microflow Target { parameter value: string; } microflow Caller { call Sales::Target (value: mxrs_expr::boolean(true)); }",
        "IntoExpr",
    );
    parameter_diagnostic(
        "microflow Target { parameter value: string { default_value mxrs_expr::boolean(true); } }",
        "IntoExpr",
    );
    parameter_diagnostic(
        "entity Order {} microflow Target { parameter value: list<Sales::Order>; } microflow Caller { parameter order: object<Sales::Order>; call Sales::Target (value: order); }",
        "IntoExpr",
    );
    parameter_diagnostic(
        "entity Order {} entity Other {} microflow Target { parameter value: object<Sales::Order>; } microflow Caller { parameter order: object<Sales::Other>; call Sales::Target (value: order); }",
        "IntoExpr",
    );
}

#[test]
fn nested_calls_keep_the_same_argument_contract() {
    parameter_diagnostic(
        "microflow Target { parameter value: string; } microflow Caller { if mxrs_expr::boolean(true) { call Sales::Target; } else {} }",
        "missing argument `value`",
    );
    parameter_diagnostic(
        "microflow Target { parameter value: string; } microflow Caller { rescue { call Sales::Target (value: mxrs_expr::boolean(true)); } }",
        "IntoExpr",
    );
}

#[test]
fn public_facade_macro_works_with_a_single_renamed_dependency_and_denied_warnings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        format!(
            r#"
[package]
name = "flow-facade-contract"
version = "0.0.0"
edition = "2024"
[dependencies]
model_api = {{ package = "mxrs", path = {:?} }}
"#,
            workspace_root().join("crates/app/mxrs")
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        r#"
#![deny(warnings)]
mod helpers { pub mod mxrs_expr { pub fn value() -> model_api::Expr<model_api::MxString> { model_api::string("value") } } }
pub fn make() -> model_api::ProjectDecl {
    model_api::project! {
        "11.12.1",
        module Sales {
            entity Order { string Number; }
            microflow Echo { parameter message: string { default_value helpers::mxrs_expr::value(); } return message; }
            microflow Caller {
                parameter message: string;
                parameter order: object<Sales::Order>;
                parameter orders: list<Sales::Order>;
                change order { Number = message.clone(); };
                for current in orders { change current { Number = message.clone(); }; }
                call Sales::Echo (message: message);
            }
        }
    }
}
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO"))
        .args(["check", "--offline"])
        .current_dir(dir.path())
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace_root().join("target/nested-flow-facade")),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
