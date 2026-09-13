//! Reproducible MXRS deployment archives.
//!
//! Archives are uncompressed POSIX ustar streams: every entry is ordered,
//! timestamped at the Unix epoch, owned by uid/gid zero, and accompanied by
//! a SHA-256 manifest. The simpler wire format is intentional: it is portable,
//! streamable, and deterministic without native compression dependencies.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
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
