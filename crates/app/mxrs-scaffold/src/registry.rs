//! Records which files each artifact scaffold created, so `mxrs scaffold
//! destroy` removes exactly those paths instead of re-deriving them from a
//! template and guessing. Ported from mxrb's `Scaffold::Registry`
//! (`lib/mxrb/scaffold/registry.rb`), including its two refusals: a path that
//! escapes the project root is rejected, and a file whose SHA-256 no longer
//! matches what the scaffold wrote is never deleted — an edited file is the
//! user's work, not the generator's.
//!
//! Aggregator *updates* are deliberately not registered. mxrb registers only
//! `transaction.created`, so destroying a scaffold removes its own files and
//! leaves the aggregator lines behind; reversing an append inside a file a
//! human may have since restructured cannot be done safely from a manifest.
//! `mxrs scaffold destroy` therefore reports the removed files and nothing
//! else, and the leftover `pub mod` line surfaces as a compile error the user
//! resolves, rather than as a silent rewrite of their source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::transaction::Transaction;
use crate::{Result, ScaffoldError, io_error};

pub(crate) const RELATIVE_PATH: &str = ".mxrs/scaffolds.json";

/// One registered scaffold and the files it created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredScaffold {
    pub key: String,
    pub files: Vec<PathBuf>,
}

pub(crate) fn stage(
    transaction: &mut Transaction,
    root: &Path,
    key: &str,
    files: &[PathBuf],
) -> Result<PathBuf> {
    let path = root.join(RELATIVE_PATH);
    let mut payload = read_payload(transaction.content(&path)?.as_deref(), &path)?;
    let mut entries = Vec::new();
    for file in files {
        let content = transaction
            .content(file)?
            .ok_or_else(|| ScaffoldError::RegistryInvalid(file.display().to_string()))?;
        entries.push(serde_json::json!({
            "path": relative(root, file),
            "sha256": format!("{:x}", Sha256::digest(content.as_bytes())),
        }));
    }
    payload.insert(key.to_string(), serde_json::json!({ "files": entries }));
    let document = serde_json::json!({ "scaffolds": payload });
    let mut text =
        serde_json::to_string_pretty(&document).map_err(|error| serialization(&path, &error))?;
    text.push('\n');
    transaction.write(&path, text)?;
    Ok(path)
}

/// Registered scaffolds keyed by `kind:name`, in the order `scaffold list`
/// prints them.
pub fn entries(root: impl AsRef<Path>) -> Result<Vec<RegisteredScaffold>> {
    let root = root.as_ref();
    let path = root.join(RELATIVE_PATH);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(io_error(&path, error)),
    };
    read_payload(Some(&text), &path)?
        .into_iter()
        .map(|(key, entry)| {
            Ok(RegisteredScaffold {
                files: files_of(&entry, &path)?
                    .into_iter()
                    .map(|(relative, _)| safe_path(root, &relative))
                    .collect::<Result<Vec<_>>>()?,
                key,
            })
        })
        .collect()
}

