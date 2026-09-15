use std::path::Path;
use std::process::{Command, Output};

use mxrs_bson::{Bson, doc};
use serde_json::json;

fn run(path: Option<&Path>, flags: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mxrs"));
    command.arg("modules");
    if let Some(path) = path {
        command.arg(path);
    }
    command.args(flags).output().unwrap()
}

#[test]
fn summaries_include_nested_documents_and_exclude_other_flow_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project with spaces.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Zulu", |module| {
        module.entity("Record", |_| {});
        module.microflow("Direct", |_| {});
        module.nanoflow("Client", |_| {});
    });
    builder.module("Alpha", |_| {});
    mxrs_writer::write_project(&path, &builder.build()).unwrap();
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let modules = mpr.units_by_containment("Modules").unwrap();
    let zulu = modules
        .iter()
        .find(|u| mpr.parse_contents(u).unwrap().get_str("Name").ok() == Some("Zulu"))
        .unwrap();
    let module_id = zulu.unit_id.clone();
    let outer = mpr
        .insert_unit(
            &module_id,
            "Folders",
            doc! { "$Type": "Projects$Folder", "Name": "Outer" },
            None,
        )
        .unwrap();
    let inner = mpr
        .insert_unit(
            &outer,
            "Folders",
            doc! { "$Type": "Projects$Folder", "Name": "Inner" },
            None,
        )
        .unwrap();
    for (kind, parent) in [
        ("Forms$Page", &module_id),
        ("Pages$Page", &inner),
        ("Microflows$Microflow", &inner),
        ("Microflows$Nanoflow", &inner),
        ("Microflows$Rule", &inner),
        ("Future$Document", &inner),
    ] {
        mpr.insert_unit(
            parent,
            "Documents",
            doc! { "$Type": kind, "Name": "Nested" },
            None,
        )
        .unwrap();
    }
    let expected: Vec<_> = modules
        .iter()
        .map(|unit| {
            let name = mpr
                .parse_contents(unit)
                .unwrap()
                .get_str("Name")
                .unwrap()
                .to_string();
            if name == "Zulu" {
                json!({"name":name,"entities":1,"pages":2,"microflows":2})
            } else {
                json!({"name":name,"entities":0,"pages":0,"microflows":0})
            }
        })
        .collect();
    drop(mpr);
    let before = std::fs::read(&path).unwrap();
    let output = run(Some(&path), &["--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!(expected)
    );
    let default = run(Some(&path), &[]);
    assert!(default.status.success());
    assert!(
        String::from_utf8(default.stdout.clone())
            .unwrap()
            .contains("\"Zulu\": entities=1 pages=2 microflows=2")
    );
    assert_eq!(run(Some(&path), &["--no-progress"]).stdout, default.stdout);
    assert_eq!(run(Some(&path), &["--names"]).stdout, b"Alpha\nZulu\n");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn empty_and_unnamed_modules_remain_distinguishable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Empty.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Unnamed", |_| {});
    mxrs_writer::write_project(&path, &builder.build()).unwrap();
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let module = mpr.units_by_containment("Modules").unwrap().remove(0);
    let mut doc = mpr.parse_contents(&module).unwrap();
    doc.insert("Name", Bson::Null);
    mpr.update_unit(&module.unit_id, doc).unwrap();
    drop(mpr);
    let output = run(Some(&path), &["--json"]);
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!([{"name":null,"entities":0,"pages":0,"microflows":0}])
    );
    assert_eq!(
        run(Some(&path), &[]).stdout,
        b"null: entities=0 pages=0 microflows=0\n"
    );
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mpr.delete_unit(&module.unit_id).unwrap();
    drop(mpr);
    assert_eq!(run(Some(&path), &["--json"]).stdout, b"[]\n");
    assert!(run(Some(&path), &[]).stdout.is_empty());
}

#[test]
fn invalid_inputs_fail_without_output_or_creating_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.mpr");
    let corrupt = dir.path().join("corrupt.mpr");
    std::fs::write(&corrupt, b"not SQLite").unwrap();
    for (path, flags) in [
        (None, vec![]),
        (Some(missing.as_path()), vec![]),
        (Some(corrupt.as_path()), vec![]),
        (Some(dir.path()), vec![]),
        (Some(missing.as_path()), vec!["extra"]),
        (Some(missing.as_path()), vec!["--unknown"]),
        (Some(missing.as_path()), vec!["--json", "--names"]),
        (Some(missing.as_path()), vec!["--json", "--json"]),
    ] {
        let result = run(path, &flags);
        assert!(!result.status.success(), "{path:?} {flags:?}");
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.is_empty());
    }
    assert!(!missing.exists());
    assert_eq!(std::fs::read(corrupt).unwrap(), b"not SQLite");
}
