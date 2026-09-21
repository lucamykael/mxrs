//! Recursive dependency resolution for installed marketplace modules —
//! ports `Mxrb::OfficialMarketplace::DependencyResolver`
//! (`lib/mxrb/official_marketplace/dependency_resolver.rb`): discover
//! dependencies from the unresolved qualified references inside each
//! package's own model, verify Content API candidates against the MPK's
//! module identity, and preview the whole install chain before writing.
//!
//! Honest scope: module dependencies resolve and install end to end;
//! required *widget bundles* are detected the same way mxrb detects them
//! (every `WidgetId` in the model, checked against the project's installed
//! `widgets/*.mpk`) but their installation is not ported yet, so a missing
//! widget bundle is always a named blocker — mapped ones name the official
//! content id to fetch manually, unmapped ones mirror mxrb's "no verified
//! official Marketplace widget mapping".

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::lifecycle::{self, ModulePackageInventory, OfficialProvenance, with_rollback};
use crate::lock::{ORIGINALS_RELATIVE, read_lock, safe_target_path};
use crate::package::ModulePackage;
use crate::transport::Transport;
use crate::{ContentApi, MarketplaceError, Package, Result, SearchQuery};

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

/// Preview for a recursively verified dependency installation — ports
/// `DependencyPlan`.
#[derive(Debug)]
pub struct DependencyPlan {
    pub root: String,
    pub dependencies: Vec<ResolvedDependency>,
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
        with_rollback(&paths, || {
            for dependency in &self.dependencies {
                let provenance = dependency
                    .package
                    .as_ref()
                    .map(|package| OfficialProvenance {
                        content_id: Some(package.content.content_id.to_string()),
                        version_id: Some(package.version.version_id.clone()),
                        version: Some(package.version.version_number.clone()),
                        source: Some("mendix".into()),
                        repository: None,
                    })
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
            Ok(())
        })
    }
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
    };
    resolver.visit(&root, &root_archive)?;
    resolver.widget_blockers()?;
    let mut seen = BTreeSet::new();
    let blockers: Vec<String> = resolver
        .blockers
        .into_iter()
        .filter(|blocker| seen.insert(blocker.clone()))
        .collect();
    Ok(DependencyPlan {
        root,
        dependencies: resolver.resolved,
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

    /// `DependencyResolver#resolve_widget_dependencies`, honestly scoped:
    /// every required-but-missing widget bundle is a blocker (see the
    /// module doc comment).
    fn widget_blockers(&mut self) -> Result<()> {
        for widget_id in std::mem::take(&mut self.required_widget_ids) {
            let installed = mxrs_widget_package::find(&self.target, &widget_id)
                .map_err(|error| MarketplaceError::InvalidPackage {
                    path: self.target.join("widgets").display().to_string(),
                    message: error.to_string(),
                })?
                .is_some();
            if installed {
                continue;
            }
            match official_widget_content_id(&widget_id) {
                Some(content_id) => self.blockers.push(format!(
                    "widget content {content_id}: official widget bundle installation is not \
                     ported; download it with `mxrs marketplace download {content_id}` and \
                     install the .mpk into widgets/ manually"
                )),
                None => self.blockers.push(format!(
                    "{widget_id}: no verified official Marketplace widget mapping"
                )),
            }
        }
        Ok(())
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
