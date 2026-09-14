//! Reading a Mendix module package (`.mpk`). Ports
//! `lib/mxrb/official_marketplace/module_package_importer.rb`'s
//! `ModulePackageReader`.
//!
//! A `.mpk` is a zip holding:
//!
//! - `package.xml` — the manifest: which module, which embedded project file,
//!   and every asset the package claims to install.
//! - `project.mpr` — a real Mendix project whose named module is what gets
//!   imported.
//! - `javasource/`, `vendorlib/`, `themesource/`, … — the declared assets.
//!
//! **Every path out of the archive is untrusted.** A package is a file from
//! the internet, and a zip entry named `../../.ssh/authorized_keys` is a
//! two-line change away from being written there. [`safe_relative_path`]
//! rejects absolute paths, `..`, empty segments, and the directories a
//! project cannot let a package overwrite — the same refusals mxrb makes, for
//! the same reason.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::{MarketplaceError, Result};

/// Top-level directories a package may never write into: version control, the
/// tool's own state, and the storage sidecar of the target `.mpr`.
const PROTECTED_ROOTS: &[&str] = &[".git", ".mxrs", ".mxrb", "mprcontents"];

/// What `package.xml` (plus an optional `manifest.json`) declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDescriptor {
    pub module_name: String,
    /// Package version from `manifest.json`; absent in packages that carry no
    /// manifest, which is common.
    pub version: Option<String>,
    /// Mendix model version the package declares. Checked against the embedded
    /// `.mpr` so a mislabelled package fails before anything is written.
    pub model_version: Option<String>,
    pub project_file: String,
    pub files: Vec<String>,
}

pub struct ModulePackage {
    archive: zip::ZipArchive<std::fs::File>,
    path: PathBuf,
}

