//! Reading and installing an official standalone widget package (a `.mpk`
//! with a `clientModule` envelope, e.g. the Combo box content). Ports
//! `lib/mxrb/official_marketplace/widget_package_installer.rb`'s
//! `PackageEnvelope`, `WidgetPackageInventory`, `WidgetBundleInventory`, and
//! `WidgetPackageInstaller`.
//!
//! A *module*-kind official package (e.g. Data Widgets, content 116540)
//! bundles its widget `.mpk`s as ordinary declared assets and installs
//! through [`crate::lifecycle::install_module`] like any other module —
//! nothing special needed there. This module exists for the OTHER envelope:
//! a widget published directly as `Widget` content, packaged as a bare
//! `clientModule` with no embedded `.mpr`, installed as one opaque project
//! file under `widgets/`.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::lifecycle::{ModulePackageInventory, io_error, with_rollback};
use crate::lock::{
    CACHE_RELATIVE, LockEntry, ORIGINALS_RELATIVE, read_lock, safe_target_path, write_lock,
};
use crate::{MarketplaceError, Result};

/// Which importer a downloaded/local archive needs — mxrb's
/// `PackageEnvelope.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    Module,
    Widget,
}

/// Reads `package.xml`'s root children to tell a module package
/// (`modelerProject`) from a widget package (`clientModule`).
pub fn envelope_kind(path: &Path) -> Result<PackageKind> {
    let file = std::fs::File::open(path).map_err(|source| MarketplaceError::PackageIo {
        path: path.display().to_string(),
        source,
    })?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| MarketplaceError::InvalidPackage {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
    let mut entry =
        archive
            .by_name("package.xml")
            .map_err(|_| MarketplaceError::InvalidPackage {
                path: path.display().to_string(),
                message: "package.xml is missing".into(),
            })?;
    let mut source = String::new();
    entry
        .read_to_string(&mut source)
        .map_err(|error| MarketplaceError::InvalidPackage {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
    drop(entry);
    let root = parse_xml(&source, path)?;
    for child in &root.children {
        match child.name.as_str() {
            "modelerProject" => return Ok(PackageKind::Module),
            "clientModule" => return Ok(PackageKind::Widget),
            _ => {}
        }
    }
    Err(MarketplaceError::InvalidPackage {
        path: path.display().to_string(),
        message: "package.xml does not declare a module or widget".into(),
    })
}

/// Fail-closed inventory of a `clientModule` widget package. Ports
/// `WidgetPackageInventory`.
#[derive(Debug, Clone)]
pub struct WidgetPackageInventory {
    pub name: String,
    pub version: String,
    pub widget_ids: Vec<String>,
    /// `widgets/<namespace>.<name>.mpk` — the single opaque file this
    /// package installs as.
    pub project_filename: String,
}

impl WidgetPackageInventory {
    pub fn read(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|source| MarketplaceError::PackageIo {
            path: path.display().to_string(),
            source,
        })?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| MarketplaceError::InvalidWidgetPackage {
                path: path.display().to_string(),
                message: error.to_string(),
            })?;
        validate_entries(&mut archive, path)?;
        let package_xml = read_entry(&mut archive, "package.xml", path)?.ok_or_else(|| {
            MarketplaceError::InvalidWidgetPackage {
                path: path.display().to_string(),
                message: "package.xml is missing".into(),
            }
        })?;
        let root = parse_xml(&package_xml, path)?;
        let client = root
            .children
            .iter()
            .find(|child| child.name == "clientModule")
            .ok_or_else(|| MarketplaceError::InvalidWidgetPackage {
                path: path.display().to_string(),
                message: "package.xml must declare exactly one clientModule".into(),
            })?;
        let name = valid_client_name(attribute(client, "name"), path)?;
        let version = valid_client_version(attribute(client, "version"), path)?;

        let mut widget_paths: Vec<String> = Vec::new();
        if let Some(files) = client.children.iter().find(|c| c.name == "widgetFiles") {
            for file in files.children.iter().filter(|c| c.name == "widgetFile") {
                if let Some(raw) = attribute(file, "path") {
                    let safe = safe_relative_widget_path(raw, path, true)?;
                    if !widget_paths.contains(&safe) {
                        widget_paths.push(safe);
                    }
                }
            }
        }
        if widget_paths.is_empty() {
            return Err(MarketplaceError::InvalidWidgetPackage {
                path: path.display().to_string(),
                message: "widget package does not declare widgetFiles".into(),
            });
        }
        validate_declared_files(&mut archive, client, path)?;

        let mut widget_ids = Vec::with_capacity(widget_paths.len());
        for relative in &widget_paths {
            widget_ids.push(widget_id(&mut archive, relative, path)?);
        }
        let project_filename = project_filename(&name, &widget_ids, path)?;
        Ok(Self {
            name,
            version,
            widget_ids,
            project_filename,
        })
    }
}

/// Every widget id delivered by an archive, whether it IS a widget package
/// or is a module package that bundles widget `.mpk`s as declared assets.
/// Ports `WidgetBundleInventory`.
pub struct WidgetBundleInventory {
    pub kind: PackageKind,
    pub widget_ids: Vec<String>,
}

impl WidgetBundleInventory {
    pub fn read(path: &Path) -> Result<Self> {
        let kind = envelope_kind(path)?;
        if kind == PackageKind::Widget {
            let inventory = WidgetPackageInventory::read(path)?;
            return Ok(Self {
                kind,
                widget_ids: inventory.widget_ids,
            });
        }
        // Module kind: read the declared assets, extract every
        // `widgets/*.mpk`, and read each one's own widget ids.
        let module = ModulePackageInventory::read(path)?;
        let staging = tempfile::tempdir().map_err(|source| MarketplaceError::PackageIo {
            path: "temporary directory".into(),
            source,
        })?;
        let file = std::fs::File::open(path).map_err(|source| MarketplaceError::PackageIo {
            path: path.display().to_string(),
            source,
        })?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| MarketplaceError::InvalidWidgetPackage {
                path: path.display().to_string(),
                message: error.to_string(),
            })?;
        let mut ids = BTreeSet::new();
        for relative in module.files.keys() {
            let lower = relative.to_lowercase();
            let is_nested_widget = lower.starts_with("widgets/")
                && lower.ends_with(".mpk")
                && !lower[8..lower.len() - 4].contains('/');
            if !is_nested_widget {
                continue;
            }
            let destination = staging.path().join(
                Path::new(relative)
                    .file_name()
                    .expect("checked non-empty basename"),
            );
            let mut entry =
                archive
                    .by_name(relative)
                    .map_err(|_| MarketplaceError::InvalidWidgetPackage {
                        path: path.display().to_string(),
                        message: format!("declared widget bundle entry is missing: {relative}"),
                    })?;
            let mut out = std::fs::File::create(&destination).map_err(|source| {
                MarketplaceError::PackageIo {
                    path: destination.display().to_string(),
                    source,
                }
            })?;
            std::io::copy(&mut entry, &mut out).map_err(|source| MarketplaceError::PackageIo {
                path: destination.display().to_string(),
                source,
            })?;
            drop(out);
            ids.extend(WidgetPackageInventory::read(&destination)?.widget_ids);
        }
        Ok(Self {
            kind,
            widget_ids: ids.into_iter().collect(),
        })
    }
}

