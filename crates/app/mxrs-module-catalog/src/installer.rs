//! Materializes a catalog entry (or a bare local directory) into
//! `<target>/modules/<module_name>`, validated against its own
//! `mxrb-module.json` manifest and recorded in `.mxrs/modules.lock.json`.
//! Ports `Mxrb::Marketplace::Installer`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::Catalog;
use crate::{Entry, ModuleCatalogError, Result};

const MANIFEST: &str = "mxrb-module.json";
const LOCK_RELATIVE: &str = ".mxrs/modules.lock.json";

/// `argv -> did it succeed` — the git clone seam, injectable so tests
/// exercise the whole install/update/remove contract without a real git
/// binary or network. Mirrors `mxrs_cli::widgets::WidgetDevelopment`'s
/// runner pattern.
pub type CloneRunner = dyn Fn(&[String]) -> std::io::Result<bool>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub entry: Entry,
    pub module_name: String,
    pub destination: PathBuf,
    pub digest: String,
}

pub struct Installer {
    target: PathBuf,
    catalog: Option<Catalog>,
    clone: Box<CloneRunner>,
}

impl Installer {
    pub fn new(target: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            catalog: None,
            clone: Box::new(|command| {
                std::process::Command::new(&command[0])
                    .args(&command[1..])
                    .status()
                    .map(|status| status.success())
            }),
        }
    }

    /// A catalog is only required to resolve a bare name/name@version
    /// identifier; installing a local directory directly needs none.
    pub fn with_catalog(mut self, catalog: Catalog) -> Self {
        self.catalog = Some(catalog);
        self
    }

    pub fn with_clone_runner(mut self, runner: Box<CloneRunner>) -> Self {
        self.clone = runner;
        self
    }

    pub fn install(&self, identifier: &str, version: Option<&str>) -> Result<Installation> {
        self.install_package(identifier, version, false, None)
    }

    pub fn update(&self, identifier: &str, version: Option<&str>) -> Result<Installation> {
        let installed_module_name = self.resolve_installed_module_name(identifier)?;
        self.install_package(identifier, version, true, Some(installed_module_name))
    }

    pub fn remove(&self, identifier: &str) -> Result<String> {
        let module_name = self.resolve_installed_module_name(identifier)?;
        self.validate_no_dependents(&module_name)?;
        let destination = self.target.join("modules").join(&module_name);
        if destination.exists() {
            std::fs::remove_dir_all(&destination).map_err(io_error(&destination))?;
        }
        self.remove_from_lock(&module_name)?;
        Ok(module_name)
    }

    fn install_package(
        &self,
        identifier: &str,
        version: Option<&str>,
        replace: bool,
        installed_module_name: Option<String>,
    ) -> Result<Installation> {
        let (entry, source) = self.resolve(identifier, version)?;
        let workspace = tempfile::tempdir().map_err(|source| ModuleCatalogError::Io {
            path: "temporary directory".into(),
            source,
        })?;
        let package = self.materialize(&source, &entry, workspace.path())?;
        let manifest = load_manifest(&package)?;
        let module_name = valid_module_name(&manifest.module_name)?;
        if let Some(installed) = &installed_module_name
            && &module_name != installed
        {
            return Err(ModuleCatalogError::RenameOnUpdate {
                installed: installed.clone(),
                replacement: module_name,
            });
        }
        let destination = self.target.join("modules").join(&module_name);
        if destination.exists() && !replace {
            return Err(ModuleCatalogError::DestinationExists(
                destination.display().to_string(),
            ));
        }
        let canonical_entry = match manifest.name.as_deref().filter(|name| !name.is_empty()) {
            Some(name) => Entry {
                name: name.to_string(),
                ..entry.clone()
            },
            None => entry.clone(),
        };
        self.validate_mendix_version(&manifest)?;
        self.validate_dependencies(&manifest)?;
        let files = package_files(&package, &manifest)?;

        let pid = std::process::id();
        let staging = self
            .target
            .join(".mxrs/staging")
            .join(format!("{module_name}-{pid}"));
        std::fs::create_dir_all(&staging).map_err(io_error(&staging))?;
        for relative in &files {
            copy_entry(&package, &staging, relative)?;
        }

        let mut backup = None;
        if replace && destination.exists() {
            let backup_path = self
                .target
                .join(".mxrs/backup")
                .join(format!("{module_name}-{pid}"));
            if let Some(parent) = backup_path.parent() {
                std::fs::create_dir_all(parent).map_err(io_error(parent))?;
            }
            std::fs::rename(&destination, &backup_path).map_err(io_error(&backup_path))?;
            backup = Some(backup_path);
        }
        let install_result = (|| -> Result<()> {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(io_error(parent))?;
            }
            std::fs::rename(&staging, &destination).map_err(io_error(&destination))
        })();
        if let Err(error) = install_result {
            if let Some(backup) = &backup
                && backup.exists()
                && !destination.exists()
            {
                let _ = std::fs::rename(backup, &destination);
            }
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        if let Some(backup) = backup {
            let _ = std::fs::remove_dir_all(backup);
        }

        let digest = tree_digest(&destination)?;
        self.write_lock(
            &canonical_entry,
            &module_name,
            &digest,
            declared_dependencies(&manifest),
        )?;
        Ok(Installation {
            entry,
            module_name,
            destination,
            digest,
        })
    }

    fn resolve(&self, identifier: &str, version: Option<&str>) -> Result<(Entry, String)> {
        let path = Path::new(identifier);
        if path.is_dir() {
            let absolute = std::path::absolute(path).map_err(io_error(path))?;
            return Ok((local_entry(&absolute), absolute.display().to_string()));
        }
        let catalog = self.catalog.as_ref().ok_or_else(|| {
            ModuleCatalogError::NotFound(format!(
                "{identifier} (no catalog configured and no local directory by that name)"
            ))
        })?;
        let entry = match version {
            Some(version) => catalog.find_version(identifier, version)?,
            None => catalog.find(identifier)?,
        };
        Ok((entry.clone(), entry.source.clone()))
    }

    fn materialize(&self, source: &str, entry: &Entry, workspace: &Path) -> Result<PathBuf> {
        if let Some(slug) = source.strip_prefix("builtin:") {
            if !valid_slug(slug) {
                return Err(ModuleCatalogError::InvalidBuiltinSlug(slug.to_string()));
            }
            return Err(ModuleCatalogError::BuiltinModulesNotBundled(
                source.to_string(),
            ));
        }
        let as_path = Path::new(source);
        if as_path.is_dir() {
            return Ok(as_path.to_path_buf());
        }
        let destination = workspace.join("repository");
        let mut command = vec![
            "git".to_string(),
            "clone".to_string(),
            "--depth".to_string(),
            "1".to_string(),
        ];
        if let Some(git_ref) = &entry.git_ref {
            command.push("--branch".to_string());
            command.push(git_ref.clone());
        }
        command.push(source.to_string());
        command.push(destination.display().to_string());
        match (self.clone)(&command) {
            Ok(true) => Ok(destination),
            Ok(false) => Err(ModuleCatalogError::FetchFailed {
                name: entry.name.clone(),
                message: format!("command failed: {}", command.join(" ")),
            }),
            Err(error) => Err(ModuleCatalogError::FetchFailed {
                name: entry.name.clone(),
                message: error.to_string(),
            }),
        }
    }

    fn resolve_installed_module_name(&self, identifier: &str) -> Result<String> {
        if !self.lock_path().is_file() {
            return Err(ModuleCatalogError::NoModulesInstalled);
        }
        let lock = self.read_lock()?;
        let found = lock
            .modules
            .iter()
            .find(|(_, info)| info.package == identifier)
            .or_else(|| {
                lock.modules
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(identifier))
            });
        found
            .map(|(name, _)| name.clone())
            .ok_or_else(|| ModuleCatalogError::NotInstalled(identifier.to_string()))
    }

    fn validate_no_dependents(&self, module_name: &str) -> Result<()> {
        if !self.lock_path().is_file() {
            return Ok(());
        }
        let lock = self.read_lock()?;
        let Some(info) = lock.modules.get(module_name) else {
            return Ok(());
        };
        let package_name = info.package.clone();
        let dependents: Vec<String> = lock
            .modules
            .iter()
            .filter(|(name, other)| {
                name.as_str() != module_name
                    && other
                        .dependencies
                        .as_deref()
                        .is_some_and(|deps| deps.contains(&package_name))
            })
            .map(|(name, _)| name.clone())
            .collect();
        if dependents.is_empty() {
            Ok(())
        } else {
            Err(ModuleCatalogError::HasDependents {
                module: module_name.to_string(),
                dependents: dependents.join(", "),
            })
        }
    }

    fn validate_dependencies(&self, manifest: &Manifest) -> Result<()> {
        let deps = declared_dependencies(manifest);
        if deps.is_empty() {
            return Ok(());
        }
        let installed_packages: Vec<String> = if self.lock_path().is_file() {
            self.read_lock()?
                .modules
                .values()
                .map(|info| info.package.clone())
                .collect()
        } else {
            Vec::new()
        };
        let missing: Vec<&String> = deps
            .iter()
            .filter(|dependency| !installed_packages.contains(dependency))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(ModuleCatalogError::MissingDependencies(
                missing
                    .iter()
                    .map(|dependency| dependency.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ))
        }
    }

    fn validate_mendix_version(&self, manifest: &Manifest) -> Result<()> {
        let Some(required) = manifest
            .mendix_version
            .as_deref()
            .map(str::trim)
            .filter(|required| !required.is_empty())
        else {
            return Ok(());
        };
        let Some(project_version) = self.detect_project_mendix_version() else {
            return Ok(());
        };
        if crate::mendix_version::compatible(&project_version, required) {
            Ok(())
        } else {
            Err(ModuleCatalogError::IncompatibleMendixVersion {
                required: required.to_string(),
                actual: project_version,
            })
        }
    }

    /// Globs `*.mpr` directly under the target — mxrb's own project
    /// detection for this subsystem, deliberately independent of the
    /// official marketplace lock's `--mpr`.
    fn detect_project_mendix_version(&self) -> Option<String> {
        let entries = std::fs::read_dir(&self.target).ok()?;
        let mpr = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("mpr"))?;
        mxrs_mpr::MprFile::open(&mpr, true)
            .ok()?
            .mendix_version()
            .ok()
            .flatten()
    }

    fn lock_path(&self) -> PathBuf {
        self.target.join(LOCK_RELATIVE)
    }

    fn read_lock(&self) -> Result<ModulesLock> {
        let path = self.lock_path();
        if !path.is_file() {
            return Ok(ModulesLock::default());
        }
        let source = std::fs::read_to_string(&path).map_err(io_error(&path))?;
        serde_json::from_str(&source).map_err(|error| ModuleCatalogError::InvalidLock {
            path: path.display().to_string(),
            message: error.to_string(),
        })
    }

    fn write_lock(
        &self,
        entry: &Entry,
        module_name: &str,
        digest: &str,
        dependencies: Vec<String>,
    ) -> Result<()> {
        let path = self.lock_path();
        let mut lock = self.read_lock()?;
        lock.modules.insert(
            module_name.to_string(),
            LockedModule {
                package: entry.name.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                git_ref: entry.git_ref.clone(),
                sha256: digest.to_string(),
                dependencies: (!dependencies.is_empty()).then_some(dependencies),
            },
        );
        write_atomic(&path, &lock)
    }

    fn remove_from_lock(&self, module_name: &str) -> Result<()> {
        let path = self.lock_path();
        if !path.is_file() {
            return Ok(());
        }
        let mut lock = self.read_lock()?;
        lock.modules.remove(module_name);
        write_atomic(&path, &lock)
    }
}