/// Deletes a registered scaffold's files and forgets the entry. Removing an
/// entry that no longer exists is an error rather than a no-op: silently
/// succeeding would tell the user a scaffold was removed when nothing was.
pub fn destroy(root: impl AsRef<Path>, key: &str) -> Result<RegisteredScaffold> {
    let root = root.as_ref();
    let path = root.join(RELATIVE_PATH);
    let text = std::fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
    let mut payload = read_payload(Some(&text), &path)?;
    let entry = payload
        .remove(key)
        .ok_or_else(|| ScaffoldError::ScaffoldNotRegistered(key.to_string()))?;
    let mut files = Vec::new();
    for (relative, digest) in files_of(&entry, &path)? {
        let file = safe_path(root, &relative)?;
        let actual = match std::fs::read(&file) {
            Ok(bytes) => Some(format!("{:x}", Sha256::digest(&bytes))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(io_error(&file, error)),
        };
        if actual.as_deref() != Some(digest.as_str()) {
            return Err(ScaffoldError::ScaffoldFileChanged(
                file.display().to_string(),
            ));
        }
        files.push(file);
    }
    for file in &files {
        std::fs::remove_file(file).map_err(|error| io_error(file, error))?;
    }
    // A page leaves the navigation with the item its scaffold gave it,
    // while that is still the line the scaffold wrote.
    // That is the page the scaffold is named for, or — for an entity's
    // CRUD — one of the pages it wrote.
    if let Some(page) = key.strip_prefix("page:") {
        let navigation = root.join("frontend/src/navigation/index.ts");
        let module = page.split('.').next().unwrap_or_default();
        let mut pages = vec![page.to_string()];
        pages.extend(files.iter().filter_map(|file| {
            let written = file.strip_prefix(root.join("frontend/src/pages")).ok()?;
            (written.extension()? == "tsx")
                .then(|| {
                    written
                        .file_stem()?
                        .to_str()
                        .map(|stem| format!("{module}.{stem}"))
                })
                .flatten()
        }));
        for page in pages {
            if let Ok(source) = std::fs::read_to_string(&navigation)
                && let Some(edited) = crate::forms::remove_navigation_item(&source, &page)
            {
                std::fs::write(&navigation, edited)
                    .map_err(|error| io_error(&navigation, error))?;
            }
        }
    }
    let document = serde_json::json!({ "scaffolds": payload });
    let mut text =
        serde_json::to_string_pretty(&document).map_err(|error| serialization(&path, &error))?;
    text.push('\n');
    std::fs::write(&path, text).map_err(|error| io_error(&path, error))?;
    Ok(RegisteredScaffold {
        key: key.to_string(),
        files,
    })
}

fn read_payload(text: Option<&str>, path: &Path) -> Result<BTreeMap<String, serde_json::Value>> {
    let Some(text) = text else {
        return Ok(BTreeMap::new());
    };
    let document: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| ScaffoldError::RegistryInvalid(format!("{}: {error}", path.display())))?;
    match document.get("scaffolds") {
        None => Ok(BTreeMap::new()),
        Some(serde_json::Value::Object(map)) => Ok(map
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()),
        Some(_) => Err(ScaffoldError::RegistryInvalid(format!(
            "{}: \"scaffolds\" is not an object",
            path.display()
        ))),
    }
}

fn files_of(entry: &serde_json::Value, path: &Path) -> Result<Vec<(String, String)>> {
    entry
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ScaffoldError::RegistryInvalid(format!("{}: entry has no file list", path.display()))
        })?
        .iter()
        .map(|file| {
            match (
                file.get("path").and_then(serde_json::Value::as_str),
                file.get("sha256").and_then(serde_json::Value::as_str),
            ) {
                (Some(relative), Some(digest)) => Ok((relative.to_string(), digest.to_string())),
                _ => Err(ScaffoldError::RegistryInvalid(format!(
                    "{}: entry file needs \"path\" and \"sha256\"",
                    path.display()
                ))),
            }
        })
        .collect()
}

/// Registry paths are data a previous run wrote; a `..` segment smuggled into
/// one must never let `destroy` delete outside the project.
fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let candidate = root.join(relative);
    if relative.is_empty()
        || candidate
            .components()
            .any(|component| component == std::path::Component::ParentDir)
        || !candidate.starts_with(root)
    {
        return Err(ScaffoldError::UnsafeScaffoldPath(relative.to_string()));
    }
    Ok(candidate)
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

