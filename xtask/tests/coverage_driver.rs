//! Tests orchestration with a fake Cargo/readelf boundary, never fabricating
//! a measured workspace result. Real coverage is run by the documented driver.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_driver(directory: &Path, mode: &str) -> Output {
    let fake_bin = directory.join("bin");
    std::fs::create_dir_all(&fake_bin).unwrap();
    executable(
        &fake_bin.join("cargo"),
        r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$TEST_DRIVER_LOG"
if [[ "$*" == *"--no-report"* ]]; then
  [[ "$CARGO_LLVM_COV_TARGET_DIR" != "$MXRS_TEST_CARGO_TARGET_DIR" ]]
  [[ "$CARGO_LLVM_COV_TARGET_DIR" == "$MXRS_COVERAGE_TMPDIR"/* ]]
  [[ "$MXRS_TEST_CARGO_TARGET_DIR" == "$MXRS_COVERAGE_TMPDIR"/* ]]
  [[ -z ${CARGO_BUILD_BUILD_DIR:-} ]]
  [[ -z ${LLVM_COV_FLAGS:-} ]]
  if [[ "$TEST_DRIVER_MODE" == fail ]]; then exit 37; fi
  mkdir -p "$CARGO_LLVM_COV_TARGET_DIR/debug/deps" "$MXRS_TEST_CARGO_TARGET_DIR/debug"
  if [[ "$TEST_DRIVER_MODE" != empty ]]; then
    touch "$CARGO_LLVM_COV_TARGET_DIR/debug/deps/mxrs-test" "$MXRS_TEST_CARGO_TARGET_DIR/debug/arbitrary-generated-app"
    chmod +x "$CARGO_LLVM_COV_TARGET_DIR/debug/deps/mxrs-test" "$MXRS_TEST_CARGO_TARGET_DIR/debug/arbitrary-generated-app"
  fi
elif [[ "$*" == *"llvm-cov report"* ]]; then
  [[ "$LLVM_COV_FLAGS" == *"arbitrary-generated-app"* ]]
  [[ "$LLVM_COV_FLAGS" == *"mxrs-test"* ]]
  [[ "$LLVM_COV_FLAGS" != *"stale-marker"* ]]
  printf '%s\n' "$LLVM_COV_FLAGS" >> "$TEST_DRIVER_LOG"
fi
"#,
    );
    executable(
        &fake_bin.join("readelf"),
        "#!/usr/bin/env bash\nprintf '__llvm_covmap\\n'\n",
    );
    executable(
        &fake_bin.join("rustc"),
        "#!/usr/bin/env bash\nprintf 'test-only toolchain boundary\\n'\n",
    );
    let mut path = vec![fake_bin];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    Command::new("bash")
        .arg(root.join("xtask/coverage.sh"))
        .arg(directory.join("coverage.json"))
        .current_dir(root)
        .env("PATH", std::env::join_paths(path).unwrap())
        .env("MXRS_COVERAGE_TMPDIR", directory)
        .env("CARGO_BUILD_BUILD_DIR", "stale-build-directory")
        .env("LLVM_COV_FLAGS", "stale-marker")
        .env("TEST_DRIVER_LOG", directory.join("commands.log"))
        .env("TEST_DRIVER_MODE", mode)
        .env_remove("GITHUB_OUTPUT")
        .output()
        .unwrap()
}

#[test]
fn fresh_measurement_keeps_nested_application_maps_and_has_no_source_exclusions() {
    let directory = tempfile::tempdir().unwrap();
    let output = run_driver(directory.path(), "success");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let second = run_driver(directory.path(), "success");
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let commands = std::fs::read_to_string(directory.path().join("commands.log")).unwrap();
    assert!(commands.contains("--workspace --branch --doctests --no-report"));
    assert!(commands.contains("report --doctests --include-build-script --json"));
    assert!(!commands.contains("ignore-filename"));
    assert!(!commands.contains("exclude"));
    let runs: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("mxrs-coverage.")
        })
        .collect();
    assert_eq!(
        runs.len(),
        2,
        "each measurement needs its own empty directory"
    );
    for run in runs {
        let objects = std::fs::read_to_string(run.path().join("objects.txt")).unwrap();
        assert_eq!(objects.lines().count(), 2);
        assert!(objects.contains("arbitrary-generated-app"));
        assert!(run.path().join("objects.sha256").is_file());
        assert!(run.path().join("source.sha256").is_file());
    }
}

#[test]
fn failed_tests_or_missing_instrumented_objects_cannot_produce_a_green_report() {
    for (mode, expected) in [("fail", 37), ("empty", 1)] {
        let directory = tempfile::tempdir().unwrap();
        let output = run_driver(directory.path(), mode);
        assert_eq!(output.status.code(), Some(expected));
        let commands = std::fs::read_to_string(directory.path().join("commands.log")).unwrap();
        assert!(!commands.contains("llvm-cov report"));
    }
}
