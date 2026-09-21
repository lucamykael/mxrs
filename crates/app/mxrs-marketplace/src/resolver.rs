//! Recursive dependency resolution for installed marketplace modules —
//! ports `Mxrb::OfficialMarketplace::DependencyResolver`
//! (`lib/mxrb/official_marketplace/dependency_resolver.rb`): discover
//! dependencies from the unresolved qualified references inside each
//! package's own model, verify Content API candidates against the MPK's
//! module identity, and preview the whole install chain before writing.
//!
//! Module dependencies and required widget bundles both resolve and
//! install end to end — every `WidgetId` in the model is checked against
//! the project's installed `widgets/*.mpk`; a missing one is resolved
//! against its mapped official content id (`official_widget_content_id`),
//! downloaded, verified to actually provide the required ids
//! (`WidgetBundleInventory`), and installed through the same envelope-kind
//! dispatch mxrb's `install_official_archive` uses — a `Widget`-kind
//! archive through [`crate::widget_package::install_widget`], a
//! `Module`-kind bundle (e.g. Data Widgets) through
//! [`lifecycle::install_module`] like any other module dependency. An
//! unmapped widget id stays a named blocker, mirroring mxrb's own "no
//! verified official Marketplace widget mapping".

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::lifecycle::{self, ModulePackageInventory, OfficialProvenance, with_rollback};
use crate::lock::{ORIGINALS_RELATIVE, read_lock, safe_target_path};
use crate::package::ModulePackage;
use crate::transport::Transport;
use crate::widget_package::{
    PackageKind, WidgetBundleInventory, WidgetPackageInventory, envelope_kind, install_widget,
};
use crate::{ContentApi, MarketplaceError, Package, Result, SearchQuery};

/// `Installer::DOWNLOADABLE_CONTENT_TYPES` — one wider than
/// `IMPORTABLE_CONTENT_TYPES`, since a standalone widget cannot be
/// imported as a module but can still be pulled and installed.
const DOWNLOADABLE_CONTENT_TYPES: [&str; 3] = ["Module", "Service", "Widget"];

/// `DependencyResolver::PLATFORM_MODULES`.
const PLATFORM_MODULES: [&str; 1] = ["System"];

/// `DependencyResolver::IMPORTABLE_CONTENT_TYPES` (via `Installer`).
const IMPORTABLE_CONTENT_TYPES: [&str; 2] = ["Module", "Service"];

/// `DependencyResolver::OFFICIAL_CONTENT_IDS` — well-known modules whose
/// Marketplace name search is unreliable.
const OFFICIAL_CONTENT_IDS: [(&str, &str); 3] = [
    ("FeedbackModule", "205506"),
    ("DataWidgets", "116540"),
    ("Administration", "23513"),
];

/// `DependencyResolver::OFFICIAL_WIDGET_CONTENT_IDS`.
fn official_widget_content_id(widget_id: &str) -> Option<&'static str> {
    let lower = widget_id.to_lowercase();
    if lower == "com.mendix.widget.web.combobox.combobox" {
        return Some("219304");
    }
    const DATA_WIDGET_FAMILIES: [&str; 9] = [
        "datagrid",
        "datagriddatefilter",
        "datagriddropdownfilter",
        "datagridnumberfilter",
        "datagridtextfilter",
        "dropdownsort",
        "gallery",
        "selectionhelper",
        "treenode",
    ];
    let rest = lower.strip_prefix("com.mendix.widget.web.")?;
    let (family, _) = rest.split_once('.')?;
    DATA_WIDGET_FAMILIES.contains(&family).then_some("116540")
}

/// One module the plan would install.
#[derive(Debug)]
pub struct ResolvedDependency {
    pub module_name: String,
    /// Official provenance when the archive was resolved and downloaded
    /// from the Content API; `None` for an already-cached local archive.
    pub package: Option<Package>,
    pub archive: PathBuf,
}

/// One resolved widget bundle the plan would install — ports
/// `WidgetDependency`.
#[derive(Debug)]
pub struct ResolvedWidgetDependency {
    pub content_id: String,
    pub package: Package,
    pub archive: PathBuf,
    pub widget_ids: Vec<String>,
}

fn provenance_for(package: &Package) -> OfficialProvenance {
    OfficialProvenance {
        content_id: Some(package.content.content_id.to_string()),
        version_id: Some(package.version.version_id.clone()),
        version: Some(package.version.version_number.clone()),
        source: Some("mendix".into()),
        repository: None,
    }
}

