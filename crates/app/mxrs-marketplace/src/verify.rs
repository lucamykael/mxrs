//! Local integrity verification of every locked package against reality
//! (`verify`, no network) and a live check of locked official components
//! for updates and known vulnerabilities (`audit`, needs the Content API).
//! Ports `OfficialMarketplace.verify`/`verify_module`/`verify_widget` and
//! `SecurityAuditor` (`lib/mxrb/official_marketplace.rb`,
//! `lib/mxrb/official_marketplace/content_api.rb`).

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::lifecycle::io_error;
use crate::lock::{LockEntry, read_lock, safe_target_path};
use crate::transport::Transport;
use crate::widget_package::WidgetPackageInventory;
use crate::{ContentApi, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyResult {
    pub valid: bool,
    pub expected: String,
    pub actual: Option<String>,
    pub detail: VerifyDetail,
}

/// Per-kind detail beyond the plain checksum comparison — mxrb reports
/// different auxiliary fields for a module vs. a widget vs. an
/// unrecognized (tree-installed) entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyDetail {
    Module {
        module_present: bool,
        files_present: bool,
    },
    Widget {
        cached: Option<String>,
        identity: bool,
    },
    Tree,
}

/// Verifies every locked package, name-sorted. Ports `OfficialMarketplace.verify`.
pub fn verify(target: &Path) -> Result<Vec<(String, VerifyResult)>> {
    let lock = read_lock(target)?;
    lock.packages
        .into_iter()
        .map(|(name, entry)| {
            let result = match entry.kind.as_str() {
                "module" => verify_module(target, &name, &entry)?,
                "widget" => verify_widget(target, &entry)?,
                _ => verify_tree(target, &entry)?,
            };
            Ok((name, result))
        })
        .collect()
}

fn verify_module(target: &Path, name: &str, entry: &LockEntry) -> Result<VerifyResult> {
    let archive = safe_target_path(target, &entry.archive)?;
    let actual = optional_sha256(&archive)?;
    let mpr_path = safe_target_path(target, &entry.destination)?;
    let module_present = module_present(&mpr_path, name, &entry.module_id)?;
    let files_present = entry
        .files
        .iter()
        .map(|relative| Ok(safe_target_path(target, relative)?.is_file()))
        .collect::<Result<Vec<bool>>>()?
        .into_iter()
        .all(|present| present);
    Ok(VerifyResult {
        valid: actual.as_deref() == Some(entry.sha256.as_str()) && module_present && files_present,
        expected: entry.sha256.clone(),
        actual,
        detail: VerifyDetail::Module {
            module_present,
            files_present,
        },
    })
}

/// mxrb rescues `MarketplaceError` around the whole body (an unreadable
/// widget destination just means "not verifiably identical"); the checked
/// steps here are the same ones that can raise there.
fn verify_widget(target: &Path, entry: &LockEntry) -> Result<VerifyResult> {
    let destination = safe_target_path(target, &entry.destination)?;
    let archive = safe_target_path(target, &entry.archive)?;
    let actual = optional_sha256(&destination)?;
    let cached = optional_sha256(&archive)?;
    let identity = if actual.as_deref() == Some(entry.sha256.as_str()) {
        WidgetPackageInventory::read(&destination)
            .map(|inventory| {
                inventory.name == entry.widget_name.as_deref().unwrap_or_default()
                    && inventory.widget_ids == entry.widget_ids.clone().unwrap_or_default()
            })
            .unwrap_or(false)
    } else {
        false
    };
    Ok(VerifyResult {
        valid: actual.as_deref() == Some(entry.sha256.as_str())
            && cached.as_deref() == Some(entry.sha256.as_str())
            && identity,
        expected: entry.sha256.clone(),
        actual,
        detail: VerifyDetail::Widget { cached, identity },
    })
}

fn verify_tree(target: &Path, entry: &LockEntry) -> Result<VerifyResult> {
    let destination = safe_target_path(target, &entry.destination)?;
    let actual = if destination.is_dir() {
        Some(tree_digest(&destination)?)
    } else {
        None
    };
    Ok(VerifyResult {
        valid: actual.as_deref() == Some(entry.sha256.as_str()),
        expected: entry.sha256.clone(),
        actual,
        detail: VerifyDetail::Tree,
    })
}