fn write_atomic(path: &Path, lock: &ModulesLock) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    let serialized = serde_json::to_string_pretty(lock).expect("lock is serializable");
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temporary, format!("{serialized}\n")).map_err(io_error(&temporary))?;
    std::fs::rename(&temporary, path).map_err(io_error(path))
}

fn io_error(path: &Path) -> impl Fn(std::io::Error) -> ModuleCatalogError + '_ {
    move |source| ModuleCatalogError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn local_entry(path: &Path) -> Entry {
    Entry {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        version: "local".to_string(),
        description: "Local MXRB module".to_string(),
        source: path.display().to_string(),
        git_ref: None,
    }
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

fn valid_module_name(value: &str) -> Result<String> {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');
    if valid {
        Ok(value.to_string())
    } else {
        Err(ModuleCatalogError::InvalidModuleName(value.to_string()))
    }
}

#[derive(Deserialize)]
struct Manifest {
    module_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    files: Option<Vec<String>>,
    #[serde(default)]
    dependencies: Vec<DependencySpec>,
    #[serde(default)]
    mendix_version: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DependencySpec {
    Name(String),
    Object { name: String },
}

fn declared_dependencies(manifest: &Manifest) -> Vec<String> {
    manifest
        .dependencies
        .iter()
        .map(|dependency| match dependency {
            DependencySpec::Name(name) => name.clone(),
            DependencySpec::Object { name } => name.clone(),
        })
        .filter(|name| !name.is_empty())
        .collect()
}

fn load_manifest(package: &Path) -> Result<Manifest> {
    let path = package.join(MANIFEST);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| ModuleCatalogError::MissingManifest(path.display().to_string()))?;
    serde_json::from_str(&text)
        .map_err(|error| ModuleCatalogError::InvalidManifest(error.to_string()))
}

fn package_files(package: &Path, manifest: &Manifest) -> Result<Vec<String>> {
    let files = match &manifest.files {
        Some(files) => files.clone(),
        None => {
            let mut names: Vec<String> = std::fs::read_dir(package)
                .map_err(io_error(package))?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name != MANIFEST)
                .collect();
            names.sort();
            names
        }
    };
    if files.is_empty() {
        return Err(ModuleCatalogError::EmptyPackage);
    }
    for relative in &files {
        let candidate = Path::new(relative);
        let unsafe_path = candidate.is_absolute()
            || candidate
                .components()
                .any(|component| matches!(component, Component::ParentDir));
        if unsafe_path {
            return Err(ModuleCatalogError::UnsafeModulePath(relative.clone()));
        }
        if !package.join(relative).exists() {
            return Err(ModuleCatalogError::MissingModuleFile(relative.clone()));
        }
    }
    Ok(files)
}

