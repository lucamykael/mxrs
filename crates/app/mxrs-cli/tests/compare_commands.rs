use std::path::Path;
use std::process::{Command, Output};

fn run(command: &str, left: &Path, right: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mxrs"))
        .arg(command)
        .arg(left)
        .arg(right)
        .args(flags)
        .output()
        .unwrap()
}

#[test]
fn compare_and_diff_share_changes_but_have_distinct_human_contracts() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left/Project.mpr");
    let right = directory.path().join("right/Project.mpr");
    for (path, default) in [(&left, "before"), (&right, "after")] {
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Record", |entity| {
                entity.string("Name").default_value = Some(default.into());
            });
        });
        mxrs_writer::write_project(path, &builder.build()).unwrap();
    }
    let before = [
        std::fs::read(&left).unwrap(),
        std::fs::read(&right).unwrap(),
    ];
    let mut json_outputs = Vec::new();
    for command in ["compare", "diff"] {
        let unchanged = run(command, &left, &left, &[]);
        assert!(unchanged.status.success());
        assert!(unchanged.stderr.is_empty());
        assert_eq!(
            unchanged.stdout,
            if command == "compare" {
                b"[mxrs] OK\n".as_slice()
            } else {
                b""
            }
        );
        let changed = run(command, &left, &right, &["--no-progress"]);
        assert_eq!(changed.status.code(), Some(1));
        assert!(changed.stderr.is_empty());
        let text = String::from_utf8(changed.stdout).unwrap();
        let record = if command == "compare" {
            "[mxrs] diff: modules.Sales.entities.Record.attributes.Name.default: \"before\" != \"after\"\n"
        } else {
            "changed\tmodules.Sales.entities.Record.attributes.Name.default\t\"before\"\t=>\t\"after\"\n"
        };
        assert!(text.contains(record), "{text}");
        let structured = run(command, &left, &right, &["--json"]);
        assert_eq!(structured.status.code(), Some(1));
        assert!(structured.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&structured.stdout).unwrap();
        assert_eq!(value["identical"], false);
        json_outputs.push(value);
        for flags in [&["--unknown"][..], &["--json", "--json"], &["extra"]] {
            let invalid = run(command, &left, &right, flags);
            assert!(!invalid.status.success());
            assert!(invalid.stdout.is_empty());
            assert!(!invalid.stderr.is_empty());
        }
    }
    assert_eq!(json_outputs[0], json_outputs[1]);
    assert_eq!(
        before,
        [std::fs::read(left).unwrap(), std::fs::read(right).unwrap()]
    );
}
