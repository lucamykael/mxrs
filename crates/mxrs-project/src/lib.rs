//! Cargo-native project support.
//!
//! An imported `.mpr` is decomposed into explicit, versioned `.mxdoc`
//! assets plus a containment manifest. The original `.mpr` is not needed
//! again: [`rebuild_imported_project`] creates a new artifact from that
//! snapshot and then applies the user-authored [`mxrs_ir::ProjectDecl`].
//! This gives unsupported model concepts a lossless generated home while
//! typed Rust coverage grows incrementally.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use mxrs_mpr::{MprFile, format};
use serde::{Deserialize, Serialize};

const SNAPSHOT_VERSION: u32 = 1;
const MANIFEST_NAME: &str = "manifest.json";

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error(transparent)]
    Bson(#[from] mxrs_bson::BsonCodecError),
    #[error(transparent)]
    Mpr(#[from] mxrs_mpr::MprError),
    #[error(transparent)]
    Writer(#[from] mxrs_writer::WriterError),
    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid imported-project manifest: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("snapshot destination {0} already contains files")]
    SnapshotExists(String),
    #[error("build output {0} already exists")]
    OutputExists(String),
    #[error("source project has no root unit")]
    MissingRoot,
    #[error("source project has no Mendix version")]
    MissingVersion,
    #[error("snapshot format {0} is unsupported")]
    UnsupportedSnapshot(u32),
    #[error("snapshot has no unit metadata for root {0}")]
    MissingRootMetadata(String),
    #[error("snapshot unit {unit_id} document id resolved to {actual_id}")]
    UnitIdentityMismatch { unit_id: String, actual_id: String },
    #[error("source unit {0} has no content")]
    MissingUnitContents(String),
    #[error("snapshot unit {unit_id} has an unsafe file path {file:?}")]
    UnsafeUnitPath { unit_id: String, file: String },
    #[error("snapshot contains units whose containers cannot be resolved: {0:?}")]
    UnresolvedContainers(Vec<String>),
}

pub type Result<T> = std::result::Result<T, ProjectError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportedUnit {
    pub unit_id: String,
    pub container_id: String,
    pub containment_name: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportedProjectManifest {
    pub snapshot_version: u32,
    pub project_name: String,
    pub mendix_version: String,
    pub schema_hash: String,
    pub root_id: String,
    pub units: Vec<ImportedUnit>,
}

/// Decomposes every unit of an existing `.mpr` into an explicit generated
/// snapshot. `destination` must not already contain files.
pub fn capture_imported_project(
    mpr_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<ImportedProjectManifest> {
    let source = MprFile::open(mpr_path, true)?;
    let destination = destination.as_ref();
    if destination.is_dir()
        && std::fs::read_dir(destination)
            .map_err(|error| io_error(destination, error))?
            .next()
            .is_some()
    {
        return Err(ProjectError::SnapshotExists(
            destination.display().to_string(),
        ));
    }
    let root = source.root_unit()?.ok_or(ProjectError::MissingRoot)?;
    let root_document = source.parse_contents(&root)?;
    let project_name = root_document
        .get_str("Name")
        .unwrap_or_else(|_| {
            source
                .path()
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Project")
        })
        .to_string();
    let mendix_version = source
        .mendix_version()?
        .filter(|version| !version.is_empty())
        .ok_or(ProjectError::MissingVersion)?;
    let schema_hash = source.schema_hash()?.unwrap_or_default();
    let units_directory = destination.join("units");
    std::fs::create_dir_all(&units_directory).map_err(|error| io_error(&units_directory, error))?;

    let mut units = source.all_units()?;
    units.sort_by(|left, right| left.unit_id.cmp(&right.unit_id));
    let imported_units = units
        .into_iter()
        .map(|unit| {
            let file = format!("units/{}.mxdoc", unit.unit_id);
            let path = destination.join(&file);
            let bytes = source
                .content_bytes(&unit)?
                .ok_or_else(|| ProjectError::MissingUnitContents(unit.unit_id.clone()))?;
            std::fs::write(&path, bytes).map_err(|error| io_error(&path, error))?;
            Ok(ImportedUnit {
                unit_id: unit.unit_id,
                container_id: unit.container_id,
                containment_name: unit.containment_name,
                file,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = ImportedProjectManifest {
        snapshot_version: SNAPSHOT_VERSION,
        project_name,
        mendix_version,
        schema_hash,
        root_id: root.unit_id,
        units: imported_units,
    };
    let manifest_path = destination.join(MANIFEST_NAME);
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    std::fs::write(&manifest_path, bytes).map_err(|error| io_error(&manifest_path, error))?;
    Ok(manifest)
}

pub fn read_imported_manifest(snapshot: impl AsRef<Path>) -> Result<ImportedProjectManifest> {
    let path = snapshot.as_ref().join(MANIFEST_NAME);
    let bytes = std::fs::read(&path).map_err(|error| io_error(&path, error))?;
    let manifest: ImportedProjectManifest = serde_json::from_slice(&bytes)?;
    if manifest.snapshot_version != SNAPSHOT_VERSION {
        return Err(ProjectError::UnsupportedSnapshot(manifest.snapshot_version));
    }
    Ok(manifest)
}

/// Reconstructs a new `.mpr` from generated import assets, then applies the
/// typed Rust declaration as an overlay. The destination must not exist.
pub fn rebuild_imported_project(
    snapshot: impl AsRef<Path>,
    output: impl AsRef<Path>,
    declaration: &mxrs_ir::ProjectDecl,
) -> Result<PathBuf> {
    let snapshot = snapshot.as_ref();
    let output = output.as_ref();
    let contents = format::contents_dir(output);
    if output.exists() || contents.exists() {
        return Err(ProjectError::OutputExists(output.display().to_string()));
    }
    let manifest = read_imported_manifest(snapshot)?;
    let root = manifest
        .units
        .iter()
        .find(|unit| unit.unit_id == manifest.root_id)
        .ok_or_else(|| ProjectError::MissingRootMetadata(manifest.root_id.clone()))?;
    let result = rebuild(snapshot, output, declaration, &manifest, root);
    if result.is_err() {
        let _ = std::fs::remove_file(output);
        let _ = std::fs::remove_dir_all(&contents);
    }
    result.map(|()| output.to_path_buf())
}

fn rebuild(
    snapshot: &Path,
    output: &Path,
    declaration: &mxrs_ir::ProjectDecl,
    manifest: &ImportedProjectManifest,
    root: &ImportedUnit,
) -> Result<()> {
    let mut target = MprFile::create_with_root_id(
        output,
        &manifest.mendix_version,
        &manifest.schema_hash,
        &manifest.root_id,
    )?;
    let root_document = read_unit(snapshot, root)?;
    target.update_unit(&manifest.root_id, root_document)?;

    let mut restored = HashSet::from([manifest.root_id.clone()]);
    let mut pending = manifest
        .units
        .iter()
        .filter(|unit| unit.unit_id != manifest.root_id)
        .collect::<Vec<_>>();
    while !pending.is_empty() {
        let before = pending.len();
        let mut index = 0;
        while index < pending.len() {
            if restored.contains(&pending[index].container_id) {
                let unit = pending.remove(index);
                let document = read_unit(snapshot, unit)?;
                let inserted = target.insert_unit(
                    &unit.container_id,
                    &unit.containment_name,
                    document,
                    Some(&unit.unit_id),
                )?;
                if inserted != unit.unit_id {
                    return Err(ProjectError::UnitIdentityMismatch {
                        unit_id: unit.unit_id.clone(),
                        actual_id: inserted,
                    });
                }
                restored.insert(unit.unit_id.clone());
            } else {
                index += 1;
            }
        }
        if pending.len() == before {
            return Err(ProjectError::UnresolvedContainers(
                pending.iter().map(|unit| unit.unit_id.clone()).collect(),
            ));
        }
    }
    drop(target);
    mxrs_writer::synchronize_project(output, declaration)?;
    Ok(())
}

fn read_unit(snapshot: &Path, unit: &ImportedUnit) -> Result<mxrs_bson::Document> {
    let relative = Path::new(&unit.file);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ProjectError::UnsafeUnitPath {
            unit_id: unit.unit_id.clone(),
            file: unit.file.clone(),
        });
    }
    let path = snapshot.join(&unit.file);
    let bytes = std::fs::read(&path).map_err(|error| io_error(&path, error))?;
    Ok(mxrs_bson::parse(&bytes)?)
}

fn io_error(path: &Path, source: std::io::Error) -> ProjectError {
    ProjectError::Io {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mxrs_dsl::ProjectBuilder;

    use super::*;

    #[test]
    fn captures_and_rebuilds_without_using_the_source_mpr() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("Source.mpr");
        let snapshot = directory.path().join("model/imported");
        let output_directory = tempfile::tempdir().unwrap();
        let output = output_directory.path().join("Built.mpr");
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
            module.microflow("ACT_Ping", |_flow| {});
        });
        let declaration = project.build();
        mxrs_writer::write_project(&source_path, &declaration).unwrap();

        let manifest = capture_imported_project(&source_path, &snapshot).unwrap();
        assert!(manifest.units.len() > 1);
        std::fs::remove_file(&source_path).unwrap();
        std::fs::remove_dir_all(format::contents_dir(&source_path)).unwrap();

        rebuild_imported_project(&snapshot, &output, &declaration).unwrap();
        let rebuilt = MprFile::open(&output, true).unwrap();
        assert_eq!(
            rebuilt.mendix_version().unwrap().as_deref(),
            Some("11.12.1")
        );
        assert_eq!(
            rebuilt.root_unit().unwrap().unwrap().unit_id,
            manifest.root_id
        );
        let documents = rebuilt
            .all_units()
            .unwrap()
            .into_iter()
            .map(|unit| {
                let id = unit.unit_id.clone();
                (id, rebuilt.parse_contents(&unit).unwrap())
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(documents.len(), manifest.units.len());
        assert!(documents.values().any(|document| {
            document.get_str("$Type").ok() == Some("Microflows$Microflow")
                && document.get_str("Name").ok() == Some("ACT_Ping")
        }));
    }

    #[test]
    fn refuses_to_overwrite_snapshot_or_build_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("Source.mpr");
        let snapshot = directory.path().join("imported");
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |_module| {});
        let declaration = project.build();
        mxrs_writer::write_project(&source, &declaration).unwrap();
        capture_imported_project(&source, &snapshot).unwrap();
        assert!(matches!(
            capture_imported_project(&source, &snapshot),
            Err(ProjectError::SnapshotExists(_))
        ));

        let output_directory = tempfile::tempdir().unwrap();
        let output = output_directory.path().join("Built.mpr");
        rebuild_imported_project(&snapshot, &output, &declaration).unwrap();
        assert!(matches!(
            rebuild_imported_project(&snapshot, &output, &declaration),
            Err(ProjectError::OutputExists(_))
        ));
    }

    #[test]
    fn rejects_unit_paths_that_escape_the_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("Source.mpr");
        let snapshot = directory.path().join("imported");
        let output_directory = tempfile::tempdir().unwrap();
        let output = output_directory.path().join("Built.mpr");
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |_module| {});
        let declaration = project.build();
        mxrs_writer::write_project(&source, &declaration).unwrap();
        let mut manifest = capture_imported_project(&source, &snapshot).unwrap();
        manifest.units[0].file = "../outside.mxdoc".to_string();
        std::fs::write(
            snapshot.join(MANIFEST_NAME),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            rebuild_imported_project(&snapshot, &output, &declaration),
            Err(ProjectError::UnsafeUnitPath { .. })
        ));
        assert!(!output.exists());
        assert!(!format::contents_dir(&output).exists());
    }
}