/// Preview for a recursively verified dependency installation — ports
/// `DependencyPlan`.
#[derive(Debug)]
pub struct DependencyPlan {
    pub root: String,
    pub dependencies: Vec<ResolvedDependency>,
    pub widget_dependencies: Vec<ResolvedWidgetDependency>,
    pub blockers: Vec<String>,
    target: PathBuf,
    mpr: PathBuf,
    /// Keeps downloaded candidate archives alive until `apply`.
    _downloads: tempfile::TempDir,
}

impl DependencyPlan {
    pub fn safe(&self) -> bool {
        self.blockers.is_empty()
    }

    pub fn changes(&self) -> Vec<String> {
        self.dependencies
            .iter()
            .map(|dependency| {
                format!(
                    "install {} {}",
                    dependency.module_name,
                    dependency
                        .package
                        .as_ref()
                        .map(|package| package.version.version_number.clone())
                        .unwrap_or_else(|| "(cached)".into())
                )
            })
            .chain(self.widget_dependencies.iter().map(|dependency| {
                format!(
                    "install widget bundle {} {}",
                    dependency.package.name(),
                    dependency.package.version.version_number
                )
            }))
            .collect()
    }

    /// Installs every resolved dependency inside one rollback scope —
    /// `DependencyPlan#apply!` + `apply_dependencies`.
    pub fn apply(self) -> Result<()> {
        if !self.safe() {
            return Err(MarketplaceError::PlanBlocked(self.blockers.join("; ")));
        }
        let mut paths = vec![
            self.mpr.clone(),
            self.mpr
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("mprcontents"),
            crate::lock::lock_path(&self.target),
            self.target.join(crate::lock::CACHE_RELATIVE),
            self.target.join(ORIGINALS_RELATIVE),
            self.target.join("theme/web/custom-variables.scss"),
        ];
        for dependency in &self.dependencies {
            let inventory = ModulePackageInventory::read(&dependency.archive)?;
            for relative in inventory.files.keys() {
                paths.push(safe_target_path(&self.target, relative)?);
            }
        }
        for dependency in &self.widget_dependencies {
            if envelope_kind(&dependency.archive)? == PackageKind::Widget {
                let inventory = WidgetPackageInventory::read(&dependency.archive)?;
                paths.push(safe_target_path(
                    &self.target,
                    &format!("widgets/{}", inventory.project_filename),
                )?);
            } else {
                let inventory = ModulePackageInventory::read(&dependency.archive)?;
                for relative in inventory.files.keys() {
                    paths.push(safe_target_path(&self.target, relative)?);
                }
            }
        }
        with_rollback(&paths, || {
            for dependency in &self.dependencies {
                let provenance = dependency
                    .package
                    .as_ref()
                    .map(provenance_for)
                    .unwrap_or_default();
                lifecycle::install_module(
                    &dependency.archive,
                    &self.mpr,
                    Some(&self.target),
                    // mxrb allows the model-version move only for official
                    // Mendix packages (`package.source == :mendix`).
                    dependency.package.is_some(),
                    &provenance,
                )?;
            }
            for dependency in &self.widget_dependencies {
                // `package_installed?` — a candidate already installed by
                // an earlier module dependency (e.g. bundled the same
                // widget) needs no separate install.
                if content_id_installed(&self.target, &dependency.content_id)? {
                    continue;
                }
                let provenance = provenance_for(&dependency.package);
                match envelope_kind(&dependency.archive)? {
                    PackageKind::Widget => {
                        install_widget(
                            &dependency.archive,
                            &self.target,
                            &dependency.package.version.version_number,
                            &provenance,
                        )?;
                    }
                    PackageKind::Module => {
                        lifecycle::install_module(
                            &dependency.archive,
                            &self.mpr,
                            Some(&self.target),
                            true,
                            &provenance,
                        )?;
                    }
                }
            }
            Ok(())
        })
    }
}