/// Installs a standalone official widget archive as one project-owned
/// opaque `.mpk` under `widgets/`, recording it in the lockfile with
/// `kind: "widget"`. Ports `WidgetPackageInstaller#install`.
pub fn install_widget(
    archive_path: &Path,
    target: &Path,
    official_version: &str,
    provenance: &crate::lifecycle::OfficialProvenance,
) -> Result<LockEntry> {
    let target = std::path::absolute(target).map_err(io_error(target))?;
    if !target.is_dir() {
        return Err(MarketplaceError::WidgetInstall(format!(
            "Marketplace target not found: {}",
            target.display()
        )));
    }
    if target.is_symlink() {
        return Err(MarketplaceError::WidgetInstall(format!(
            "Marketplace target is a symbolic link: {}",
            target.display()
        )));
    }

    let inventory = WidgetPackageInventory::read(archive_path)?;
    if inventory.version != official_version {
        return Err(MarketplaceError::WidgetInstall(format!(
            "widget package version {} does not match Marketplace {official_version}",
            inventory.version
        )));
    }
    let digest = sha256_file(archive_path)?;
    let destination =
        safe_target_path(&target, &format!("widgets/{}", inventory.project_filename))?;
    let cache_stem = Path::new(&inventory.project_filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&inventory.name);
    let version_slug = slug(official_version);
    let cache = safe_target_path(
        &target,
        &format!("{CACHE_RELATIVE}/{cache_stem}-{version_slug}.mpk"),
    )?;
    let lock = read_lock(&target)?;

    ensure_no_symlink_components(&target, &destination)?;
    ensure_no_symlink_components(&target, &cache)?;
    if destination.is_symlink() {
        return Err(MarketplaceError::WidgetInstall(format!(
            "widget destination is a symbolic link: {}",
            destination.display()
        )));
    }
    if destination.is_dir() {
        return Err(MarketplaceError::WidgetInstall(format!(
            "widget destination is a directory: {}",
            destination.display()
        )));
    }

    let current = lock.packages.iter().find(|(name, entry)| {
        name.eq_ignore_ascii_case(&inventory.name)
            || entry.content_id.as_deref() == provenance.content_id.as_deref()
                && provenance.content_id.is_some()
    });
    let destination_relative = relative(&target, &destination);
    if let Some((name, entry)) = current {
        if entry.kind != "widget" {
            return Err(MarketplaceError::WidgetInstall(format!(
                "Marketplace package {name:?} is not a widget"
            )));
        }
        if entry.destination != destination_relative {
            return Err(MarketplaceError::WidgetInstall(format!(
                "Marketplace widget {name:?} changed its destination"
            )));
        }
        if entry.sha256 != sha256_optional_file(&target.join(&entry.destination))? {
            return Err(MarketplaceError::WidgetInstall(format!(
                "installed Marketplace widget {name:?} failed verification"
            )));
        }
        if entry.version.as_deref() == Some(official_version) {
            return Err(MarketplaceError::WidgetInstall(format!(
                "widget version {official_version} is already installed"
            )));
        }
    } else {
        check_asset_owners(&target, &lock, &destination_relative)?;
    }
    let current_archive = current
        .map(|(_, entry)| safe_target_path(&target, &entry.archive))
        .transpose()?;
    if cache.exists() && current_archive.as_deref() != Some(cache.as_path()) {
        return Err(MarketplaceError::WidgetInstall(format!(
            "widget cache destination already exists: {}",
            cache.display()
        )));
    }

    let original = current.and_then(|(_, entry)| entry.asset_original.clone());
    let backup = match &original {
        Some(relative) => safe_target_path(&target, relative)?,
        None => safe_target_path(
            &target,
            &format!(
                "{ORIGINALS_RELATIVE}/{}/{destination_relative}",
                widget_owner_slug(provenance, &inventory.name)
            ),
        )?,
    };
    let mut paths = vec![
        destination.clone(),
        cache.clone(),
        crate::lock::lock_path(&target),
        backup.clone(),
    ];
    if let Some(old) = &current_archive {
        paths.push(old.clone());
    }

    let installed = with_rollback(&paths, || {
        if original.is_none() && destination.is_file() {
            if let Some(parent) = backup.parent() {
                std::fs::create_dir_all(parent).map_err(io_error(parent))?;
            }
            std::fs::copy(&destination, &backup).map_err(io_error(&backup))?;
        }
        atomic_copy(archive_path, &destination)?;
        atomic_copy(archive_path, &cache)?;
        verify_copy(&destination, &digest)?;
        verify_copy(&cache, &digest)?;
        let mut lock = read_lock(&target)?;
        if let Some((name, _)) = current
            && name != &inventory.name
        {
            lock.packages.remove(name);
        }
        let asset_original = if destination.is_file() || original.is_some() {
            Some(relative(&target, &backup))
        } else {
            None
        };
        let entry = LockEntry {
            kind: "widget".into(),
            version: Some(official_version.to_string()),
            source: provenance.source.clone(),
            repository: provenance.repository.clone(),
            sha256: digest.clone(),
            destination: destination_relative.clone(),
            archive: relative(&target, &cache),
            module_id: String::new(),
            units: 0,
            files: vec![destination_relative.clone()],
            asset_originals: Default::default(),
            asset_original,
            content_id: provenance.content_id.clone(),
            version_id: provenance.version_id.clone(),
            widget_name: Some(inventory.name.clone()),
            widget_ids: Some(inventory.widget_ids.clone()),
        };
        lock.packages.insert(inventory.name.clone(), entry.clone());
        write_lock(&target, &lock)?;
        if let Some(old) = &current_archive
            && old != &cache
        {
            let _ = std::fs::remove_file(old);
        }
        Ok(entry)
    })?;
    Ok(installed)
}

