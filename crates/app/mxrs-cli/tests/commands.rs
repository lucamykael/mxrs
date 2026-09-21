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

fn cli_with_env(args: &[&str], key: &str, value: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mxrs"))
        .args(args)
        .env(key, value)
        .output()
        .unwrap()
}

fn cli_with_envs(args: &[&str], environment: &[(&str, &std::ffi::OsStr)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mxrs"))
        .args(args)
        .envs(environment.iter().copied())
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
        result_type: None,
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
        if !matches!(name, "changelog" | "doctor" | "help" | "env") {
            assert!(
                !cli(&[name]).status.success(),
                "{name} accepted no arguments"
            );
        }
    }
    for required in [
        "validate",
        "changelog",
        "import",
        "export",
        "callees",
        "callers",
        "describe",
        "tree",
        "lint",
        "report",
        "db",
        "team-server",
        "portability",
        "test",
        "functional-test",
        "functional-instrument",
        "evaluate",
        "evaluation",
        "validation",
        "integration",
        "protocols",
        "ci",
    ] {
        assert!(names.contains(required));
    }
}

#[test]
fn changelog_rejects_an_unsafe_version_without_opening_the_network() {
    let output = cli(&["changelog", "../latest"]);
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("invalid release version"));
    assert!(error.contains("0.4.0"));
}

#[test]
fn team_server_login_stores_only_a_pat_pointer_and_status_stays_offline() {
    let directory = tempfile::tempdir().unwrap();
    let pat = directory.path().join("pat");
    std::fs::write(&pat, "secret-never-copied").unwrap();
    let config = directory.path().join("config");
    let output = cli_with_env(
        &[
            "team-server",
            "login",
            "--pat-file",
            pat.to_str().unwrap(),
            "--json",
        ],
        "XDG_CONFIG_HOME",
        &config,
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    let stored = std::fs::read_to_string(config.join("mxrs/credentials")).unwrap();
    assert!(stored.contains(pat.to_str().unwrap()));
    assert!(!stored.contains("secret-never-copied"));

    let repository = directory.path().join("repository");
    std::fs::create_dir(&repository).unwrap();
    for arguments in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "https://git.api.mendix.com/12345678-1234-abcd-9876-1234567890ab.git",
        ],
    ] {
        assert!(
            Command::new("git")
                .args(arguments)
                .current_dir(&repository)
                .status()
                .unwrap()
                .success()
        );
    }
    let output = cli(&[
        "team-server",
        "status",
        repository.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["repository_url"],
        "https://git.api.mendix.com/12345678-1234-abcd-9876-1234567890ab.git"
    );
    assert!(!cli(&["team-server", "status", "/missing"]).status.success());
}

#[test]
fn db_rejects_invalid_ports_and_invalid_projects_before_contacting_docker() {
    for port in ["0", "65536", "invalid"] {
        assert!(
            !cli(&["db", "status", "/missing.mpr", "--port", port])
                .status
                .success()
        );
    }
    assert!(!cli(&["db", "status", "/missing.mpr"]).status.success());
    assert!(!cli(&["db", "sql", "/missing.mpr"]).status.success());
}

