//! Cargo-native project support.
//!
//! An imported `.mpr` is decomposed into explicit, versioned `.mxdoc`
//! assets plus a containment manifest. The original `.mpr` is not needed
//! again: [`rebuild_imported_project`] creates a new artifact from that
//! snapshot and then applies the user-authored [`mxrs_ir::ProjectDecl`].
//! This gives unsupported model concepts a lossless generated home while
//! typed Rust coverage grows incrementally.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use mxrs_mpr::{MprFile, format};
use serde::{Deserialize, Serialize};

const SNAPSHOT_VERSION: u32 = 2;
const MANIFEST_NAME: &str = "manifest.json";
const DECLARED_NAME: &str = "declared.json";
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
    #[error(
        "{folder} is missing, and a build without it would remove the {count} unit(s) the import declared there; restore it, or create it empty to remove them"
    )]
    MissingDeclarationSource { folder: String, count: usize },
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

/// A unit of the snapshot the import wrote as a declaration in source: its
/// type, its module and its name, which a declaration matches it by.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeclaredUnit {
    pub unit_id: String,
    pub native_type: String,
    pub module: String,
    pub name: String,
    /// The folder of the project, relative to its root, a build reads the
    /// declaration from when that is not the Rust source: one a checkout
    /// lacks is not read, which says nothing about what it declares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Records beside the snapshot which of its units the import declared in
/// source, given as `(type, module, name)`: a build leaves out of the model
/// each one the source no longer declares. A unit the import kept in the
/// snapshot only is not one of them.
pub fn record_declared_units(
    snapshot: impl AsRef<Path>,
    manifest: &ImportedProjectManifest,
    declared: &BTreeSet<(String, String, String)>,
) -> Result<Vec<DeclaredUnit>> {
    let declared = declared.iter().map(|key| (key.clone(), None)).collect();
    record_declared_units_from(snapshot, manifest, &declared)
}

/// [`record_declared_units`], each unit with the folder a build reads its
/// declaration from when that is not the Rust source.
pub fn record_declared_units_from(
    snapshot: impl AsRef<Path>,
    manifest: &ImportedProjectManifest,
    declared: &BTreeMap<(String, String, String), Option<String>>,
) -> Result<Vec<DeclaredUnit>> {
    let units: HashMap<&str, &ImportedUnit> = manifest
        .units
        .iter()
        .map(|unit| (unit.unit_id.as_str(), unit))
        .collect();
    // A document's module is the module its folders are in; the project,
    // which contains itself, is in none.
    let module_of = |unit: &ImportedUnit| {
        let mut seen = HashSet::new();
        let mut container = units.get(unit.container_id.as_str());
        while let Some(parent) = container {
            if !seen.insert(parent.unit_id.as_str()) {
                return None;
            }
            if parent.native_type.starts_with("Projects$Module") {
                return parent.name.clone();
            }
            container = units.get(parent.container_id.as_str());
        }
        None
    };
    let mut recorded: Vec<DeclaredUnit> = manifest
        .units
        .iter()
        .filter_map(|unit| {
            let name = unit.name.clone()?;
            let module = module_of(unit)?;
            let key = (unit.native_type.clone(), module, name);
            let source = declared.get(&key)?.clone();
            Some(DeclaredUnit {
                unit_id: unit.unit_id.clone(),
                native_type: key.0,
                module: key.1,
                name: key.2,
                source,
            })
        })
        .collect();
    recorded.sort();
    let path = snapshot.as_ref().join(DECLARED_NAME);
    let bytes = serde_json::to_vec_pretty(&recorded)?;
    std::fs::write(&path, bytes).map_err(|error| io_error(&path, error))?;
    Ok(recorded)
}

/// The units the import declared in source that `declaration` no longer
/// declares, but for those `kept` names as `(type, module, name)`: what the
/// source keeps from the imported model. A snapshot taken before imports
/// recorded them has none.
pub fn undeclared_units(
    snapshot: impl AsRef<Path>,
    declaration: &mxrs_ir::ProjectDecl,
    kept: &BTreeSet<(String, String, String)>,
) -> Result<Vec<DeclaredUnit>> {
    let path = snapshot.as_ref().join(DECLARED_NAME);
    let recorded: Vec<DeclaredUnit> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(&path, error)),
    };
    Ok(recorded
        .into_iter()
        .filter(|unit| {
            !declares(declaration, unit)
                && !kept.contains(&(
                    unit.native_type.clone(),
                    unit.module.clone(),
                    unit.name.clone(),
                ))
        })
        .collect())
}

