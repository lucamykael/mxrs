//! A small JSON document naming installable modules. Ports
//! `Mxrb::Marketplace::{Entry,Catalog}`.

use serde::Deserialize;

use crate::{ModuleCatalogError, Result};

/// The only network need in this crate — fetching a remote `catalog.json`.
/// A trait so the default (real HTTPS) implementation is swappable for an
/// offline test double, the same seam `mxrs-marketplace` uses for its own
/// transport.
pub trait CatalogTransport {
    fn get(&self, url: &str) -> Result<String>;
}

/// [`ureq`]-backed default transport — HTTPS only, matching mxrb's own
/// `URI::HTTPS` requirement.
pub struct UreqCatalogTransport;

impl CatalogTransport for UreqCatalogTransport {
    fn get(&self, url: &str) -> Result<String> {
        let agent = ureq::Agent::new_with_defaults();
        let mut response = agent.get(url).call().map_err(|error| match error {
            ureq::Error::StatusCode(status) => ModuleCatalogError::CatalogRequestFailed(status),
            other => ModuleCatalogError::CatalogUnreadable(other.to_string()),
        })?;
        response
            .body_mut()
            .read_to_string()
            .map_err(|error| ModuleCatalogError::CatalogUnreadable(error.to_string()))
    }
}

/// One catalog entry — ports `Mxrb::Marketplace::Entry`. `git_ref` is
/// mxrb's `ref` (a reserved word in Rust).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: String,
    pub git_ref: Option<String>,
}

#[derive(Deserialize)]
struct RawCatalog {
    modules: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    source: String,
    #[serde(rename = "ref", default)]
    git_ref: Option<String>,
}

/// A resolved list of installable modules — ports `Mxrb::Marketplace::Catalog`.
pub struct Catalog {
    entries: Vec<Entry>,
}

impl Catalog {
    /// Reads a catalog from a local path or an `https://` URL through
    /// `transport`. mxrb's default catalog (a small file bundled with the
    /// gem) has no mxrs equivalent — a caller with no explicit source must
    /// supply their own catalog.json path or URL.
    pub fn read(source: &str, transport: &dyn CatalogTransport) -> Result<Self> {
        let body = if let Some(url) = source.strip_prefix("https://") {
            transport.get(&format!("https://{url}"))?
        } else if source.starts_with("http://") {
            return Err(ModuleCatalogError::InsecureCatalogSource);
        } else {
            std::fs::read_to_string(source)
                .map_err(|error| ModuleCatalogError::CatalogUnreadable(error.to_string()))?
        };
        let raw: RawCatalog = serde_json::from_str(&body)
            .map_err(|error| ModuleCatalogError::InvalidCatalog(error.to_string()))?;
        let entries = raw
            .modules
            .into_iter()
            .map(|item| Entry {
                name: item.name,
                version: item.version,
                description: item.description,
                source: item.source,
                git_ref: item.git_ref,
            })
            .collect();
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Case-sensitive substring match over name and description, mirroring
    /// `Entry#search`'s downcased `include?`. An empty/absent query
    /// returns every entry.
    pub fn search(&self, query: Option<&str>) -> Vec<&Entry> {
        let term = query.unwrap_or("").to_lowercase();
        if term.is_empty() {
            return self.entries.iter().collect();
        }
        self.entries
            .iter()
            .filter(|entry| {
                entry.name.to_lowercase().contains(&term)
                    || entry.description.to_lowercase().contains(&term)
            })
            .collect()
    }

    pub fn find(&self, name: &str) -> Result<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| ModuleCatalogError::NotFound(name.to_string()))
    }

    pub fn find_version(&self, name: &str, version: &str) -> Result<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.name == name && entry.version == version)
            .ok_or_else(|| ModuleCatalogError::VersionNotFound {
                name: name.to_string(),
                version: version.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(&'static str);
    impl CatalogTransport for Fake {
        fn get(&self, _url: &str) -> Result<String> {
            Ok(self.0.to_string())
        }
    }

    const SAMPLE: &str = r#"{"modules":[
        {"name":"shared-kernel","version":"1.0.0","description":"Cross-module concepts","source":"builtin:shared-kernel"},
        {"name":"audit-log","version":"2.1.0","description":"Reusable audit trail","source":"https://example.invalid/audit-log.git","ref":"v2.1.0"}
    ]}"#;

    #[test]
    fn reads_searches_and_finds_entries_from_a_local_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        std::fs::write(&path, SAMPLE).unwrap();
        let catalog = Catalog::read(path.to_str().unwrap(), &Fake("")).unwrap();

        assert_eq!(catalog.entries().len(), 2);
        assert_eq!(catalog.search(None).len(), 2);
        assert_eq!(catalog.search(Some("audit")).len(), 1);
        assert_eq!(catalog.search(Some("AUDIT")).len(), 1);
        assert!(catalog.search(Some("nothing-matches")).is_empty());

        let entry = catalog.find("audit-log").unwrap();
        assert_eq!(entry.git_ref.as_deref(), Some("v2.1.0"));
        assert!(catalog.find("ghost").is_err());

        assert!(catalog.find_version("audit-log", "2.1.0").is_ok());
        assert!(catalog.find_version("audit-log", "9.9.9").is_err());
    }

    #[test]
    fn https_sources_go_through_the_transport_and_http_is_refused() {
        let catalog = Catalog::read("https://example.invalid/catalog.json", &Fake(SAMPLE)).unwrap();
        assert_eq!(catalog.entries().len(), 2);

        assert!(matches!(
            Catalog::read("http://example.invalid/catalog.json", &Fake("")),
            Err(ModuleCatalogError::InsecureCatalogSource)
        ));
    }

    #[test]
    fn a_malformed_catalog_is_a_named_error_not_a_panic() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(matches!(
            Catalog::read(path.to_str().unwrap(), &Fake("")),
            Err(ModuleCatalogError::InvalidCatalog(_))
        ));
    }
}