#[cfg(unix)]
#[test]
fn db_cli_runs_the_owned_lifecycle_without_putting_its_password_on_the_command_line() {
    use std::os::unix::fs::PermissionsExt;

    let (directory, path) = fixture(false);
    let binary_directory = directory.path().join("bin");
    let fake_root = directory.path().join("fake-docker");
    let state_root = directory.path().join("state");
    std::fs::create_dir_all(&binary_directory).unwrap();
    std::fs::create_dir_all(&fake_root).unwrap();
    let docker = binary_directory.join("docker");
    std::fs::write(
        &docker,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$MXRS_FAKE_DOCKER_ROOT/trace"
case "$1 ${2-}" in
  "container inspect")
    if [ -f "$MXRS_FAKE_DOCKER_ROOT/container" ]; then
      printf 'true|%s|%s\n' "$(cat "$MXRS_FAKE_DOCKER_ROOT/key")" "$(cat "$MXRS_FAKE_DOCKER_ROOT/container")"
    else
      printf 'no such container\n' >&2
      exit 1
    fi ;;
  "volume inspect")
    if [ -f "$MXRS_FAKE_DOCKER_ROOT/volume" ]; then
      printf 'true|%s|present\n' "$(cat "$MXRS_FAKE_DOCKER_ROOT/key")"
    else
      printf 'no such volume\n' >&2
      exit 1
    fi ;;
  "volume create")
    for argument in "$@"; do
      case "$argument" in io.mxrs.project=*) printf '%s' "${argument#*=}" > "$MXRS_FAKE_DOCKER_ROOT/key" ;; esac
    done
    : > "$MXRS_FAKE_DOCKER_ROOT/volume"
    printf 'volume\n' ;;
  "volume rm")
    rm "$MXRS_FAKE_DOCKER_ROOT/volume"
    printf 'removed\n' ;;
  "run --detach")
    printf 'running' > "$MXRS_FAKE_DOCKER_ROOT/container"
    printf 'container\n' ;;
  "start "*)
    printf 'running' > "$MXRS_FAKE_DOCKER_ROOT/container"
    printf 'started\n' ;;
  "stop "*)
    printf 'exited' > "$MXRS_FAKE_DOCKER_ROOT/container"
    printf 'stopped\n' ;;
  "rm --force")
    rm "$MXRS_FAKE_DOCKER_ROOT/container"
    printf 'removed\n' ;;
  *) printf 'unexpected fake Docker call: %s\n' "$*" >&2; exit 2 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut path_value = binary_directory.into_os_string();
    path_value.push(":");
    path_value.push(std::env::var_os("PATH").unwrap_or_default());
    let environment = [
        ("PATH", path_value.as_os_str()),
        ("MXRS_STATE_DIR", state_root.as_os_str()),
        ("MXRS_FAKE_DOCKER_ROOT", fake_root.as_os_str()),
    ];
    let run = |action: &str| {
        cli_with_envs(
            &[
                "db",
                action,
                path.to_str().unwrap(),
                "--port",
                "55439",
                "--json",
            ],
            &environment,
        )
    };
    for (action, expected) in [("up", "running"), ("status", "running"), ("down", "exited")] {
        let output = run(action);
        assert!(output.status.success(), "{action}: {:?}", output.stderr);
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["container_state"],
            expected
        );
    }
    for action in ["credentials", "url"] {
        let output = run(action);
        assert!(output.status.success(), "{action}: {:?}", output.stderr);
    }
    let secret = std::fs::read_to_string(
        std::fs::read_dir(state_root.join("mxrs/db"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "password")
            })
            .unwrap(),
    )
    .unwrap();
    assert!(
        !std::fs::read_to_string(fake_root.join("trace"))
            .unwrap()
            .contains(secret.trim())
    );
    let destroyed = run("destroy");
    assert!(destroyed.status.success(), "{:?}", destroyed.stderr);
    assert_eq!(
        serde_json::from_slice::<Value>(&destroyed.stdout).unwrap()["initialized"],
        false
    );
}

#[test]
fn semantic_cache_cli_warms_hits_and_clears_outside_the_model() {
    let (directory, path) = fixture(false);
    let cache = directory.path().join("cache");
    let run = |action: &str| {
        cli_with_env(
            &["cache", action, path.to_str().unwrap(), "--json"],
            "MXRS_CACHE_DIR",
            &cache,
        )
    };
    let warm = run("warm");
    assert!(warm.status.success(), "{:?}", warm.stderr);
    let report: Value = serde_json::from_slice(&warm.stdout).unwrap();
    assert_eq!(report["present"], true);
    assert_eq!(report["hit"], true);
    assert!(!path.with_extension("cache").exists());

    let status = run("status");
    assert!(status.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap()["hit"],
        true
    );
    let clear = run("clear");
    assert!(clear.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&clear.stdout).unwrap()["removed"],
        1
    );
}