/// Whether `declaration` states the unit: a document of its type and name
/// in its module. A type a build does not remove is always stated.
fn declares(declaration: &mxrs_ir::ProjectDecl, unit: &DeclaredUnit) -> bool {
    let Some(module) = declaration
        .modules
        .iter()
        .find(|module| module.name == unit.module)
    else {
        return false;
    };
    let name = unit.name.as_str();
    let form = |kind: &str| {
        module
            .forms
            .iter()
            .any(|form| form.kind() == kind && form.name() == name)
    };
    match unit.native_type.as_str() {
        "Microflows$Microflow" => module.microflows.iter().any(|flow| flow.name == name),
        "Microflows$Nanoflow" => module.nanoflows.iter().any(|flow| flow.name == name),
        "Forms$Page" => module.pages.iter().any(|page| page.name == name) || form("Forms$Page"),
        "Forms$Layout" => {
            module.layouts.iter().any(|layout| layout.name == name) || form("Forms$Layout")
        }
        "Forms$Snippet" => form("Forms$Snippet"),
        "Enumerations$Enumeration" => module.enumerations.iter().any(|item| item.name == name),
        "Constants$Constant" => module.constants.iter().any(|item| item.name == name),
        "RegularExpressions$RegularExpression" => module
            .regular_expressions
            .iter()
            .any(|item| item.name == name),
        "ScheduledEvents$ScheduledEvent" => {
            module.scheduled_events.iter().any(|item| item.name == name)
        }
        "Menus$MenuDocument" => module.menus.iter().any(|item| item.name == name),
        "Queues$Queue" => module.task_queues.iter().any(|item| item.name == name),
        "JavaScriptActions$JavaScriptAction" => module
            .javascript_actions
            .iter()
            .any(|item| item.name == name),
        _ => true,
    }
}

