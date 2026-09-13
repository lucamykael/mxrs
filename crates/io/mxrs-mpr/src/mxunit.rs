//! Storage adapter for MPR v2 unit files.
//!
//! Mendix stores the serialized unit payload outside SQLite for v2 storage.
//! The payload is kept opaque: known BSON payloads decode normally, while
//! parse failures propagate instead of the file being silently overwritten.
//!
//! Ports `Mxrb::IO::MxunitCodec` from `lib/mxrb/io/mxunit_codec.rb`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;

/// Content-addressed path for a unit's `.mxunit` file: the UUID's first two
/// hex-digit pairs (without dashes) become two nesting directory levels.
pub fn path_for(contents_dir: &Path, uuid: &str) -> PathBuf {
    let hex: String = uuid
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_lowercase();
    contents_dir
        .join(&hex[0..2])
        .join(&hex[2..4])
        .join(format!("{}.mxunit", uuid.to_lowercase()))
}

/// Reads and BSON-decodes a `.mxunit` file.
pub fn read(path: &Path) -> Result<mxrs_bson::Document> {
    let bytes = fs::read(path)?;
    Ok(mxrs_bson::parse(&bytes)?)
}

/// Writes `bytes` to `path` atomically: write to a temp sibling file, then
/// rename over the destination. Creates parent directories as needed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temp_path = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mxunit"),
        std::process::id()
    ));
    fs::write(&temp_path, bytes)?;
    fs::rename(&temp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_for_nests_by_the_first_two_hex_pairs() {
        let uuid = "C67C5271-DA7D-45F1-81DF-CEB6946B8ABE";
        let path = path_for(Path::new("/proj/mprcontents"), uuid);
        assert_eq!(
            path,
            Path::new("/proj/mprcontents/c6/7c/c67c5271-da7d-45f1-81df-ceb6946b8abe.mxunit")
        );
    }

    #[test]
    fn write_atomic_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_for(dir.path(), "c67c5271-da7d-45f1-81df-ceb6946b8abe");
        let doc =
            mxrs_bson::doc! { "$ID": "c67c5271-da7d-45f1-81df-ceb6946b8abe", "Name": "Order" };
        let bytes = mxrs_bson::serialize(&doc).unwrap();

        write_atomic(&path, &bytes).unwrap();
        assert_eq!(read(&path).unwrap(), doc);
    }

    #[test]
    fn write_atomic_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_for(dir.path(), "c67c5271-da7d-45f1-81df-ceb6946b8abe");
        write_atomic(&path, b"anything").unwrap();

        let siblings: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(siblings.len(), 1);
    }
}