fn copy_entry(package: &Path, staging: &Path, relative: &str) -> Result<()> {
    let source = package.join(relative);
    let destination = staging.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    copy_recursively(&source, &destination)
}

fn copy_recursively(source: &Path, destination: &Path) -> Result<()> {
    if source.is_dir() {
        std::fs::create_dir_all(destination).map_err(io_error(destination))?;
        for entry in std::fs::read_dir(source).map_err(io_error(source))? {
            let entry = entry.map_err(io_error(source))?;
            copy_recursively(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(source, destination).map_err(io_error(destination))?;
        Ok(())
    }
}

/// A deterministic digest over every file's relative path and content,
/// sorted for reproducibility — mxrb's `tree_digest`.
fn tree_digest(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    for relative in files {
        let bytes = std::fs::read(root.join(&relative)).map_err(io_error(root))?;
        digest.update(relative.as_bytes());
        digest.update(b"\0");
        digest.update(&bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn collect_files(root: &Path, directory: &Path, found: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(directory).map_err(io_error(directory))? {
        let entry = entry.map_err(io_error(directory))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, found)?;
        } else {
            found.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Default)]
struct ModulesLock {
    #[serde(default)]
    modules: BTreeMap<String, LockedModule>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct LockedModule {
    package: String,
    version: String,
    source: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    git_ref: Option<String>,
    sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependencies: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogTransport;

    fn write_manifest(directory: &Path, manifest: &str) {
        std::fs::write(directory.join(MANIFEST), manifest).unwrap();
    }

    fn local_module(directory: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = directory.join(name);
        std::fs::create_dir_all(&root).unwrap();
        write_manifest(
            &root,
            &format!(r#"{{"module_name":"{name}","files":[{}]}}"#, {
                let mut names: Vec<String> =
                    files.iter().map(|(path, _)| format!("{path:?}")).collect();
                names.sort();
                names.join(",")
            }),
        );
        for (relative, content) in files {
            let target = root.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(target, content).unwrap();
        }
        root
    }

    #[test]
    fn a_local_directory_installs_and_removes_by_name() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("project");
        std::fs::create_dir_all(&target).unwrap();
        let module_source = local_module(directory.path(), "Billing", &[("module.rb", "# v1")]);

        let installer = Installer::new(&target);
        let installation = installer
            .install(module_source.to_str().unwrap(), None)
            .unwrap();
        assert_eq!(installation.module_name, "Billing");
        assert!(target.join("modules/Billing/module.rb").is_file());
        assert!(target.join(LOCK_RELATIVE).is_file());

        // A second plain install of the same identifier refuses the
        // existing destination rather than silently overwriting it.
        assert!(matches!(
            installer.install(module_source.to_str().unwrap(), None),
            Err(ModuleCatalogError::DestinationExists(_))
        ));

        // Remove is identified by the installed module/package name — a
        // local install's `package` field is the directory's own basename
        // (mxrb's `local_entry`), so "Billing" resolves it directly; the
        // original directory path never needs to be re-typed.
        let removed = installer.remove("Billing").unwrap();
        assert_eq!(removed, "Billing");
        assert!(!target.join("modules/Billing").exists());
        let lock: ModulesLock =
            serde_json::from_str(&std::fs::read_to_string(target.join(LOCK_RELATIVE)).unwrap())
                .unwrap();
        assert!(!lock.modules.contains_key("Billing"));
    }

    #[test]
    fn a_catalog_sourced_module_updates_by_its_catalog_name() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("project");
        std::fs::create_dir_all(&target).unwrap();
        let module_source = local_module(directory.path(), "Billing", &[("module.rb", "# v1")]);

        struct NoNetwork;
        impl CatalogTransport for NoNetwork {
            fn get(&self, _url: &str) -> Result<String> {
                unreachable!()
            }
        }
        let catalog_json = format!(
            r#"{{"modules":[{{"name":"billing-kit","version":"1.0.0","description":"","source":{:?}}}]}}"#,
            module_source.to_str().unwrap()
        );
        let catalog_path = directory.path().join("catalog.json");
        std::fs::write(&catalog_path, &catalog_json).unwrap();
        let catalog = || Catalog::read(catalog_path.to_str().unwrap(), &NoNetwork).unwrap();

        let installer = Installer::new(&target).with_catalog(catalog());
        installer.install("billing-kit", None).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("modules/Billing/module.rb")).unwrap(),
            "# v1"
        );

        // A fresh Installer instance (a new catalog, exactly like a new CLI
        // invocation) can still update it — the lock, not in-memory state,
        // is what makes the identifier resolvable across runs.
        std::fs::write(module_source.join("module.rb"), "# v2").unwrap();
        let installer = Installer::new(&target).with_catalog(catalog());
        installer.update("billing-kit", None).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("modules/Billing/module.rb")).unwrap(),
            "# v2"
        );

        // Attempting to rename the module on update is refused.
        let renamed_source = local_module(directory.path(), "Payments", &[("module.rb", "x")]);
        let renamed_catalog = format!(
            r#"{{"modules":[{{"name":"billing-kit","version":"2.0.0","description":"","source":{:?}}}]}}"#,
            renamed_source.to_str().unwrap()
        );
        std::fs::write(&catalog_path, renamed_catalog).unwrap();
        let installer = Installer::new(&target).with_catalog(catalog());
        assert!(matches!(
            installer.update("billing-kit", None),
            Err(ModuleCatalogError::RenameOnUpdate { .. })
        ));
    }

    #[test]
    fn a_dependent_module_blocks_removal_and_a_missing_dependency_blocks_install() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("project");
        std::fs::create_dir_all(&target).unwrap();
        let installer = Installer::new(&target);

        let core = local_module(directory.path(), "Core", &[("module.rb", "core")]);
        installer.install(core.to_str().unwrap(), None).unwrap();

        // A manifest naming an uninstalled dependency is refused up front.
        let needs_core = directory.path().join("Billing");
        std::fs::create_dir_all(&needs_core).unwrap();
        write_manifest(
            &needs_core,
            r#"{"module_name":"Billing","files":["module.rb"],"dependencies":["Core","Ghost"]}"#,
        );
        std::fs::write(needs_core.join("module.rb"), "billing").unwrap();
        let error = installer
            .install(needs_core.to_str().unwrap(), None)
            .unwrap_err();
        assert!(
            matches!(&error, ModuleCatalogError::MissingDependencies(list) if list == "Ghost"),
            "{error}"
        );

        // Once the real dependency exists, install succeeds and Core can
        // no longer be removed while Billing depends on it.
        write_manifest(
            &needs_core,
            r#"{"module_name":"Billing","files":["module.rb"],"dependencies":["Core"]}"#,
        );
        installer
            .install(needs_core.to_str().unwrap(), None)
            .unwrap();
        let error = installer.remove("Core").unwrap_err();
        assert!(
            matches!(error, ModuleCatalogError::HasDependents { .. }),
            "{error}"
        );

        // Billing can be removed on its own, and Core can then follow.
        installer.remove("Billing").unwrap();
        installer.remove("Core").unwrap();
    }

    #[test]
    fn a_builtin_source_is_refused_honestly_and_git_goes_through_the_runner_seam() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("project");
        std::fs::create_dir_all(&target).unwrap();

        struct NoNetwork;
        impl CatalogTransport for NoNetwork {
            fn get(&self, _url: &str) -> Result<String> {
                unreachable!("catalog fetch not exercised here")
            }
        }
        let catalog_json = r#"{"modules":[
            {"name":"shared-kernel","version":"1.0.0","description":"","source":"builtin:shared-kernel"},
            {"name":"audit-log","version":"1.0.0","description":"","source":"https://example.invalid/audit-log.git"}
        ]}"#;
        let catalog_path = directory.path().join("catalog.json");
        std::fs::write(&catalog_path, catalog_json).unwrap();
        let catalog = Catalog::read(catalog_path.to_str().unwrap(), &NoNetwork).unwrap();

        let installer = Installer::new(&target).with_catalog(catalog);
        assert!(matches!(
            installer.install("shared-kernel", None),
            Err(ModuleCatalogError::BuiltinModulesNotBundled(_))
        ));

        // A git source is cloned through the injected runner; the fake
        // "clones" by writing a fixed module tree instead of touching the
        // network.
        let calls: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>> = Default::default();
        let recorded = calls.clone();
        let installer = Installer::new(&target)
            .with_catalog(Catalog::read(catalog_path.to_str().unwrap(), &NoNetwork).unwrap())
            .with_clone_runner(Box::new(move |command| {
                recorded.lock().unwrap().push(command.to_vec());
                let destination = std::path::Path::new(command.last().unwrap());
                std::fs::create_dir_all(destination).unwrap();
                std::fs::write(
                    destination.join(MANIFEST),
                    r#"{"module_name":"AuditLog","files":["module.rb"]}"#,
                )
                .unwrap();
                std::fs::write(destination.join("module.rb"), "audit").unwrap();
                Ok(true)
            }));
        let installation = installer.install("audit-log", None).unwrap();
        assert_eq!(installation.module_name, "AuditLog");
        assert_eq!(
            calls.lock().unwrap()[0][..4],
            ["git", "clone", "--depth", "1"]
        );
        assert!(target.join("modules/AuditLog/module.rb").is_file());
    }

    #[test]
    fn a_mendix_version_mismatch_refuses_install() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("project");
        std::fs::create_dir_all(&target).unwrap();
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Existing", |_module| {});
        mxrs_writer::write_project(target.join("App.mpr"), &builder.build()).unwrap();

        let module_source = local_module(directory.path(), "Legacy", &[("module.rb", "x")]);
        write_manifest(
            &module_source,
            r#"{"module_name":"Legacy","files":["module.rb"],"mendix_version":"9.x"}"#,
        );
        let installer = Installer::new(&target);
        let error = installer
            .install(module_source.to_str().unwrap(), None)
            .unwrap_err();
        assert!(
            matches!(error, ModuleCatalogError::IncompatibleMendixVersion { .. }),
            "{error}"
        );
    }
}
