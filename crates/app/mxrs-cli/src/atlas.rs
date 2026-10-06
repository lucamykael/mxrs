//! A new project with Atlas: the look Studio Pro gives an application —
//! Atlas Core's theme and layouts, and Atlas Web Content's page templates
//! and building blocks — both from the Marketplace, as the user has them.
//!
//! The project is made the way one with installed modules is made: a model
//! with the project's own module, the packages installed into it, and the
//! whole imported as a Cargo-native project — the importer's layout for
//! installed modules (`src/packages/`, `assets/`) being the one a project
//! has for them.

use std::path::{Path, PathBuf};

use mxrs_marketplace::{ContentApi, Credentials, plan_install, ureq_transport::UreqTransport};

/// A Marketplace package a new project is given.
pub struct AtlasPackage {
    pub name: &'static str,
    /// The Marketplace content id.
    pub content_id: &'static str,
    /// The file it is cached as.
    pub file: &'static str,
}

/// Atlas Core first: the others build on its theme. Data Widgets is what
/// Atlas Web Content's templates are made of — the data grid, the gallery
/// and their filters — and what styles them.
pub const PACKAGES: [AtlasPackage; 3] = [
    AtlasPackage {
        name: "Atlas Core",
        content_id: "117187",
        file: "Atlas_Core.mpk",
    },
    AtlasPackage {
        name: "Atlas Web Content",
        content_id: "117183",
        file: "Atlas_Web_Content.mpk",
    },
    AtlasPackage {
        name: "Data Widgets",
        content_id: "116540",
        file: "Data_Widgets.mpk",
    },
];

/// Where downloaded packages are kept: `$XDG_CACHE_HOME/mxrs/marketplace`.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("mxrs").join("marketplace")
}

/// Every Atlas package, from the cache or — with the user's Marketplace
/// token — downloaded into it, for Mendix `mendix_version`.
pub fn packages(mendix_version: &str) -> Result<Vec<PathBuf>, String> {
    packages_in(&cache_dir(), mendix_version)
}

/// [`packages`], kept under `cache`.
pub fn packages_in(cache: &Path, mendix_version: &str) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::with_capacity(PACKAGES.len());
    let mut client = None;
    for package in &PACKAGES {
        let path = cache.join(package.file);
        if !path.is_file() {
            let api = match &client {
                Some(api) => api,
                None => {
                    let pat = Credentials::default().resolve().map_err(|error| {
                        format!(
                            "{} is not in {} and cannot be downloaded: {error}",
                            package.name,
                            cache.display()
                        )
                    })?;
                    client.insert(ContentApi::new(UreqTransport::new(), pat))
                }
            };
            let resolved = api
                .resolve(package.content_id, None, Some(mendix_version))
                .map_err(|error| format!("{}: {error}", package.name))?;
            std::fs::create_dir_all(cache)
                .map_err(|error| format!("{}: {error}", cache.display()))?;
            let bytes = api
                .download(&resolved, &path)
                .map_err(|error| format!("{}: {error}", package.name))?;
            println!(
                "[mxrs] downloaded {} {} ({bytes} bytes)",
                resolved.name(),
                resolved.version.version_number
            );
        }
        found.push(path);
    }
    Ok(found)
}

/// The files a project's own theme starts with: the `theme/web` Studio Pro
/// gives a blank app (Atlas, Apache-2.0) — the variables Atlas Core's
/// stylesheet reads from the project, every exclusion switched off, a
/// `main.scss` for the project's own styling, and the settings naming the
/// compiled stylesheet. Atlas Core itself ships none of them.
const THEME_FILES: [(&str, &str); 4] = [
    (
        "theme/web/custom-variables.scss",
        include_str!("../assets/atlas/theme/web/custom-variables.scss"),
    ),
    (
        "theme/web/exclusion-variables.scss",
        include_str!("../assets/atlas/theme/web/exclusion-variables.scss"),
    ),
    (
        "theme/web/main.scss",
        include_str!("../assets/atlas/theme/web/main.scss"),
    ),
    (
        "theme/web/settings.json",
        include_str!("../assets/atlas/theme/web/settings.json"),
    ),
];

