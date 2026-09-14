use std::process::Command;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn export(covered: u64) -> Vec<u8> {
    let metric = json!({"count":100,"covered":covered,"percent":covered as f64});
    let summary = json!({"lines":metric,"functions":metric,"regions":metric,"branches":metric});
    serde_json::to_vec(&json!({
        "type":"llvm.coverage.json.export", "version":"3.1.0",
        "data":[{"totals":summary,"files":[{"filename":"src/lib.rs","summary":summary}]}]
    }))
    .unwrap()
}

#[test]
fn release_coverage_failure_preserves_machine_readable_evidence_and_reports_nonzero_status() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("coverage.json");
    let bytes = export(82);
    std::fs::write(&path, &bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "coverage-gate",
            path.to_str().unwrap(),
            "--require-complete",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["gate"], "release_100_percent");
    assert_eq!(
        report["source_sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    assert_eq!(report["coverage"]["development_passes"], true);
    assert_eq!(report["coverage"]["coverage_complete"], false);
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("branches:"));
    assert!(error.contains("required 100%"));
}

#[test]
fn complete_coverage_and_development_coverage_have_distinct_successful_gates() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("coverage.json");
    for (covered, flags) in [(100, vec!["--require-complete"]), (82, vec![])] {
        std::fs::write(&path, export(covered)).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["coverage-gate", path.to_str().unwrap()])
            .args(flags)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("this does not establish Studio Pro parity")
        );
    }
}

#[test]
fn previous_development_baseline_no_longer_passes_the_ratchet() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("coverage.json");
    std::fs::write(&path, export(70)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["coverage-gate", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["coverage"]["development_passes"], false);
    let error = String::from_utf8(output.stderr).unwrap();
    for name in ["lines", "functions", "regions"] {
        assert!(error.contains(&format!("{name}:")));
    }
    assert!(!error.contains("branches:"));
}

#[test]
fn invalid_command_arguments_exit_without_a_rust_panic() {
    for arguments in [
        vec![],
        vec!["unknown"],
        vec!["fixture-gen"],
        vec!["coverage-gate", "--json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    }
}