/// What a unit is, in a developer's words.
fn kind_of(native_type: &str) -> &str {
    match native_type {
        "Microflows$Microflow" => "microflow",
        "Microflows$Nanoflow" => "nanoflow",
        "Forms$Page" => "page",
        "Forms$Layout" => "layout",
        "Forms$Snippet" => "snippet",
        "Enumerations$Enumeration" => "enumeration",
        "Constants$Constant" => "constant",
        "RegularExpressions$RegularExpression" => "regular expression",
        "ScheduledEvents$ScheduledEvent" => "scheduled event",
        "Menus$MenuDocument" => "menu",
        "Queues$Queue" => "task queue",
        "JavaScriptActions$JavaScriptAction" => "JavaScript action",
        other => other,
    }
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

/// Materializes a Cargo project's Java — the source of its Java actions,
/// kept in `java/` with Mendix's own layout — as the `javasource/` a built
/// `.mpr` runs it from.
pub fn materialize_java_sources(
    java: impl AsRef<Path>,
    output_mpr: impl AsRef<Path>,
) -> Result<usize> {
    let output = std::path::absolute(output_mpr.as_ref())
        .map_err(|error| io_error(output_mpr.as_ref(), error))?;
    let output_root = output.parent().unwrap_or_else(|| Path::new("."));
    copy_regular_tree(java.as_ref(), &output_root.join("javasource"))
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
    rebuild_imported_project_keeping(snapshot, output, declaration, &BTreeSet::new())
}

/// [`rebuild_imported_project`], keeping what the source names as the
/// imported model's (`(type, module, name)`) though it declares it no more.
pub fn rebuild_imported_project_keeping(
    snapshot: impl AsRef<Path>,
    output: impl AsRef<Path>,
    declaration: &mxrs_ir::ProjectDecl,
    kept: &BTreeSet<(String, String, String)>,
) -> Result<PathBuf> {
    let snapshot = snapshot.as_ref();
    let output = output.as_ref();
    let contents = format::contents_dir(output);
    restore_imported_project(snapshot, output)?;
    let built = mxrs_writer::synchronize_project(output, declaration)
        .map_err(ProjectError::from)
        .and_then(|()| remove_undeclared(snapshot, output, declaration, kept));
    if let Err(error) = built {
        let _ = std::fs::remove_file(output);
        let _ = std::fs::remove_dir_all(&contents);
        return Err(error);
    }
    Ok(output.to_path_buf())
}

/// Leaves out of the built model what the import declared in source and
/// the source no longer declares — a document deleted, or renamed, in Rust
/// or in the frontend — and says what it left out.
fn remove_undeclared(
    snapshot: &Path,
    output: &Path,
    declaration: &mxrs_ir::ProjectDecl,
    kept: &BTreeSet<(String, String, String)>,
) -> Result<()> {
    let removed = undeclared_units(snapshot, declaration, kept)?;
    if removed.is_empty() {
        return Ok(());
    }
    // The snapshot is `<project>/model/imported`.
    if let Some(root) = snapshot.parent().and_then(Path::parent) {
        let mut missing: BTreeMap<&str, usize> = BTreeMap::new();
        for source in removed.iter().filter_map(|unit| unit.source.as_deref()) {
            if !root.join(source).is_dir() {
                *missing.entry(source).or_default() += 1;
            }
        }
        if let Some((folder, count)) = missing.into_iter().next() {
            return Err(ProjectError::MissingDeclarationSource {
                folder: folder.to_string(),
                count,
            });
        }
    }
    let mut mpr = MprFile::open(output, false)?;
    mpr.transaction(|mpr| {
        for unit in &removed {
            mpr.delete_unit(&unit.unit_id)?;
        }
        Ok(())
    })?;
    for unit in &removed {
        eprintln!(
            "[mxrs] removed {} {}.{}: the source no longer declares it",
            kind_of(&unit.native_type),
            unit.module,
            unit.name
        );
    }
    Ok(())
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
    replace_imported_project_keeping(snapshot, output, declaration, &BTreeSet::new())
}

/// [`replace_imported_project`], keeping what the source names as the
/// imported model's (`(type, module, name)`) though it declares it no more.
pub fn replace_imported_project_keeping(
    snapshot: impl AsRef<Path>,
    output: impl AsRef<Path>,
    declaration: &mxrs_ir::ProjectDecl,
    kept: &BTreeSet<(String, String, String)>,
) -> Result<PathBuf> {
    let output =
        std::path::absolute(output.as_ref()).map_err(|error| io_error(output.as_ref(), error))?;
    let output_contents = format::contents_dir(&output);
    if !output.exists() && !output_contents.exists() {
        return rebuild_imported_project_keeping(snapshot, &output, declaration, kept);
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let sequence = BUILD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = parent.join(format!(".mxrs-build-{}-{sequence}", std::process::id()));
    let fresh_directory = staging.join("fresh");
    let previous_directory = staging.join("previous");
    prepare_build_staging(&staging, &fresh_directory, &previous_directory)?;
    let file_name = output.file_name().unwrap_or_default();
    let fresh_output = fresh_directory.join(file_name);
    if let Err(error) = rebuild_imported_project_keeping(snapshot, &fresh_output, declaration, kept)
    {
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

fn prepare_build_staging(
    staging: &Path,
    fresh_directory: &Path,
    previous_directory: &Path,
) -> Result<()> {
    // An interrupted build can leave this process-scoped path behind. At
    // sequence allocation time no live build in this process can own it, so
    // clearing it is both safe and necessary for retryability (PID reuse is
    // common in containers).
    if staging.exists() {
        std::fs::remove_dir_all(staging).map_err(|error| io_error(staging, error))?;
    }
    std::fs::create_dir_all(fresh_directory).map_err(|error| io_error(fresh_directory, error))?;
    std::fs::create_dir_all(previous_directory)
        .map_err(|error| io_error(previous_directory, error))?;
    Ok(())
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

/// The modules the imported model holds as installed from the Marketplace
/// (`FromAppStore`), by name: what the project declares nothing of in
/// Rust, and writes only what the frontend states of.
pub fn installed_modules(snapshot: impl AsRef<Path>) -> Result<std::collections::BTreeSet<String>> {
    let snapshot = snapshot.as_ref();
    // The manifest's units alone: a snapshot a test seeds may say no more.
    let manifest_path = snapshot.join("manifest.json");
    let text =
        std::fs::read_to_string(&manifest_path).map_err(|error| io_error(&manifest_path, error))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(ProjectError::Manifest)?;
    let mut installed = std::collections::BTreeSet::new();
    for unit in manifest["units"].as_array().into_iter().flatten() {
        let native_type = unit["native_type"].as_str().unwrap_or_default();
        if !matches!(native_type, "Projects$Module" | "Projects$ModuleImpl") {
            continue;
        }
        let (Some(file), Some(name)) = (unit["file"].as_str(), unit["name"].as_str()) else {
            continue;
        };
        let path = snapshot.join(file);
        let bytes = std::fs::read(&path).map_err(|error| io_error(&path, error))?;
        let document = mxrs_bson::parse(&bytes)?;
        if document.get_bool("FromAppStore").unwrap_or(false) {
            installed.insert(name.to_string());
        }
    }
    Ok(installed)
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

    /// What the import declared and the source no longer declares — a flow
    /// deleted or renamed — is left out of the build; what the import kept
    /// in the snapshot only, and what is still declared, stay.
    #[test]
    fn a_declaration_deleted_from_source_leaves_the_built_model() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("Source.mpr");
        let snapshot = directory.path().join("model/imported");
        let output_directory = tempfile::tempdir().unwrap();
        let output = output_directory.path().join("Built.mpr");
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.microflow("ACT_Kept", |_flow| {});
            module.microflow("ACT_Deleted", |_flow| {});
            module.microflow("ACT_Imported", |_flow| {});
        });
        mxrs_writer::write_project(&source_path, &project.build()).unwrap();
        let manifest = capture_imported_project(&source_path, &snapshot).unwrap();
        let declared = ["ACT_Kept", "ACT_Deleted"]
            .map(|name| {
                (
                    "Microflows$Microflow".to_string(),
                    "Sales".to_string(),
                    name.to_string(),
                )
            })
            .into_iter()
            .collect();
        let recorded = record_declared_units(&snapshot, &manifest, &declared).unwrap();
        assert_eq!(recorded.len(), 2);

        let mut source = ProjectBuilder::new("11.12.1");
        source.module("Sales", |module| {
            module.microflow("ACT_Kept", |_flow| {});
        });
        let declaration = source.build();
        assert_eq!(
            undeclared_units(&snapshot, &declaration, &BTreeSet::new())
                .unwrap()
                .iter()
                .map(|unit| unit.name.as_str())
                .collect::<Vec<_>>(),
            ["ACT_Deleted"]
        );
        rebuild_imported_project(&snapshot, &output, &declaration).unwrap();
        let rebuilt = MprFile::open(&output, true).unwrap();
        let mut flows = rebuilt
            .all_units()
            .unwrap()
            .into_iter()
            .filter_map(|unit| {
                let document = rebuilt.parse_contents(&unit).unwrap();
                (document.get_str("$Type").ok() == Some("Microflows$Microflow"))
                    .then(|| document.get_str("Name").unwrap().to_string())
            })
            .collect::<Vec<_>>();
        flows.sort();
        assert_eq!(flows, ["ACT_Imported", "ACT_Kept"]);
        // One the source names as the imported model's is kept.
        let kept = BTreeSet::from([(
            "Microflows$Microflow".to_string(),
            "Sales".to_string(),
            "ACT_Deleted".to_string(),
        )]);
        assert!(
            undeclared_units(&snapshot, &declaration, &kept)
                .unwrap()
                .is_empty()
        );

        // A snapshot taken before imports recorded what they declared
        // removes nothing.
        std::fs::remove_file(snapshot.join(DECLARED_NAME)).unwrap();
        assert!(
            undeclared_units(&snapshot, &declaration, &BTreeSet::new())
                .unwrap()
                .is_empty()
        );
    }

    /// A declaration the frontend states is not removed because the
    /// frontend's folder is missing: the build is refused, and goes on once
    /// the folder is there, empty.
    #[test]
    fn a_missing_frontend_folder_removes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("Source.mpr");
        let snapshot = directory.path().join("model/imported");
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.nanoflow("NAV_Open", |_flow| {});
        });
        mxrs_writer::write_project(&source_path, &project.build()).unwrap();
        let manifest = capture_imported_project(&source_path, &snapshot).unwrap();
        let declared = BTreeMap::from([(
            (
                "Microflows$Nanoflow".to_string(),
                "Sales".to_string(),
                "NAV_Open".to_string(),
            ),
            Some("frontend/src/services".to_string()),
        )]);
        record_declared_units_from(&snapshot, &manifest, &declared).unwrap();
        let mut source = ProjectBuilder::new("11.12.1");
        source.module("Sales", |_module| {});
        let declaration = source.build();
        let output_directory = tempfile::tempdir().unwrap();
        let output = output_directory.path().join("Built.mpr");
        let refused = rebuild_imported_project(&snapshot, &output, &declaration).unwrap_err();
        assert!(
            matches!(&refused, ProjectError::MissingDeclarationSource { folder, count: 1 } if folder == "frontend/src/services"),
            "{refused}"
        );
        assert!(!output.exists());
        std::fs::create_dir_all(directory.path().join("frontend/src/services")).unwrap();
        rebuild_imported_project(&snapshot, &output, &declaration).unwrap();
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

    /// Pages participate in the same "restore the opaque snapshot, then
    /// overlay the typed `ProjectDecl`" pipeline domain entities and
    /// microflows already use — nothing in this crate is page-specific,
    /// which is the point: `mxrs_writer::synchronize_project` (called by
    /// `rebuild_imported_project` below) now knows how to upsert a
    /// `ModuleDecl.pages` entry by name, so a page absent from the
    /// declaration passed to a given rebuild stays exactly what the
    /// snapshot restored (opaque, untouched) while a page present in both
    /// gets its typed content written on top.
    #[test]
    fn typed_pages_upsert_by_name_while_snapshot_only_pages_stay_opaque() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("Source.mpr");
        let snapshot = directory.path().join("model/imported");

        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.page("Legacy", |p| {
                p.layout("Atlas_Core.ApplicationLayout", "Main");
                p.text("hand-authored in Studio Pro, never re-declared in Rust");
            });
            module.page("Typed", |p| {
                p.layout("Atlas_Core.ApplicationLayout", "Main");
                p.text("v1");
            });
        });
        let declaration = project.build();
        mxrs_writer::write_project(&source_path, &declaration).unwrap();
        let manifest = capture_imported_project(&source_path, &snapshot).unwrap();

        // Rebuild with only "Typed" re-declared (different content) — mirrors
        // a Cargo project where the developer took over authoring one page
        // in Rust but left another entirely alone.
        let mut redeclare = ProjectBuilder::new("11.12.1");
        redeclare.module("Sales", |module| {
            module.page("Typed", |p| {
                p.layout("Atlas_Core.ApplicationLayout", "Main");
                p.text("v2");
            });
        });
        let redeclared = redeclare.build();

        let output_directory = tempfile::tempdir().unwrap();
        let output = output_directory.path().join("Built.mpr");
        rebuild_imported_project(&snapshot, &output, &redeclared).unwrap();

        let original_legacy_unit = manifest
            .units
            .iter()
            .find(|u| u.name.as_deref() == Some("Legacy"))
            .unwrap();
        let original_legacy_doc =
            mxrs_bson::parse(&std::fs::read(snapshot.join(&original_legacy_unit.file)).unwrap())
                .unwrap();

        let rebuilt = MprFile::open(&output, true).unwrap();
        let pages: Vec<mxrs_bson::Document> = rebuilt
            .all_units()
            .unwrap()
            .into_iter()
            .filter_map(|unit| {
                let document = rebuilt.parse_contents(&unit).ok()?;
                (document.get_str("$Type").ok() == Some("Forms$Page")).then_some(document)
            })
            .collect();
        assert_eq!(pages.len(), 2, "the untouched Legacy page must survive");
        let legacy = pages
            .iter()
            .find(|d| d.get_str("Name").ok() == Some("Legacy"))
            .expect("Legacy page restored from the opaque snapshot");
        assert_eq!(
            legacy, &original_legacy_doc,
            "a page absent from the redeclared ProjectDecl must round-trip byte-identical"
        );
        assert!(
            pages
                .iter()
                .any(|d| d.get_str("Name").ok() == Some("Typed")),
            "Typed page rewritten from the redeclared ProjectDecl must still exist"
        );
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

    #[test]
    fn build_retry_replaces_a_stale_process_staging_directory() {
        let directory = tempfile::tempdir().unwrap();
        let staging = directory.path().join(".mxrs-build-stale");
        let fresh = staging.join("fresh");
        let previous = staging.join("previous");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(fresh.join("interrupted.mpr"), "partial").unwrap();

        prepare_build_staging(&staging, &fresh, &previous).unwrap();

        assert!(fresh.is_dir());
        assert!(previous.is_dir());
        assert!(!fresh.join("interrupted.mpr").exists());
    }
}
