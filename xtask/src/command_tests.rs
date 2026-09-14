use super::*;

#[test]
fn missing_and_extra_arguments_produce_usage_errors_without_panicking() {
    for words in [
        vec![],
        vec!["fixture-gen"],
        vec!["fixture-gen", "name"],
        vec!["fixture-gen", "name", "source.rb", "extra"],
        vec!["fixture-gen", "../outside", "source.rb"],
        vec!["oracle-diff"],
        vec!["oracle-diff", "fixture", "extra"],
        vec!["mxbuild-oracle"],
        vec!["mxbuild-oracle", "app", "extra"],
        vec!["noise-audit", "extra"],
        vec!["capability-matrix", "--unknown"],
        vec!["coverage-gate"],
        vec!["coverage-gate", "--json"],
        vec!["coverage-gate", "file.json", "--unknown"],
    ] {
        let arguments = words
            .iter()
            .map(|word| word.to_string())
            .collect::<Vec<_>>();
        assert!(
            dispatch(&arguments).unwrap_err().contains("usage:"),
            "{words:?}"
        );
    }
    assert!(
        dispatch(&["unknown".into()])
            .unwrap_err()
            .contains("unknown xtask command")
    );
}

#[test]
fn fixture_names_cannot_escape_or_replace_parent_directories() {
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "/tmp/outside",
        "dir/name",
        "dir\\name",
        "-flag",
        "1name",
        "na mé",
    ] {
        assert!(!valid_fixture_name(name), "{name:?}");
    }
    for name in ["minimal", "with_page", "Version11-alpha"] {
        assert!(valid_fixture_name(name), "{name:?}");
    }
}

#[test]
fn missing_fixtures_and_coverage_files_fail_before_running_external_commands() {
    let directory = tempfile::tempdir().unwrap();
    let path = path_str(directory.path());
    assert!(
        dispatch(&["oracle-diff".into(), path.clone()])
            .unwrap_err()
            .contains("no .mpr file")
    );
    assert!(
        dispatch(&["mxbuild-oracle".into(), path])
            .unwrap_err()
            .contains("no .mpr file")
    );
    let missing = path_str(&directory.path().join("missing.json"));
    assert!(
        dispatch(&["coverage-gate".into(), missing])
            .unwrap_err()
            .contains("cannot read")
    );
    let invalid = directory.path().join("invalid.json");
    fs::write(&invalid, "invalid JSON").unwrap();
    assert!(
        dispatch(&["coverage-gate".into(), path_str(&invalid)])
            .unwrap_err()
            .contains("invalid LLVM coverage JSON")
    );
}

#[test]
fn fixture_manifests_are_sorted_and_exclude_their_own_previous_digest() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("nested")).unwrap();
    fs::write(directory.path().join("z.txt"), b"last").unwrap();
    fs::write(directory.path().join("nested/a.txt"), b"first").unwrap();
    fs::write(directory.path().join("manifest.sha256"), b"stale manifest").unwrap();
    write_manifest(directory.path()).unwrap();
    let manifest = fs::read_to_string(directory.path().join("manifest.sha256")).unwrap();
    assert_eq!(
        manifest,
        format!(
            "{:x}  nested/a.txt\n{:x}  z.txt\n",
            Sha256::digest(b"first"),
            Sha256::digest(b"last")
        )
    );
    write_manifest(directory.path()).unwrap();
    assert_eq!(
        fs::read_to_string(directory.path().join("manifest.sha256")).unwrap(),
        manifest
    );
}
