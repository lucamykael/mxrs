//! Transactional lifecycle for installed marketplace modules — ports
//! `Mxrb::OfficialMarketplace::Lifecycle` and the lock-recording half of
//! `Installer#import_module` (`lib/mxrb/official_marketplace/lifecycle.rb`,
//! `official_marketplace.rb`). Everything here revolves around the
//! [`crate::lock`] file: installs record what they wrote (cached archive,
//! module identity, owned files, pre-install asset backups) so removal can
//! later verify, restore, and roll back honestly.
//!
//! One deliberate strengthening over mxrb: its external-reference guard
//! substring-searches `JSON.generate(document)`, which in this codebase
//! would silently miss UUIDs stored as BSON binaries. The Rust port walks
//! the decoded document and compares both string and binary-UUID values.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use mxrs_mpr::MprFile;

use crate::installer::{InstallReport, plan_install};
use crate::lock::{
    CACHE_RELATIVE, Lock, LockEntry, ORIGINALS_RELATIVE, read_lock, safe_target_path, write_lock,
};
use crate::package::ModulePackage;
use crate::{MarketplaceError, Result};

pub const ATLAS_VARIABLES_IMPORT: &str = "@import \"../../themesource/atlas_core/web/variables\";";

/// Read-only view of a module package used to compare lifecycle boundaries.
/// Ports `Mxrb::OfficialMarketplace::ModulePackageInventory`.
#[derive(Debug, Clone)]
pub struct ModulePackageInventory {
    pub name: String,
    pub version: Option<String>,
    pub module_id: String,
    pub unit_ids: Vec<String>,
    /// Declared asset path → SHA-256 of the packaged content.
    pub files: BTreeMap<String, String>,
}

impl ModulePackageInventory {
    pub fn read(path: &Path) -> Result<Self> {
        let staging = tempfile::tempdir().map_err(|source| MarketplaceError::PackageIo {
            path: "temporary directory".into(),
            source,
        })?;
        let mut package = ModulePackage::open(path)?;
        let descriptor = package.descriptor()?;
        let source_path = package.extract_project(&descriptor, staging.path())?;
        let staged = package.stage_files(&descriptor, staging.path())?;
        let source = MprFile::open(&source_path, true)?;
        let module_unit = source
            .units_by_containment("Modules")?
            .into_iter()
            .find(|unit| {
                source
                    .parse_contents(unit)
                    .ok()
                    .and_then(|doc| doc.get_str("Name").ok().map(str::to_string))
                    .as_deref()
                    == Some(descriptor.module_name.as_str())
            })
            .ok_or_else(|| {
                MarketplaceError::ModuleAbsentFromPackage(descriptor.module_name.clone())
            })?;
        let unit_ids = subtree_ids(&source, &module_unit.unit_id)?;
        let mut files = BTreeMap::new();
        for (relative, staged_path) in &staged {
            files.insert(relative.clone(), sha256_file(staged_path)?);
        }
        Ok(Self {
            name: descriptor.module_name,
            version: descriptor.version,
            module_id: module_unit.unit_id,
            unit_ids,
            files,
        })
    }
}