impl ModulePackage {
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|source| MarketplaceError::PackageIo {
            path: path.display().to_string(),
            source,
        })?;
        let archive =
            zip::ZipArchive::new(file).map_err(|error| MarketplaceError::InvalidPackage {
                path: path.display().to_string(),
                message: error.to_string(),
            })?;
        Ok(Self {
            archive,
            path: path.to_path_buf(),
        })
    }

    pub fn descriptor(&mut self) -> Result<PackageDescriptor> {
        let manifest = self.read_optional("manifest.json")?;
        let package_xml =
            self.read_optional("package.xml")?
                .ok_or_else(|| MarketplaceError::InvalidPackage {
                    path: self.path.display().to_string(),
                    message: "package.xml is missing".into(),
                })?;
        let parsed = parse_package_xml(&package_xml, &self.path.display().to_string())?;
        let (version, model_version, package_type) = parse_manifest(manifest.as_deref())?;
        // mxrb accepts an absent type and refuses anything that is not a
        // module; a widget package needs a different installer entirely.
        if let Some(package_type) = &package_type
            && !package_type.eq_ignore_ascii_case("module")
        {
            return Err(MarketplaceError::UnsupportedPackageType(
                package_type.clone(),
            ));
        }
        Ok(PackageDescriptor {
            module_name: parsed.module_name,
            version,
            model_version,
            project_file: parsed.project_file,
            files: parsed.files,
        })
    }

    /// Extracts the embedded project to `destination`, returning its path.
    ///
    /// A Mendix v2 `.mpr` keeps its unit contents in a sibling `mprcontents/`
    /// directory rather than inline, and opening one without that directory
    /// fails outright. Packages published today embed a v1 (self-contained)
    /// project, so this usually copies a single file — but a v2 package would
    /// otherwise be unopenable, so the sidecar comes along when present.
    ///
    /// The sidecar is extracted *implicitly*, as part of the project. It is
    /// still refused as a **declared** asset path (see [`PROTECTED_ROOTS`]):
    /// shipping the project's own storage as an installable file would let a
    /// package overwrite the target's units.
    pub fn extract_project(
        &mut self,
        descriptor: &PackageDescriptor,
        destination: &Path,
    ) -> Result<PathBuf> {
        let target = destination.join("source.mpr");
        self.extract_entry(&descriptor.project_file, &target)?;
        self.extract_sidecar(&descriptor.project_file, destination)?;
        Ok(target)
    }

    /// Copies the `mprcontents/` directory that sits beside the project file,
    /// renaming it to match the extracted `source.mpr`.
    fn extract_sidecar(&mut self, project_file: &str, destination: &Path) -> Result<()> {
        let prefix = match project_file.rsplit_once('/') {
            Some((parent, _)) => format!("{parent}/mprcontents/"),
            None => "mprcontents/".to_string(),
        };
        let entries: Vec<String> = self
            .archive
            .file_names()
            .filter(|name| {
                let normalized = name.replace('\\', "/");
                normalized.starts_with(&prefix) && !normalized.ends_with('/')
            })
            .map(str::to_string)
            .collect();
        for name in entries {
            let relative = name.replace('\\', "/");
            let suffix = &relative[prefix.len()..];
            let target = destination.join("mprcontents").join(suffix);
            self.extract_entry(&name, &target)?;
        }
        Ok(())
    }

    /// Extracts every declared asset under `destination`, preserving relative
    /// layout, and returns `(relative, staged)` pairs.
    ///
    /// Staging before installing is what makes rollback possible: by the time
    /// anything touches the real project, every file is known to exist and to
    /// have a safe destination.
    pub fn stage_files(
        &mut self,
        descriptor: &PackageDescriptor,
        destination: &Path,
    ) -> Result<Vec<(String, PathBuf)>> {
        let mut staged = Vec::with_capacity(descriptor.files.len());
        for relative in &descriptor.files {
            let target = destination.join(relative);
            self.extract_entry(relative, &target)?;
            staged.push((relative.clone(), target));
        }
        Ok(staged)
    }

    fn extract_entry(&mut self, relative: &str, target: &Path) -> Result<()> {
        // Windows-authored packages may spell separators either way. Resolve
        // the name first so the archive is borrowed only once.
        let backslashed = relative.replace('/', "\\");
        let name = if self.archive.index_for_name(relative).is_some() {
            relative.to_string()
        } else if self.archive.index_for_name(&backslashed).is_some() {
            backslashed
        } else {
            return Err(MarketplaceError::MissingPackageFile {
                path: self.path.display().to_string(),
                file: relative.to_string(),
            });
        };
        let mut entry =
            self.archive
                .by_name(&name)
                .map_err(|_| MarketplaceError::MissingPackageFile {
                    path: self.path.display().to_string(),
                    file: relative.to_string(),
                })?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|source| MarketplaceError::PackageIo {
                path: parent.display().to_string(),
                source,
            })?;
        }
        let mut out =
            std::fs::File::create(target).map_err(|source| MarketplaceError::PackageIo {
                path: target.display().to_string(),
                source,
            })?;
        std::io::copy(&mut entry, &mut out).map_err(|source| MarketplaceError::PackageIo {
            path: target.display().to_string(),
            source,
        })?;
        Ok(())
    }

    fn read_optional(&mut self, name: &str) -> Result<Option<String>> {
        let Ok(mut entry) = self.archive.by_name(name) else {
            return Ok(None);
        };
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .map_err(|source| MarketplaceError::PackageIo {
                path: name.to_string(),
                source,
            })?;
        Ok(Some(text))
    }
}

struct ParsedPackageXml {
    module_name: String,
    project_file: String,
    files: Vec<String>,
}

