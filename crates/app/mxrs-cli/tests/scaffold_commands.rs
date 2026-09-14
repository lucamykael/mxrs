//! CLI contract of the artifact generators. The generated *content* is proven
//! by `mxrs-scaffold`'s own tests (which compile and run the project they
//! scaffold); what matters here is the surface a user types against: action
//! words, option handling, exit codes, and the two renderings.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mxrs_scaffold::SCAFFOLD_COMMANDS;
use serde_json::Value;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mxrs"))
        .args(args)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn project(directory: &Path) -> PathBuf {
    let root = directory.join("app");
    let output = cli(&[
        "new",
        "Order Portal",
        "--output",
        root.to_str().unwrap(),
        "--mxrs-workspace",
        workspace().to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{:?}", output.stderr);
    root
}

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .expect("the CLI crate always lives inside the workspace")
}

fn scaffold(root: &Path, arguments: &[&str]) -> Output {
    let mut args = arguments.to_vec();
    args.extend(["--target", root.to_str().unwrap()]);
    cli(&args)
}

#[test]
fn the_dispatcher_usage_line_matches_the_generator_catalog_for_every_command() {
    let output = cli(&["--commands", "--json"]);
    let catalog: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    for command in SCAFFOLD_COMMANDS {
        let entry = catalog
            .iter()
            .find(|entry| entry["name"] == command.name)
            .unwrap_or_else(|| panic!("{} is not dispatchable", command.name));
        assert_eq!(
            entry["usage"].as_str().unwrap(),
            format!("mxrs {}", mxrs_cli::scaffold::usage(command.kind)),
            "{}",
            command.name
        );
    }
    for required in ["module", "project", "scaffold", "security", "use-case"] {
        assert!(catalog.iter().any(|entry| entry["name"] == required));
    }
}

#[test]
fn generating_an_artifact_reports_created_files_and_the_build_command() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    let output = scaffold(&root, &["entity", "new", "Sales.Order"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let rendered = text(&output);
    assert!(rendered.contains(&format!(
        "  create  {}",
        root.join("src/domain/modules/sales/entities/order.rs").display()
    )));
    assert!(rendered.contains(&format!(
        "  update  {}",
        root.join("src/domain/modules/sales/mod.rs").display()
    )));
    assert!(rendered.ends_with("\nDone. Run:\n  cargo mxrs build\n"));
}

#[test]
fn a_wrong_action_word_or_missing_project_fails_without_writing_anything() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    for arguments in [
        vec!["entity", "create", "Sales.Order"],
        vec!["entity", "new"],
        vec!["security", "new", "Sales"],
        vec!["module", "init", "Sales"],
        vec!["entity", "new", "Sales.Order", "--role", "Sales.User"],
        vec![
            "page",
            "new",
            "Sales.Home",
            "--target",
            "a",
            "--target",
            "b",
        ],
    ] {
        let output = scaffold(&root, &arguments);
        assert!(!output.status.success(), "{arguments:?}");
    }
    assert!(!root.join("src/domain/modules").exists());
    let outside = cli(&[
        "entity",
        "new",
        "Sales.Order",
        "--target",
        directory.path().to_str().unwrap(),
    ]);
    assert!(!outside.status.success());
    assert!(
        String::from_utf8(outside.stderr)
            .unwrap()
            .contains("not a Cargo-native MXRS project")
    );
}

#[test]
fn a_dry_run_renders_the_json_document_without_touching_the_project() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    let output = scaffold(
        &root,
        &[
            "page",
            "new",
            "Sales.OrderOverview",
            "--role",
            "Sales.User",
            "--role",
            "Sales.Administrator",
            "--dry-run",
            "--json",
        ],
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["kind"], "page");
    assert_eq!(document["name"], "Sales.OrderOverview");
    assert_eq!(document["dry_run"], true);
    let files = document["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .all(|file| !Path::new(file.as_str().unwrap()).exists())
    );
    assert!(!document["updated"].as_array().unwrap().is_empty());

    let written = scaffold(
        &root,
        &[
            "page",
            "new",
            "Sales.OrderOverview",
            "--role",
            "Sales.User",
            "--json",
        ],
    );
    assert!(written.status.success());
    let written: Value = serde_json::from_slice(&written.stdout).unwrap();
    assert_eq!(written["files"], document["files"]);
    assert!(
        std::fs::read_to_string(root.join("src/domain/modules/sales/pages/order_overview.rs"))
            .unwrap()
            .contains("page.allow_role(\"Sales.User\");")
    );
}

#[test]
fn listing_advertises_only_catalogued_generators_and_destroying_removes_their_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    assert!(
        scaffold(&root, &["enumeration", "new", "Sales.Status"])
            .status
            .success()
    );
    let listed = text(&scaffold(&root, &["scaffold", "list"]));
    let (generators, registered) = listed.split_once("\nregistered scaffold\tfiles\n").unwrap();
    let rows = generators.lines().skip(1).collect::<Vec<_>>();
    assert_eq!(rows.len(), SCAFFOLD_COMMANDS.len());
    for command in SCAFFOLD_COMMANDS {
        assert!(rows.contains(&format!("{}\t{}", command.name, command.destination).as_str()));
    }
    assert!(registered.contains("enumeration:Sales.Status\t2"));

    let destroyed = scaffold(&root, &["scaffold", "destroy", "enumeration:Sales.Status"]);
    assert!(destroyed.status.success());
    assert!(text(&destroyed).contains("  remove  "));
    // Only files are removed, as in mxrb: an emptied directory is left in
    // place rather than pruned, so nothing outside the manifest is touched.
    assert!(
        !root
            .join("src/domain/modules/sales/enumerations/mod.rs")
            .exists()
    );
    assert!(
        !root
            .join("src/domain/modules/sales/enumerations/status.rs")
            .exists()
    );
    assert!(
        !scaffold(&root, &["scaffold", "destroy", "enumeration:Sales.Status"])
            .status
            .success()
    );
}

#[test]
fn inspecting_reports_the_declared_version_modules_and_registered_scaffolds() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    assert!(
        scaffold(&root, &["security", "init", "Sales"])
            .status
            .success()
    );
    let output = cli(&["project", "inspect", root.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["declared_version"], "11.12.1");
    assert_eq!(document["manifest"], true);
    assert_eq!(document["domain_module"], true);
    assert_eq!(document["modules"][0], "sales");
    assert_eq!(document["mprs"].as_array().unwrap().len(), 0);
    assert_eq!(
        document["registered_scaffolds"],
        serde_json::json!(["module:Sales", "security:Sales"])
    );
    let rendered = text(&cli(&["project", "inspect", root.to_str().unwrap()]));
    assert!(rendered.contains("declared_version: 11.12.1\n"));
    assert!(rendered.contains("registered_scaffolds: module:Sales, security:Sales\n"));
}