fn module_present(mpr_path: &Path, name: &str, module_id: &str) -> Result<bool> {
    if !mpr_path.is_file() {
        return Ok(false);
    }
    let Ok(mpr) = mxrs_mpr::MprFile::open(mpr_path, true) else {
        return Ok(false);
    };
    let unit = if module_id.is_empty() {
        mpr.units_by_containment("Modules").ok().and_then(|units| {
            units.into_iter().find(|unit| {
                mpr.parse_contents(unit)
                    .ok()
                    .and_then(|doc| doc.get_str("Name").ok().map(str::to_string))
                    .as_deref()
                    == Some(name)
            })
        })
    } else {
        mpr.unit(module_id).ok().flatten()
    };
    Ok(unit.is_some_and(|unit| {
        mpr.parse_contents(&unit)
            .ok()
            .and_then(|doc| doc.get_str("Name").ok().map(str::to_string))
            .as_deref()
            == Some(name)
    }))
}

fn optional_sha256(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    Ok(Some(format!("{:x}", Sha256::digest(bytes))))
}

// ── Audit ──────────────────────────────────────────────────────────────

/// One locked official component checked against the Content API — ports
/// `SecurityAuditor::AuditResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditResult {
    pub name: String,
    pub installed_version: String,
    pub latest_version: String,
    pub version_type: String,
    pub issues: Vec<String>,
    pub outdated: bool,
    /// `false` when the installed version cannot be identified in the
    /// Content API's own version list at all, or is currently flagged
    /// `Vulnerable` — mxrb's `valid`.
    pub valid: bool,
}

/// Checks every locked package with an official content id for updates and
/// known vulnerabilities — ports `SecurityAuditor#audit`. Locked local or
/// GitHub-sourced packages (no `content_id`, or `source != "mendix"`) are
/// skipped, matching mxrb exactly.
pub fn audit<T: Transport>(
    target: &Path,
    mendix_version: Option<&str>,
    api: &ContentApi<T>,
) -> Result<Vec<AuditResult>> {
    let lock = read_lock(target)?;
    lock.packages
        .into_iter()
        .filter(|(_, entry)| {
            entry.source.as_deref() == Some("mendix") && entry.content_id.is_some()
        })
        .map(|(name, entry)| audit_entry(&name, &entry, mendix_version, api))
        .collect()
}

fn audit_entry<T: Transport>(
    name: &str,
    entry: &LockEntry,
    mendix_version: Option<&str>,
    api: &ContentApi<T>,
) -> Result<AuditResult> {
    let content_id = entry.content_id.as_deref().expect("filtered by caller");
    let installed = api
        .versions(content_id, None)?
        .into_iter()
        .find(|version| Some(version.version_id.as_str()) == entry.version_id.as_deref());
    // `allow_vulnerable: true` — an audit that refused to even look at a
    // vulnerable latest release would be worse than useless here; this
    // crate's `resolve` never rejects on vulnerability status at all, so
    // no extra flag is needed to get that behavior.
    let latest = api.resolve(content_id, None, mendix_version)?;
    let (version_type, issues, installed_version) = match &installed {
        Some(version) => (
            version
                .version_type
                .clone()
                .unwrap_or_else(|| "Unknown".into()),
            version
                .vulnerabilities
                .iter()
                .filter_map(|issue| issue.code.clone())
                .collect(),
            version.version_number.clone(),
        ),
        None => (
            "Unknown".to_string(),
            Vec::new(),
            entry.version.clone().unwrap_or_default(),
        ),
    };
    Ok(AuditResult {
        name: name.to_string(),
        outdated: installed_version != latest.version.version_number,
        valid: installed.is_some() && version_type != "Vulnerable",
        installed_version,
        latest_version: latest.version.version_number,
        version_type,
        issues,
    })
}

