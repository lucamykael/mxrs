//! Stages every file an artifact scaffold touches and publishes them
//! together. Ported from mxrb's `Scaffold::Transaction`
//! (`lib/mxrb/scaffold/transaction.rb`), keeping the two properties that
//! make its scaffolds safe to run against a project with uncommitted work:
//!
//! 1. **An existing file is never replaced by a `create`.** A scaffold that
//!    silently overwrote hand-edited source would be indistinguishable from
//!    data loss, so [`Transaction::create`] fails closed instead.
//! 2. **A partial write is rolled back.** An artifact scaffold usually adds a
//!    source file *and* edits one or more aggregators; leaving a project with
//!    the aggregator edited but the file missing would not even compile.
//!
//! Deliberately narrower than mxrb's version in one place: mxrb carries a
//! per-change Unix mode because its `demo-user` recipe writes a `0o600`
//! `.env` secret. No mxrs scaffold writes a secret, so modes are left to the
//! platform default rather than carried as a field nothing sets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{Result, ScaffoldError, io_error};

struct Change {
    content: String,
    original: Option<Vec<u8>>,
}

#[derive(Default)]
pub(crate) struct Transaction {
    changes: BTreeMap<PathBuf, Change>,
    created: Vec<PathBuf>,
    updated: Vec<PathBuf>,
}

impl Transaction {
    /// `Ok(None)` means "absent"; a file that exists but is not UTF-8 is an
    /// error rather than an absence, because treating it as absent would let
    /// `create` report `FileExists` for a path the caller was told was free.
    pub(crate) fn content(&self, path: &Path) -> Result<Option<String>> {
        if let Some(change) = self.changes.get(path) {
            return Ok(Some(change.content.clone()));
        }
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(path, error)),
        }
    }

    pub(crate) fn create(&mut self, path: impl Into<PathBuf>, content: String) -> Result<()> {
        let path = path.into();
        if self.changes.contains_key(&path) || path.symlink_metadata().is_ok() {
            return Err(ScaffoldError::FileExists(path.display().to_string()));
        }
        self.created.push(path.clone());
        self.changes.insert(
            path,
            Change {
                content,
                original: None,
            },
        );
        Ok(())
    }

    /// Replaces `path`'s contents, or creates it when absent. Rewriting a file
    /// with the exact bytes it already has is not reported as an update, so an
    /// idempotent re-run prints nothing instead of claiming work it did not do.
    pub(crate) fn write(&mut self, path: impl Into<PathBuf>, content: String) -> Result<()> {
        let path = path.into();
        let Some(original) = self.content(&path)? else {
            return self.create(path, content);
        };
        if original == content {
            return Ok(());
        }
        if !self.changes.contains_key(&path) {
            self.updated.push(path.clone());
        }
        self.changes.insert(
            path,
            Change {
                content,
                original: Some(original.into_bytes()),
            },
        );
        Ok(())
    }

    pub(crate) fn created(&self) -> &[PathBuf] {
        &self.created
    }

    pub(crate) fn updated(&self) -> &[PathBuf] {
        &self.updated
    }

    pub(crate) fn commit(self) -> Result<()> {
        let mut applied: Vec<(&Path, Option<&[u8]>)> = Vec::new();
        for (path, change) in &self.changes {
            if let Err(error) = apply(path, &change.content) {
                for (path, original) in applied.into_iter().rev() {
                    rollback(path, original);
                }
                return Err(error);
            }
            applied.push((path, change.original.as_deref()));
        }
        Ok(())
    }
}

fn apply(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let staging = tempfile::Builder::new()
        .prefix(".mxrs-scaffold-")
        .tempfile_in(parent)
        .map_err(|error| io_error(parent, error))?;
    std::fs::write(staging.path(), content).map_err(|error| io_error(staging.path(), error))?;
    staging
        .persist(path)
        .map_err(|error| io_error(path, error.error))?;
    Ok(())
}

fn rollback(path: &Path, original: Option<&[u8]>) {
    match original {
        Some(original) => {
            let _ = std::fs::write(path, original);
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creating_over_an_existing_path_fails_instead_of_replacing_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kept.rs");
        std::fs::write(&path, "hand written").unwrap();
        let mut transaction = Transaction::default();
        assert!(matches!(
            transaction.create(&path, "generated".into()),
            Err(ScaffoldError::FileExists(_))
        ));
        transaction
            .create(directory.path().join("new.rs"), "new".into())
            .unwrap();
        assert!(matches!(
            transaction.create(directory.path().join("new.rs"), "again".into()),
            Err(ScaffoldError::FileExists(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hand written");
    }

    #[test]
    fn rewriting_identical_content_is_not_reported_as_an_update() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("aggregator.rs");
        std::fs::write(&path, "pub mod order;\n").unwrap();
        let mut transaction = Transaction::default();
        transaction.write(&path, "pub mod order;\n".into()).unwrap();
        assert!(transaction.updated().is_empty());
        transaction
            .write(&path, "pub mod order;\npub mod invoice;\n".into())
            .unwrap();
        assert_eq!(transaction.updated(), std::slice::from_ref(&path));
        transaction
            .write(
                &path,
                "pub mod order;\npub mod invoice;\npub mod line;\n".into(),
            )
            .unwrap();
        assert_eq!(transaction.updated(), std::slice::from_ref(&path));
        transaction.commit().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "pub mod order;\npub mod invoice;\npub mod line;\n"
        );
    }

    #[test]
    fn a_failed_publication_restores_every_already_applied_change() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory.path().join("a_existing.rs");
        std::fs::write(&existing, "original").unwrap();
        let created = directory.path().join("b_created.rs");
        // A directory occupying the third path makes its publication fail
        // after the first two have already been written.
        let blocked = directory.path().join("c_blocked.rs");
        std::fs::create_dir(&blocked).unwrap();
        let mut transaction = Transaction::default();
        transaction.write(&existing, "replaced".into()).unwrap();
        transaction.create(&created, "new".into()).unwrap();
        transaction.changes.insert(
            blocked.clone(),
            Change {
                content: "unwritable".into(),
                original: None,
            },
        );
        assert!(transaction.commit().is_err());
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "original");
        assert!(!created.exists());
        assert!(blocked.is_dir());
    }

    #[test]
    fn non_utf8_sources_are_an_error_rather_than_a_silently_absent_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binary.rs");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        let transaction = Transaction::default();
        assert!(matches!(
            transaction.content(&path),
            Err(ScaffoldError::Io { .. })
        ));
        assert!(
            transaction
                .content(&directory.path().join("absent.rs"))
                .unwrap()
                .is_none()
        );
    }
}
