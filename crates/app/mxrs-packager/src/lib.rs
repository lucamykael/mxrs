//! Reproducible MXRS deployment archives.
//!
//! Archives are uncompressed POSIX ustar streams: every entry is ordered,
//! timestamped at the Unix epoch, owned by uid/gid zero, and accompanied by
//! a SHA-256 manifest. The simpler wire format is intentional: it is portable,
//! streamable, and deterministic without native compression dependencies.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const BLOCK: usize = 512;
const MANIFEST_PATH: &str = "package-manifest.json";

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid package metadata: {0}")]
    Json(#[from] serde_json::Error),
    #[error("required package input is missing: {0}")]
    Missing(String),
    #[error("package input is not a regular file: {0}")]
    NonRegular(String),
    #[error("unsafe or unsupported archive path: {0}")]
    UnsafePath(String),
    #[error("duplicate archive path: {0}")]
    Duplicate(String),
    #[error("invalid archive: {0}")]
    InvalidArchive(String),
    #[error("frontend inventory references missing asset: {0}")]
    MissingFrontendAsset(String),
    #[error("package output cannot be inside its frontend input: {0}")]
    OutputInsideInput(String),
    #[error("invalid MDA {path}: {reason}")]
    InvalidMda { path: String, reason: String },
    #[error("{0}: file already exists")]
    OutputExists(String),
    #[error("{0}: deployment directory not found")]
    DeploymentNotFound(String),
    #[error("audited native compilation supports Mendix 6.x, 7.x, 9.x, 10.x, and 11.x; got {0}")]
    UnsupportedMendixVersion(String),
    #[error("deployment is not materialized; missing {0}")]
    DeploymentNotMaterialized(String),
    #[error("invalid model/metadata.json: {0}")]
    InvalidDeploymentMetadata(String),
    #[error("deployment targets Mendix {actual}, but MPR targets {target}")]
    DeploymentRuntimeMismatch { actual: String, target: String },
    #[error("deployment is stale: {compiled} is older than {mpr}")]
    StaleDeployment { compiled: String, mpr: String },
    #[error("deployment contains symlink {0}")]
    DeploymentSymlink(String),
}

pub type Result<T> = std::result::Result<T, PackageError>;

#[derive(Debug, Clone)]
pub struct PackageOptions {
    pub mpr: PathBuf,
    pub web: PathBuf,
    pub output: PathBuf,
}