/// Resolves, downloads, and previews an official update for an installed
/// module — ports `Installer#update_official` +
/// `Lifecycle#plan_update`.
pub fn plan_update_official<T: Transport>(
    target: &Path,
    identifier: &str,
    version: Option<&str>,
    mendix_version: Option<&str>,
    api: &ContentApi<T>,
) -> Result<lifecycle::UpdatePlan> {
    let target_absolute =
        std::path::absolute(target).map_err(|source| MarketplaceError::PackageIo {
            path: target.display().to_string(),
            source,
        })?;
    let (_name, entry) = lifecycle::installed(&target_absolute, identifier)?;
    let content_id = entry
        .content_id
        .clone()
        .unwrap_or_else(|| identifier.to_string());
    let mpr = safe_target_path(&target_absolute, &entry.destination)?;
    let detected_mendix_version = mxrs_mpr::MprFile::open(&mpr, true)?.mendix_version()?;
    let mendix_version = mendix_version.or(detected_mendix_version.as_deref());
    let package = api.resolve(&content_id, version, mendix_version)?;
    if !DOWNLOADABLE_CONTENT_TYPES.contains(&package.content.content_type.as_str()) {
        return Err(MarketplaceError::WidgetInstall(format!(
            "Marketplace content {:?} is {}, not a Module, Service, or Widget",
            package.name(),
            package.content.content_type
        )));
    }
    let downloads = tempfile::tempdir().map_err(|source| MarketplaceError::PackageIo {
        path: "temporary directory".into(),
        source,
    })?;
    let archive = downloads.path().join("package.mpk");
    api.download(&package, &archive)?;
    let plan = lifecycle::plan_update(
        &target_absolute,
        identifier,
        &archive,
        &package.version.version_number,
        provenance_for(&package),
    )?;
    Ok(lifecycle::attach_download(plan, downloads))
}

/// Whether any locked package (module or widget) already carries this
/// content id — mxrb's `package_installed?`.
fn content_id_installed(target: &Path, content_id: &str) -> Result<bool> {
    Ok(read_lock(target)?
        .packages
        .values()
        .any(|entry| entry.content_id.as_deref() == Some(content_id)))
}

pub fn plan_dependencies<T: Transport>(
    target: &Path,
    identifier: &str,
    api: Option<&ContentApi<T>>,
    mendix_version: Option<&str>,
) -> Result<DependencyPlan> {
    let target = std::path::absolute(target).map_err(|source| MarketplaceError::PackageIo {
        path: target.display().to_string(),
        source,
    })?;
    let (root, entry) = lifecycle::installed(&target, identifier)?;
    let mpr = safe_target_path(&target, &entry.destination)?;
    let root_archive = safe_target_path(&target, &entry.archive)?;
    if !root_archive.is_file() {
        return Err(MarketplaceError::MissingCachedPackage(
            root_archive.display().to_string(),
        ));
    }
    let downloads = tempfile::tempdir().map_err(|source| MarketplaceError::PackageIo {
        path: "temporary directory".into(),
        source,
    })?;
    let mut resolver = Resolver {
        target: target.clone(),
        api,
        mendix_version,
        downloads: downloads.path().to_path_buf(),
        project_modules: project_module_names(&mpr)?,
        required_widget_ids: widget_ids(&mpr)?,
        resolved: Vec::new(),
        visited: BTreeSet::new(),
        blockers: Vec::new(),
        widget_archives: std::collections::BTreeMap::new(),
    };
    resolver.visit(&root, &root_archive)?;
    let widget_dependencies = resolver.resolve_widget_dependencies()?;
    let mut seen = BTreeSet::new();
    let blockers: Vec<String> = resolver
        .blockers
        .into_iter()
        .filter(|blocker| seen.insert(blocker.clone()))
        .collect();
    Ok(DependencyPlan {
        root,
        dependencies: resolver.resolved,
        widget_dependencies,
        blockers,
        target,
        mpr,
        _downloads: downloads,
    })
}

struct Resolver<'api, T: Transport> {
    target: PathBuf,
    api: Option<&'api ContentApi<T>>,
    mendix_version: Option<&'api str>,
    downloads: PathBuf,
    project_modules: Vec<String>,
    required_widget_ids: BTreeSet<String>,
    resolved: Vec<ResolvedDependency>,
    visited: BTreeSet<String>,
    blockers: Vec<String>,
    /// Content id → already-resolved (package, downloaded archive), so a
    /// widget content the module-dependency walk already touched is never
    /// re-resolved or re-downloaded.
    widget_archives: std::collections::BTreeMap<String, (Package, PathBuf)>,
}