fn check_asset_owners(
    target: &Path,
    lock: &crate::lock::Lock,
    destination_relative: &str,
) -> Result<()> {
    let owners: Vec<(&String, &LockEntry)> = lock
        .packages
        .iter()
        .filter(|(_, entry)| entry.files.iter().any(|file| file == destination_relative))
        .collect();
    if owners.is_empty() {
        return Ok(());
    }
    let destination = safe_target_path(target, destination_relative)?;
    if !destination.is_file() {
        return Ok(());
    }
    let actual = sha256_file(&destination)?;
    let mut expected = Vec::new();
    for (_, entry) in &owners {
        if entry.kind == "widget" && entry.destination == destination_relative {
            expected.push(entry.sha256.clone());
        } else if entry.kind == "module" {
            let archive = safe_target_path(target, &entry.archive)?;
            if archive.is_file()
                && let Ok(inventory) = ModulePackageInventory::read(&archive)
                && let Some(digest) = inventory.files.get(destination_relative)
            {
                expected.push(digest.clone());
            }
        }
    }
    if expected.contains(&actual) {
        return Ok(());
    }
    Err(MarketplaceError::WidgetInstall(format!(
        "owned widget asset changed: {destination_relative}"
    )))
}

fn atomic_copy(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    let temporary = destination.with_extension(format!(
        "{}.tmp-{}",
        destination
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mpk"),
        std::process::id()
    ));
    std::fs::copy(source, &temporary).map_err(io_error(&temporary))?;
    std::fs::rename(&temporary, destination).map_err(io_error(destination))
}

