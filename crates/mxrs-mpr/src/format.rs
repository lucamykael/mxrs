//! MPR storage format detection.
//!
//! Ports the `detect_format`/`contents_dir`/`contents_column?` logic from
//! `lib/mxrb/io/mpr_file.rb`. mxrs targets Mendix 11.x (Studio Pro 10+) for
//! v1, so only the v2 (`.mxunit` content-addressed file) path is exercised
//! in practice, but detection still distinguishes both so opening an older
//! v1-only `.mpr` fails with a clear error rather than silently
//! misinterpreting it.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::{MprError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageFormat {
    V1,
    V2,
}

/// The `mprcontents` sibling directory that holds v2 `.mxunit` files.
pub fn contents_dir(mpr_path: &Path) -> PathBuf {
    mpr_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("mprcontents")
}

/// Whether the `Unit` table has a `Contents` BLOB column at all.
pub fn contents_column(conn: &Connection) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(Unit)")?;
    let has_contents = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(std::result::Result::ok)
        .any(|name| name == "Contents");
    Ok(has_contents)
}

/// Detects whether `mpr_path`'s `Unit` table stores contents inline (v1) or
/// externally in `.mxunit` files under [`contents_dir`] (v2).
pub fn detect_format(conn: &Connection, mpr_path: &Path) -> Result<StorageFormat> {
    let dir = contents_dir(mpr_path);
    if !contents_column(conn)? {
        if !dir.is_dir() {
            return Err(MprError::IncompletePackage(format!(
                "{}: MPR schema stores unit contents externally but sibling directory {} is missing",
                mpr_path.display(),
                dir.display()
            )));
        }
        return Ok(StorageFormat::V2);
    }

    Ok(if dir.is_dir() {
        StorageFormat::V2
    } else {
        StorageFormat::V1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_dir_is_a_sibling_of_the_mpr_file() {
        let path = Path::new("/some/project/App.mpr");
        assert_eq!(contents_dir(path), Path::new("/some/project/mprcontents"));
    }
}