impl PackageOptions {
    pub fn new(
        mpr: impl Into<PathBuf>,
        web: impl Into<PathBuf>,
        output: impl Into<PathBuf>,
    ) -> Self {
        Self {
            mpr: mpr.into(),
            web: web.into(),
            output: output.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageManifest {
    pub format: u32,
    pub application: String,
    pub mendix_version: String,
    pub entrypoints: Entrypoints,
    pub files: Vec<PackageFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entrypoints {
    pub model: String,
    pub web: String,
    pub runtime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageReport {
    pub output: PathBuf,
    pub archive_bytes: u64,
    pub archive_sha256: String,
    pub payload_files: usize,
    pub manifest: PackageManifest,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MdaEntry {
    pub path: String,
    pub size: u64,
    pub crc32: u32,
    pub sha256: Option<String>,
    pub directory: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MdaInspection {
    pub path: PathBuf,
    pub metadata: serde_json::Value,
    pub entries: Vec<MdaEntry>,
    pub sha256: String,
}

impl MdaInspection {
    pub fn files(&self) -> impl Iterator<Item = &MdaEntry> {
        self.entries.iter().filter(|entry| !entry.directory)
    }

    pub fn roots(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|entry| entry.path.split('/').next())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MdaDifferenceStatus {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MdaDifference {
    pub path: String,
    pub status: MdaDifferenceStatus,
    pub left_sha256: Option<String>,
    pub right_sha256: Option<String>,
}

/// Safely inventories a Mendix deployment archive. This is deliberately
/// distinct from [`verify_package`]: an MDA is a ZIP with Mendix metadata,
/// while an MXRS package is a deterministic ustar stream.
pub fn inspect_mda(path: impl AsRef<Path>) -> Result<MdaInspection> {
    let path = absolute(path.as_ref())?;
    let file = std::fs::File::open(&path).map_err(|source| io_error(&path, source))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| invalid_mda(&path, error))?;
    verify_mda_entry_count(&path, archive.central_directory_start(), archive.len())?;
    let mut entries = Vec::with_capacity(archive.len());
    let mut metadata: Option<serde_json::Value> = None;
    let mut seen = BTreeSet::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| invalid_mda(&path, error))?;
        let entry_path = safe_mda_path(file.name()).map_err(|reason| PackageError::InvalidMda {
            path: path.display().to_string(),
            reason,
        })?;
        if !seen.insert(entry_path.clone()) {
            return Err(PackageError::InvalidMda {
                path: path.display().to_string(),
                reason: format!("duplicate entry {entry_path:?}"),
            });
        }
        let directory = file.is_dir();
        let digest = if directory {
            None
        } else {
            let mut hasher = Sha256::new();
            if entry_path == "model/metadata.json" {
                let mut bytes = Vec::new();
                std::io::copy(&mut file.by_ref(), &mut bytes)
                    .map_err(|error| invalid_mda(&path, error))?;
                hasher.update(&bytes);
                metadata = Some(
                    serde_json::from_slice(&bytes).map_err(|error| invalid_mda(&path, error))?,
                );
            } else {
                std::io::copy(&mut file.by_ref(), &mut hasher)
                    .map_err(|error| invalid_mda(&path, error))?;
            }
            Some(format!("{:x}", hasher.finalize()))
        };
        entries.push(MdaEntry {
            path: entry_path,
            size: file.size(),
            crc32: file.crc32(),
            sha256: digest,
            directory,
        });
    }
    let metadata = metadata.ok_or_else(|| PackageError::InvalidMda {
        path: path.display().to_string(),
        reason: "archive has no model/metadata.json".to_string(),
    })?;
    if !metadata.is_object()
        || ["RuntimeVersion", "ProjectName"].iter().any(|key| {
            metadata
                .get(key)
                .is_some_and(|value| !value.is_null() && !value.is_string())
        })
    {
        return Err(invalid_mda(
            &path,
            "metadata must be an object with optional string RuntimeVersion/ProjectName",
        ));
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let archive_bytes = std::fs::read(&path).map_err(|source| io_error(&path, source))?;
    Ok(MdaInspection {
        path,
        metadata,
        entries,
        sha256: sha256(&archive_bytes),
    })
}

/// The deployment subtrees an MDA carries, in `Adapter::ROOTS` order. A
/// deployment directory may hold anything else beside them; only these are
/// packaged, so stray build scratch never reaches the archive.
const DEPLOYMENT_ROOTS: [&str; 5] = ["model", "web", "native", "sass", "tmp"];

/// Files whose absence means the deployment was never materialized. Checked
/// by name rather than by walking, so the diagnostic can list exactly what is
/// missing instead of saying the directory "looks wrong".
const DEPLOYMENT_REQUIRED_FILES: [&str; 4] = [
    "model/model.mdp",
    "model/metadata.json",
    "model/bundles/project.jar",
    "web/index.html",
];

/// Mendix major versions whose deployment layout this writer has been audited
/// against. Ports `Adapter.for`'s table rather than accepting anything that
/// parses: packaging an unaudited layout would produce an archive that only
/// looks right.
const AUDITED_MAJORS: [u64; 5] = [6, 7, 9, 10, 11];

/// Every MDA entry is stamped 2000-01-01T00:00:00Z so that packaging the same
/// deployment twice produces the same archive. Ports MXRB's `FIXED_TIME`.
const MDA_FIXED_TIME: (u16, u8, u8, u8, u8, u8) = (2000, 1, 1, 0, 0, 0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdaPackReport {
    pub path: PathBuf,
    pub mendix_version: String,
    pub files: usize,
    pub sha256: String,
    pub metadata: serde_json::Value,
}

/// Packages an already-materialized Mendix deployment directory into an MDA.
///
/// This is the container half of MXRB's `pack`, ported from
/// `compiler/packager.rb`, and it deliberately stops where that file does: it
/// never invokes `mx`/`mxbuild` and never compiles anything. Materializing
/// `deployment/` from the model — page and widget bundles, Java proxies, the
/// project jar — is the separate, much larger half that is not ported.
///
/// Refusing to package an unmaterialized, stale, or version-mismatched
/// deployment is the whole point of the validation below: an MDA that is
/// structurally a valid ZIP but built from yesterday's model is the failure
/// mode that costs a deployment, and it is invisible in the artifact.
pub fn pack_mda(
    mpr: impl AsRef<Path>,
    deployment: Option<&Path>,
    output: impl AsRef<Path>,
    force: bool,
) -> Result<MdaPackReport> {
    let mpr = absolute(mpr.as_ref())?;
    let project_root = mpr.parent().unwrap_or(Path::new("."));
    let deployment = match deployment {
        Some(path) => absolute(path)?,
        None => absolute(&project_root.join("deployment"))?,
    };
    let output = absolute(output.as_ref())?;

    if output.exists() && !force {
        return Err(PackageError::OutputExists(output.display().to_string()));
    }
    if !deployment.is_dir() {
        return Err(PackageError::DeploymentNotFound(
            deployment.display().to_string(),
        ));
    }

    let project = mxrs_model::Project::open(&mpr, true)?;
    let version = project.mendix_version()?.unwrap_or_default();
    audit_mendix_version(&version)?;
    let metadata = validate_deployment(&deployment, &version)?;
    validate_deployment_freshness(&mpr, &deployment)?;

    let (files, directories) = deployment_inventory(&deployment)?;
    write_mda_atomically(&output, &deployment, &files, &directories)?;

    let inspection = inspect_mda(&output)?;
    Ok(MdaPackReport {
        path: output,
        mendix_version: version,
        files: inspection.files().count(),
        sha256: inspection.sha256,
        metadata,
    })
}

fn audit_mendix_version(version: &str) -> Result<()> {
    let major = version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u64>().ok());
    match major {
        Some(major) if AUDITED_MAJORS.contains(&major) => Ok(()),
        _ => Err(PackageError::UnsupportedMendixVersion(version.to_string())),
    }
}

/// Ports `Adapter#validate_deployment!`: the required files must exist, the
/// metadata must parse, and its `RuntimeVersion` must agree with the model's
/// on the first three components.
fn validate_deployment(deployment: &Path, version: &str) -> Result<serde_json::Value> {
    let missing: Vec<&str> = DEPLOYMENT_REQUIRED_FILES
        .iter()
        .copied()
        .filter(|relative| !deployment.join(relative).is_file())
        .collect();
    if !missing.is_empty() {
        return Err(PackageError::DeploymentNotMaterialized(missing.join(", ")));
    }
    let metadata_path = deployment.join("model/metadata.json");
    let bytes = std::fs::read(&metadata_path)
        .map_err(|source| PackageError::InvalidDeploymentMetadata(source.to_string()))?;
    let metadata: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| PackageError::InvalidDeploymentMetadata(error.to_string()))?;
    let runtime = metadata.get("RuntimeVersion").ok_or_else(|| {
        PackageError::InvalidDeploymentMetadata("key not found: \"RuntimeVersion\"".to_string())
    })?;
    // MXRB's `.to_s` accepts any scalar here; a non-string is rendered rather
    // than refused, so the mismatch message stays the one a user can act on.
    let runtime = match runtime {
        serde_json::Value::String(value) => value.clone(),
        other => other.to_string(),
    };
    if version_prefix(&runtime) == version_prefix(version) {
        Ok(metadata)
    } else {
        Err(PackageError::DeploymentRuntimeMismatch {
            actual: runtime,
            target: version.to_string(),
        })
    }
}

fn version_prefix(version: &str) -> Vec<&str> {
    version.split('.').take(3).collect()
}

/// Ports `Adapter#validate_freshness!`. A compiled model older than the `.mpr`
/// means the deployment does not describe the model being packaged.
fn validate_deployment_freshness(mpr: &Path, deployment: &Path) -> Result<()> {
    let compiled = deployment.join("model/model.mdp");
    let compiled_time = std::fs::metadata(&compiled)
        .and_then(|metadata| metadata.modified())
        .map_err(|source| io_error(&compiled, source))?;
    let mpr_time = std::fs::metadata(mpr)
        .and_then(|metadata| metadata.modified())
        .map_err(|source| io_error(mpr, source))?;
    if compiled_time >= mpr_time {
        return Ok(());
    }
    Err(PackageError::StaleDeployment {
        compiled: compiled.display().to_string(),
        mpr: mpr.display().to_string(),
    })
}

/// Collects the deployment roots and everything beneath them, hidden entries
/// included, sorted by archive path so the inventory does not depend on
/// directory iteration order.
///
/// A symlink anywhere is refused rather than followed or stored: following one
/// silently pulls content from outside the deployment into the archive, and
/// storing one produces an MDA whose meaning depends on the machine that
/// unpacks it.
fn deployment_inventory(deployment: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for root in DEPLOYMENT_ROOTS {
        let root = deployment.join(root);
        if !root.exists() {
            continue;
        }
        collect_deployment_paths(&root, &mut files, &mut directories)?;
    }
    let key = |path: &PathBuf| mda_relative_path(deployment, path);
    files.sort_by_key(key);
    directories.sort_by_key(key);
    Ok((files, directories))
}

fn collect_deployment_paths(
    path: &Path,
    files: &mut Vec<PathBuf>,
    directories: &mut Vec<PathBuf>,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink() {
        return Err(PackageError::DeploymentSymlink(path.display().to_string()));
    }
    if metadata.is_dir() {
        directories.push(path.to_path_buf());
        let mut children: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|source| io_error(path, source))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<_, _>>()
            .map_err(|source| io_error(path, source))?;
        children.sort();
        for child in children {
            collect_deployment_paths(&child, files, directories)?;
        }
    } else {
        files.push(path.to_path_buf());
    }
    Ok(())
}

fn mda_relative_path(deployment: &Path, path: &Path) -> String {
    path.strip_prefix(deployment)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Writes through a temporary file beside the output, then renames. A crash
/// mid-write leaves the previous archive intact rather than a truncated one
/// that still opens as a ZIP.
fn write_mda_atomically(
    output: &Path,
    deployment: &Path,
    files: &[PathBuf],
    directories: &[PathBuf],
) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let temporary = output.with_extension(format!("mda-partial-{}", std::process::id()));
    let result = write_mda(&temporary, deployment, files, directories);
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return result;
    }
    std::fs::rename(&temporary, output).map_err(|source| {
        let _ = std::fs::remove_file(&temporary);
        io_error(output, source)
    })
}

fn write_mda(
    temporary: &Path,
    deployment: &Path,
    files: &[PathBuf],
    directories: &[PathBuf],
) -> Result<()> {
    let (year, month, day, hour, minute, second) = MDA_FIXED_TIME;
    let fixed = zip::DateTime::from_date_and_time(year, month, day, hour, minute, second)
        .map_err(|error| PackageError::InvalidArchive(error.to_string()))?;
    let handle = std::fs::File::create(temporary).map_err(|source| io_error(temporary, source))?;
    let mut archive = zip::ZipWriter::new(std::io::BufWriter::new(handle));
    for directory in directories {
        let options = mda_entry_options(fixed, directory)?;
        archive
            .add_directory(mda_relative_path(deployment, directory), options)
            .map_err(|error| PackageError::InvalidArchive(error.to_string()))?;
    }
    for file in files {
        let options = mda_entry_options(fixed, file)?;
        archive
            .start_file(mda_relative_path(deployment, file), options)
            .map_err(|error| PackageError::InvalidArchive(error.to_string()))?;
        let mut source = std::fs::File::open(file).map_err(|source| io_error(file, source))?;
        std::io::copy(&mut source, &mut archive).map_err(|source| io_error(file, source))?;
    }
    archive
        .finish()
        .map_err(|error| PackageError::InvalidArchive(error.to_string()))?;
    Ok(())
}

fn mda_entry_options(fixed: zip::DateTime, source: &Path) -> Result<zip::write::SimpleFileOptions> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(fixed);
    Ok(options.unix_permissions(deployment_permissions(source)?))
}

/// Mendix's runtime reads executable bits off some deployment entries, so the
/// mode is carried through rather than normalized — the one thing about an
/// entry that is not fixed.
#[cfg(unix)]
fn deployment_permissions(source: &Path) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    let metadata = std::fs::metadata(source).map_err(|error| io_error(source, error))?;
    Ok(metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn deployment_permissions(_source: &Path) -> Result<u32> {
    // Windows has no POSIX mode to carry; MXRB reads `File.stat(...).mode`,
    // which Ruby synthesizes there too. 0o644/0o755 would be a guess, so the
    // archive records the conventional file mode and lets the runtime decide.
    Ok(0o644)
}

pub fn compare_mda(left: impl AsRef<Path>, right: impl AsRef<Path>) -> Result<Vec<MdaDifference>> {
    let left = inspect_mda(left)?;
    let right = inspect_mda(right)?;
    let left = left
        .files()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let right = right
        .files()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let paths = left
        .keys()
        .chain(right.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    Ok(paths
        .into_iter()
        .filter_map(|path| {
            let lhs = left.get(path);
            let rhs = right.get(path);
            let status = match (lhs, rhs) {
                (None, Some(_)) => MdaDifferenceStatus::Added,
                (Some(_), None) => MdaDifferenceStatus::Removed,
                (Some(lhs), Some(rhs)) if lhs.sha256 != rhs.sha256 => MdaDifferenceStatus::Changed,
                _ => return None,
            };
            Some(MdaDifference {
                path: path.to_string(),
                status,
                left_sha256: lhs.and_then(|entry| entry.sha256.clone()),
                right_sha256: rhs.and_then(|entry| entry.sha256.clone()),
            })
        })
        .collect())
}

/// zip 2 indexes entries by decoded filename and silently collapses duplicates.
/// Count physical central-directory records before trusting that inventory.
/// The archive reader has already validated the directory offset (including
/// ZIP64/prefixed archives); variable name, extra and comment data is skipped.
fn verify_mda_entry_count(path: &Path, directory_start: u64, indexed: usize) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut input = std::fs::File::open(path).map_err(|source| io_error(path, source))?;
    input
        .seek(SeekFrom::Start(directory_start))
        .map_err(|error| invalid_mda(path, error))?;
    let mut count = 0;
    loop {
        let mut header = [0_u8; 46];
        input
            .read_exact(&mut header[..4])
            .map_err(|error| invalid_mda(path, error))?;
        if &header[..4] != b"PK\x01\x02" {
            break;
        }
        input
            .read_exact(&mut header[4..])
            .map_err(|error| invalid_mda(path, error))?;
        let variable_bytes: i64 = [28, 30, 32]
            .iter()
            .map(|offset| i64::from(u16::from_le_bytes([header[*offset], header[*offset + 1]])))
            .sum();
        input
            .seek(SeekFrom::Current(variable_bytes))
            .map_err(|error| invalid_mda(path, error))?;
        count += 1;
        if count > indexed {
            return Err(invalid_mda(path, "duplicate or collapsed ZIP entry names"));
        }
    }
    if count != indexed {
        return Err(invalid_mda(
            path,
            "ZIP entry inventory does not match its central directory",
        ));
    }
    Ok(())
}

fn safe_mda_path(path: &str) -> std::result::Result<String, String> {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.is_empty()
        || normalized
            .strip_suffix('/')
            .unwrap_or(&normalized)
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(format!("unsafe entry {path:?}"));
    }
    Ok(normalized)
}

fn invalid_mda(path: &Path, error: impl std::fmt::Display) -> PackageError {
    PackageError::InvalidMda {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

/// Packages a validated MPR, its v2 content files, frontend, and project
/// resource directories located beside the MPR.
pub fn package(options: &PackageOptions) -> Result<PackageReport> {
    let mpr = absolute(&options.mpr)?;
    let web = absolute(&options.web)?;
    let output = absolute(&options.output)?;
    require_file(&mpr)?;
    if !web.is_dir() {
        return Err(PackageError::Missing(web.display().to_string()));
    }
    if output.starts_with(&web) {
        return Err(PackageError::OutputInsideInput(
            output.display().to_string(),
        ));
    }

    let project = mxrs_model::Project::open(&mpr, true)?;
    let application = project
        .name()?
        .filter(|name| !name.is_empty())
        .or_else(|| {
            mpr.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Application".to_string());
    let mendix_version = project.mendix_version()?.unwrap_or_default();
    drop(project);

    validate_frontend_inventory(&web)?;
    let mpr_name = mpr
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| PackageError::UnsafePath(mpr.display().to_string()))?;
    let model_entry = format!("model/{mpr_name}");
    let mut payloads = BTreeMap::new();
    insert_file(&mut payloads, &model_entry, &mpr)?;

    let contents = mxrs_mpr::format::contents_dir(&mpr);
    if contents.is_dir() {
        collect_tree(&mut payloads, &contents, Path::new("model/mprcontents"))?;
    }
    collect_tree(&mut payloads, &web, Path::new("web"))?;

    let root = mpr.parent().unwrap_or_else(|| Path::new("."));
    for directory in mxrs_project::PROJECT_ASSET_DIRECTORIES {
        let source = root.join(directory);
        if source.is_dir() {
            collect_tree(
                &mut payloads,
                &source,
                &Path::new("project").join(directory),
            )?;
        }
    }

    let deployment = serde_json::to_vec_pretty(&serde_json::json!({
        "format": 1,
        "model": model_entry,
        "web": "web/index.html",
        "runtime": null,
    }))?;
    insert_bytes(
        &mut payloads,
        "deployment/mxrs.json",
        with_newline(deployment),
    )?;

    let files = payloads
        .iter()
        .map(|(path, bytes)| PackageFile {
            path: path.clone(),
            bytes: bytes.len() as u64,
            sha256: sha256(bytes),
        })
        .collect::<Vec<_>>();
    let manifest = PackageManifest {
        format: 1,
        application,
        mendix_version,
        entrypoints: Entrypoints {
            model: model_entry,
            web: "web/index.html".to_string(),
            runtime: None,
        },
        files,
    };
    let manifest_bytes = with_newline(serde_json::to_vec_pretty(&manifest)?);
    insert_bytes(&mut payloads, MANIFEST_PATH, manifest_bytes)?;

    let archive = encode_tar(&payloads)?;
    verify_archive_bytes(&archive)?;
    write_atomic(&output, &archive)?;
    Ok(PackageReport {
        output,
        archive_bytes: archive.len() as u64,
        archive_sha256: sha256(&archive),
        payload_files: manifest.files.len(),
        manifest,
    })
}

/// Verifies tar structure, the embedded manifest, all hashes, and that the
/// archive contains no undeclared payloads.
pub fn verify_package(path: impl AsRef<Path>) -> Result<PackageManifest> {
    let bytes = std::fs::read(path.as_ref()).map_err(|source| io_error(path.as_ref(), source))?;
    verify_archive_bytes(&bytes)
}

fn verify_archive_bytes(bytes: &[u8]) -> Result<PackageManifest> {
    let entries = decode_tar(bytes)?;
    let manifest_bytes = entries
        .get(MANIFEST_PATH)
        .ok_or_else(|| PackageError::InvalidArchive("package manifest is missing".to_string()))?;
    let manifest: PackageManifest = serde_json::from_slice(manifest_bytes)?;
    if manifest.format != 1 {
        return Err(PackageError::InvalidArchive(format!(
            "unsupported manifest format {}",
            manifest.format
        )));
    }
    let declared = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    if declared.len() != manifest.files.len() {
        return Err(PackageError::InvalidArchive(
            "manifest contains duplicate file paths".to_string(),
        ));
    }
    for file in &manifest.files {
        safe_archive_path(&file.path)?;
        let actual = entries.get(&file.path).ok_or_else(|| {
            PackageError::InvalidArchive(format!("declared file {} is missing", file.path))
        })?;
        if actual.len() as u64 != file.bytes || sha256(actual) != file.sha256 {
            return Err(PackageError::InvalidArchive(format!(
                "integrity mismatch for {}",
                file.path
            )));
        }
    }
    let actual = entries
        .keys()
        .filter(|path| path.as_str() != MANIFEST_PATH)
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual != declared {
        return Err(PackageError::InvalidArchive(
            "archive has undeclared payload files".to_string(),
        ));
    }
    for required in [
        manifest.entrypoints.model.as_str(),
        manifest.entrypoints.web.as_str(),
    ] {
        if !declared.contains(required) {
            return Err(PackageError::InvalidArchive(format!(
                "entrypoint {required} is not packaged"
            )));
        }
    }
    Ok(manifest)
}

fn validate_frontend_inventory(web: &Path) -> Result<()> {
    for required in ["index.html", "model.json", ".mxrs-assets.json"] {
        require_file(&web.join(required))?;
    }
    let inventory_path = web.join(".mxrs-assets.json");
    let inventory: Vec<String> = serde_json::from_slice(
        &std::fs::read(&inventory_path).map_err(|source| io_error(&inventory_path, source))?,
    )?;
    for relative in inventory {
        safe_archive_path(&relative)?;
        if !web.join(&relative).is_file() {
            return Err(PackageError::MissingFrontendAsset(relative));
        }
    }
    Ok(())
}

fn collect_tree(
    payloads: &mut BTreeMap<String, Vec<u8>>,
    source: &Path,
    prefix: &Path,
) -> Result<()> {
    let mut entries = std::fs::read_dir(source)
        .map_err(|error| io_error(source, error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| io_error(source, error))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| io_error(&path, error))?;
        let target = prefix.join(entry.file_name());
        if file_type.is_dir() {
            collect_tree(payloads, &path, &target)?;
        } else if file_type.is_file() {
            insert_file(payloads, &path_string(&target)?, &path)?;
        } else {
            return Err(PackageError::NonRegular(path.display().to_string()));
        }
    }
    Ok(())
}

fn insert_file(payloads: &mut BTreeMap<String, Vec<u8>>, name: &str, path: &Path) -> Result<()> {
    require_file(path)?;
    let bytes = std::fs::read(path).map_err(|source| io_error(path, source))?;
    insert_bytes(payloads, name, bytes)
}

fn insert_bytes(
    payloads: &mut BTreeMap<String, Vec<u8>>,
    name: &str,
    bytes: Vec<u8>,
) -> Result<()> {
    safe_archive_path(name)?;
    if payloads.insert(name.to_string(), bytes).is_some() {
        return Err(PackageError::Duplicate(name.to_string()));
    }
    Ok(())
}

fn encode_tar(entries: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    let capacity = entries
        .values()
        .map(|bytes| BLOCK + bytes.len().div_ceil(BLOCK) * BLOCK)
        .sum::<usize>()
        + BLOCK * 2;
    let mut archive = Vec::with_capacity(capacity);
    for (path, bytes) in entries {
        let mut header = [0_u8; BLOCK];
        write_tar_name(&mut header, path)?;
        write_octal(&mut header[100..108], 0o644)?;
        write_octal(&mut header[108..116], 0)?;
        write_octal(&mut header[116..124], 0)?;
        write_octal(&mut header[124..136], bytes.len() as u64)?;
        write_octal(&mut header[136..148], 0)?;
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum = header.iter().map(|byte| u64::from(*byte)).sum();
        write_checksum(&mut header[148..156], checksum)?;
        archive.extend_from_slice(&header);
        archive.extend_from_slice(bytes);
        archive.resize(archive.len().next_multiple_of(BLOCK), 0);
    }
    archive.resize(archive.len() + BLOCK * 2, 0);
    Ok(archive)
}

fn decode_tar(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    if bytes.len() < BLOCK * 2 || !bytes.len().is_multiple_of(BLOCK) {
        return Err(PackageError::InvalidArchive(
            "length is not a complete tar stream".to_string(),
        ));
    }
    let mut entries = BTreeMap::new();
    let mut offset = 0;
    while offset + BLOCK <= bytes.len() {
        let header = &bytes[offset..offset + BLOCK];
        if header.iter().all(|byte| *byte == 0) {
            if bytes[offset..].iter().any(|byte| *byte != 0) {
                return Err(PackageError::InvalidArchive(
                    "non-zero data follows end marker".to_string(),
                ));
            }
            return Ok(entries);
        }
        if &header[257..263] != b"ustar\0" || header[156] != b'0' {
            return Err(PackageError::InvalidArchive(
                "unsupported tar header".to_string(),
            ));
        }
        let expected = parse_octal(&header[148..156])?;
        let mut checked = header.to_vec();
        checked[148..156].fill(b' ');
        let actual = checked.iter().map(|byte| u64::from(*byte)).sum::<u64>();
        if expected != actual {
            return Err(PackageError::InvalidArchive(
                "tar header checksum mismatch".to_string(),
            ));
        }
        let path = read_tar_name(header)?;
        safe_archive_path(&path)?;
        let size = usize::try_from(parse_octal(&header[124..136])?)
            .map_err(|_| PackageError::InvalidArchive("file is too large".to_string()))?;
        let start = offset + BLOCK;
        let end = start.checked_add(size).ok_or_else(|| {
            PackageError::InvalidArchive("file size overflows archive".to_string())
        })?;
        if end > bytes.len() {
            return Err(PackageError::InvalidArchive(
                "file extends beyond archive".to_string(),
            ));
        }
        if entries
            .insert(path.clone(), bytes[start..end].to_vec())
            .is_some()
        {
            return Err(PackageError::Duplicate(path));
        }
        offset = end.next_multiple_of(BLOCK);
    }
    Err(PackageError::InvalidArchive(
        "tar end marker is missing".to_string(),
    ))
}

fn write_tar_name(header: &mut [u8; BLOCK], path: &str) -> Result<()> {
    let bytes = path.as_bytes();
    if bytes.len() <= 100 {
        header[..bytes.len()].copy_from_slice(bytes);
        return Ok(());
    }
    let split = path
        .match_indices('/')
        .map(|(index, _)| index)
        .rfind(|index| *index <= 155 && bytes.len() - index - 1 <= 100)
        .ok_or_else(|| PackageError::UnsafePath(path.to_string()))?;
    header[..bytes.len() - split - 1].copy_from_slice(&bytes[split + 1..]);
    header[345..345 + split].copy_from_slice(&bytes[..split]);
    Ok(())
}

fn read_tar_name(header: &[u8]) -> Result<String> {
    let name = nul_string(&header[..100])?;
    let prefix = nul_string(&header[345..500])?;
    if prefix.is_empty() {
        Ok(name)
    } else {
        Ok(format!("{prefix}/{name}"))
    }
}

fn nul_string(bytes: &[u8]) -> Result<String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end])
        .map(str::to_owned)
        .map_err(|_| PackageError::InvalidArchive("non-UTF-8 path".to_string()))
}

fn write_octal(field: &mut [u8], value: u64) -> Result<()> {
    let width = field.len() - 1;
    let encoded = format!("{value:0width$o}");
    if encoded.len() > width {
        return Err(PackageError::InvalidArchive(
            "value cannot be represented in tar header".to_string(),
        ));
    }
    field[..width].copy_from_slice(encoded.as_bytes());
    field[width] = 0;
    Ok(())
}

fn write_checksum(field: &mut [u8], value: u64) -> Result<()> {
    let encoded = format!("{value:06o}");
    if encoded.len() != 6 {
        return Err(PackageError::InvalidArchive(
            "checksum cannot be represented in tar header".to_string(),
        ));
    }
    field[..6].copy_from_slice(encoded.as_bytes());
    field[6] = 0;
    field[7] = b' ';
    Ok(())
}

fn parse_octal(field: &[u8]) -> Result<u64> {
    let value = field
        .iter()
        .copied()
        .take_while(|byte| *byte != 0 && *byte != b' ')
        .filter(|byte| *byte != b' ')
        .collect::<Vec<_>>();
    let value = std::str::from_utf8(&value)
        .map_err(|_| PackageError::InvalidArchive("invalid octal field".to_string()))?;
    u64::from_str_radix(value.trim_start_matches('0').if_empty("0"), 8)
        .map_err(|_| PackageError::InvalidArchive("invalid octal field".to_string()))
}

trait EmptyString {
    fn if_empty<'a>(&'a self, fallback: &'a str) -> &'a str;
}

impl EmptyString for str {
    fn if_empty<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.is_empty() { fallback } else { self }
    }
}

fn path_string(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| PackageError::UnsafePath(path.display().to_string()))?
        .replace('\\', "/");
    safe_archive_path(&value)?;
    Ok(value)
}