impl<T: Transport> Resolver<'_, T> {
    /// `DependencyResolver#visit`.
    fn visit(&mut self, module_name: &str, archive: &Path) -> Result<()> {
        if !self.visited.insert(module_name.to_string()) {
            return Ok(());
        }
        for dependency_name in self.dependency_names(archive)? {
            if PLATFORM_MODULES.contains(&dependency_name.as_str())
                || dependency_name == module_name
            {
                continue;
            }
            let lock = read_lock(&self.target)?;
            if self.project_modules.contains(&dependency_name)
                && !lock.packages.contains_key(&dependency_name)
            {
                continue;
            }
            let dependency = match self.installed_dependency(&dependency_name)? {
                Some(dependency) => Some(dependency),
                None => self.resolve_dependency(&dependency_name)?,
            };
            let Some(dependency) = dependency else {
                continue;
            };
            let name = dependency.module_name.clone();
            let archive = dependency.archive.clone();
            // Recurse first, record after — post-order, so the deepest
            // dependency installs before whatever needs it (mxrb's
            // `visit` inserts into `@resolved` after recursing).
            self.visit(&name, &archive)?;
            let installed = read_lock(&self.target)?.packages.contains_key(&name);
            if !installed
                && !self
                    .resolved
                    .iter()
                    .any(|existing| existing.module_name == name)
            {
                self.resolved.push(dependency);
            }
        }
        Ok(())
    }

    /// `DependencyResolver#dependency_names` — the first path segment of
    /// every unresolved qualified reference inside the package's model,
    /// while also accumulating the widget ids that model requires.
    fn dependency_names(&mut self, archive: &Path) -> Result<Vec<String>> {
        let staging = tempfile::tempdir().map_err(|source| MarketplaceError::PackageIo {
            path: "temporary directory".into(),
            source,
        })?;
        let mut package = ModulePackage::open(archive)?;
        let descriptor = package.descriptor()?;
        let project_path = package.extract_project(&descriptor, staging.path())?;
        self.required_widget_ids.extend(widget_ids(&project_path)?);
        let project = mxrs_model::Project::open(&project_path, true).map_err(|error| {
            MarketplaceError::InvalidPackage {
                path: archive.display().to_string(),
                message: error.to_string(),
            }
        })?;
        let index = mxrs_semantic::SemanticIndex::build(&project).map_err(|error| {
            MarketplaceError::InvalidPackage {
                path: archive.display().to_string(),
                message: error.to_string(),
            }
        })?;
        let mut names: Vec<String> = index
            .unresolved_reference_targets()
            .iter()
            .filter_map(|target| {
                target
                    .split(['.', '/'])
                    .next()
                    .filter(|segment| !segment.is_empty())
                    .map(str::to_string)
            })
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// `DependencyResolver#installed_dependency`.
    fn installed_dependency(&self, name: &str) -> Result<Option<ResolvedDependency>> {
        let lock = read_lock(&self.target)?;
        let Some((module_name, entry)) = lock
            .packages
            .iter()
            .find(|(module_name, _)| module_name.eq_ignore_ascii_case(name))
        else {
            return Ok(None);
        };
        Ok(Some(ResolvedDependency {
            module_name: module_name.clone(),
            package: None,
            archive: safe_target_path(&self.target, &entry.archive)?,
        }))
    }

    /// `DependencyResolver#resolve_dependency` — candidates come from the
    /// well-known content ids plus name searches; each is resolved,
    /// downloaded, and verified against the MPK's own module identity.
    fn resolve_dependency(&mut self, name: &str) -> Result<Option<ResolvedDependency>> {
        let Some(api) = self.api else {
            self.blockers.push(format!(
                "{name}: a Mendix credential is required to resolve Marketplace dependencies"
            ));
            return Ok(None);
        };
        let mut failures = Vec::new();
        let mut seen = BTreeSet::new();
        for content_id in self.candidate_ids(api, name, &mut failures) {
            if !seen.insert(content_id.clone()) {
                continue;
            }
            match self.try_candidate(api, name, &content_id) {
                Ok(Some(dependency)) => return Ok(Some(dependency)),
                Ok(None) => {}
                Err(error) => {
                    failures.push(format!("{name}: candidate {content_id} failed: {error}"));
                }
            }
        }
        self.blockers.extend(failures);
        self.blockers.push(format!(
            "{name}: no official Marketplace package matched the module identity"
        ));
        Ok(None)
    }

    fn candidate_ids(
        &self,
        api: &ContentApi<T>,
        name: &str,
        failures: &mut Vec<String>,
    ) -> Vec<String> {
        let mut ids = Vec::new();
        if let Some((_, content_id)) = OFFICIAL_CONTENT_IDS
            .iter()
            .find(|(module, _)| module.eq_ignore_ascii_case(name))
        {
            ids.push((*content_id).to_string());
        }
        for query in queries(name) {
            match api.search(&SearchQuery {
                name: Some(query),
                limit: Some(100),
                ..SearchQuery::default()
            }) {
                Ok(results) => ids.extend(
                    results
                        .into_iter()
                        .filter(|content| {
                            IMPORTABLE_CONTENT_TYPES.contains(&content.content_type.as_str())
                        })
                        .map(|content| content.content_id.to_string()),
                ),
                Err(error) => failures.push(format!("{name}: search failed: {error}")),
            }
        }
        ids
    }

    fn try_candidate(
        &mut self,
        api: &ContentApi<T>,
        name: &str,
        content_id: &str,
    ) -> Result<Option<ResolvedDependency>> {
        let package = api.resolve(content_id, None, self.mendix_version)?;
        let destination = self.downloads.join(format!(
            "{}-{}.mpk",
            package.content.content_id, package.version.version_id
        ));
        if !destination.is_file() {
            api.download(&package, &destination)?;
        }
        let inventory = ModulePackageInventory::read(&destination)?;
        if inventory.name != name {
            return Ok(None);
        }
        Ok(Some(ResolvedDependency {
            module_name: name.to_string(),
            package: Some(package),
            archive: destination,
        }))
    }

    /// `DependencyResolver#resolve_widget_dependencies` — every
    /// required-but-missing widget id is grouped by its mapped official
    /// content id, then each group is resolved, downloaded, and verified
    /// to actually provide every required id in that group. An unmapped
    /// id, or a group whose resolution fails, becomes a named blocker
    /// instead of failing the whole plan.
    fn resolve_widget_dependencies(&mut self) -> Result<Vec<ResolvedWidgetDependency>> {
        let mut missing: Vec<String> = Vec::new();
        for widget_id in std::mem::take(&mut self.required_widget_ids) {
            let installed = mxrs_widget_package::find(&self.target, &widget_id)
                .map_err(|error| MarketplaceError::InvalidPackage {
                    path: self.target.join("widgets").display().to_string(),
                    message: error.to_string(),
                })?
                .is_some();
            if !installed {
                missing.push(widget_id);
            }
        }
        let mut grouped: std::collections::BTreeMap<&'static str, Vec<String>> =
            std::collections::BTreeMap::new();
        for widget_id in missing {
            match official_widget_content_id(&widget_id) {
                Some(content_id) => grouped.entry(content_id).or_default().push(widget_id),
                None => self.blockers.push(format!(
                    "{widget_id}: no verified official Marketplace widget mapping"
                )),
            }
        }
        let mut resolved = Vec::new();
        for (content_id, mut required_ids) in grouped {
            required_ids.sort();
            match self.resolve_widget_dependency(content_id, &required_ids) {
                Ok(Some(dependency)) => resolved.push(dependency),
                Ok(None) => {}
                Err(error) => self
                    .blockers
                    .push(format!("widget content {content_id}: {error}")),
            }
        }
        Ok(resolved)
    }

    /// `DependencyResolver#resolve_widget_dependency`.
    fn resolve_widget_dependency(
        &mut self,
        content_id: &str,
        required_ids: &[String],
    ) -> Result<Option<ResolvedWidgetDependency>> {
        if content_id_installed(&self.target, content_id)? {
            return Err(MarketplaceError::WidgetInstall(format!(
                "locked official package is present but does not provide {}",
                required_ids.join(", ")
            )));
        }
        let Some(api) = self.api else {
            self.blockers.push(format!(
                "widget content {content_id}: a Mendix credential is required to resolve \
                 Marketplace widget dependencies"
            ));
            return Ok(None);
        };
        let existing = self
            .widget_archives
            .get(content_id)
            .map(|(package, archive)| (package.clone(), archive.clone()));
        let (package, archive) = match existing {
            Some(found) => found,
            None => {
                let package = api.resolve(content_id, None, self.mendix_version)?;
                if !DOWNLOADABLE_CONTENT_TYPES.contains(&package.content.content_type.as_str()) {
                    return Err(MarketplaceError::WidgetInstall(format!(
                        "Marketplace content {:?} is {}, not a Module, Service, or Widget",
                        package.name(),
                        package.content.content_type
                    )));
                }
                let destination = self.downloads.join(format!(
                    "{}-{}.mpk",
                    package.content.content_id, package.version.version_id
                ));
                if !destination.is_file() {
                    api.download(&package, &destination)?;
                }
                self.widget_archives.insert(
                    content_id.to_string(),
                    (package.clone(), destination.clone()),
                );
                (package, destination)
            }
        };
        let inventory = WidgetBundleInventory::read(&archive)?;
        let missing: Vec<&String> = required_ids
            .iter()
            .filter(|id| !inventory.widget_ids.contains(id))
            .collect();
        if !missing.is_empty() {
            return Err(MarketplaceError::WidgetInstall(format!(
                "downloaded package does not provide {}",
                missing
                    .iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(Some(ResolvedWidgetDependency {
            content_id: content_id.to_string(),
            package,
            archive,
            widget_ids: inventory.widget_ids,
        }))
    }
}

/// `DependencyResolver#queries` — the humanized name first, then the raw
/// module name.
fn queries(name: &str) -> Vec<String> {
    let mut humanized = String::new();
    let mut previous: Option<char> = None;
    for character in name.chars() {
        if character == '_' {
            humanized.push(' ');
            previous = Some(' ');
            continue;
        }
        if character.is_ascii_uppercase()
            && previous.is_some_and(|prior| prior.is_ascii_lowercase() || prior.is_ascii_digit())
        {
            humanized.push(' ');
        }
        humanized.push(character);
        previous = Some(character);
    }
    let mut queries = vec![humanized];
    if !queries.contains(&name.to_string()) {
        queries.push(name.to_string());
    }
    queries
}

fn project_module_names(mpr: &Path) -> Result<Vec<String>> {
    let project =
        mxrs_model::Project::open(mpr, true).map_err(|error| MarketplaceError::InvalidPackage {
            path: mpr.display().to_string(),
            message: error.to_string(),
        })?;
    Ok(project
        .modules()
        .map_err(|error| MarketplaceError::InvalidPackage {
            path: mpr.display().to_string(),
            message: error.to_string(),
        })?
        .into_iter()
        .filter_map(|module| module.name)
        .collect())
}

/// `DependencyResolver#widget_ids` — every `WidgetId` value anywhere in the
/// model's decoded units.
fn widget_ids(mpr_path: &Path) -> Result<BTreeSet<String>> {
    let mpr = mxrs_mpr::MprFile::open(mpr_path, true)?;
    let mut ids = BTreeSet::new();
    for unit in mpr.all_units()? {
        let document = mpr.parse_contents(&unit)?;
        collect_widget_ids(&mxrs_bson::Bson::Document(document), &mut ids);
    }
    Ok(ids)
}

fn collect_widget_ids(value: &mxrs_bson::Bson, ids: &mut BTreeSet<String>) {
    match value {
        mxrs_bson::Bson::Document(document) => {
            if let Ok(widget_id) = document.get_str("WidgetId")
                && !widget_id.is_empty()
            {
                ids.insert(widget_id.to_string());
            }
            for (_, nested) in document.iter() {
                collect_widget_ids(nested, ids);
            }
        }
        mxrs_bson::Bson::Array(items) => {
            for item in items {
                collect_widget_ids(item, ids);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_widget_ids_map_to_their_content_and_names_humanize() {
        assert_eq!(
            official_widget_content_id("com.mendix.widget.web.combobox.Combobox"),
            Some("219304")
        );
        assert_eq!(
            official_widget_content_id("com.mendix.widget.web.datagrid.Datagrid"),
            Some("116540")
        );
        assert_eq!(
            official_widget_content_id("com.mendix.widget.web.dropdownsort.DropdownSort"),
            Some("116540")
        );
        assert_eq!(
            official_widget_content_id("com.example.rating.Rating"),
            None
        );
        assert_eq!(
            queries("CommunityCommons"),
            ["Community Commons", "CommunityCommons"]
        );
        assert_eq!(queries("Data_Widgets2"), ["Data Widgets2", "Data_Widgets2"]);
    }
}