/// Creates the project `name` at `destination`, for Mendix `version`,
/// with Atlas installed: the model is written, the packages installed into
/// it, and the result imported as the project — whose first page, `Home`,
/// is shown in Atlas's default layout.
pub fn create(
    name: &str,
    version: &str,
    destination: &Path,
    workspace: Option<&Path>,
    packages: &[PathBuf],
) -> Result<(), String> {
    if destination.symlink_metadata().is_ok() {
        return Err(format!(
            "scaffold destination already exists: {}",
            destination.display()
        ));
    }
    let staging = tempfile::tempdir().map_err(|error| format!("temporary directory: {error}"))?;
    let mpr = staging.path().join(format!("{name}.mpr"));
    let mut builder = mxrs_dsl::ProjectBuilder::new(version);
    builder.module("Main", |_| {});
    // The home page is scaffolded once the project exists, with Atlas's
    // layout: the profile names it then.
    builder.navigation(|navigation| {
        navigation.profile("Responsive", |_| {});
    });
    mxrs_writer::write_project(&mpr, &builder.build())
        .map_err(|error| format!("writing the model: {error}"))?;
    for package in packages {
        let plan = plan_install(package, &mpr, Some(staging.path()), true)
            .map_err(|error| format!("{}: {error}", package.display()))?;
        let installed = plan
            .apply()
            .map_err(|error| format!("{}: {error}", package.display()))?;
        println!(
            "[mxrs] installed {} ({} units, {} files)",
            installed.module_name, installed.units, installed.files
        );
    }
    for (relative, text) in THEME_FILES {
        let path = staging.path().join(relative);
        if path.exists() {
            continue;
        }
        std::fs::create_dir_all(path.parent().expect("a file has a folder"))
            .and_then(|()| std::fs::write(&path, text))
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    mxrs_exporter::import_cargo_project(&mpr, destination, workspace)
        .map_err(|error| format!("importing the model: {error}"))?;
    // The project's own module, declared the way a plain project declares
    // it: the importer writes nothing for a module that holds nothing yet.
    let main = mxrs_scaffold::ArtifactScaffold::new(
        mxrs_scaffold::ArtifactKind::Module,
        "Main",
        destination,
    );
    mxrs_scaffold::scaffold_artifact(&main).map_err(|error| format!("the Main module: {error}"))?;
    // The page the navigation opens, in Atlas's layout.
    let home = mxrs_scaffold::ArtifactScaffold::new(
        mxrs_scaffold::ArtifactKind::Page,
        "Main.Home",
        destination,
    )
    .page_template(Some("starter".to_string()));
    mxrs_scaffold::scaffold_artifact(&home).map_err(|error| format!("the Home page: {error}"))?;
    let navigation = destination.join("frontend/src/navigation/index.ts");
    let source = std::fs::read_to_string(&navigation)
        .map_err(|error| format!("{}: {error}", navigation.display()))?;
    let profile = "      name: \"Responsive\",\n";
    if !source.contains("homePage:") && source.contains(profile) {
        let named = source.replacen(
            profile,
            &format!("{profile}      homePage: \"Main.Home\",\n"),
            1,
        );
        std::fs::write(&navigation, named)
            .map_err(|error| format!("{}: {error}", navigation.display()))?;
    } else if !source.contains("homePage:") {
        eprintln!(
            "[mxrs] warning: {} is not laid out the way mxrs writes it: set `homePage: \"Main.Home\"` on its Responsive profile yourself",
            navigation.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_packages_are_found_without_the_marketplace() {
        let cache = tempfile::tempdir().unwrap();
        for package in &PACKAGES {
            std::fs::write(cache.path().join(package.file), b"not really a package").unwrap();
        }
        let found = packages_in(cache.path(), "11.12.1").unwrap();
        assert_eq!(
            found,
            PACKAGES
                .iter()
                .map(|package| cache.path().join(package.file))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_theme_files_are_the_ones_atlas_core_reads() {
        let file = |name: &str| {
            THEME_FILES
                .iter()
                .find(|(path, _)| *path == name)
                .map(|(_, text)| *text)
                .unwrap_or_else(|| panic!("{name} is written"))
        };
        // The variables Atlas Core's stylesheet imports from the project.
        assert!(file("theme/web/custom-variables.scss").contains("$brand-primary:"));
        assert!(file("theme/web/exclusion-variables.scss").contains("$exclude-bootstrap: false;"));
        assert!(file("theme/web/main.scss").contains("@import \"custom-variables\";"));
        assert!(file("theme/web/settings.json").contains("\"theme.compiled.css\""));
    }
}
