//! Proves bad `task_queue` grammar fails `project! {}` at compile time with
//! a message pointing at the actual mistake. Same real-nested-`cargo build`
//! rationale as `diagnostics.rs` (pinning exact rustc/syn diagnostic text
//! rots across versions); kept as its own file to avoid colliding with that
//! one's edits while the task_queue grammar lands.

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
    let name = format!("macros-task-queue-diagnostics-fixture-{unique}");
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

fn queue_project(body: &str) -> String {
    format!(
        r#"
        pub fn make() -> mxrs_ir::ProjectDecl {{
            mxrs_macros::project! {{
                "11.12.1",
                module Jobs {{
                    task_queue Imports {{
                        {body}
                    }}
                }}
            }}
        }}
        "#
    )
}

#[test]
fn declaring_neither_parallelism_nor_an_expression_fails_to_compile() {
    let output = try_compile(&queue_project("documentation \"no config\";"));
    assert!(
        !output.status.success(),
        "expected a compile failure for a task_queue with no parallelism config"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires exactly one of parallelism or parallelism_expression"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn declaring_both_parallelism_and_an_expression_fails_to_compile() {
    let output = try_compile(&queue_project(
        "parallelism 4; parallelism_expression \"4\";",
    ));
    assert!(
        !output.status.success(),
        "expected a compile failure for a task_queue declaring both configs"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires exactly one of parallelism or parallelism_expression"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn a_scope_on_a_fixed_task_queue_fails_to_compile() {
    let output = try_compile(&queue_project("parallelism 4; scope ClusterWide;"));
    assert!(
        !output.status.success(),
        "expected a compile failure for `scope` on a fixed task_queue"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("scope requires parallelism_expression"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn a_zero_fixed_parallelism_literal_fails_to_compile() {
    let output = try_compile(&queue_project("parallelism 0;"));
    assert!(
        !output.status.success(),
        "expected a compile failure for a zero parallelism literal"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("parallelism must be between 1 and 2147483647"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn a_parallelism_literal_over_i32_max_fails_to_compile() {
    let output = try_compile(&queue_project("parallelism 2147483648;"));
    assert!(
        !output.status.success(),
        "expected a compile failure for a parallelism literal over i32::MAX"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("parallelism must be between 1 and 2147483647"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn an_empty_parallelism_expression_fails_to_compile() {
    let output = try_compile(&queue_project("parallelism_expression \"\";"));
    assert!(
        !output.status.success(),
        "expected a compile failure for an empty parallelism expression"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("parallelism expression must not be empty"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn an_unknown_scope_value_fails_with_a_message_naming_it() {
    let output = try_compile(&queue_project(
        "parallelism_expression \"4\"; scope Everywhere;",
    ));
    assert!(
        !output.status.success(),
        "expected a compile failure for an unknown scope value"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown task queue scope"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn an_unknown_export_level_fails_with_a_message_naming_the_option() {
    let output = try_compile(&queue_project("parallelism 4; export_level Draft;"));
    assert!(
        !output.status.success(),
        "expected a compile failure for an unknown export level"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown export level"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn an_unknown_task_queue_option_fails_with_a_message_naming_the_mistake() {
    let output = try_compile(&queue_project("parallelism 4; retries 3;"));
    assert!(
        !output.status.success(),
        "expected a compile failure for an unknown task_queue option"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown task queue option"),
        "unexpected diagnostic:\n{stderr}"
    );
}

#[test]
fn a_duplicate_parallelism_option_fails_to_compile() {
    let output = try_compile(&queue_project("parallelism 4; parallelism 8;"));
    assert!(
        !output.status.success(),
        "expected a compile failure for a duplicate parallelism option"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate"),
        "unexpected diagnostic:\n{stderr}"
    );
}