/// `OfficialMarketplace.tree_digest` — a deterministic digest over every
/// file's relative path and content, sorted for reproducibility.
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
    let entries = std::fs::read_dir(directory).map_err(io_error(directory))?;
    for entry in entries {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{Lock, write_lock};

    #[test]
    fn verify_reports_a_healthy_module_and_a_tampered_one() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path();
        std::fs::create_dir_all(target.join(".mxrs/marketplace")).unwrap();
        let archive = target.join(".mxrs/marketplace/Toolkit-1.0.0.mpk");
        std::fs::write(&archive, b"archive bytes").unwrap();
        let digest = format!("{:x}", Sha256::digest(b"archive bytes"));
        std::fs::write(target.join("App.mpr"), b"not a real mpr").unwrap();
        std::fs::create_dir_all(target.join("javasource/toolkit")).unwrap();
        std::fs::write(target.join("javasource/toolkit/Helper.java"), b"ok").unwrap();

        let mut lock = Lock::default();
        lock.packages.insert(
            "Toolkit".into(),
            LockEntry {
                kind: "module".into(),
                sha256: digest.clone(),
                destination: "App.mpr".into(),
                archive: ".mxrs/marketplace/Toolkit-1.0.0.mpk".into(),
                module_id: String::new(),
                files: vec!["javasource/toolkit/Helper.java".into()],
                ..LockEntry::default()
            },
        );
        write_lock(target, &lock).unwrap();

        let results = verify(target).unwrap();
        assert_eq!(results.len(), 1);
        let (name, result) = &results[0];
        assert_eq!(name, "Toolkit");
        // The archive checksum matches; module presence can't be proven by
        // this fake `App.mpr`, so the module-level `valid` is honestly false.
        assert_eq!(result.actual.as_deref(), Some(digest.as_str()));
        assert!(!result.valid);

        // Tampering with the cached archive is caught too.
        std::fs::write(&archive, b"tampered").unwrap();
        let results = verify(target).unwrap();
        assert_ne!(results[0].1.actual, Some(digest));
        assert!(!results[0].1.valid);

        // A missing file is a clean `None`, not an error.
        let missing = directory.path().join("missing-root");
        let empty = verify(&missing).unwrap();
        assert!(empty.is_empty());
    }

    struct FakeAuditApi;
    impl Transport for FakeAuditApi {
        fn get(&self, url: &str, _authorization: &str) -> Result<String> {
            if url.contains("/content/170/versions") {
                return Ok(r#"{"items":[
                    {"name":"Community Commons","versionId":"latest","versionNumber":"11.0.0","versionType":"Regular","vulnerabilities":[]},
                    {"name":"Community Commons","versionId":"old","versionNumber":"10.0.0","versionType":"Regular","vulnerabilities":[]},
                    {"name":"Community Commons","versionId":"vuln","versionNumber":"9.0.0","versionType":"Vulnerable","vulnerabilities":[{"code":"CVE-1234"}]}
                ]}"#.into());
            }
            if url.contains("/content/170") {
                return Ok(r#"{"contentId":170,"publisher":"Mendix","type":"Module","isPrivate":false,"isCompanyApproved":true,"latestVersion":{"name":"Community Commons","versionId":"latest","versionNumber":"11.0.0"}}"#.into());
            }
            Err(crate::MarketplaceError::Status {
                status: 404,
                url: url.to_string(),
            })
        }
        fn download(
            &self,
            _url: &str,
            _authorization: Option<&str>,
            _destination: &Path,
        ) -> Result<crate::Download> {
            unreachable!("audit never downloads")
        }
    }

    fn locked(name: &str, version_id: &str, version: &str) -> Lock {
        let mut lock = Lock::default();
        lock.packages.insert(
            name.to_string(),
            LockEntry {
                kind: "module".into(),
                source: Some("mendix".into()),
                content_id: Some("170".into()),
                version_id: Some(version_id.to_string()),
                version: Some(version.to_string()),
                ..LockEntry::default()
            },
        );
        lock
    }

    #[test]
    fn audit_reports_outdated_and_flags_a_vulnerable_installed_version() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path();
        write_lock(target, &locked("CommunityCommons", "old", "10.0.0")).unwrap();
        let api = ContentApi::new(FakeAuditApi, crate::Pat::new("test-token").unwrap());

        let results = audit(target, Some("11.12.1"), &api).unwrap();
        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.installed_version, "10.0.0");
        assert_eq!(result.latest_version, "11.0.0");
        assert_eq!(result.version_type, "Regular");
        assert!(result.outdated);
        assert!(result.valid);
        assert!(result.issues.is_empty());

        // A vulnerable installed version is invalid even though it is
        // findable in the API's own version list.
        write_lock(target, &locked("CommunityCommons", "vuln", "9.0.0")).unwrap();
        let results = audit(target, Some("11.12.1"), &api).unwrap();
        assert_eq!(results[0].version_type, "Vulnerable");
        assert!(!results[0].valid);
        assert_eq!(results[0].issues, ["CVE-1234"]);

        // A local (non-Mendix-sourced) package is skipped entirely.
        let mut lock = locked("Local", "x", "1.0.0");
        lock.packages.get_mut("Local").unwrap().source = Some("local".into());
        write_lock(target, &lock).unwrap();
        assert!(audit(target, Some("11.12.1"), &api).unwrap().is_empty());
    }
}
