use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mxrs_ir::Activity;
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

fn fixture(cyclic: bool) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Commands.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.entity("Order", |entity| {
            entity.string("Number");
        });
        module.microflow("Start", |_| {});
        module.microflow("Save", |_| {});
        module.microflow("Unused", |_| {});
    });
    let mut project = builder.build();
    let call = |name: &str| Activity::CallMicroflow {
        name: name.into(),
        result_variable: None,
        use_return: false,
        mappings: vec![],
    };
    project.modules[0].microflows[0].activities = vec![call("Sales.Save"), call("Sales.Save")];
    if cyclic {
        project.modules[0].microflows[1].activities = vec![call("Sales.Start")];
    }
    mxrs_writer::write_project(&path, &project).unwrap();
    (directory, path)
}

fn query(command: &str, path: &Path, suffix: &[&str]) -> Output {
    let mut args = vec![command, path.to_str().unwrap()];
    args.extend(suffix);
    cli(&args)
}

#[test]
fn every_discoverable_command_has_working_help_and_rejects_missing_arguments() {
    let output = cli(&["--commands", "--json"]);
    assert!(output.status.success());
    let catalog: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    let mut names = std::collections::BTreeSet::new();
    for entry in catalog {
        let name = entry["name"].as_str().unwrap();
        assert!(names.insert(name.to_string()), "duplicate command {name}");
        for flag in ["--help", "-h"] {
            let output = cli(&[name, flag]);
            assert!(output.status.success(), "{name}: {:?}", output.stderr);
            assert!(text(&output).contains(entry["usage"].as_str().unwrap()));
        }
        assert!(cli(&["help", name]).status.success());
        if name != "help" {
            assert!(
                !cli(&[name]).status.success(),
                "{name} accepted no arguments"
            );
        }
    }
    for required in [
        "validate", "import", "export", "callees", "callers", "describe", "tree", "lint", "report",
    ] {
        assert!(names.contains(required));
    }
}

