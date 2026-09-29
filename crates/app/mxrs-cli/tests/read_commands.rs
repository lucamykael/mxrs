//! Regression tests for the read-command contracts, independent of MXRB.
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mxrs"))
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap()
}

fn failure(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn sql_preserves_dynamic_types_and_every_byte_including_invalid_utf8_text() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Project.mpr");
    drop(mxrs_mpr::MprFile::create(&path, "11.12.1", "").unwrap());
    let before = std::fs::read(&path).unwrap();
    let path = path.to_str().unwrap();
    let query = "SELECT NULL, -9223372036854775808, 9223372036854775807, 1.0, 'Olá', char(0,10,34,92), X'00ff80', CAST(X'80ff' AS TEXT), ?, :unset";
    assert_eq!(
        success(cli(&["sql", path, query])),
        "[NULL, -9223372036854775808, 9223372036854775807, 1.0, \"Olá\", \"\\u0000\\n\\\"\\\\\", X'00ff80', TEXT X'80ff', NULL, NULL]\n(1 rows)\n"
    );
    let bytes: Vec<u8> = (0..=255).collect();
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        success(cli(&[
            "sql",
            path,
            &format!("SELECT X'{hex}'"),
            "--no-progress"
        ])),
        format!("[X'{hex}']\n(1 rows)\n")
    );
    for query in ["", "-- comment", "SELECT 1; SELECT 2", "DELETE FROM Unit"] {
        failure(cli(&["sql", path, query]));
    }
    assert_eq!(before, std::fs::read(path).unwrap());
}

#[test]
fn units_reads_names_without_hiding_corrupt_bson() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Fallback.mpr");
    let mut mpr = mxrs_mpr::MprFile::create(&path, "11.12.1", "").unwrap();
    let root = mpr.root_unit().unwrap().unwrap();
    mpr.update_unit(
        &root.unit_id,
        mxrs_bson::doc! { "$Type": "Projects$Project", "name": "Lowercase" },
    )
    .unwrap();
    let content = mpr
        .content_path(&mpr.unit(&root.unit_id).unwrap().unwrap())
        .unwrap();
    drop(mpr);
    assert!(success(cli(&["units", path.to_str().unwrap()])).starts_with("Project : Lowercase\n"));
    std::fs::write(content, b"broken").unwrap();
    failure(cli(&["units", path.to_str().unwrap(), "--no-progress"]));
}

fn archive(path: &Path, metadata: &str, additional: &[&str]) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.add_directory("model/", options).unwrap();
    zip.add_directory("empty/", options).unwrap();
    zip.start_file("model/metadata.json", options).unwrap();
    zip.write_all(metadata.as_bytes()).unwrap();
    for name in additional {
        zip.start_file(*name, options).unwrap();
        zip.write_all(b"content").unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn mda_accepts_directory_entries_and_compares_files_with_native_status_names() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left.mda");
    let right = directory.path().join("right.mda");
    archive(&left, "{}", &["model/removed.bin"]);
    archive(&right, "{}", &["model/added.bin"]);
    let out = success(cli(&["mda", "inspect", left.to_str().unwrap(), "--json"]));
    let data: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(data["files"], 2);
    assert_eq!(data["roots"], serde_json::json!(["empty", "model"]));
    assert!(
        success(cli(&["mda", "inspect", left.to_str().unwrap()]))
            .starts_with("Runtime       : \nProject       : \n")
    );
    assert_eq!(
        success(cli(&[
            "mda",
            "compare",
            left.to_str().unwrap(),
            right.to_str().unwrap()
        ])),
        "added\tmodel/added.bin\nremoved\tmodel/removed.bin\n[mxrs] 2 difference(s)\n"
    );
    for metadata in ["null", "[]", "{\"RuntimeVersion\":42}"] {
        archive(&right, metadata, &[]);
        failure(cli(&["mda", "inspect", right.to_str().unwrap()]));
    }
}

#[test]
fn project_inspection_normalizes_root_and_reports_unreadable_inventory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // The modules a project declares are the files in the registry, one per
    // Mendix module — a layer-first tree has no module folders.
    std::fs::create_dir_all(root.join("src/domain/modules")).unwrap();
    std::fs::write(root.join("src/domain/modules/mod.rs"), "").unwrap();
    std::fs::write(root.join("src/domain/modules/zulu.rs"), "").unwrap();
    std::fs::write(root.join("src/domain/modules/.hidden.rs"), "").unwrap();
    std::fs::write(root.join(".hidden.mpr"), "inventory only").unwrap();
    let path = root.join("absent/..");
    let data: serde_json::Value = serde_json::from_str(&success(cli(&[
        "project",
        "inspect",
        path.to_str().unwrap(),
        "--json",
    ])))
    .unwrap();
    assert_eq!(data["root"], root.to_str().unwrap());
    assert_eq!(data["modules"], serde_json::json!(["zulu"]));
    assert_eq!(data["mprs"], serde_json::json!([]));
    // A broken build directory must not masquerade as an empty inventory.
    std::fs::write(root.join("build"), "not a directory").unwrap();
    failure(cli(&["project", "inspect", root.to_str().unwrap()]));
}

#[test]
fn project_version_uses_rust_syntax_and_ignores_comments_and_string_literals() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("src")).unwrap();
    let source = r##"
// #[mxrs::application(version = "wrong")]
const EXAMPLE: &str = r#"#[mxrs::application(version = "also wrong")]"#;
#[mxrs::application(
    project = crate::build,
    version = r"11.12.1",
)]
pub struct Application;
"##;
    std::fs::write(root.join("src/lib.rs"), source).unwrap();
    let output = success(cli(&[
        "project",
        "inspect",
        root.to_str().unwrap(),
        "--json",
    ]));
    let data: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(data["declared_version"], "11.12.1");
    std::fs::write(
        root.join("src/lib.rs"),
        format!("{source}\n#[mxrs::application(version = \"12.0.0\")] struct Other;"),
    )
    .unwrap();
    failure(cli(&["project", "inspect", root.to_str().unwrap()]));
}

#[test]
fn mda_rejects_duplicate_names_before_zip_can_silently_collapse_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("duplicate.mda");
    archive(&path, "{}", &["web/file", "web/filx"]);
    let mut bytes = std::fs::read(&path).unwrap();
    let offsets: Vec<usize> = bytes
        .windows(8)
        .enumerate()
        .filter_map(|(index, value)| (value == b"web/filx").then_some(index))
        .collect();
    assert_eq!(offsets.len(), 2); // local header and central-directory record
    for offset in offsets {
        bytes[offset + 7] = b'e';
    }
    std::fs::write(&path, &bytes).unwrap();
    failure(cli(&["mda", "inspect", path.to_str().unwrap()]));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
