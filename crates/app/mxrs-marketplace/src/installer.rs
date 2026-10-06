//! Installs a `.mpk` module package into an existing `.mpr`. Ports
//! `lib/mxrb/official_marketplace/module_package_importer.rb`'s
//! `ModulePackageImporter` and `ModulePackageAssets`.
//!
//! **Plan first, then apply** — the same contract the refactoring commands
//! use. [`plan_install`] opens the package, validates everything it can reach
//! without writing, and reports what would change. Nothing touches the target
//! until [`InstallPlan::apply`].
//!
//! **Two things change, and they must succeed or fail together**: model units
//! go into the `.mpr`, and declared assets go onto the filesystem beside it.
//! The units are written inside an `mxrs-mpr` transaction; the assets are
//! written with a backup of anything they overwrite, and restored if the
//! transaction fails. An install that left Java sources from a module whose
//! model was rolled back would be worse than one that never started.
//!
//! **Model versions must match.** mxrb refuses to import a module built for a
//! different Mendix version unless explicitly allowed to move forward, because
//! a pure-Ruby (here, pure-Rust) import does no model migration. The same
//! refusal applies.

use std::path::{Path, PathBuf};

use mxrs_mpr::MprFile;

use crate::package::{ModulePackage, PackageDescriptor, safe_destination};
use crate::{MarketplaceError, Result};

/// `$Type`s a module unit may carry across Mendix versions.
const MODULE_TYPES: &[&str] = &["Projects$ModuleImpl", "Projects$Module"];

/// What an install would do.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub module_name: String,
    pub package_version: Option<String>,
    /// Mendix version of the model inside the package.
    pub source_version: Option<String>,
    /// Mendix version of the project being installed into.
    pub target_version: Option<String>,
    /// Model units that would be inserted, module unit first.
    pub units: Vec<String>,
    /// Declared assets, as `(relative path, would overwrite an existing file)`.
    pub files: Vec<(String, bool)>,
    package_path: PathBuf,
    mpr_path: PathBuf,
    target_root: PathBuf,
}

impl InstallPlan {
    /// Assets that would replace a file already in the project. Worth
    /// surfacing before an apply: the originals are backed up and restored on
    /// failure, but a successful install keeps the package's version.
    pub fn overwrites(&self) -> usize {
        self.files.iter().filter(|(_, exists)| *exists).count()
    }