fn serialization(path: &Path, error: &serde_json::Error) -> ScaffoldError {
    ScaffoldError::RegistryInvalid(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(root: &Path, body: &str) {
        std::fs::create_dir_all(root.join(".mxrs")).unwrap();
        std::fs::write(root.join(RELATIVE_PATH), body).unwrap();
    }

    #[test]
    fn an_edited_or_missing_scaffold_file_is_never_deleted() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("order.rs"), "generated").unwrap();
        let digest = format!("{:x}", Sha256::digest(b"generated"));
        registry(
            root,
            &format!(
                r#"{{"scaffolds":{{"entity:Sales.Order":{{"files":[{{"path":"order.rs","sha256":"{digest}"}}]}}}}}}"#
            ),
        );
        std::fs::write(root.join("order.rs"), "hand edited").unwrap();
        assert!(matches!(
            destroy(root, "entity:Sales.Order"),
            Err(ScaffoldError::ScaffoldFileChanged(_))
        ));
        assert_eq!(
            std::fs::read_to_string(root.join("order.rs")).unwrap(),
            "hand edited"
        );
        std::fs::remove_file(root.join("order.rs")).unwrap();
        assert!(matches!(
            destroy(root, "entity:Sales.Order"),
            Err(ScaffoldError::ScaffoldFileChanged(_))
        ));
    }

    #[test]
    fn destroying_removes_registered_files_and_forgets_only_that_entry() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("order.rs"), "generated").unwrap();
        let digest = format!("{:x}", Sha256::digest(b"generated"));
        registry(
            root,
            &format!(
                r#"{{"scaffolds":{{"entity:Sales.Order":{{"files":[{{"path":"order.rs","sha256":"{digest}"}}]}},"entity:Sales.Invoice":{{"files":[]}}}}}}"#
            ),
        );
        let removal = destroy(root, "entity:Sales.Order").unwrap();
        assert_eq!(removal.files, [root.join("order.rs")]);
        assert!(!root.join("order.rs").exists());
        assert_eq!(
            entries(root).unwrap(),
            [RegisteredScaffold {
                key: "entity:Sales.Invoice".into(),
                files: vec![]
            }]
        );
        assert!(matches!(
            destroy(root, "entity:Sales.Order"),
            Err(ScaffoldError::ScaffoldNotRegistered(_))
        ));
    }

    #[test]
    fn registry_paths_cannot_escape_the_project_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let outside = directory.path().join("outside.rs");
        std::fs::write(&outside, "keep").unwrap();
        registry(
            &root,
            r#"{"scaffolds":{"entity:Sales.Order":{"files":[{"path":"../outside.rs","sha256":"0"}]}}}"#,
        );
        assert!(matches!(
            destroy(&root, "entity:Sales.Order"),
            Err(ScaffoldError::UnsafeScaffoldPath(_))
        ));
        assert!(matches!(
            entries(&root),
            Err(ScaffoldError::UnsafeScaffoldPath(_))
        ));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
    }

    #[test]
    fn a_malformed_registry_fails_loudly_instead_of_being_reset() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        registry(root, "not json");
        assert!(matches!(
            entries(root),
            Err(ScaffoldError::RegistryInvalid(_))
        ));
        registry(root, r#"{"scaffolds":[]}"#);
        assert!(matches!(
            entries(root),
            Err(ScaffoldError::RegistryInvalid(_))
        ));
        registry(root, r#"{"scaffolds":{"entity:A":{}}}"#);
        assert!(matches!(
            entries(root),
            Err(ScaffoldError::RegistryInvalid(_))
        ));
        registry(
            root,
            r#"{"scaffolds":{"entity:A":{"files":[{"path":"a.rs"}]}}}"#,
        );
        assert!(matches!(
            entries(root),
            Err(ScaffoldError::RegistryInvalid(_))
        ));
        registry(root, r#"{}"#);
        assert!(entries(root).unwrap().is_empty());
        assert!(entries(directory.path().join("absent")).unwrap().is_empty());
    }

    #[test]
    fn staging_records_the_digest_of_the_staged_content_not_the_disk_content() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let file = root.join("src/order.rs");
        let mut transaction = Transaction::default();
        transaction.create(&file, "generated".into()).unwrap();
        let path = stage(&mut transaction, root, "entity:Sales.Order", &[file]).unwrap();
        assert_eq!(path, root.join(RELATIVE_PATH));
        transaction.commit().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&format!("{:x}", Sha256::digest(b"generated"))));
        assert!(text.contains("src/order.rs"));
        assert!(text.ends_with("}\n"));
        assert_eq!(destroy(root, "entity:Sales.Order").unwrap().files.len(), 1);
    }
}
