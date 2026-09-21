//! The marketplace lockfile — which packages are installed where, backed by
//! which cached archive, and which project files they own. Ports the
//! `marketplace.lock.json` shape `Mxrb::OfficialMarketplace` writes
//! (`lib/mxrb/official_marketplace.rb`'s `lock`/`write_module_lock`/
//! `write_lock_atomically`), stored under `.mxrs/` because this CLI manages
//! its own installs; the JSON field names stay mxrb's so a lock is readable
//! by anyone who knows that format.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{MarketplaceError, Result};

pub const LOCK_RELATIVE: &str = ".mxrs/marketplace.lock.json";
pub const CACHE_RELATIVE: &str = ".mxrs/marketplace";
pub const ORIGINALS_RELATIVE: &str = ".mxrs/marketplace-originals";

/// One installed package. `asset_originals` maps each owned project file to
/// the backup taken before the package first overwrote it (`None` when the
/// file did not exist), so removal can restore what the install replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LockEntry {
    #[serde(default)]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub destination: String,
    #[serde(default)]
    pub archive: String,
    #[serde(default)]
    pub module_id: String,
    #[serde(default)]
    pub units: usize,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub asset_originals: BTreeMap<String, Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Lock {
    #[serde(default)]
    pub packages: BTreeMap<String, LockEntry>,
}

pub fn lock_path(target: &Path) -> PathBuf {
    target.join(LOCK_RELATIVE)
}

/// Reads the lockfile; a missing file is an empty lock, a corrupt one is a
/// loud error (mxrb's `invalid marketplace lockfile`).
pub fn read_lock(target: &Path) -> Result<Lock> {
    let path = lock_path(target);
    if !path.is_file() {
        return Ok(Lock::default());
    }
    let source = std::fs::read_to_string(&path).map_err(|source| MarketplaceError::Download {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&source).map_err(|error| MarketplaceError::InvalidLock {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

/// Writes the lockfile through a same-directory temporary plus rename, so a
/// crash never leaves a half-written lock behind.
pub fn write_lock(target: &Path, lock: &Lock) -> Result<()> {
    let path = lock_path(target);
    let io_error = |source: std::io::Error| MarketplaceError::Download {
        path: path.display().to_string(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    let serialized =
        serde_json::to_string_pretty(lock).expect("lock entries are always serializable");
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temporary, format!("{serialized}\n")).map_err(io_error)?;
    std::fs::rename(&temporary, &path).map_err(io_error)
}

/// Resolves a lock-relative path under `target`, refusing anything that
/// escapes it — the same guard mxrb's `safe_path` applies to every path the
/// lockfile supplies.
pub fn safe_target_path(target: &Path, relative: &str) -> Result<PathBuf> {
    let target = std::path::absolute(target).map_err(|source| MarketplaceError::Download {
        path: target.display().to_string(),
        source,
    })?;
    let mut resolved = target.clone();
    for component in Path::new(relative).components() {
        match component {
            std::path::Component::Normal(part) => resolved.push(part),
            std::path::Component::CurDir => {}
            _ => {
                return Err(MarketplaceError::UnsafePackagePath(relative.to_string()));
            }
        }
    }
    if !resolved.starts_with(&target) || resolved == target {
        return Err(MarketplaceError::UnsafePackagePath(relative.to_string()));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_lock_is_empty_and_round_trips_through_write() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(read_lock(directory.path()).unwrap(), Lock::default());

        let mut lock = Lock::default();
        lock.packages.insert(
            "CommunityCommons".into(),
            LockEntry {
                kind: "module".into(),
                version: Some("10.0.0".into()),
                sha256: "abc".into(),
                destination: "App.mpr".into(),
                archive: ".mxrs/marketplace/CommunityCommons-10.0.0.mpk".into(),
                module_id: "id-1".into(),
                units: 12,
                files: vec!["javasource/x.java".into()],
                ..LockEntry::default()
            },
        );
        write_lock(directory.path(), &lock).unwrap();
        assert_eq!(read_lock(directory.path()).unwrap(), lock);

        std::fs::write(lock_path(directory.path()), "not json").unwrap();
        assert!(read_lock(directory.path()).is_err());
    }

    #[test]
    fn lock_relative_paths_cannot_escape_the_target() {
        let directory = tempfile::tempdir().unwrap();
        assert!(safe_target_path(directory.path(), "javasource/ok.java").is_ok());
        for hostile in ["../outside", "/etc/passwd", "a/../../outside", ""] {
            assert!(
                safe_target_path(directory.path(), hostile).is_err(),
                "{hostile}"
            );
        }
    }
}