fn verify_copy(path: &Path, digest: &str) -> Result<()> {
    if sha256_file(path)? == digest {
        Ok(())
    } else {
        Err(MarketplaceError::CacheChecksumMismatch)
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn sha256_optional_file(path: &Path) -> Result<String> {
    if path.is_file() {
        sha256_file(path)
    } else {
        Ok(String::new())
    }
}

fn relative(target: &Path, path: &Path) -> String {
    path.strip_prefix(target)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn widget_owner_slug(provenance: &crate::lifecycle::OfficialProvenance, name: &str) -> String {
    let raw = provenance
        .content_id
        .clone()
        .map(|id| format!("Widget-{id}"))
        .unwrap_or_else(|| format!("Widget-{}", slug(name)));
    slug(&raw)
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn ensure_no_symlink_components(target: &Path, path: &Path) -> Result<()> {
    let relative = relative(target, path);
    let mut current = target.to_path_buf();
    let parts: Vec<&str> = relative.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        current.push(part);
        if current.is_symlink() {
            return Err(MarketplaceError::WidgetInstall(format!(
                "widget destination traverses a symbolic link: {}",
                current.display()
            )));
        }
    }
    Ok(())
}

// ── package.xml / widget definition XML parsing ─────────────────────────

struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Element>,
}

fn parse_xml(source: &str, package: &Path) -> Result<Element> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(source);
    let mut stack: Vec<Element> = Vec::new();
    let err = |message: String| MarketplaceError::InvalidWidgetPackage {
        path: package.display().to_string(),
        message,
    };
    loop {
        match reader
            .read_event()
            .map_err(|error| err(error.to_string()))?
        {
            Event::Start(start) => stack.push(element_from(&start)?),
            Event::Empty(start) => {
                let element = element_from(&start)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => return Ok(element),
                }
            }
            Event::End(_) => {
                let element = stack
                    .pop()
                    .ok_or_else(|| err("unbalanced closing tag".into()))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => return Ok(element),
                }
            }
            Event::Eof => return Err(err("unexpected end of document".into())),
            _ => {}
        }
    }
}

fn element_from(start: &quick_xml::events::BytesStart<'_>) -> Result<Element> {
    let name = local_name(&String::from_utf8_lossy(start.name().as_ref()));
    let mut attributes = Vec::new();
    for attribute in start.attributes().flatten() {
        let key = local_name(&String::from_utf8_lossy(attribute.key.as_ref()));
        if key == "xmlns" {
            continue;
        }
        let value = String::from_utf8_lossy(&attribute.value).to_string();
        attributes.push((key, value));
    }
    Ok(Element {
        name,
        attributes,
        children: Vec::new(),
    })
}