fn safe_archive_path(path: &str) -> Result<()> {
    let path_value = Path::new(path);
    if path.is_empty()
        || path.as_bytes().contains(&0)
        || path_value.is_absolute()
        || path_value
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PackageError::UnsafePath(path.to_string()));
    }
    Ok(())
}

fn require_file(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            PackageError::Missing(path.display().to_string())
        } else {
            io_error(path, source)
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(PackageError::NonRegular(path.display().to_string()));
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("package");
    let temporary = path.with_file_name(format!(".{name}.mxrs-{}.tmp", std::process::id()));
    let result = (|| {
        let mut file =
            std::fs::File::create(&temporary).map_err(|source| io_error(&temporary, source))?;
        file.write_all(bytes)
            .map_err(|source| io_error(&temporary, source))?;
        file.sync_all()
            .map_err(|source| io_error(&temporary, source))?;
        std::fs::rename(&temporary, path).map_err(|source| io_error(path, source))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|source| io_error(path, source))
}

fn with_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.push(b'\n');
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn io_error(path: &Path, source: std::io::Error) -> PackageError {
    PackageError::Io {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mda(path: &Path, files: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        archive.start_file("model/metadata.json", options).unwrap();
        archive
            .write_all(br#"{"RuntimeVersion":"11.12.1","ProjectName":"Shop"}"#)
            .unwrap();
        for (name, bytes) in files {
            archive.start_file(*name, options).unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn mda_inspection_and_comparison_are_content_based_and_path_safe() {
        let directory = tempfile::tempdir().unwrap();
        let left = directory.path().join("left.mda");
        let right = directory.path().join("right.mda");
        mda(
            &left,
            &[("web/index.html", b"left"), ("model/old.bin", b"old")],
        );
        mda(
            &right,
            &[("web/index.html", b"right"), ("model/new.bin", b"new")],
        );
        let inspection = inspect_mda(&left).unwrap();
        assert_eq!(inspection.metadata["ProjectName"], "Shop");
        assert_eq!(inspection.roots(), ["model", "web"]);
        assert_eq!(inspection.files().count(), 3);
        let differences = compare_mda(&left, &right).unwrap();
        assert_eq!(
            differences
                .iter()
                .map(|difference| (difference.path.as_str(), difference.status))
                .collect::<Vec<_>>(),
            [
                ("model/new.bin", MdaDifferenceStatus::Added),
                ("model/old.bin", MdaDifferenceStatus::Removed),
                ("web/index.html", MdaDifferenceStatus::Changed),
            ]
        );

        let unsafe_archive = directory.path().join("unsafe.mda");
        mda(&unsafe_archive, &[("../secret", b"no")]);
        assert!(matches!(
            inspect_mda(unsafe_archive),
            Err(PackageError::InvalidMda { .. })
        ));
        let absolute_archive = directory.path().join("absolute.mda");
        mda(&absolute_archive, &[("/secret", b"no")]);
        assert!(matches!(
            inspect_mda(absolute_archive),
            Err(PackageError::InvalidMda { .. })
        ));
    }

    fn fixture() -> (tempfile::TempDir, PackageOptions) {
        let directory = tempfile::tempdir().unwrap();
        let mpr = directory.path().join("Demo.mpr");
        let web = directory.path().join("web");
        let output = directory.path().join("Demo.mxrs.tar");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
            module.page("Home", |page| {
                page.layout("Atlas_Core.ApplicationLayout", "Main");
                page.text("Orders");
            });
        });
        mxrs_writer::write_project(&mpr, &builder.build()).unwrap();
        mxrs_materializers::materialize_mpr(&mpr, &web).unwrap();
        let resources = directory.path().join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(resources.join("settings.json"), "{}\n").unwrap();
        let options = PackageOptions::new(&mpr, &web, &output);
        (directory, options)
    }

    #[test]
    fn package_is_reproducible_and_self_verifying() {
        let (_directory, options) = fixture();
        let first = package(&options).unwrap();
        let first_bytes = std::fs::read(&options.output).unwrap();
        let second = package(&options).unwrap();
        let second_bytes = std::fs::read(&options.output).unwrap();
        assert_eq!(first_bytes, second_bytes);
        assert_eq!(first.archive_sha256, second.archive_sha256);
        let verified = verify_package(&options.output).unwrap();
        assert_eq!(verified, first.manifest);
        assert!(
            verified
                .files
                .iter()
                .any(|file| file.path == "web/index.html")
        );
        assert!(
            verified
                .files
                .iter()
                .any(|file| file.path == "project/resources/settings.json")
        );
    }

    #[test]
    fn missing_frontend_inventory_reference_fails_closed() {
        let (_directory, options) = fixture();
        std::fs::write(options.web.join(".mxrs-assets.json"), "[\"missing.js\"]\n").unwrap();
        assert!(matches!(
            package(&options),
            Err(PackageError::MissingFrontendAsset(path)) if path == "missing.js"
        ));
    }

    #[test]
    fn corruption_is_detected_before_a_package_is_accepted() {
        let (_directory, options) = fixture();
        package(&options).unwrap();
        let mut bytes = std::fs::read(&options.output).unwrap();
        let position = BLOCK + 3;
        bytes[position] ^= 0xff;
        std::fs::write(&options.output, bytes).unwrap();
        assert!(verify_package(&options.output).is_err());
    }

    #[test]
    fn long_ustar_paths_round_trip() {
        let path = format!("{}/file.txt", "segment/".repeat(14));
        let mut entries = BTreeMap::new();
        entries.insert(path.clone(), b"content".to_vec());
        entries.insert(
            MANIFEST_PATH.to_string(),
            serde_json::to_vec(&PackageManifest {
                format: 1,
                application: "Demo".to_string(),
                mendix_version: "11.12.1".to_string(),
                entrypoints: Entrypoints {
                    model: path.clone(),
                    web: path.clone(),
                    runtime: None,
                },
                files: vec![PackageFile {
                    path: path.clone(),
                    bytes: 7,
                    sha256: sha256(b"content"),
                }],
            })
            .unwrap(),
        );
        let archive = encode_tar(&entries).unwrap();
        let decoded = decode_tar(&archive).unwrap();
        assert_eq!(decoded.get(&path).unwrap(), b"content");
    }
}