/// Pulls the three things that matter out of `package.xml`.
///
/// Matches on local names so the document's namespaces — which vary between
/// Mendix versions — do not have to be modelled.
fn parse_package_xml(source: &str, package: &str) -> Result<ParsedPackageXml> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(source);
    let mut module_name = None;
    let mut project_file = None;
    let mut files = Vec::new();
    let mut buffer = Vec::new();

    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            MarketplaceError::InvalidPackage {
                path: package.to_string(),
                message: format!("package.xml is not valid XML: {error}"),
            }
        })?;
        match event {
            Event::Eof => break,
            Event::Start(element) | Event::Empty(element) => {
                let name = element.local_name();
                let local = String::from_utf8_lossy(name.as_ref()).to_string();
                match local.as_str() {
                    "module" if module_name.is_none() => {
                        module_name = attribute(&element, "name");
                    }
                    "projectFile" if project_file.is_none() => {
                        project_file = attribute(&element, "path");
                    }
                    "file" => {
                        if let Some(path) = attribute(&element, "path") {
                            files.push(path);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        buffer.clear();
    }

    let module_name = module_name.ok_or_else(|| MarketplaceError::InvalidPackage {
        path: package.to_string(),
        message: "package does not name a Mendix module".into(),
    })?;
    let project_file = project_file.ok_or_else(|| MarketplaceError::InvalidPackage {
        path: package.to_string(),
        message: "package does not declare a project file".into(),
    })?;

    let mut safe_files: Vec<String> = Vec::new();
    for file in files {
        let safe = safe_relative_path(&file)?;
        if !safe_files.contains(&safe) {
            safe_files.push(safe);
        }
    }
    Ok(ParsedPackageXml {
        module_name: valid_module_name(&module_name)?,
        project_file: safe_relative_path(&project_file)?,
        files: safe_files,
    })
}

fn attribute(element: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        let key = attribute.key.local_name();
        (String::from_utf8_lossy(key.as_ref()) == name)
            .then(|| String::from_utf8_lossy(&attribute.value).to_string())
    })
}

/// Reads `manifest.json` when present: `(version, model-version, type)`.
fn parse_manifest(
    manifest: Option<&str>,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let Some(manifest) = manifest else {
        return Ok((None, None, None));
    };
    let value: serde_json::Value =
        serde_json::from_str(manifest).map_err(|source| MarketplaceError::InvalidManifest {
            message: source.to_string(),
        })?;
    let string = |value: Option<&serde_json::Value>| {
        value
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let package = value.get("package");
    Ok((
        string(package.and_then(|package| package.get("version"))),
        string(value.get("model-version")),
        string(package.and_then(|package| package.get("type"))),
    ))
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
        Err(MarketplaceError::InvalidModuleName(value.to_string()))
    }
}

/// Normalizes a declared package path and refuses the ones that could escape
/// the project or clobber something a package has no business touching.
///
/// Rejected: absolute paths, Windows drive letters, any `..` or `.` segment,
/// empty segments (`a//b`), the [`PROTECTED_ROOTS`], and a nested `.mpr`,
/// which would overwrite the project being installed into.
pub(crate) fn safe_relative_path(value: &str) -> Result<String> {
    let normalized = value.replace('\\', "/");
    let unsafe_path = normalized.is_empty()
        || normalized.starts_with('/')
        // `C:/…` and `C:\…` both arrive here as `C:/…`.
        || normalized
            .split_once(':')
            .is_some_and(|(prefix, _)| prefix.len() == 1)
        || normalized
            .split('/')
            .any(|segment| segment.is_empty() || segment == ".." || segment == ".");
    if unsafe_path {
        return Err(MarketplaceError::UnsafePackagePath(value.to_string()));
    }
    let segments: Vec<&str> = normalized.split('/').collect();
    let protected_root = segments.first().is_some_and(|first| {
        PROTECTED_ROOTS
            .iter()
            .any(|root| first.eq_ignore_ascii_case(root))
    });
    // A package's own top-level `project.mpr` is the model it ships; a `.mpr`
    // anywhere *below* the top level would be overwriting something else.
    let nested_mpr = segments.len() > 1
        && segments
            .last()
            .is_some_and(|last| last.to_ascii_lowercase().ends_with(".mpr"));
    if protected_root || nested_mpr {
        return Err(MarketplaceError::ProtectedPackagePath(value.to_string()));
    }
    Ok(normalized)
}

/// Resolves `relative` under `root`, refusing anything that would land
/// outside — the last check before a write, after [`safe_relative_path`] has
/// already screened the declared path.
pub(crate) fn safe_destination(root: &Path, relative: &str) -> Result<PathBuf> {
    let candidate = root.join(relative);
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(MarketplaceError::UnsafePackagePath(relative.to_string()));
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_that_could_escape_the_project_is_refused() {
        for hostile in [
            "../outside.txt",
            "/etc/passwd",
            "a/../../b",
            "javasource//x.java",
            "./x",
            "C:/Windows/system32",
            "C:\\Windows\\system32",
            "",
        ] {
            assert!(
                matches!(
                    safe_relative_path(hostile),
                    Err(MarketplaceError::UnsafePackagePath(_))
                ),
                "{hostile:?} was accepted"
            );
        }
    }

    #[test]
    fn a_package_cannot_write_into_version_control_tool_state_or_another_mpr() {
        for protected in [
            ".git/config",
            ".GIT/hooks/pre-commit",
            ".mxrs/credentials",
            ".mxrb/state.json",
            "mprcontents/unit.mxunit",
            "nested/Other.mpr",
        ] {
            assert!(
                matches!(
                    safe_relative_path(protected),
                    Err(MarketplaceError::ProtectedPackagePath(_))
                ),
                "{protected:?} was accepted"
            );
        }
        // The package's own top-level project file is legitimate.
        assert_eq!(safe_relative_path("project.mpr").unwrap(), "project.mpr");
        // Ordinary assets pass, and Windows separators are normalized.
        assert_eq!(
            safe_relative_path("javasource\\communitycommons\\Misc.java").unwrap(),
            "javasource/communitycommons/Misc.java"
        );
    }

    #[test]
    fn module_names_must_be_mendix_identifiers() {
        assert_eq!(
            valid_module_name("CommunityCommons").unwrap(),
            "CommunityCommons"
        );
        assert_eq!(valid_module_name("My_Module2").unwrap(), "My_Module2");
        for invalid in ["", "9Lives", "has space", "has-dash", "Has.Dot", "../etc"] {
            assert!(
                valid_module_name(invalid).is_err(),
                "{invalid:?} was accepted"
            );
        }
    }

    #[test]
    fn package_xml_is_read_through_local_names_so_namespaces_do_not_matter() {
        let source = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.mendix.com/package/1.0/">
  <modelerProject xmlns="http://www.mendix.com/modelerProject/1.0/">
    <module name="CommunityCommons" />
    <projectFile path="project.mpr" />
    <files>
      <file path="javasource/communitycommons/Misc.java" />
      <file path="javasource\communitycommons\ORM.java" />
      <file path="javasource/communitycommons/Misc.java" />
    </files>
  </modelerProject>
</package>"#;
        let parsed = parse_package_xml(source, "test.mpk").unwrap();
        assert_eq!(parsed.module_name, "CommunityCommons");
        assert_eq!(parsed.project_file, "project.mpr");
        // Separators normalized, duplicates collapsed.
        assert_eq!(
            parsed.files,
            [
                "javasource/communitycommons/Misc.java",
                "javasource/communitycommons/ORM.java"
            ]
        );
    }

    #[test]
    fn a_package_xml_missing_its_module_or_project_is_rejected() {
        let no_module = r#"<package><projectFile path="project.mpr" /></package>"#;
        assert!(matches!(
            parse_package_xml(no_module, "t.mpk"),
            Err(MarketplaceError::InvalidPackage { .. })
        ));
        let no_project = r#"<package><module name="X" /></package>"#;
        assert!(matches!(
            parse_package_xml(no_project, "t.mpk"),
            Err(MarketplaceError::InvalidPackage { .. })
        ));
        // A hostile file path fails the whole parse rather than being skipped.
        let hostile = r#"<package><module name="X" /><projectFile path="project.mpr" />
            <file path="../../escape.txt" /></package>"#;
        assert!(matches!(
            parse_package_xml(hostile, "t.mpk"),
            Err(MarketplaceError::UnsafePackagePath(_))
        ));
    }

    #[test]
    fn an_absent_manifest_is_normal_and_a_broken_one_is_an_error() {
        assert_eq!(parse_manifest(None).unwrap(), (None, None, None));
        let manifest =
            r#"{"package":{"version":"11.5.1","type":"Module"},"model-version":"11.12.1"}"#;
        assert_eq!(
            parse_manifest(Some(manifest)).unwrap(),
            (
                Some("11.5.1".into()),
                Some("11.12.1".into()),
                Some("Module".into())
            )
        );
        assert!(matches!(
            parse_manifest(Some("{not json")),
            Err(MarketplaceError::InvalidManifest { .. })
        ));
    }
}
