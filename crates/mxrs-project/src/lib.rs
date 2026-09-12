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
use std::sync::atomic::{AtomicU64, Ordering};

use mxrs_mpr::{MprFile, format};
use serde::{Deserialize, Serialize};

const SNAPSHOT_VERSION: u32 = 2;
const MANIFEST_NAME: &str = "manifest.json";
static BUILD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub const PROJECT_ASSET_DIRECTORIES: &[&str] = &[
    "theme",
    "theme-cache",
    "themesource",
    "resources",
    "widgets",
    "javasource",
    "javascriptsource",
    "userlib",
    "vendorlib",
];

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
    pub native_type: String,
    pub name: Option<String>,
    pub qualified_name: Option<String>,
    pub file: String,
}

/// Compile-time entry emitted into an imported application's generated Rust
/// registry. The `.mxdoc` remains the lossless payload; this metadata makes
/// every opaque document discoverable without parsing the snapshot manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportedDocumentRef {
    pub unit_id: &'static str,
    pub container_id: &'static str,
    pub containment_name: &'static str,
    pub native_type: &'static str,
    pub name: Option<&'static str>,
    pub qualified_name: Option<&'static str>,
    pub file: &'static str,
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
            let document = mxrs_bson::parse(&bytes)?;
            std::fs::write(&path, bytes).map_err(|error| io_error(&path, error))?;
            Ok(ImportedUnit {
                unit_id: unit.unit_id,
                container_id: unit.container_id,
                containment_name: unit.containment_name,
                native_type: document.get_str("$Type").unwrap_or("").to_string(),
                name: document
                    .get_str("Name")
                    .or_else(|_| document.get_str("name"))
                    .ok()
                    .map(str::to_string),
                qualified_name: document
                    .get_str("$QualifiedName")
                    .or_else(|_| document.get_str("QualifiedName"))
                    .ok()
                    .map(str::to_string),
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

/// Copies Mendix project assets into the Cargo project's editable `assets/`
/// tree. Symbolic links are skipped so an import cannot escape its source
/// project directory.
pub fn capture_project_assets(
    mpr_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<usize> {
    let source_root = mpr_path.as_ref().parent().unwrap_or_else(|| Path::new("."));
    let destination = destination.as_ref();
    let mut copied = 0;
    for directory in PROJECT_ASSET_DIRECTORIES {
        copied += copy_regular_tree(&source_root.join(directory), &destination.join(directory))?;
    }
    Ok(copied)
}

/// Materializes editable Cargo-project assets next to a built `.mpr`.
pub fn materialize_project_assets(
    assets: impl AsRef<Path>,
    output_mpr: impl AsRef<Path>,
) -> Result<usize> {
    let output = std::path::absolute(output_mpr.as_ref())
        .map_err(|error| io_error(output_mpr.as_ref(), error))?;
    let output_root = output.parent().unwrap_or_else(|| Path::new("."));
    let mut copied = 0;
    for directory in PROJECT_ASSET_DIRECTORIES {
        copied += copy_regular_tree(
            &assets.as_ref().join(directory),
            &output_root.join(directory),
        )?;
    }
    Ok(copied)
}

fn copy_regular_tree(source: &Path, destination: &Path) -> Result<usize> {
    if !source.is_dir() {
        return Ok(0);
    }
    std::fs::create_dir_all(destination).map_err(|error| io_error(destination, error))?;
    let mut copied = 0;
    for entry in std::fs::read_dir(source).map_err(|error| io_error(source, error))? {
        let entry = entry.map_err(|error| io_error(source, error))?;
        let file_type = entry
            .file_type()
            .map_err(|error| io_error(&entry.path(), error))?;
        if file_type.is_symlink() {
            continue;
        }
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copied += copy_regular_tree(&entry.path(), &target)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &target).map_err(|error| io_error(&target, error))?;
            copied += 1;
        }
    }
    Ok(copied)
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
    restore_imported_project(snapshot, output)?;
    if let Err(error) = mxrs_writer::synchronize_project(output, declaration) {
        let _ = std::fs::remove_file(output);
        let _ = std::fs::remove_dir_all(&contents);
        return Err(error.into());
    }
    Ok(output.to_path_buf())
}

/// Reconstructs exactly the imported `.mpr` snapshot without applying the
/// typed Rust overlay. Used as the baseline for `cargo mxrs diff`.
pub fn restore_imported_project(
    snapshot: impl AsRef<Path>,
    output: impl AsRef<Path>,
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
    let result = restore(snapshot, output, &manifest, root);
    if result.is_err() {
        let _ = std::fs::remove_file(output);
        let _ = std::fs::remove_dir_all(&contents);
    }
    result.map(|()| output.to_path_buf())
}

/// Builds through a sibling staging directory and replaces an earlier build
/// only after the new `.mpr` and `mprcontents` pair has completed.
pub fn replace_imported_project(
    snapshot: impl AsRef<Path>,
    output: impl AsRef<Path>,
    declaration: &mxrs_ir::ProjectDecl,
) -> Result<PathBuf> {
    let output =
        std::path::absolute(output.as_ref()).map_err(|error| io_error(output.as_ref(), error))?;
    let output_contents = format::contents_dir(&output);
    if !output.exists() && !output_contents.exists() {
        return rebuild_imported_project(snapshot, &output, declaration);
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let sequence = BUILD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = parent.join(format!(".mxrs-build-{}-{sequence}", std::process::id()));
    let fresh_directory = staging.join("fresh");
    let previous_directory = staging.join("previous");
    std::fs::create_dir_all(&fresh_directory).map_err(|error| io_error(&fresh_directory, error))?;
    std::fs::create_dir_all(&previous_directory)
        .map_err(|error| io_error(&previous_directory, error))?;
    let file_name = output.file_name().unwrap_or_default();
    let fresh_output = fresh_directory.join(file_name);
    if let Err(error) = rebuild_imported_project(snapshot, &fresh_output, declaration) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    let previous_output = previous_directory.join(file_name);
    let previous_contents = previous_directory.join("mprcontents");
    let had_output = output.exists();
    let had_contents = output_contents.exists();
    let replacement = (|| -> Result<()> {
        if had_output {
            std::fs::rename(&output, &previous_output).map_err(|error| io_error(&output, error))?;
        }
        if had_contents {
            std::fs::rename(&output_contents, &previous_contents)
                .map_err(|error| io_error(&output_contents, error))?;
        }
        std::fs::rename(&fresh_output, &output).map_err(|error| io_error(&output, error))?;
        std::fs::rename(format::contents_dir(&fresh_output), &output_contents)
            .map_err(|error| io_error(&output_contents, error))?;
        Ok(())
    })();
    if let Err(error) = replacement {
        let _ = std::fs::remove_file(&output);
        let _ = std::fs::remove_dir_all(&output_contents);
        if had_output {
            let _ = std::fs::rename(&previous_output, &output);
        }
        if had_contents {
            let _ = std::fs::rename(&previous_contents, &output_contents);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    std::fs::remove_dir_all(&staging).map_err(|error| io_error(&staging, error))?;
    Ok(output)
}

fn restore(
    snapshot: &Path,
    output: &Path,
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
        assert!(manifest.units.iter().any(|unit| {
            unit.native_type == "Microflows$Microflow" && unit.name.as_deref() == Some("ACT_Ping")
        }));
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

    #[test]
    fn replace_build_supports_repeatable_artifact_builds() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("Source.mpr");
        let snapshot = directory.path().join("imported");
        let output_directory = tempfile::tempdir().unwrap();
        let output = output_directory.path().join("Built.mpr");
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
        });
        let declaration = project.build();
        mxrs_writer::write_project(&source, &declaration).unwrap();
        capture_imported_project(&source, &snapshot).unwrap();

        replace_imported_project(&snapshot, &output, &declaration).unwrap();
        replace_imported_project(&snapshot, &output, &declaration).unwrap();

        let rebuilt = MprFile::open(&output, true).unwrap();
        assert_eq!(
            rebuilt.mendix_version().unwrap().as_deref(),
            Some("11.12.1")
        );
        assert!(format::contents_dir(&output).is_dir());
        assert!(
            std::fs::read_dir(output_directory.path())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".mxrs-build-"))
        );
    }
}