fn local_name(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

fn attribute<'e>(element: &'e Element, name: &str) -> Option<&'e str> {
    element
        .attributes
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn read_entry(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
    package: &Path,
) -> Result<Option<String>> {
    let Ok(mut entry) = archive.by_name(name) else {
        return Ok(None);
    };
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|source| MarketplaceError::PackageIo {
            path: package.display().to_string(),
            source,
        })?;
    Ok(Some(text))
}

/// Every entry's normalized path is unique (case-insensitively) and no
/// entry is a symlink — mxrb's `validate_entries!`.
fn validate_entries(archive: &mut zip::ZipArchive<std::fs::File>, package: &Path) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let entry =
            archive
                .by_index(index)
                .map_err(|error| MarketplaceError::InvalidWidgetPackage {
                    path: package.display().to_string(),
                    message: error.to_string(),
                })?;
        let normalized = entry.name().replace('\\', "/");
        let normalized = normalized.strip_suffix('/').unwrap_or(&normalized);
        let key = normalized.to_lowercase();
        if !seen.insert(key) {
            return Err(MarketplaceError::InvalidWidgetPackage {
                path: package.display().to_string(),
                message: format!("duplicate widget package path {:?}", entry.name()),
            });
        }
        if entry.is_symlink() {
            return Err(MarketplaceError::InvalidWidgetPackage {
                path: package.display().to_string(),
                message: format!("symbolic link in widget package: {}", entry.name()),
            });
        }
    }
    Ok(())
}

/// Every declared `<files><file path="..."/></files>` entry is present in
/// the archive (as a file, or as a prefix for a directory declaration) —
/// mxrb's `validate_declared_files!`.
fn validate_declared_files(
    archive: &mut zip::ZipArchive<std::fs::File>,
    client: &Element,
    package: &Path,
) -> Result<()> {
    let Some(files) = client.children.iter().find(|c| c.name == "files") else {
        return Ok(());
    };
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    for file in files.children.iter().filter(|c| c.name == "file") {
        let Some(raw) = attribute(file, "path") else {
            continue;
        };
        let relative = safe_relative_widget_path(raw, package, false)?;
        let present = names.iter().any(|name| {
            let normalized = name.replace('\\', "/");
            let normalized = normalized.strip_suffix('/').unwrap_or(&normalized);
            normalized == relative || normalized.starts_with(&format!("{relative}/"))
        });
        if !present {
            return Err(MarketplaceError::InvalidWidgetPackage {
                path: package.display().to_string(),
                message: format!("declared widget file is missing: {relative}"),
            });
        }
    }
    Ok(())
}

fn widget_id(
    archive: &mut zip::ZipArchive<std::fs::File>,
    relative: &str,
    package: &Path,
) -> Result<String> {
    let source = read_entry(archive, relative, package)?.ok_or_else(|| {
        MarketplaceError::InvalidWidgetPackage {
            path: package.display().to_string(),
            message: format!("declared widget definition is missing: {relative}"),
        }
    })?;
    let root = parse_xml(&source, package)?;
    let value = if root.name == "widget" {
        attribute(&root, "id").unwrap_or_default().to_string()
    } else {
        String::new()
    };
    if is_valid_widget_id(&value) {
        Ok(value)
    } else {
        Err(MarketplaceError::InvalidWidgetPackage {
            path: package.display().to_string(),
            message: format!("invalid widget ID {value:?} in {relative}"),
        })
    }
}

/// `\A[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+\z` — at least two
/// dot-separated segments, each a Mendix-style identifier.
fn is_valid_widget_id(value: &str) -> bool {
    let segments: Vec<&str> = value.split('.').collect();
    segments.len() >= 2
        && segments.iter().all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && characters.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Drops the widget id's last TWO dot-segments (its folder plus class name,
/// e.g. `combobox.Combobox`) to recover the shared package namespace, then
/// appends the clientModule's own name — mxrb's `project_filename`.
fn project_filename(name: &str, widget_ids: &[String], package: &Path) -> Result<String> {
    let mut namespaces: Vec<Vec<&str>> = widget_ids
        .iter()
        .map(|id| {
            let segments: Vec<&str> = id.split('.').collect();
            segments[..segments.len().saturating_sub(2)].to_vec()
        })
        .collect();
    namespaces.dedup();
    let [namespace] = namespaces.as_slice() else {
        return Err(MarketplaceError::InvalidWidgetPackage {
            path: package.display().to_string(),
            message: "widget definitions do not share one package namespace".into(),
        });
    };
    let mut parts = namespace.to_vec();
    parts.push(name);
    Ok(format!("{}.mpk", parts.join(".")))
}

fn valid_client_name(value: Option<&str>, package: &Path) -> Result<String> {
    let value = value.unwrap_or_default();
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(value.to_string())
    } else {
        Err(MarketplaceError::InvalidWidgetPackage {
            path: package.display().to_string(),
            message: format!("invalid clientModule name {value:?}"),
        })
    }
}

