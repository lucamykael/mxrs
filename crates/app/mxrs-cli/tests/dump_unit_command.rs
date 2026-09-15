use std::path::Path;
use std::process::{Command, Output};

use mxrs_bson::{Binary, BinarySubtype, doc};

fn run(path: &Path, id: &str, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mxrs"))
        .arg("dump-unit")
        .arg(path)
        .arg(id)
        .args(flags)
        .output()
        .unwrap()
}

#[test]
fn metadata_and_all_bytes_are_reported_without_reencoding_or_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project with spaces.mpr");
    let mut mpr = mxrs_mpr::MprFile::create(&path, "11.12.1", "").unwrap();
    let root = mpr.root_unit().unwrap().unwrap().unit_id;
    let payload: Vec<u8> = (0..=255).cycle().take(66000).collect();
    let id = mpr
        .insert_unit(
            &root,
            "Documents",
            doc! {
                "$Type": "Future$Diagnostic", "Name": "á\nquoted \"name\"",
                "Payload": Binary { subtype: BinarySubtype::Generic, bytes: payload },
            },
            None,
        )
        .unwrap();
    let unit = mpr.unit(&id).unwrap().unwrap();
    let bytes = mpr.content_bytes(&unit).unwrap().unwrap();
    let content_path = mpr.content_path(&unit).unwrap();
    let hash = unit.contents_hash.unwrap();
    drop(mpr);
    let before = std::fs::read(&path).unwrap();
    let result = run(&path, &id, &[]);
    assert!(result.status.success(), "{:?}", result.stderr);
    assert!(result.stderr.is_empty());
    let text = String::from_utf8(result.stdout.clone()).unwrap();
    assert!(text.starts_with(&format!("UnitID           : {id}\nContainerID      : {root}\nContainmentName  : Documents\nTypeName         : Future$Diagnostic\nContentsHash     : {hash}\nContents (hex)   :\n")));
    let mut recovered = Vec::new();
    for line in text.lines().skip(6) {
        let (offset, rest) = line.trim_start().split_once("  ").unwrap();
        assert_eq!(usize::from_str_radix(offset, 16).unwrap(), recovered.len());
        let hex = &rest[..48];
        let row: Vec<u8> = hex
            .split_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).unwrap())
            .collect();
        assert!(!row.is_empty() && row.len() <= 16);
        let ascii = &rest[50..];
        assert_eq!(ascii.len(), row.len());
        for (byte, character) in row.iter().zip(ascii.bytes()) {
            assert_eq!(
                character,
                if byte.is_ascii_graphic() || *byte == b' ' {
                    *byte
                } else {
                    b'.'
                }
            );
        }
        recovered.extend(row);
    }
    assert_eq!(recovered, bytes);
    assert!(text.contains("  10000  "));
    for identifier in [id.to_uppercase(), id.replace('-', "")] {
        let output = run(&path, &identifier, &["--no-progress"]);
        assert!(output.status.success());
        assert_eq!(output.stdout, result.stdout);
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(content_path).unwrap(), bytes);
}

#[test]
fn absent_and_zero_length_contents_have_distinct_native_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Empty.mpr");
    let mpr = mxrs_mpr::MprFile::create(&path, "11.12.1", "").unwrap();
    let unit = mpr.root_unit().unwrap().unwrap();
    let content = mpr.content_path(&unit).unwrap();
    std::fs::write(&content, []).unwrap();
    let empty = run(&path, &unit.unit_id, &[]);
    assert!(empty.status.success());
    assert!(
        String::from_utf8(empty.stdout.clone())
            .unwrap()
            .ends_with("Contents (hex)   :\n")
    );
    std::fs::remove_file(content).unwrap();
    let absent = run(&path, &unit.unit_id, &[]);
    assert!(absent.status.success());
    assert_eq!(
        absent.stdout,
        [empty.stdout, b"  (empty)\n".to_vec()].concat()
    );
    mpr.raw_query("ALTER TABLE Unit ADD COLUMN Contents BLOB")
        .unwrap();
    mpr.raw_query("UPDATE Unit SET Contents = X'', ContentsHash = NULL")
        .unwrap();
    drop(mpr);
    std::fs::remove_dir_all(dir.path().join("mprcontents")).unwrap();
    let empty = run(&path, &unit.unit_id, &[]);
    assert!(empty.status.success());
    let text = String::from_utf8(empty.stdout.clone()).unwrap();
    assert!(text.contains("ContentsHash     : \n"));
    assert!(text.ends_with("Contents (hex)   :\n"));
    let mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    mpr.raw_query("UPDATE Unit SET Contents = NULL").unwrap();
    drop(mpr);
    assert_eq!(
        run(&path, &unit.unit_id, &[]).stdout,
        [empty.stdout, b"  (empty)\n".to_vec()].concat()
    );
}

#[test]
fn corrupt_contents_and_invalid_requests_fail_before_printing_a_dump() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Corrupt.mpr");
    let mpr = mxrs_mpr::MprFile::create(&path, "11.12.1", "").unwrap();
    let unit = mpr.root_unit().unwrap().unwrap();
    let content = mpr.content_path(&unit).unwrap();
    drop(mpr);
    std::fs::write(&content, b"broken BSON").unwrap();
    let before = std::fs::read(&path).unwrap();
    let corrupt = run(&path, &unit.unit_id, &[]);
    assert!(!corrupt.status.success());
    assert!(corrupt.stdout.is_empty());
    assert!(String::from_utf8(corrupt.stderr).unwrap().contains("BSON"));
    for (id, flags) in [
        ("not-an-id", vec![]),
        ("00000000-0000-0000-0000-000000000000", vec![]),
        (&unit.unit_id, vec!["extra"]),
        (&unit.unit_id, vec!["--unknown"]),
        (&unit.unit_id, vec!["--no-progress", "--no-progress"]),
    ] {
        let result = run(&path, id, &flags);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.is_empty());
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(content).unwrap(), b"broken BSON");
}