fn write_mda(path: &Path, web: &[u8]) {
    use std::io::Write;

    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    archive.start_file("model/metadata.json", options).unwrap();
    archive
        .write_all(br#"{"RuntimeVersion":"11.12.1","ProjectName":"CLI"}"#)
        .unwrap();
    archive.start_file("web/index.html", options).unwrap();
    archive.write_all(web).unwrap();
    archive.finish().unwrap();
}

#[test]
fn mda_cli_inspects_metadata_and_compares_content() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left.mda");
    let right = directory.path().join("right.mda");
    write_mda(&left, b"left");
    write_mda(&right, b"right");
    let inspect = cli(&["mda", "inspect", left.to_str().unwrap(), "--json"]);
    assert!(inspect.status.success(), "{:?}", inspect.stderr);
    let report: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(report["metadata"]["ProjectName"], "CLI");
    assert_eq!(report["files"], 2);
    let compare = cli(&[
        "mda",
        "compare",
        left.to_str().unwrap(),
        right.to_str().unwrap(),
    ]);
    assert!(compare.status.success(), "{:?}", compare.stderr);
    assert!(text(&compare).contains("changed\tweb/index.html"));
    assert!(!cli(&["mda", "inspect", "/missing.mda"]).status.success());
}

#[test]
fn env_layers_profiles_and_reports_only_key_names() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join(".env"), "BASE_SECRET=base-value\n").unwrap();
    std::fs::create_dir_all(directory.path().join("config/environments")).unwrap();
    std::fs::write(
        directory.path().join("config/environments/qa.env"),
        "QA_SECRET=qa-value\n",
    )
    .unwrap();
    let output = cli(&[
        "env",
        directory.path().to_str().unwrap(),
        "--environment=qa",
        "--json",
    ]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["environment"], "qa");
    assert!(
        payload["keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "BASE_SECRET")
    );
    assert!(
        payload["keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "QA_SECRET")
    );
    let stdout = text(&output);
    assert!(!stdout.contains("base-value"));
    assert!(!stdout.contains("qa-value"));
    assert!(!cli(&["env", ".", "extra"]).status.success());
    assert!(!cli(&["env", "--environment=../prod"]).status.success());
}

#[test]
fn doctor_distinguishes_required_project_state_from_optional_tools() {
    let directory = tempfile::tempdir().unwrap();
    for relative in ["Cargo.toml", "src/lib.rs", "src/domain/mod.rs"] {
        let path = directory.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    let output = cli(&["doctor", directory.path().to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["valid"], true);
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| { check["name"] == "mpr" && check["status"] == "warning" })
    );

    std::fs::remove_file(directory.path().join("src/lib.rs")).unwrap();
    assert!(
        !cli(&["doctor", directory.path().to_str().unwrap()])
            .status
            .success()
    );
    assert!(!cli(&["doctor", ".", "extra"]).status.success());
}

#[test]
fn preflight_audits_a_real_mpr_and_has_machine_readable_inventory() {
    let (_directory, path) = fixture(false);
    let output = query("preflight", &path, &["--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["compatible"], true);
    assert_eq!(report["mendix_version"], "11.12.1");
    assert!(report["stats"]["units"].as_u64().unwrap() > 0);
    assert!(!cli(&["preflight", "/missing.mpr"]).status.success());
}

#[test]
fn evaluation_runs_typed_checks_and_protocol_audit_fails_closed() {
    let (directory, path) = fixture(false);
    let definition = directory.path().join("evaluation.json");
    std::fs::write(
        &definition,
        r#"{"checks":[{"type":"no_call_cycles"},{"type":"no_missing_internal_references"},{"type":"artifact","name":"Sales.Order","kind":"entity"},{"type":"reference","from":"Sales.Start","to":"Sales.Save","relation":"calls"},{"type":"artifact","name":"Sales.Missing","severity":"warning"}]}"#,
    )
    .unwrap();
    let output = cli(&[
        "evaluate",
        path.to_str().unwrap(),
        definition.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["passed"], true);
    assert_eq!(result["score"], 80.0);

    std::fs::write(
        &definition,
        r#"{"checks":[{"type":"artifact","name":"Sales.Missing"}]}"#,
    )
    .unwrap();
    assert!(
        !cli(&[
            "evaluate",
            path.to_str().unwrap(),
            definition.to_str().unwrap()
        ])
        .status
        .success()
    );

    let output = query("protocols", &path, &["--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let audit: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(audit["connectors"].as_array().unwrap().is_empty());
    assert!(
        audit["unknown_marketplace_modules"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let sales = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let mut document = mpr.parse_contents(&unit).ok()?;
            (document.get_str("$Type").ok() == Some("Projects$Module")
                && document.get_str("Name").ok() == Some("Sales"))
            .then(|| {
                document.insert("FromAppStore", true);
                document.insert("AppStoreGuid", "not-in-evidence-registry");
                (unit.unit_id, document)
            })
        })
        .unwrap();
    mpr.update_unit(&sales.0, sales.1).unwrap();
    drop(mpr);
    let output = query("protocols", &path, &["--json"]);
    let audit: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(audit["unknown_marketplace_modules"][0], "Sales");
}

#[test]
fn portability_separates_exact_storage_from_typed_authoring_and_verifies_documents() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Portable.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.enumeration("Status", |enumeration| {
            enumeration.value("Open").captions = vec![("en_US".to_string(), "Open".to_string())];
        });
        module.constant("Limit", |constant| {
            constant
                .value_type(mxrs_ir::ConstantType::Integer)
                .value("10");
        });
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();

    let output = query("portability", &path, &["--verify-round-trip", "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["portability"]["model_lossless"], true);
    assert_eq!(report["portability"]["fully_typed"], false);
    assert_eq!(report["round_trip"]["candidate_units"], 2);
    assert_eq!(report["round_trip"]["byte_identical_units"], 2);
    assert_eq!(report["round_trip"]["passed"], true);

    let strict = query("portability", &path, &["--require-typed"]);
    assert!(!strict.status.success());
    assert!(
        String::from_utf8(strict.stderr)
            .unwrap()
            .contains("still require model/imported")
    );
    assert!(!cli(&["portability", "/missing.mpr"]).status.success());
}

#[test]
fn functional_test_plan_validates_real_mpr_targets_and_refuses_fake_execution() {
    let (directory, path) = fixture(false);
    let suite = directory.path().join("functional.json");
    std::fs::write(
        &suite,
        r#"{"tests":[{"name":"starts","call":"Sales.Start","before":{"call":"Sales.Save"},"after":{"call":"Sales.Unused"},"expect":{"count":[{"entity":"Sales.Order","equals":0}]}}]}"#,
    )
    .unwrap();

    let output = cli(&[
        "test",
        path.to_str().unwrap(),
        suite.to_str().unwrap(),
        "--plan",
        "--json",
    ]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["execution_supported"], false);
    assert_eq!(plan["tests"][0]["target"], "Sales.Start");

    let instrument = cli(&[
        "functional-instrument",
        path.to_str().unwrap(),
        suite.to_str().unwrap(),
        "--json",
    ]);
    assert!(instrument.status.success(), "{:?}", instrument.stderr);
    let report: Value = serde_json::from_slice(&instrument.stdout).unwrap();
    assert_eq!(report["runner"], "MxrsTests.RunAll");
    assert_eq!(report["tests"], 1);
    assert!(query("validate", &path, &[]).status.success());

    let project = mxrs_model::Project::open(&path, true).unwrap();
    let modules = project.modules().unwrap();
    let tests_module = modules
        .iter()
        .find(|module| module.name.as_deref() == Some("MxrsTests"))
        .unwrap();
    assert_eq!(tests_module.microflows.len(), 2);
    let action_types = tests_module
        .microflows
        .iter()
        .flat_map(|flow| &flow.objects)
        .filter_map(|object| object.get_document("Action").ok())
        .filter_map(|action| action.get_str("$Type").ok())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(action_types.contains("Microflows$RetrieveAction"));
    assert!(action_types.contains("Microflows$AggregateAction"));
    assert!(action_types.contains("Microflows$LogMessageAction"));
    drop(project);

    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    let settings = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            (document.get_str("$Type").ok() == Some("Settings$ProjectSettings")).then_some(document)
        })
        .unwrap();
    let model_settings = mxrs_bson::parse_array(
        settings
            .get_array("Settings")
            .ok()
            .map(std::vec::Vec::as_slice),
    )
    .items
    .into_iter()
    .find_map(|value| match value {
        mxrs_bson::Bson::Document(document)
            if document.get_str("$Type").ok() == Some("Settings$ModelSettings") =>
        {
            Some(document)
        }
        _ => None,
    })
    .unwrap();
    assert_eq!(
        model_settings.get_str("AfterStartupMicroflow").unwrap(),
        "MxrsTests.RunAll"
    );
    drop(mpr);
    assert!(
        !cli(&[
            "functional-instrument",
            path.to_str().unwrap(),
            suite.to_str().unwrap(),
        ])
        .status
        .success()
    );

    let execute = cli(&["test", path.to_str().unwrap(), suite.to_str().unwrap()]);
    assert!(!execute.status.success());
    assert!(
        String::from_utf8(execute.stderr)
            .unwrap()
            .contains("not implemented")
    );

    std::fs::write(
        &suite,
        r#"{"tests":[{"name":"missing","call":"Sales.Missing"}]}"#,
    )
    .unwrap();
    assert!(
        !cli(&[
            "test",
            path.to_str().unwrap(),
            suite.to_str().unwrap(),
            "--plan"
        ])
        .status
        .success()
    );
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
    assert!(text(&query("tree", &path, &[])).contains("    Sales.Order.Number"));
    assert!(query("tree", &path, &["Missing"]).status.success());
    assert!(text(&query("tree", &path, &["Missing"])).is_empty());
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
    assert!(text(&export).contains("Editable domain model declarations"));
    let output = query("export", &path, &["--allow-lossy"]);
    assert!(!output.status.success());
    let exported = directory.path().join("export.rs");
    assert!(
        query("export", &path, &["-o", exported.to_str().unwrap()])
            .status
            .success()
    );
    assert!(exported.is_file());
    assert!(
        !query("export", &path, &["-o", directory.path().to_str().unwrap()])
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

#[test]
fn renaming_previews_by_default_and_writes_only_under_apply() {
    let (_directory, path) = fixture(false);
    let file = path.to_str().unwrap();

    let preview = cli(&["rename", file, "Sales.Save", "Persist"]);
    assert!(preview.status.success(), "{:?}", preview.stderr);
    let rendered = text(&preview);
    assert!(rendered.contains("=> \"Persist\""), "{rendered}");
    assert!(rendered.contains("[mxrs] Preview:"), "{rendered}");
    // A preview must not have touched the file.
    assert!(text(&cli(&["modules", file])).contains("Sales"));
    assert!(text(&cli(&["describe", file, "Sales.Save"])).contains("Sales.Save"));

    let applied = cli(&["rename", file, "Sales.Save", "Persist", "--apply"]);
    assert!(applied.status.success(), "{:?}", applied.stderr);
    assert!(text(&applied).contains("[mxrs] Renamed:"));
    // The caller that referenced the old name now references the new one.
    let callees = text(&cli(&["callees", file, "Sales.Start"]));
    assert!(callees.contains("Sales.Persist"), "{callees}");
    assert!(!callees.contains("Sales.Save"), "{callees}");
    assert!(!cli(&["describe", file, "Sales.Save"]).status.success());
}

#[test]
fn a_blocked_removal_exits_nonzero_and_a_safe_one_succeeds() {
    let (_directory, path) = fixture(false);
    let file = path.to_str().unwrap();

    // `Sales.Save` is called by `Sales.Start`, so removing it would dangle.
    let blocked = cli(&["remove", file, "Sales.Save"]);
    assert!(!blocked.status.success());
    let rendered = text(&blocked);
    assert!(rendered.contains("[mxrs] Blocked removal"), "{rendered}");
    assert!(rendered.contains("Sales.Start"), "{rendered}");

    // Even with --apply, a blocked removal must not delete anything.
    assert!(
        !cli(&["remove", file, "Sales.Save", "--apply"])
            .status
            .success()
    );
    assert!(cli(&["describe", file, "Sales.Save"]).status.success());

    let safe = cli(&["remove", file, "Sales.Unused"]);
    assert!(safe.status.success(), "{:?}", safe.stderr);
    assert!(text(&safe).contains("[mxrs] Safe removal preview"));
    assert!(cli(&["describe", file, "Sales.Unused"]).status.success());

    let applied = cli(&["remove", file, "Sales.Unused", "--apply"]);
    assert!(applied.status.success(), "{:?}", applied.stderr);
    assert!(text(&applied).contains("[mxrs] Removed"));
    assert!(!cli(&["describe", file, "Sales.Unused"]).status.success());
}

#[test]
fn refactoring_commands_render_json_and_reject_malformed_invocations() {
    let (_directory, path) = fixture(false);
    let file = path.to_str().unwrap();

    let output = cli(&["rename", file, "Sales.Save", "Persist", "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["target"], "Sales.Persist");
    assert_eq!(document["applied"], false);
    assert!(
        document["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| {
                change["before"] == "Sales.Save" && change["after"] == "Sales.Persist"
            })
    );

    let output = cli(&["remove", file, "Sales.Save", "--json"]);
    assert!(!output.status.success());
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["safe"], false);
    assert_eq!(document["artifact"]["kind"], "microflow");

    let output = cli(&["move", file, "Sales.Save", "Sales", "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["before_container"], document["after_container"]);

    // Wrong arity and unknown artifacts are usage errors, not panics.
    for arguments in [
        vec!["rename", file, "Sales.Save"],
        vec!["remove", file],
        vec!["move", file, "Sales.Save"],
        vec!["rename", file, "Sales.NoSuch", "Whatever"],
    ] {
        let output = cli(&arguments);
        assert!(!output.status.success(), "{arguments:?} was accepted");
        assert!(String::from_utf8_lossy(&output.stderr).contains("[mxrs] error:"));
    }
}

/// The marketplace command is the only one that would reach the network, so
/// these assert the paths that must work *without* a credential: usage errors,
/// and a missing token reported as such instead of as an HTTP failure.
fn marketplace(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mxrs"))
        .args(args)
        // Cleared explicitly: a developer with a token in their environment
        // must not turn these into live calls.
        .env_remove("MXRS_MENDIX_PAT")
        .env_remove("MXRS_MENDIX_PAT_FILE")
        .output()
        .unwrap()
}

#[test]
fn marketplace_without_a_credential_says_so_instead_of_failing_at_the_network() {
    let output = marketplace(&["marketplace", "search", "Community Commons"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no Mendix credential"), "{stderr}");
    assert!(stderr.contains("MXRS_MENDIX_PAT"), "{stderr}");
    // The message must name the variables without inventing a default path to
    // read a secret from.
    assert!(!stderr.contains(".ssh"), "{stderr}");
}

#[test]
fn marketplace_rejects_unknown_subcommands_and_wrong_arity_before_touching_credentials() {
    for arguments in [
        vec!["marketplace"],
        vec!["marketplace", "install", "170"],
        vec!["marketplace", "search"],
        vec!["marketplace", "show", "170", "extra"],
        vec!["marketplace", "download"],
    ] {
        let output = marketplace(&arguments);
        assert!(!output.status.success(), "{arguments:?} was accepted");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("usage: mxrs marketplace"),
            "{arguments:?}: {stderr}"
        );
        // A usage error must not be reported as a missing credential: the
        // invocation is wrong regardless of whether a token exists.
        assert!(
            !stderr.contains("no Mendix credential"),
            "{arguments:?}: {stderr}"
        );
    }
}

#[test]
fn marketplace_install_needs_no_credential_and_previews_before_writing() {
    // `install` works on a local .mpk, so unlike the other subcommands it must
    // not demand a token. It still has to fail cleanly on missing inputs.
    let output = marketplace(&[
        "marketplace",
        "install",
        "/nonexistent.mpk",
        "/nonexistent.mpr",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("package not found"), "{stderr}");
    assert!(!stderr.contains("no Mendix credential"), "{stderr}");
}

#[test]
fn marketplace_is_discoverable_and_its_options_are_validated() {
    let listed = text(&cli(&["--commands"]));
    assert!(listed.contains("marketplace"), "{listed}");
    assert!(cli(&["help", "marketplace"]).status.success());

    // An unknown option is a usage error rather than being passed through as
    // a positional argument.
    let output = marketplace(&["marketplace", "search", "x", "--not-an-option"]);
    assert!(!output.status.success());
}

#[test]
fn widgets_actions_validate_their_inputs_before_touching_any_toolchain() {
    let bare = cli(&["widgets"]);
    assert!(!bare.status.success());
    let stderr = String::from_utf8_lossy(&bare.stderr);
    assert!(stderr.contains("Usage: mxrs widgets"), "{stderr}");

    let unknown = cli(&["widgets", "publish"]);
    assert!(!unknown.status.success());
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("unknown widgets action"), "{stderr}");

    let hostile = cli(&["widgets", "new", "; rm -rf /"]);
    assert!(!hostile.status.success());
    let stderr = String::from_utf8_lossy(&hostile.stderr);
    assert!(stderr.contains("widget name"), "{stderr}");

    let missing = cli(&["widgets", "build", "/nonexistent-widget"]);
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("widget package not found"), "{stderr}");

    let no_project = cli(&["widgets", "sync", "/nonexistent-project", "/tmp/out.mpr"]);
    assert!(!no_project.status.success());
    let stderr = String::from_utf8_lossy(&no_project.stderr);
    assert!(stderr.contains("does not exist"), "{stderr}");
}

#[test]
fn update_flags_are_mutually_exclusive_and_arguments_are_validated() {
    let both = cli(&["update", "--check", "--changelog"]);
    assert!(!both.status.success());
    let stderr = String::from_utf8_lossy(&both.stderr);
    assert!(
        stderr.contains("only one of --check or --changelog"),
        "{stderr}"
    );

    let extra = cli(&["update", "now"]);
    assert!(!extra.status.success());
    let stderr = String::from_utf8_lossy(&extra.stderr);
    assert!(stderr.contains("Usage: mxrs update"), "{stderr}");

    let unknown = cli(&["update", "--force"]);
    assert!(!unknown.status.success());
}

#[test]
fn diagram_er_refuses_the_browser_lifecycle_and_validates_arguments() {
    for action in ["up", "down", "status", "destroy", "__serve"] {
        let output = cli(&["diagram-er", action, "/tmp/x.mpr"]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("not ported"), "{action}: {stderr}");
    }
    let missing = cli(&["diagram-er", "/nonexistent.mpr"]);
    assert!(!missing.status.success());
    let bare = cli(&["diagram-er"]);
    assert!(!bare.status.success());
    let stderr = String::from_utf8_lossy(&bare.stderr);
    assert!(stderr.contains("Usage: mxrs diagram-er"), "{stderr}");
}

#[test]
fn marketplace_lifecycle_actions_validate_offline_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_str().unwrap().to_string();

    let listed = cli(&["marketplace", "list", "--target-root", &root]);
    assert!(listed.status.success());
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("0 package(s)"), "{stdout}");

    let removed = cli(&["marketplace", "remove", "Ghost", "--target-root", &root]);
    assert!(!removed.status.success());
    let stderr = String::from_utf8_lossy(&removed.stderr);
    assert!(stderr.contains("not installed"), "{stderr}");

    let dependencies = cli(&[
        "marketplace",
        "dependencies",
        "Ghost",
        "--target-root",
        &root,
    ]);
    assert!(!dependencies.status.success());
    let stderr = String::from_utf8_lossy(&dependencies.stderr);
    assert!(stderr.contains("not installed"), "{stderr}");
}