fn valid_client_version(value: Option<&str>, package: &Path) -> Result<String> {
    let value = value.unwrap_or_default();
    let valid = value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '_' | '-'));
    if valid {
        Ok(value.to_string())
    } else {
        Err(MarketplaceError::InvalidWidgetPackage {
            path: package.display().to_string(),
            message: format!("invalid clientModule version {value:?}"),
        })
    }
}

fn safe_relative_widget_path(value: &str, package: &Path, require_xml: bool) -> Result<String> {
    let path = value.replace('\\', "/");
    let path = path.strip_suffix('/').unwrap_or(&path).to_string();
    let parts: Vec<&str> = path.split('/').collect();
    let unsafe_path = path.is_empty()
        || path.starts_with('/')
        || parts.contains(&"..")
        || parts.contains(&"")
        || path.contains('\0');
    if unsafe_path {
        return Err(MarketplaceError::InvalidWidgetPackage {
            path: package.display().to_string(),
            message: format!("unsafe widget package path {value:?}"),
        });
    }
    if require_xml
        && Path::new(&path)
            .extension()
            .map(|extension| extension.to_ascii_lowercase())
            != Some("xml".into())
    {
        return Err(MarketplaceError::InvalidWidgetPackage {
            path: package.display().to_string(),
            message: format!("widget definition is not XML: {value:?}"),
        });
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_widget_mpk(path: &Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    const COMBOBOX_PACKAGE_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.mendix.com/package/1.0/">
  <clientModule name="Combobox" version="2.9.0" xmlns="http://www.mendix.com/clientModule/1.0/">
    <widgetFiles><widgetFile path="Combobox.xml" /></widgetFiles>
    <files><file path="com/mendix/widget/web/combobox/" /></files>
  </clientModule>
</package>"#;
    const COMBOBOX_WIDGET_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<widget id="com.mendix.widget.web.combobox.Combobox" pluginWidget="true" xmlns="http://www.mendix.com/widget/1.0/">
  <name>Combo box</name>
</widget>"#;

    fn combobox_archive(path: &Path) {
        write_widget_mpk(
            path,
            &[
                ("package.xml", COMBOBOX_PACKAGE_XML),
                ("Combobox.xml", COMBOBOX_WIDGET_XML),
                ("com/mendix/widget/web/combobox/Combobox.mjs", "export {};"),
            ],
        );
    }

    #[test]
    fn envelope_kind_distinguishes_module_and_widget_packages() {
        let directory = tempfile::tempdir().unwrap();
        let widget = directory.path().join("widget.mpk");
        combobox_archive(&widget);
        assert_eq!(envelope_kind(&widget).unwrap(), PackageKind::Widget);

        let module = directory.path().join("module.mpk");
        write_widget_mpk(
            &module,
            &[(
                "package.xml",
                r#"<package><modelerProject><module name="X"/><projectFile path="project.mpr"/></modelerProject></package>"#,
            )],
        );
        assert_eq!(envelope_kind(&module).unwrap(), PackageKind::Module);

        let neither = directory.path().join("neither.mpk");
        write_widget_mpk(&neither, &[("package.xml", "<package><other/></package>")]);
        assert!(envelope_kind(&neither).is_err());
    }

    #[test]
    fn a_widget_package_inventory_reads_the_real_combobox_shape() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widget.mpk");
        combobox_archive(&path);
        let inventory = WidgetPackageInventory::read(&path).unwrap();
        assert_eq!(inventory.name, "Combobox");
        assert_eq!(inventory.version, "2.9.0");
        assert_eq!(
            inventory.widget_ids,
            ["com.mendix.widget.web.combobox.Combobox"]
        );
        assert_eq!(
            inventory.project_filename,
            "com.mendix.widget.web.Combobox.mpk"
        );

        let bundle = WidgetBundleInventory::read(&path).unwrap();
        assert_eq!(bundle.kind, PackageKind::Widget);
        assert_eq!(bundle.widget_ids, inventory.widget_ids);
    }

    #[test]
    fn a_missing_widget_file_or_invalid_id_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let missing_file = directory.path().join("missing.mpk");
        write_widget_mpk(
            &missing_file,
            &[
                (
                    "package.xml",
                    r#"<package><clientModule name="X" version="1.0.0">
                     <widgetFiles><widgetFile path="X.xml"/></widgetFiles>
                     <files><file path="declared/"/></files>
                   </clientModule></package>"#,
                ),
                ("X.xml", r#"<widget id="com.example.web.X"/>"#),
            ],
        );
        let error = WidgetPackageInventory::read(&missing_file).unwrap_err();
        assert!(
            error.to_string().contains("declared widget file"),
            "{error}"
        );

        let bad_id = directory.path().join("badid.mpk");
        write_widget_mpk(
            &bad_id,
            &[
                (
                    "package.xml",
                    r#"<package><clientModule name="X" version="1.0.0">
                     <widgetFiles><widgetFile path="X.xml"/></widgetFiles>
                   </clientModule></package>"#,
                ),
                ("X.xml", r#"<widget id="not-a-widget-id"/>"#),
            ],
        );
        let error = WidgetPackageInventory::read(&bad_id).unwrap_err();
        assert!(error.to_string().contains("invalid widget ID"), "{error}");
    }

    #[test]
    fn install_places_the_widget_records_the_lock_and_is_idempotent_on_reinstall() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path();
        let archive = target.join("source.mpk");
        combobox_archive(&archive);
        let provenance = crate::lifecycle::OfficialProvenance {
            content_id: Some("219304".into()),
            version_id: Some("v1".into()),
            version: Some("2.9.0".into()),
            source: Some("mendix".into()),
            repository: None,
        };

        let entry = install_widget(&archive, target, "2.9.0", &provenance).unwrap();
        assert_eq!(entry.kind, "widget");
        assert_eq!(
            entry.destination,
            "widgets/com.mendix.widget.web.Combobox.mpk"
        );
        assert!(target.join(&entry.destination).is_file());
        assert!(target.join(&entry.archive).is_file());
        let lock = read_lock(target).unwrap();
        assert_eq!(lock.packages["Combobox"].version.as_deref(), Some("2.9.0"));

        // A version mismatch against the resolved official package is
        // refused before anything is touched.
        let error = install_widget(&archive, target, "3.0.0", &provenance).unwrap_err();
        assert!(
            error.to_string().contains("does not match Marketplace"),
            "{error}"
        );

        // The same version is already installed.
        let error = install_widget(&archive, target, "2.9.0", &provenance).unwrap_err();
        assert!(error.to_string().contains("already installed"), "{error}");
    }

    #[test]
    fn install_refuses_to_overwrite_an_asset_owned_by_another_locked_package() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path();
        let owned = target.join("widgets/com.mendix.widget.web.Combobox.mpk");
        std::fs::create_dir_all(owned.parent().unwrap()).unwrap();
        std::fs::write(&owned, b"someone else's file").unwrap();
        let mut lock = crate::lock::Lock::default();
        lock.packages.insert(
            "Other".into(),
            LockEntry {
                kind: "widget".into(),
                destination: "widgets/com.mendix.widget.web.Combobox.mpk".into(),
                sha256: format!("{:x}", Sha256::digest(b"different content")),
                files: vec!["widgets/com.mendix.widget.web.Combobox.mpk".into()],
                ..LockEntry::default()
            },
        );
        write_lock(target, &lock).unwrap();

        let archive = target.join("source.mpk");
        combobox_archive(&archive);
        let provenance = crate::lifecycle::OfficialProvenance {
            content_id: Some("219304".into()),
            version_id: Some("v1".into()),
            version: Some("2.9.0".into()),
            source: Some("mendix".into()),
            repository: None,
        };
        let error = install_widget(&archive, target, "2.9.0", &provenance).unwrap_err();
        assert!(
            error.to_string().contains("owned widget asset changed"),
            "{error}"
        );
        // Nothing was touched.
        assert_eq!(std::fs::read(&owned).unwrap(), b"someone else's file");
    }
}
