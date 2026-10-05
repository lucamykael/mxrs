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
//! Like mxrb's version, a change can carry a Unix mode: the `demo-user`
//! recipe writes a `0o600` `.env` secret, and a world-readable credential
//! file would defeat the point of keeping the password out of source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{Result, ScaffoldError, io_error};

struct Change {
    content: String,
    original: Option<Vec<u8>>,
    /// Unix permissions applied on publish (`None` keeps platform defaults).
    mode: Option<u32>,
}

#[derive(Default)]
pub(crate) struct Transaction {
    changes: BTreeMap<PathBuf, Change>,
    created: Vec<PathBuf>,
    updated: Vec<PathBuf>,
    /// Created files this scaffold does not own: the project's, when no
    /// key is given, or another scaffold's by its key.
    elsewhere: Vec<(Option<String>, PathBuf)>,
    /// What the scaffold could not do and leaves to its user.
    notes: Vec<String>,
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
        self.create_with_mode(path, content, None)
    }

    /// `create` with explicit Unix permissions — for secrets that must not be
    /// world-readable (the `.env` file a demo-user scaffold writes).
    pub(crate) fn create_private(
        &mut self,
        path: impl Into<PathBuf>,
        content: String,
    ) -> Result<()> {
        self.create_with_mode(path, content, Some(0o600))
    }

    fn create_with_mode(
        &mut self,
        path: impl Into<PathBuf>,
        content: String,
        mode: Option<u32>,
    ) -> Result<()> {
        let path = path.into();
        if self.changes.contains_key(&path) || path.symlink_metadata().is_ok() {
            return Err(ScaffoldError::FileExists(path.display().to_string()));
        }
        // A file this scaffold creates is formatted the way rustfmt would
        // leave it. Files it merely edits are the user's, and are not
        // reformatted behind their back.
        let content = if path.extension().is_some_and(|extension| extension == "rs") {
            crate::format_rust(content)
        } else {
            content
        };
        self.created.push(path.clone());
        self.changes.insert(
            path,
            Change {
                content,
                original: None,
                mode,
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
        let mode = self.changes.get(&path).and_then(|change| change.mode);
        self.changes.insert(
            path,
            Change {
                content,
                original: Some(original.into_bytes()),
                mode,
            },
        );
        Ok(())
    }

    pub(crate) fn created(&self) -> &[PathBuf] {
        &self.created
    }

    /// Says that a file this transaction creates is not the running
    /// scaffold's to remove: it belongs to `owner`, or — without one — to
    /// the project, which keeps it whatever is destroyed.
    pub(crate) fn owned_elsewhere(&mut self, owner: Option<String>, path: impl Into<PathBuf>) {
        self.elsewhere.push((owner, path.into()));
    }

    /// The created files that belong to another scaffold, by its key.
    pub(crate) fn elsewhere(&self) -> &[(Option<String>, PathBuf)] {
        &self.elsewhere
    }

    pub(crate) fn note(&mut self, note: String) {
        self.notes.push(note);
    }

    pub(crate) fn notes(&self) -> &[String] {
        &self.notes
    }

    pub(crate) fn updated(&self) -> &[PathBuf] {
        &self.updated
    }

    pub(crate) fn commit(self) -> Result<()> {
        let mut applied: Vec<(&Path, Option<&[u8]>)> = Vec::new();
        for (path, change) in &self.changes {
            if let Err(error) = apply(path, &change.content, change.mode) {
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

fn apply(path: &Path, content: &str, mode: Option<u32>) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let staging = tempfile::Builder::new()
        .prefix(".mxrs-scaffold-")
        .tempfile_in(parent)
        .map_err(|error| io_error(parent, error))?;
    std::fs::write(staging.path(), content).map_err(|error| io_error(staging.path(), error))?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staging.path(), std::fs::Permissions::from_mode(mode))
            .map_err(|error| io_error(staging.path(), error))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
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
                mode: None,
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
