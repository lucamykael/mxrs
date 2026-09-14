use std::path::Path;
use std::process::{Command, Output};

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

fn cargo_mxrs(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-mxrs"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn cargo_plugin_discovers_commands_with_and_without_cargo_prefix() {
    for prefix in [vec![], vec!["mxrs"]] {
        for suffix in [
            vec![],
            vec!["--help"],
            vec!["-h"],
            vec!["help"],
            vec!["--commands"],
            vec!["--version"],
            vec!["-V"],
            vec!["-v"],
        ] {
            let mut args = prefix.clone();
            args.extend(suffix);
            let output = cargo_mxrs(&args);
            assert!(output.status.success(), "{args:?}");
            assert!(!output.stdout.is_empty());
            assert!(output.stderr.is_empty());
        }
    }
    for name in ["build", "diff", "frontend-dev", "package"] {
        for args in [vec![name, "--help"], vec![name, "-h"], vec!["help", name]] {
            let output = cargo_mxrs(&args);
            assert!(output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stdout).contains(&format!("cargo mxrs {name}"))
            );
        }
    }
    for args in [
        vec!["unknown"],
        vec!["help", "unknown"],
        vec!["unknown", "--help"],
        vec!["build"],
        vec!["package"],
        vec!["build", "--output", "--release"],
        vec!["build", "--output", "out.mpr", "--bogus"],
        vec!["diff", "--bogus"],
        vec!["frontend-dev", "--bogus"],
    ] {
        assert!(
            !cargo_mxrs(&args).status.success(),
            "invalid arguments accepted: {args:?}"
        );
    }
}

#[test]
fn cargo_plugin_reports_engine_failures_and_materializes_real_frontend_sources() {
    let directory = tempfile::tempdir().unwrap();
    let frontend = directory.path().join("frontend");
    let output = cargo_mxrs(&["frontend-dev", "--output", frontend.to_str().unwrap()]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(frontend.join("package-lock.json").is_file());
    assert!(frontend.join("src/main.tsx").is_file());
    let file = directory.path().join("file");
    std::fs::write(&file, "keep").unwrap();
    assert!(
        !cargo_mxrs(&["frontend-dev", "--output", file.to_str().unwrap()])
            .status
            .success()
    );
    for options in [vec![], vec!["--web-output", "web"]] {
        let mut args = vec![
            "build",
            "--manifest-path",
            "/missing.toml",
            "--output",
            "unused.mpr",
        ];
        args.extend(options);
        assert!(!cargo_mxrs(&args).status.success());
    }
    assert!(
        !cargo_mxrs(&[
            "diff",
            "--manifest-path",
            "/missing.toml",
            "--snapshot",
            "/missing-snapshot",
            "--offline"
        ])
        .status
        .success()
    );
    assert!(
        !cargo_mxrs(&[
            "package",
            "--mpr",
            "/missing.mpr",
            "--web",
            directory.path().to_str().unwrap(),
            "--output",
            directory.path().join("missing.tar").to_str().unwrap()
        ])
        .status
        .success()
    );
}

#[test]
fn cargo_build_diff_and_package_execute_a_generated_application_without_node_or_ruby() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    mxrs_scaffold::generate_project(
        &mxrs_scaffold::ProjectScaffold::new("Pipeline", "11.12.1", &source).dependency(
            mxrs_scaffold::MxrsDependency::Path(workspace.join("crates/app/mxrs")),
        ),
    )
    .unwrap();
    let manifest = source.join("Cargo.toml");
    let mpr = directory.path().join("built/Pipeline.mpr");
    let web = directory.path().join("public");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cargo-mxrs"))
            .args(args)
            .env(
                "CARGO_TARGET_DIR",
                nested_cargo::target_dir(workspace.join("target")),
            )
            .output()
            .unwrap()
    };
    let output = run(&[
        "mxrs",
        "build",
        "--manifest-path",
        manifest.to_str().unwrap(),
        "--output",
        mpr.to_str().unwrap(),
        "--web-output",
        web.to_str().unwrap(),
        "--offline",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(mxrs_cli::validate::validate(&mpr).unwrap().is_valid());
    let project = mxrs_model::Project::open(&mpr, true).unwrap();
    let index = mxrs_semantic::SemanticIndex::build(&project).unwrap();
    assert_eq!(
        index.require("Main.ApplicationLayout").unwrap().kind,
        mxrs_semantic::ArtifactKind::Layout
    );
    assert!(index.analyze().valid(), "{:?}", index.diagnostics());
    assert_eq!(
        project.navigation().unwrap().profiles[0]
            .home_page
            .as_deref(),
        Some("Main.Home")
    );
    assert!(web.join("model.json").is_file());
    let imported = directory.path().join("imported");
    mxrs_exporter::import_cargo_project(&mpr, &imported, Some(workspace)).unwrap();
    let imported_manifest = imported.join("Cargo.toml");
    for json in [false, true] {
        let mut args = vec![
            "diff",
            "--manifest-path",
            imported_manifest.to_str().unwrap(),
            "--offline",
        ];
        if json {
            args.push("--json");
        }
        let output = run(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if json {
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["changes"], serde_json::json!([]));
        }
    }
    for action in ["plan", "check"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mxrs"))
            .args(["migrate", action])
            .arg(&imported)
            .arg("--json")
            .env(
                "CARGO_TARGET_DIR",
                nested_cargo::target_dir(workspace.join("target")),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["clean"], true);
        assert_eq!(report["changed"], serde_json::json!([]));
    }
    let archive = directory.path().join("pipeline.tar");
    let output = run(&[
        "package",
        "--mpr",
        mpr.to_str().unwrap(),
        "--web",
        web.to_str().unwrap(),
        "--output",
        archive.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !mxrs_packager::verify_package(&archive)
            .unwrap()
            .files
            .is_empty()
    );
}