#[test]
fn global_discovery_flags_have_stable_exit_status_and_reject_unknown_options() {
    for args in [
        vec![],
        vec!["--help"],
        vec!["-h"],
        vec!["help"],
        vec!["--commands"],
    ] {
        let output = cli(&args);
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    for flag in ["--version", "-V", "-v"] {
        let output = cli(&[flag]);
        assert!(output.status.success());
        assert_eq!(
            text(&output),
            format!("mxrs {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
    for args in [
        vec!["unknown"],
        vec!["--version", "extra"],
        vec!["help", "unknown"],
        vec!["help", "validate", "extra"],
        vec!["--commands", "--bogus"],
        vec!["--commands", "--json", "--json"],
    ] {
        assert!(
            !cli(&args).status.success(),
            "accepted invalid arguments {args:?}"
        );
    }
}

#[test]
fn caller_and_callee_queries_use_call_edges_without_duplicates_or_containment() {
    let (_directory, path) = fixture(false);
    let output = query("callers", &path, &["Sales.Save"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(text(&output), "Sales.Start\tmicroflow\n");
    let output = query("callees", &path, &["Sales.Start", "--json"]);
    assert!(output.status.success());
    let artifacts: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["qualified_name"], "Sales.Save");
    assert_eq!(text(&query("callers", &path, &["Sales.Unused"])), "");
    assert_eq!(text(&query("callees", &path, &["Sales"])), "");
    assert!(query("callees", &path, &["Sales.Start"]).status.success());
    assert!(
        query("callers", &path, &["Sales.Save", "--json"])
            .status
            .success()
    );
}

#[test]
fn describe_refs_impact_and_tree_resolve_real_artifacts_and_fail_on_typos() {
    let (_directory, path) = fixture(false);
    let output = query("describe", &path, &["Sales.Start", "--json"]);
    assert!(output.status.success());
    let details: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(details["artifact"]["qualified_name"], "Sales.Start");
    assert!(
        details["outgoing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reference| reference["relation"] == "calls")
    );
    assert!(text(&query("describe", &path, &["Sales.Start"])).contains("Sales.Save\tcalls"));
    for command in ["refs", "impact"] {
        assert!(
            query(command, &path, &["Sales.Save", "--json"])
                .status
                .success()
        );
        assert!(query(command, &path, &["Sales.Save"]).status.success());
    }
    for command in ["describe", "refs", "impact", "callers", "callees"] {
        let output = query(command, &path, &["Sales.Missing"]);
        assert!(
            !output.status.success(),
            "{command} silently accepted unknown artifact"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("unknown Mendix artifact"));
        assert!(
            !cli(&[command, "/missing.mpr", "Sales.Save"])
                .status
                .success()
        );
    }
    let output = query("tree", &path, &["Sales", "--json"]);
    assert!(output.status.success());
    let tree: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        tree["Sales"]["microflow"],
        serde_json::json!(["Sales.Save", "Sales.Start", "Sales.Unused"])
    );
    assert!(text(&query("tree", &path, &[])).contains("    Sales.Order/Number"));
    assert!(!query("tree", &path, &["Missing"]).status.success());
    assert!(!query("tree", &path, &["--bogus"]).status.success());
    assert!(!cli(&["tree", "/missing.mpr"]).status.success());
}

#[test]
fn lint_and_report_fail_for_cycles_and_disclose_their_analysis_boundary() {
    for cyclic in [false, true] {
        let (_directory, path) = fixture(cyclic);
        for command in ["lint", "report"] {
            let output = query(command, &path, &["--json"]);
            assert_eq!(output.status.success(), !cyclic, "{}", text(&output));
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["scope"], "explicit_reference_graph");
            assert!(!report["limitations"].as_array().unwrap().is_empty());
            assert_eq!(
                report["call_cycles"].as_array().unwrap().len(),
                usize::from(cyclic)
            );
            let output = query(command, &path, &[]);
            assert_eq!(output.status.success(), !cyclic);
            assert!(text(&output).contains("not a full Studio Pro"));
            assert!(!cli(&[command, "/missing.mpr"]).status.success());
        }
    }
}

#[test]
fn invalid_search_limits_are_errors_instead_of_silent_defaults() {
    let (_directory, path) = fixture(false);
    for limit in ["0", "-1", "invalid", "18446744073709551616"] {
        assert!(
            !query("search", &path, &["Sales", "--limit", limit])
                .status
                .success()
        );
    }
    assert!(
        !query("search", &path, &["Sales", "--limit"])
            .status
            .success()
    );
    assert!(
        !query("search", &path, &["--limit", "--json"])
            .status
            .success()
    );
    let output = query("search", &path, &["Sales", "--limit", "1", "--json"]);
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Vec<Value>>(&output.stdout)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn storage_commands_report_real_model_data_and_reject_unrecognized_arguments() {
    let (_directory, path) = fixture(false);
    for command in ["validate", "inspect"] {
        for suffix in [vec![], vec!["--json"]] {
            let output = query(command, &path, &suffix);
            assert!(output.status.success(), "{:?}", output.stderr);
            if !suffix.is_empty() {
                assert!(serde_json::from_slice::<Value>(&output.stdout).is_ok());
            }
        }
    }
    for command in ["validate", "inspect", "units", "modules", "export"] {
        assert!(
            !query(command, &path, &["--unknown"]).status.success(),
            "{command} ignored unknown flag"
        );
        assert!(!cli(&[command, "/missing.mpr"]).status.success());
    }
    assert!(text(&query("units", &path, &[])).contains("Microflows$Microflow"));
    assert!(text(&query("modules", &path, &[])).contains("Sales"));
    assert!(text(&query("sql", &path, &["SELECT COUNT(*) FROM Unit"])).contains("(1 rows)"));
    assert!(!query("sql", &path, &["DELETE FROM Unit"]).status.success());
    let external = _directory.path().join("forbidden-external.sqlite");
    let attach = format!(
        "ATTACH '{}' AS external",
        external.to_str().unwrap().replace('\'', "''")
    );
    assert!(!query("sql", &path, &[&attach]).status.success());
    assert!(!external.exists());
    let project = mxrs_model::Project::open(&path, true).unwrap();
    let id = &project.modules().unwrap()[0].id;
    assert!(text(&query("dump-unit", &path, &[id])).contains("Contents (hex)"));
    assert!(
        !query(
            "dump-unit",
            &path,
            &["00000000-0000-0000-0000-000000000000"]
        )
        .status
        .success()
    );
    assert!(!cli(&["dump-unit", "/missing.mpr", id]).status.success());
    for suffix in [
        vec![path.to_str().unwrap()],
        vec![path.to_str().unwrap(), "--json"],
    ] {
        assert!(query("compare", &path, &suffix).status.success());
    }
    assert!(!query("compare", &path, &["/missing.mpr"]).status.success());
    let (_other_directory, other) = fixture(true);
    for suffix in [
        vec![other.to_str().unwrap()],
        vec![other.to_str().unwrap(), "--json"],
    ] {
        assert!(!query("compare", &path, &suffix).status.success());
    }
    assert!(query("search", &path, &["order"]).status.success());
    assert!(!cli(&["search", "/missing.mpr", "order"]).status.success());
}

#[test]
fn missing_values_do_not_turn_flags_into_new_project_names_or_write_output() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("must-not-exist");
    for flag in ["--version", "--json", "--unknown"] {
        let output = cli(&["new", flag, "--output", destination.to_str().unwrap()]);
        assert!(!output.status.success());
        assert!(!destination.exists());
    }
}

#[test]
fn query_commands_expose_supported_projection_and_fail_on_unsafe_or_invalid_dialects() {
    let (_directory, path) = fixture(false);
    for suffix in [vec![], vec!["--json"], vec!["--dialect", "ansi"]] {
        assert!(query("oql", &path, &suffix).status.success());
    }
    for command in ["oql", "translate-oql"] {
        assert!(
            !cli(&[command, "input", "--dialect", "unknown"])
                .status
                .success()
        );
    }
    assert!(!cli(&["oql", "/missing.mpr"]).status.success());
    for dialect in ["ansi", "postgresql", "sql_server"] {
        let output = cli(&[
            "translate-oql",
            "SELECT o/Number FROM Sales.Order o",
            "--dialect",
            dialect,
        ]);
        assert!(output.status.success(), "{:?}", output.stderr);
        assert!(text(&output).contains("SELECT"));
    }
    assert!(
        !cli(&["translate-oql", "DELETE FROM Sales.Order"])
            .status
            .success()
    );
}

#[test]
fn cargo_import_export_scaffold_and_java_generation_have_real_filesystem_effects() {
    let (directory, path) = fixture(false);
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .unwrap();
    let imported = directory.path().join("imported");
    let suffix = [
        "--output",
        imported.to_str().unwrap(),
        "--mxrs-workspace",
        workspace.to_str().unwrap(),
    ];
    let output = query("import", &path, &suffix);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(imported.join("Cargo.toml").is_file());
    assert!(!query("import", &path, &suffix).status.success());
    let export = query("export", &path, &[]);
    assert!(export.status.success());
    assert!(text(&export).contains("Domain model only"));
    let output = query("export", &path, &["--allow-lossy"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(text(&output).contains("Order"));
    let exported = directory.path().join("export.rs");
    assert!(
        query(
            "export",
            &path,
            &["--allow-lossy", "-o", exported.to_str().unwrap()]
        )
        .status
        .success()
    );
    assert!(exported.is_file());
    assert!(
        !query(
            "export",
            &path,
            &["--allow-lossy", "-o", directory.path().to_str().unwrap()]
        )
        .status
        .success()
    );
    let generated = directory.path().join("scaffold");
    let args = [
        "new",
        "New App",
        "--output",
        generated.to_str().unwrap(),
        "--mxrs-workspace",
        workspace.to_str().unwrap(),
    ];
    assert!(cli(&args).status.success());
    assert!(!cli(&args).status.success());
    assert!(generated.join("src/main.rs").is_file());
    assert!(
        !cli(&[
            "new",
            "Invalid",
            "--output",
            directory.path().join("invalid").to_str().unwrap(),
            "--version",
            "not-a-version"
        ])
        .status
        .success()
    );
    assert!(query("javagen", &path, &[]).status.success());
    let proxies = directory.path().join("proxies");
    std::fs::create_dir_all(proxies.join("javasource/custom")).unwrap();
    std::fs::write(
        proxies.join("javasource/custom/UsesOrder.java"),
        "package custom; import sales.proxies.Order; class UsesOrder { Order order; }",
    )
    .unwrap();
    assert!(
        query(
            "javagen",
            &path,
            &["--project-root", proxies.to_str().unwrap()]
        )
        .status
        .success()
    );
    assert!(
        proxies
            .join("javasource/sales/proxies/Order.java")
            .is_file()
    );
    assert!(!cli(&["javagen", "/missing.mpr"]).status.success());
}

#[test]
fn package_commands_build_verify_and_reject_corrupt_archives() {
    let (directory, path) = fixture(false);
    let web = directory.path().join("web");
    mxrs_materializers::materialize_mpr(&path, &web).unwrap();
    let archive = directory.path().join("application.tar");
    let output = query(
        "package",
        &path,
        &[
            "--web",
            web.to_str().unwrap(),
            "-o",
            archive.to_str().unwrap(),
        ],
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(
        cli(&["verify-package", archive.to_str().unwrap()])
            .status
            .success()
    );
    assert!(
        !query(
            "package",
            &path,
            &[
                "--web",
                "/missing",
                "--output",
                directory.path().join("invalid.tar").to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    let corrupt = directory.path().join("corrupt.tar");
    std::fs::write(&corrupt, b"not an archive").unwrap();
    assert!(
        !cli(&["verify-package", corrupt.to_str().unwrap()])
            .status
            .success()
    );
}