    pub fn apply(self) -> Result<InstallReport> {
        let staging = tempfile::tempdir().map_err(|source| MarketplaceError::PackageIo {
            path: "temporary directory".into(),
            source,
        })?;
        let mut package = ModulePackage::open(&self.package_path)?;
        let descriptor = package.descriptor()?;
        let source_path = package.extract_project(&descriptor, staging.path())?;
        let staged = package.stage_files(&descriptor, staging.path())?;
        // A widget the project already has stays unless this package's is newer.
        let staged: Vec<(String, PathBuf)> = staged
            .into_iter()
            .filter(|(relative, source)| {
                !shared_widget(relative)
                    || safe_destination(&self.target_root, relative)
                        .is_ok_and(|existing| replaces_widget(source, &existing))
            })
            .collect();

        let source = MprFile::open(&source_path, true)?;
        let mut target = MprFile::open(&self.mpr_path, false)?;
        let units = collect_module_units(&source, &descriptor.module_name)?;
        // Re-checked at apply time rather than trusted from planning: the
        // target may have changed since the plan was built.
        validate_target(&mut target, &descriptor.module_name, &units)?;

        let root_id = target
            .root_unit()?
            .ok_or(MarketplaceError::TargetHasNoRoot)?
            .unit_id;

        let mut assets = AssetInstall::new(&self.target_root, staging.path().join("backups"));
        let outcome = (|| -> Result<usize> {
            let inserted = target.transaction(|target| {
                let mut inserted = 0;
                for unit in &units {
                    // The module unit is re-parented under the target's root;
                    // everything below it keeps the container it had in the
                    // package, which is still valid because the whole subtree
                    // moves together.
                    let container = if unit.unit_id == units[0].unit_id {
                        root_id.clone()
                    } else {
                        unit.container_id.clone()
                    };
                    let mut document = source.parse_contents(unit)?;
                    // Installed, the module is the Marketplace's — as
                    // Studio Pro marks one it imports — and the project
                    // treats it as a dependency, not as its own code.
                    if unit.unit_id == units[0].unit_id {
                        document.insert("FromAppStore", true);
                        if let Some(version) = &descriptor.version {
                            document.insert("AppStoreVersion", version.clone());
                        }
                    }
                    target.insert_unit(
                        &container,
                        &unit.containment_name,
                        document,
                        Some(&unit.unit_id),
                    )?;
                    inserted += 1;
                }
                Ok(inserted)
            })?;
            assets.install(&staged)?;
            Ok(inserted)
        })();

        match outcome {
            Ok(inserted) => Ok(InstallReport {
                module_name: descriptor.module_name,
                package_version: descriptor.version,
                units: inserted,
                files: staged.len(),
                overwritten: assets.overwritten(),
            }),
            Err(error) => {
                // Units are already rolled back by the transaction; assets are
                // not, so restore them before surfacing the failure.
                assets.rollback();
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub module_name: String,
    pub package_version: Option<String>,
    pub units: usize,
    pub files: usize,
    pub overwritten: usize,
}

/// Plans installing `package_path` into `mpr_path`.
///
/// `target_root` defaults to the directory holding the `.mpr`, which is where
/// a Mendix project keeps `javasource/` and friends.
pub fn plan_install(
    package_path: &Path,
    mpr_path: &Path,
    target_root: Option<&Path>,
    allow_model_upgrade: bool,
) -> Result<InstallPlan> {
    if !package_path.is_file() {
        return Err(MarketplaceError::PackageNotFound(
            package_path.display().to_string(),
        ));
    }
    if !mpr_path.is_file() {
        return Err(MarketplaceError::TargetNotFound(
            mpr_path.display().to_string(),
        ));
    }
    let target_root = target_root
        .map(Path::to_path_buf)
        .or_else(|| mpr_path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    let staging = tempfile::tempdir().map_err(|source| MarketplaceError::PackageIo {
        path: "temporary directory".into(),
        source,
    })?;
    let mut package = ModulePackage::open(package_path)?;
    let descriptor = package.descriptor()?;
    let source_path = package.extract_project(&descriptor, staging.path())?;

    let source = MprFile::open(&source_path, true)?;
    let mut target = MprFile::open(mpr_path, true)?;
    let source_version = source.mendix_version()?;
    let target_version = target.mendix_version()?;
    validate_versions(
        &descriptor,
        source_version.as_deref(),
        target_version.as_deref(),
        allow_model_upgrade,
    )?;

    let units = collect_module_units(&source, &descriptor.module_name)?;
    validate_target(&mut target, &descriptor.module_name, &units)?;
    let lock = crate::lock::read_lock(&target_root)?;

    // Assets owned by a *different* locked package are protected: a package
    // may not silently overwrite what another install put there — mxrb's
    // `protected_package_assets` guard.
    let protected: std::collections::BTreeSet<&String> = lock
        .packages
        .iter()
        .filter(|(name, _)| name.as_str() != descriptor.module_name)
        .flat_map(|(_, entry)| entry.files.iter())
        .collect();
    let files = descriptor
        .files
        .iter()
        .map(|relative| {
            // A widget is the project's, shared by the modules that use
            // it: another package's is not overwritten blindly, but the
            // newer of the two is the one that stays (decided at apply).
            if protected.contains(relative) && !shared_widget(relative) {
                return Err(MarketplaceError::ProtectedPackagePath(relative.clone()));
            }
            let destination = safe_destination(&target_root, relative)?;
            Ok((relative.clone(), destination.is_file()))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(InstallPlan {
        module_name: descriptor.module_name,
        package_version: descriptor.version,
        source_version,
        target_version,
        units: units.iter().map(|unit| unit.unit_id.clone()).collect(),
        files,
        package_path: package_path.to_path_buf(),
        mpr_path: mpr_path.to_path_buf(),
        target_root,
    })
}

/// The module unit plus every unit stored beneath it, module first.
fn collect_module_units(source: &MprFile, module_name: &str) -> Result<Vec<mxrs_mpr::RawUnit>> {
    // Parse failures are propagated rather than treated as "no match": a
    // package whose unit contents cannot be read is a broken package, and
    // reporting it as "module absent" sends the reader looking in the wrong
    // place — which is exactly what happened while building this.
    let mut module = None;
    for unit in source.units_by_containment("Modules")? {
        let document = source.parse_contents(&unit)?;
        let is_module = document
            .get_str("$Type")
            .is_ok_and(|kind| MODULE_TYPES.contains(&kind))
            && document
                .get_str("Name")
                .is_ok_and(|name| name == module_name);
        if is_module {
            module = Some(unit);
            break;
        }
    }
    let module =
        module.ok_or_else(|| MarketplaceError::ModuleAbsentFromPackage(module_name.to_string()))?;

    let all = source.all_units()?;
    let mut collected = vec![module];
    // Breadth-first over container ids, because a package's units are not
    // ordered and a child may be listed before its parent.
    let mut frontier = 0;
    while frontier < collected.len() {
        let parents: Vec<String> = collected[frontier..]
            .iter()
            .map(|unit| unit.unit_id.clone())
            .collect();
        frontier = collected.len();
        for unit in &all {
            if parents.contains(&unit.container_id)
                && !collected.iter().any(|seen| seen.unit_id == unit.unit_id)
            {
                collected.push(unit.clone());
            }
        }
    }
    Ok(collected)
}

fn validate_versions(
    descriptor: &PackageDescriptor,
    source: Option<&str>,
    target: Option<&str>,
    allow_model_upgrade: bool,
) -> Result<()> {
    // A manifest that disagrees with the model it ships is a broken package,
    // and trusting either half would be a guess.
    if let (Some(declared), Some(source)) = (descriptor.model_version.as_deref(), source)
        && declared != source
    {
        return Err(MarketplaceError::ManifestVersionMismatch {
            declared: declared.to_string(),
            actual: source.to_string(),
        });
    }
    let (Some(source), Some(target)) = (source, target) else {
        return Ok(());
    };
    if source == target {
        return Ok(());
    }
    // Importing forward is at least plausible; importing backward would need a
    // model downgrade nothing here performs.
    if allow_model_upgrade && numeric_version(target) >= numeric_version(source) {
        return Ok(());
    }
    Err(MarketplaceError::ModelVersionMismatch {
        package: source.to_string(),
        project: target.to_string(),
    })
}

fn numeric_version(value: &str) -> Vec<u64> {
    let mut parts: Vec<u64> = value
        .split(['.', '-'])
        .map(|segment| segment.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    parts
}

/// Whether a declared asset is a pluggable widget: a file the project's
/// modules share.
fn shared_widget(relative: &str) -> bool {
    relative
        .strip_prefix("widgets/")
        .is_some_and(|name| !name.contains('/') && name.to_ascii_lowercase().ends_with(".mpk"))
}

/// Whether the widget a package brings takes the place of the one the
/// project has: when there is none, or when it is a newer version. The same
/// file, an older one, or one whose version cannot be read leaves the
/// project's alone.
fn replaces_widget(incoming: &Path, existing: &Path) -> bool {
    if !existing.is_file() {
        return true;
    }
    match (widget_version(incoming), widget_version(existing)) {
        (Some(incoming), Some(existing)) => incoming > existing,
        _ => false,
    }
}

/// The version a widget package states for its client module.
fn widget_version(path: &Path) -> Option<Vec<u64>> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("package.xml").ok()?, &mut text).ok()?;
    let module = &text[text.find("<clientModule")?..];
    let stated = module[..module.find('>')?].split("version=\"").nth(1)?;
    stated[..stated.find('"')?]
        .split('.')
        .map(|part| part.parse().ok())
        .collect()
}

fn validate_target(
    target: &mut MprFile,
    module_name: &str,
    units: &[mxrs_mpr::RawUnit],
) -> Result<()> {
    let mut existing_module = false;
    for unit in target.units_by_containment("Modules")? {
        let document = target.parse_contents(&unit)?;
        if document
            .get_str("Name")
            .is_ok_and(|name| name == module_name)
        {
            existing_module = true;
            break;
        }
    }
    if existing_module {
        return Err(MarketplaceError::ModuleAlreadyInstalled(
            module_name.to_string(),
        ));
    }
    // Unit ids are global. A collision means the package and the project share
    // history, and overwriting would silently replace an unrelated document.
    let existing_ids: Vec<String> = target
        .all_units()?
        .into_iter()
        .map(|unit| unit.unit_id)
        .collect();
    if let Some(collision) = units
        .iter()
        .find(|unit| existing_ids.contains(&unit.unit_id))
    {
        return Err(MarketplaceError::UnitIdCollision(collision.unit_id.clone()));
    }
    Ok(())
}

/// Copies staged assets into the project, keeping a backup of whatever it
/// replaces so a later failure can put it back.
struct AssetInstall {
    root: PathBuf,
    backups: PathBuf,
    /// `(destination, backup)`; `None` means the file did not exist before.
    changes: Vec<(PathBuf, Option<PathBuf>)>,
}

impl AssetInstall {
    fn new(root: &Path, backups: PathBuf) -> Self {
        Self {
            root: root.to_path_buf(),
            backups,
            changes: Vec::new(),
        }
    }

    fn overwritten(&self) -> usize {
        self.changes
            .iter()
            .filter(|(_, backup)| backup.is_some())
            .count()
    }

    fn install(&mut self, staged: &[(String, PathBuf)]) -> Result<()> {
        for (relative, source) in staged {
            let destination = safe_destination(&self.root, relative)?;
            // A symlink destination would redirect the write outside the
            // project; a directory would fail confusingly mid-copy.
            let metadata = std::fs::symlink_metadata(&destination).ok();
            if let Some(metadata) = &metadata {
                if metadata.file_type().is_symlink() {
                    return Err(MarketplaceError::AssetIsSymlink(relative.clone()));
                }
                if metadata.is_dir() {
                    return Err(MarketplaceError::AssetIsDirectory(relative.clone()));
                }
            }
            let backup = if metadata.is_some() {
                let backup = self.backups.join(relative);
                create_parent(&backup)?;
                copy(&destination, &backup)?;
                Some(backup)
            } else {
                None
            };
            create_parent(&destination)?;
            copy(source, &destination)?;
            self.changes.push((destination, backup));
        }
        Ok(())
    }

    /// Best-effort restore, newest change first. Errors are deliberately
    /// swallowed: this runs while another error is already being reported, and
    /// replacing that error with a rollback error would hide the cause.
    fn rollback(&mut self) {
        for (destination, backup) in self.changes.iter().rev() {
            match backup {
                Some(backup) => {
                    let _ = std::fs::copy(backup, destination);
                }
                None => {
                    let _ = std::fs::remove_file(destination);
                }
            }
        }
        self.changes.clear();
    }
}

fn create_parent(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|source| MarketplaceError::PackageIo {
        path: parent.display().to_string(),
        source,
    })
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|source| MarketplaceError::PackageIo {
            path: to.display().to_string(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(model_version: Option<&str>) -> PackageDescriptor {
        PackageDescriptor {
            module_name: "CommunityCommons".into(),
            version: Some("11.5.1".into()),
            model_version: model_version.map(str::to_string),
            project_file: "project.mpr".into(),
            files: vec![],
        }
    }

    #[test]
    fn a_manifest_that_disagrees_with_the_model_it_ships_is_rejected() {
        assert!(matches!(
            validate_versions(
                &descriptor(Some("10.24.0")),
                Some("11.12.1"),
                Some("11.12.1"),
                false
            ),
            Err(MarketplaceError::ManifestVersionMismatch { .. })
        ));
        assert!(
            validate_versions(
                &descriptor(Some("11.12.1")),
                Some("11.12.1"),
                Some("11.12.1"),
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn importing_across_model_versions_needs_an_explicit_opt_in_and_only_goes_forward() {
        // Same version is always fine.
        assert!(
            validate_versions(&descriptor(None), Some("11.12.1"), Some("11.12.1"), false).is_ok()
        );
        // Differing versions are refused by default: nothing here migrates a model.
        assert!(matches!(
            validate_versions(&descriptor(None), Some("10.24.0"), Some("11.12.1"), false),
            Err(MarketplaceError::ModelVersionMismatch { .. })
        ));
        // Opted in, a forward import is allowed...
        assert!(
            validate_versions(&descriptor(None), Some("10.24.0"), Some("11.12.1"), true).is_ok()
        );
        // ...and a backward one still is not.
        assert!(matches!(
            validate_versions(&descriptor(None), Some("11.12.1"), Some("10.24.0"), true),
            Err(MarketplaceError::ModelVersionMismatch { .. })
        ));
    }

    #[test]
    fn version_comparison_is_numeric_rather_than_lexicographic() {
        // "10.24.0" sorts below "9.0.0" as text; as versions it is newer.
        assert!(numeric_version("10.24.0") > numeric_version("9.0.0"));
        assert!(numeric_version("11.12.1") > numeric_version("11.5.1"));
        assert_eq!(numeric_version("11.12.1"), numeric_version("11.12.1.0"));
    }
}