/// The module unit plus every unit stored beneath it, module first —
/// `ModulePackageInventory.subtree_ids`.
pub fn subtree_ids(mpr: &MprFile, root_id: &str) -> Result<Vec<String>> {
    let mut ids = vec![root_id.to_string()];
    let mut frontier = vec![root_id.to_string()];
    while let Some(parent) = frontier.pop() {
        for child in mpr.children_of(&parent)? {
            if !ids.contains(&child.unit_id) {
                ids.push(child.unit_id.clone());
                frontier.push(child.unit_id);
            }
        }
    }
    Ok(ids)
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|source| MarketplaceError::PackageIo {
        path: path.display().to_string(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn io_error(path: &Path) -> impl Fn(std::io::Error) -> MarketplaceError + '_ {
    move |source| MarketplaceError::PackageIo {
        path: path.display().to_string(),
        source,
    }
}

// ── Install recording ──────────────────────────────────────────────────

/// Extra provenance recorded when the archive came from the official
/// Marketplace (a local `.mpk` install records none of it).
#[derive(Debug, Clone, Default)]
pub struct OfficialProvenance {
    pub content_id: Option<String>,
    pub version_id: Option<String>,
    pub source: Option<String>,
    pub repository: Option<String>,
}

/// Installs a module package and records it in the lockfile — the
/// lock-recording flow of mxrb's `Installer#import_module`: preserve asset
/// originals first, import, ensure the Atlas theme import, cache the
/// archive by checksum, then write the lock entry atomically.
pub fn install_module(
    package_path: &Path,
    mpr_path: &Path,
    target_root: Option<&Path>,
    allow_model_upgrade: bool,
    provenance: &OfficialProvenance,
) -> Result<InstallReport> {
    let target = resolve_target(mpr_path, target_root)?;
    let inventory = ModulePackageInventory::read(package_path)?;
    let (originals, created_backups) = preserve_asset_originals(&target, &inventory)?;
    let plan = plan_install(package_path, mpr_path, Some(&target), allow_model_upgrade)?;
    let report = match plan.apply() {
        Ok(report) => report,
        Err(error) => {
            for backup in created_backups {
                let _ = std::fs::remove_file(backup);
            }
            return Err(error);
        }
    };
    ensure_atlas_theme_variables(&target)?;
    let sha256 = sha256_file(package_path)?;
    let cached = cache_package(&target, &inventory, package_path, &sha256)?;
    let mut lock = read_lock(&target)?;
    lock.packages.insert(
        inventory.name.clone(),
        LockEntry {
            kind: "module".into(),
            version: inventory.version.clone(),
            source: provenance.source.clone(),
            repository: provenance.repository.clone(),
            sha256,
            destination: relative_target_path(&target, mpr_path)?,
            archive: relative_target_path(&target, &cached)?,
            module_id: inventory.module_id.clone(),
            units: report.units,
            files: inventory.files.keys().cloned().collect(),
            asset_originals: originals,
            content_id: provenance.content_id.clone(),
            version_id: provenance.version_id.clone(),
        },
    );
    write_lock(&target, &lock)?;
    Ok(report)
}

fn resolve_target(mpr_path: &Path, target_root: Option<&Path>) -> Result<PathBuf> {
    let target = target_root
        .map(Path::to_path_buf)
        .or_else(|| mpr_path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    std::path::absolute(&target).map_err(|source| MarketplaceError::PackageIo {
        path: target.display().to_string(),
        source,
    })
}

fn relative_target_path(target: &Path, path: &Path) -> Result<String> {
    let absolute = std::path::absolute(path).map_err(|source| MarketplaceError::PackageIo {
        path: path.display().to_string(),
        source,
    })?;
    let relative = absolute
        .strip_prefix(target)
        .map_err(|_| MarketplaceError::UnsafePackagePath(path.display().to_string()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

/// Backs up every project file the package is about to own, once — a
/// reinstall keeps the original backup rather than overwriting it with the
/// previous package version. Ports `preserve_asset_originals`.
#[allow(clippy::type_complexity)]
fn preserve_asset_originals(
    target: &Path,
    inventory: &ModulePackageInventory,
) -> Result<(BTreeMap<String, Option<String>>, Vec<PathBuf>)> {
    let lock = read_lock(target)?;
    let retained = lock
        .packages
        .get(&inventory.name)
        .map(|entry| entry.asset_originals.clone())
        .unwrap_or_default();
    let mut originals = BTreeMap::new();
    let mut created = Vec::new();
    for relative in inventory.files.keys() {
        if let Some(existing) = retained.get(relative) {
            originals.insert(relative.clone(), existing.clone());
            continue;
        }
        let source = safe_target_path(target, relative)?;
        if source.is_symlink() {
            return Err(MarketplaceError::UnsafePackagePath(format!(
                "package asset destination is a symbolic link: {relative}"
            )));
        }
        if !source.is_file() {
            originals.insert(relative.clone(), None);
            continue;
        }
        let backup_relative = format!(
            "{ORIGINALS_RELATIVE}/{}/{relative}",
            valid_name(&inventory.name)?
        );
        let backup = safe_target_path(target, &backup_relative)?;
        if let Some(parent) = backup.parent() {
            std::fs::create_dir_all(parent).map_err(io_error(parent))?;
        }
        std::fs::copy(&source, &backup).map_err(io_error(&backup))?;
        created.push(backup);
        originals.insert(relative.clone(), Some(backup_relative));
    }
    Ok((originals, created))
}

fn valid_name(value: &str) -> Result<String> {
    let name: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let valid = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic());
    if !valid {
        return Err(MarketplaceError::InvalidModuleName(value.to_string()));
    }
    Ok(name)
}

fn cache_package(
    target: &Path,
    inventory: &ModulePackageInventory,
    archive: &Path,
    sha256: &str,
) -> Result<PathBuf> {
    let version: String = inventory
        .version
        .clone()
        .unwrap_or_else(|| "unknown".into())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let destination = safe_target_path(
        target,
        &format!(
            "{CACHE_RELATIVE}/{}-{version}.mpk",
            valid_name(&inventory.name)?
        ),
    )?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    let absolute = std::path::absolute(archive).map_err(io_error(archive))?;
    if absolute != destination {
        std::fs::copy(&absolute, &destination).map_err(io_error(&destination))?;
    }
    if sha256_file(&destination)? != sha256 {
        return Err(MarketplaceError::CacheChecksumMismatch);
    }
    Ok(destination)
}

/// Appends the Atlas variables import to the theme's custom variables when
/// Atlas is present — `ensure_atlas_theme_variables`.
fn ensure_atlas_theme_variables(target: &Path) -> Result<()> {
    let atlas = target.join("themesource/atlas_core/web/_variables.scss");
    if !atlas.is_file() {
        return Ok(());
    }
    let custom = target.join("theme/web/custom-variables.scss");
    let content = std::fs::read_to_string(&custom).unwrap_or_default();
    if content.contains(ATLAS_VARIABLES_IMPORT) {
        return Ok(());
    }
    if let Some(parent) = custom.parent() {
        std::fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    let parts: Vec<&str> = [content.trim_end(), ATLAS_VARIABLES_IMPORT, ""]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    std::fs::write(&custom, parts.join("\n\n")).map_err(io_error(&custom))
}

// ── Removal ────────────────────────────────────────────────────────────

/// Everything installed in the target's lockfile, name-sorted.
pub fn list(target: &Path) -> Result<Vec<(String, LockEntry)>> {
    Ok(read_lock(target)?.packages.into_iter().collect())
}

/// Finds a locked package by name (case-insensitive) or content id, and
/// requires it to be an imported module — `Lifecycle#installed`.
pub fn installed(target: &Path, identifier: &str) -> Result<(String, LockEntry)> {
    let lock = read_lock(target)?;
    let pair = lock.packages.into_iter().find(|(name, entry)| {
        name.eq_ignore_ascii_case(identifier) || entry.content_id.as_deref() == Some(identifier)
    });
    let (name, entry) =
        pair.ok_or_else(|| MarketplaceError::NotInstalled(identifier.to_string()))?;
    if entry.kind != "module" {
        return Err(MarketplaceError::NotAModule(name));
    }
    Ok((name, entry))
}

/// Immutable preview of a removal — blockers computed up front, mutation
/// only through [`LifecyclePlan::apply`]. Ports `LifecyclePlan` +
/// `plan_remove`.
#[derive(Debug)]
pub struct LifecyclePlan {
    pub action: &'static str,
    pub name: String,
    pub installed_version: Option<String>,
    pub changes: Vec<String>,
    pub blockers: Vec<String>,
    target: PathBuf,
    entry: LockEntry,
    mpr: PathBuf,
    unit_ids: Vec<String>,
    removable_files: Vec<String>,
}

impl LifecyclePlan {
    pub fn safe(&self) -> bool {
        self.blockers.is_empty()
    }

    pub fn apply(self) -> Result<()> {
        if !self.safe() {
            return Err(MarketplaceError::PlanBlocked(self.blockers.join("; ")));
        }
        apply_remove(&self)
    }
}

pub fn plan_remove(
    target: &Path,
    identifier: &str,
    mpr_override: Option<&Path>,
) -> Result<LifecyclePlan> {
    let target = std::path::absolute(target).map_err(io_error(target))?;
    let (name, entry) = installed(&target, identifier)?;
    let mpr = safe_target_path(&target, &entry.destination)?;
    if let Some(selected) = mpr_override {
        let selected = std::path::absolute(selected).map_err(io_error(selected))?;
        if selected != mpr {
            return Err(MarketplaceError::PlanBlocked(
                "selected MPR does not match the Marketplace lock".into(),
            ));
        }
    }
    let unit_ids = target_unit_ids(&mpr, &entry)?;
    let inventory = cached_inventory(&target, &entry)?;
    let lock = read_lock(&target)?;
    let shared = shared_files(&lock, &name);
    let mut blockers = Vec::new();
    if inventory.module_id != entry.module_id {
        blockers.push("locked module identity does not match cached package".into());
    }
    if entry.units != unit_ids.len() {
        blockers.push("locked unit count does not match target module tree".into());
    }
    if inventory.name != name {
        blockers.push(format!(
            "locked module name does not match cached package {}",
            inventory.name
        ));
    }
    blockers.extend(external_reference_blockers(&mpr, &unit_ids)?);
    blockers.extend(modified_asset_blockers(&target, &inventory, &shared)?);
    let removable_files: Vec<String> = inventory
        .files
        .keys()
        .filter(|relative| !shared.contains(*relative))
        .cloned()
        .collect();
    let changes = vec![
        format!("delete {} MPR units", unit_ids.len()),
        format!("delete {} package assets", removable_files.len()),
        "delete cached package and lock entry".to_string(),
    ];
    Ok(LifecyclePlan {
        action: "remove",
        installed_version: entry.version.clone(),
        changes,
        blockers,
        target,
        name,
        entry,
        mpr,
        unit_ids,
        removable_files,
    })
}

fn cached_inventory(target: &Path, entry: &LockEntry) -> Result<ModulePackageInventory> {
    let archive = safe_target_path(target, &entry.archive)?;
    if !archive.is_file() {
        return Err(MarketplaceError::MissingCachedPackage(
            archive.display().to_string(),
        ));
    }
    ModulePackageInventory::read(&archive)
}

fn target_unit_ids(mpr_path: &Path, entry: &LockEntry) -> Result<Vec<String>> {
    if !mpr_path.is_file() {
        return Err(MarketplaceError::TargetNotFound(
            mpr_path.display().to_string(),
        ));
    }
    let mpr = MprFile::open(mpr_path, true)?;
    if mpr.unit(&entry.module_id)?.is_none() {
        return Err(MarketplaceError::PlanBlocked(format!(
            "module {} is absent from target MPR",
            entry.module_id
        )));
    }
    subtree_ids(&mpr, &entry.module_id)
}

/// Any unit outside the removal set that still references a unit inside it
/// is a blocker. Walks decoded documents (strings AND binary UUIDs) rather
/// than substring-matching serialized JSON — see the module doc comment.
fn external_reference_blockers(mpr_path: &Path, ids: &[String]) -> Result<Vec<String>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let id_set: BTreeSet<String> = ids.iter().map(|id| id.to_lowercase()).collect();
    let mpr = MprFile::open(mpr_path, true)?;
    let mut blockers = Vec::new();
    for unit in mpr.all_units()? {
        if id_set.contains(&unit.unit_id.to_lowercase()) {
            continue;
        }
        let document = mpr.parse_contents(&unit)?;
        let mut matches = BTreeSet::new();
        collect_id_matches(
            &mxrs_bson::Bson::Document(document.clone()),
            &id_set,
            &mut matches,
        );
        if matches.is_empty() {
            continue;
        }
        let label = document
            .get_str("Name")
            .ok()
            .map(str::to_string)
            .or_else(|| document.get_str("$Type").ok().map(str::to_string))
            .unwrap_or_else(|| unit.unit_id.clone());
        blockers.push(format!(
            "{label} references {} unit(s) scheduled for removal",
            matches.len()
        ));
    }
    Ok(blockers)
}

fn collect_id_matches(
    value: &mxrs_bson::Bson,
    id_set: &BTreeSet<String>,
    matches: &mut BTreeSet<String>,
) {
    match value {
        mxrs_bson::Bson::String(text) => {
            let lowered = text.to_lowercase();
            for id in id_set {
                if lowered.contains(id) {
                    matches.insert(id.clone());
                }
            }
        }
        mxrs_bson::Bson::Binary(binary) => {
            if let Some(uuid) = mxrs_bson::blob_to_uuid(&binary.bytes) {
                let uuid = uuid.to_lowercase();
                if id_set.contains(&uuid) {
                    matches.insert(uuid);
                }
            }
        }
        mxrs_bson::Bson::Document(document) => {
            for (_, nested) in document.iter() {
                collect_id_matches(nested, id_set, matches);
            }
        }
        mxrs_bson::Bson::Array(items) => {
            for item in items {
                collect_id_matches(item, id_set, matches);
            }
        }
        _ => {}
    }
}

/// Assets whose current content no longer matches the package are blockers:
/// removal would destroy local edits. Shared files (owned by another locked
/// package too) are skipped entirely.
fn modified_asset_blockers(
    target: &Path,
    inventory: &ModulePackageInventory,
    shared: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let mut blockers = Vec::new();
    for (relative, expected) in &inventory.files {
        if shared.contains(relative) {
            continue;
        }
        let path = safe_target_path(target, relative)?;
        let actual = if path.is_file() && !path.is_symlink() {
            Some(sha256_file(&path)?)
        } else {
            None
        };
        if actual.as_deref() != Some(expected.as_str()) {
            blockers.push(format!("package asset changed or missing: {relative}"));
        }
    }
    Ok(blockers)
}

fn shared_files(lock: &Lock, owner: &str) -> BTreeSet<String> {
    lock.packages
        .iter()
        .filter(|(name, _)| name.as_str() != owner)
        .flat_map(|(_, entry)| entry.files.iter().cloned())
        .collect()
}

fn apply_remove(plan: &LifecyclePlan) -> Result<()> {
    let mut paths = vec![
        plan.mpr.clone(),
        plan.mpr
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("mprcontents"),
        crate::lock::lock_path(&plan.target),
        plan.target.join("theme/web/custom-variables.scss"),
        plan.target.join(ORIGINALS_RELATIVE),
        safe_target_path(&plan.target, &plan.entry.archive)?,
    ];
    for relative in &plan.removable_files {
        paths.push(safe_target_path(&plan.target, relative)?);
    }
    with_rollback(&paths, || {
        delete_units(&plan.mpr, &plan.unit_ids)?;
        restore_assets(&plan.target, &plan.entry, &plan.removable_files)?;
        if plan.name == "Atlas_Core" {
            remove_atlas_variables(&plan.target)?;
        }
        let archive = safe_target_path(&plan.target, &plan.entry.archive)?;
        let _ = std::fs::remove_file(archive);
        delete_asset_backups(&plan.target, &plan.entry)?;
        let mut lock = read_lock(&plan.target)?;
        lock.packages.remove(&plan.name);
        write_lock(&plan.target, &lock)
    })
}

fn delete_units(mpr_path: &Path, ids: &[String]) -> Result<()> {
    let mut mpr = MprFile::open(mpr_path, false)?;
    mpr.transaction(|mpr| {
        for id in ids.iter().rev() {
            mpr.delete_unit(id)?;
        }
        Ok(())
    })?;
    Ok(())
}

fn restore_assets(target: &Path, entry: &LockEntry, files: &[String]) -> Result<()> {
    for relative in files {
        let destination = safe_target_path(target, relative)?;
        let backup = entry.asset_originals.get(relative).and_then(Clone::clone);
        match backup {
            Some(backup_relative) => {
                let backup = safe_target_path(target, &backup_relative)?;
                if backup.is_file() {
                    if let Some(parent) = destination.parent() {
                        std::fs::create_dir_all(parent).map_err(io_error(parent))?;
                    }
                    std::fs::copy(&backup, &destination).map_err(io_error(&destination))?;
                } else {
                    let _ = std::fs::remove_file(&destination);
                }
            }
            None => {
                let _ = std::fs::remove_file(&destination);
            }
        }
    }
    Ok(())
}

fn delete_asset_backups(target: &Path, entry: &LockEntry) -> Result<()> {
    for backup in entry.asset_originals.values().flatten() {
        let path = safe_target_path(target, backup)?;
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

fn remove_atlas_variables(target: &Path) -> Result<()> {
    let path = target.join("theme/web/custom-variables.scss");
    if !path.is_file() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&path).map_err(io_error(&path))?;
    let filtered: Vec<&str> = content
        .lines()
        .filter(|line| line.trim() != ATLAS_VARIABLES_IMPORT)
        .collect();
    std::fs::write(&path, format!("{}\n", filtered.join("\n"))).map_err(io_error(&path))
}

// ── Rollback snapshots ─────────────────────────────────────────────────

enum SnapshotKind {
    File,
    Directory,
    Missing,
}

/// Snapshots every path, runs the operation, and restores everything in
/// reverse order on failure — `Lifecycle#with_rollback`.
pub(crate) fn with_rollback(
    paths: &[PathBuf],
    operation: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let temporary = tempfile::tempdir().map_err(|source| MarketplaceError::PackageIo {
        path: "temporary directory".into(),
        source,
    })?;
    let mut snapshots = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, path) in paths.iter().enumerate() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let backup = temporary.path().join(index.to_string());
        let kind = if path.is_dir() {
            copy_recursively(path, &backup)?;
            SnapshotKind::Directory
        } else if path.is_file() {
            std::fs::copy(path, &backup).map_err(io_error(&backup))?;
            SnapshotKind::File
        } else {
            SnapshotKind::Missing
        };
        snapshots.push((path.clone(), backup, kind));
    }
    match operation() {
        Ok(()) => Ok(()),
        Err(error) => {
            for (path, backup, kind) in snapshots.iter().rev() {
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                } else if path.is_file() {
                    let _ = std::fs::remove_file(path);
                }
                match kind {
                    SnapshotKind::Missing => {}
                    SnapshotKind::File => {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::copy(backup, path);
                    }
                    SnapshotKind::Directory => {
                        let _ = copy_recursively(backup, path);
                    }
                }
            }
            Err(error)
        }
    }
}

fn copy_recursively(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(io_error(destination))?;
    for entry in std::fs::read_dir(source).map_err(io_error(source))? {
        let entry = entry.map_err(io_error(source))?;
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_recursively(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(io_error(&target))?;
        }
    }
    Ok(())
}
