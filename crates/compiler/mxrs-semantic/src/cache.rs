//! Persistent, derivative semantic-index cache.
//!
//! Cache files live outside the MPR and are keyed by its absolute path. The
//! source fingerprint is derived from the MPR's unit identities, containment,
//! and native content hashes, so a normal writer transaction invalidates a
//! stale entry without reparsing every BSON document.

use std::path::{Path, PathBuf};

use mxrs_model::Project;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, SemanticError, SemanticIndex};

const CACHE_FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CacheInfo {
    pub path: PathBuf,
    pub present: bool,
    pub hit: bool,
    pub entries: usize,
    pub bytes: u64,
    pub current_fingerprint: String,
    pub stored_fingerprint: Option<String>,
    pub removed: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: u32,
    source_fingerprint: String,
    index: SemanticIndex,
}

#[derive(Debug, Clone)]
pub struct SemanticCache {
    root: PathBuf,
}

impl Default for SemanticCache {
    fn default() -> Self {
        let root = std::env::var_os("MXRS_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("mxrs/semantic");
        Self { root }
    }
}

impl SemanticCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn status(&self, project: &Project) -> Result<CacheInfo> {
        let path = self.path_for(project)?;
        let current_fingerprint = source_fingerprint(project)?;
        if !path.is_file() {
            return Ok(CacheInfo {
                path,
                present: false,
                hit: false,
                entries: 0,
                bytes: 0,
                current_fingerprint,
                stored_fingerprint: None,
                removed: None,
            });
        }
        let bytes = std::fs::read(&path).map_err(|source| cache_io(&path, source))?;
        let envelope = decode(&path, &bytes)?;
        Ok(CacheInfo {
            path,
            present: true,
            hit: envelope.source_fingerprint == current_fingerprint,
            entries: envelope.index.artifacts().count(),
            bytes: bytes.len() as u64,
            current_fingerprint,
            stored_fingerprint: Some(envelope.source_fingerprint),
            removed: None,
        })
    }

    pub fn warm(&self, project: &Project) -> Result<CacheInfo> {
        let path = self.path_for(project)?;
        let source_fingerprint = source_fingerprint(project)?;
        let index = SemanticIndex::build(project)?;
        let envelope = Envelope {
            format: CACHE_FORMAT,
            source_fingerprint,
            index,
        };
        let bytes = serde_json::to_vec(&envelope).map_err(|error| invalid(&path, error))?;
        write_atomic(&path, &bytes)?;
        self.status(project)
    }

    pub fn clear(&self, project: &Project) -> Result<CacheInfo> {
        let path = self.path_for(project)?;
        let current_fingerprint = source_fingerprint(project)?;
        let present = path.is_file();
        if present {
            std::fs::remove_file(&path).map_err(|source| cache_io(&path, source))?;
        }
        Ok(CacheInfo {
            path,
            present: false,
            hit: false,
            entries: 0,
            bytes: 0,
            current_fingerprint,
            stored_fingerprint: None,
            removed: Some(usize::from(present)),
        })
    }

    pub fn get(&self, project: &Project) -> Result<Option<SemanticIndex>> {
        let path = self.path_for(project)?;
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|source| cache_io(&path, source))?;
        let envelope = decode(&path, &bytes)?;
        Ok((envelope.source_fingerprint == source_fingerprint(project)?).then_some(envelope.index))
    }

    fn path_for(&self, project: &Project) -> Result<PathBuf> {
        let path = std::path::absolute(project.mpr().path())
            .map_err(|source| cache_io(project.mpr().path(), source))?;
        let key = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
        Ok(self.root.join(format!("{key}.json")))
    }
}

pub fn cached_or_build(project: &Project) -> Result<SemanticIndex> {
    SemanticCache::default()
        .get(project)?
        .map_or_else(|| SemanticIndex::build(project), Ok)
}

fn source_fingerprint(project: &Project) -> Result<String> {
    let mut units = project
        .mpr()
        .all_units()
        .map_err(mxrs_model::ModelError::from)?;
    units.sort_by(|left, right| left.unit_id.cmp(&right.unit_id));
    let mut hasher = Sha256::new();
    if let Some(version) = project.mendix_version()? {
        hasher.update(version.as_bytes());
    }
    if let Some(schema) = project
        .mpr()
        .schema_hash()
        .map_err(mxrs_model::ModelError::from)?
    {
        hasher.update(schema.as_bytes());
    }
    for unit in units {
        for value in [
            unit.unit_id.as_str(),
            unit.container_id.as_str(),
            unit.containment_name.as_str(),
            unit.contents_hash.as_deref().unwrap_or(""),
        ] {
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn decode(path: &Path, bytes: &[u8]) -> Result<Envelope> {
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|error| invalid(path, error))?;
    if envelope.format != CACHE_FORMAT {
        return Err(SemanticError::InvalidCache {
            path: path.display().to_string(),
            reason: format!("unsupported format {}", envelope.format),
        });
    }
    Ok(envelope)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| cache_io(parent, source))?;
    for attempt in 0..100_u8 {
        let temporary = path.with_extension(format!("{}.{}.tmp", std::process::id(), attempt));
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary);
        let mut file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(cache_io(&temporary, error)),
        };
        let result = (|| {
            file.write_all(bytes)
                .map_err(|source| cache_io(&temporary, source))?;
            file.sync_all()
                .map_err(|source| cache_io(&temporary, source))?;
            std::fs::rename(&temporary, path).map_err(|source| cache_io(path, source))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        return result;
    }
    Err(SemanticError::InvalidCache {
        path: path.display().to_string(),
        reason: "could not allocate an atomic cache temporary file".to_string(),
    })
}

fn cache_io(path: &Path, source: std::io::Error) -> SemanticError {
    SemanticError::CacheIo {
        path: path.display().to_string(),
        source,
    }
}

fn invalid(path: &Path, error: impl std::fmt::Display) -> SemanticError {
    SemanticError::InvalidCache {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_hit_invalidation_and_clear_follow_the_source_fingerprint() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.microflow("Start", |_| {});
        });
        let mut declaration = builder.build();
        mxrs_writer::write_project(&path, &declaration).unwrap();
        let cache = SemanticCache::new(directory.path().join("cache"));
        let project = Project::open(&path, true).unwrap();
        assert!(!cache.status(&project).unwrap().present);
        let warm = cache.warm(&project).unwrap();
        assert!(warm.present && warm.hit && warm.entries > 0);
        assert_eq!(
            cache.get(&project).unwrap().unwrap().fingerprint(),
            SemanticIndex::build(&project).unwrap().fingerprint()
        );
        drop(project);

        declaration.modules[0].microflows[0].documentation = "changed".to_string();
        mxrs_writer::synchronize_project(&path, &declaration).unwrap();
        let project = Project::open(&path, true).unwrap();
        assert!(!cache.status(&project).unwrap().hit);
        assert!(cache.get(&project).unwrap().is_none());
        let cache_path = cache.path_for(&project).unwrap();
        std::fs::write(&cache_path, "corrupt").unwrap();
        assert!(cache.status(&project).is_err());
        let cleared = cache.clear(&project).unwrap();
        assert_eq!(cleared.removed, Some(1));
        assert!(!cleared.present);
    }
}
