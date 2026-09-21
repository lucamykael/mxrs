//! Client for the official Mendix Marketplace Content API — the network half
//! of what mxrb's `marketplace`/`module add` commands do
//! (`lib/mxrb/official_marketplace/content_api.rb`).
//!
//! **The transport is a trait.** [`ContentApi`] is generic over [`Transport`],
//! so URL construction, parameter validation, response parsing and error
//! mapping are all exercised offline against responses recorded from the real
//! API (`tests/fixtures/`). Only [`ureq_transport::UreqTransport`] touches the
//! network, and only an explicitly opted-in test exercises it. A test suite
//! that needed credentials and connectivity to run would not be run.
//!
//! **Scope.** This crate finds and downloads a package. Installing one into an
//! `.mpr` — unpacking the `.mpk`, merging units, resolving the dependency
//! graph, writing a lockfile, rolling back — is a separate and much larger
//! problem that mxrb spreads across four files, and is not here.
//!
//! **Credentials never leave [`credentials`].** See that module for why the
//! token is a type with a redacting `Debug` rather than a `String`.

use std::path::Path;

use serde::Deserialize;

pub mod credentials;
pub mod installer;
pub mod lifecycle;
pub mod lock;
pub mod package;
pub mod resolver;
mod transport;
pub mod ureq_transport;
pub mod verify;
pub mod widget_package;

pub use credentials::{Credentials, Pat};
pub use installer::{InstallPlan, InstallReport, plan_install};
pub use package::{ModulePackage, PackageDescriptor};
pub use transport::{Download, Transport};

const BASE_URL: &str = "https://marketplace-api.mendix.com/v1";

/// Hosts an `Authorization` header may be sent to.
///
/// `downloadUrl` arrives inside an API response, so it is attacker-influenced
/// if the API is ever compromised or spoofed. Sending the user's account token
/// to whatever host that field names would turn one bad response into a
/// credential leak, so the header is attached only for these hosts — mirroring
/// mxrb's `AUTHORIZED_HOSTS`.
const AUTHORIZED_HOSTS: &[&str] = &["marketplace-api.mendix.com", "marketplace.mendix.com"];

/// Hosts a package may be *fetched* from, which is a wider set than the hosts
/// a credential may be *sent* to.
///
/// The download endpoint answers 303 towards `files.appstore.mendix.com`,
/// which serves a pre-signed URL carrying its own authorization. Following
/// that hop is necessary; carrying the account token into it is not, and would
/// hand a bearer credential to a host that never needed it.
const DOWNLOAD_HOSTS: &[&str] = &[
    "marketplace-api.mendix.com",
    "marketplace.mendix.com",
    "files.appstore.mendix.com",
];

/// A package download is one endpoint plus its redirect. More hops than this
/// means something is looping or the service changed shape, and following an
/// unbounded chain is how a client gets walked somewhere unintended.
const MAX_DOWNLOAD_HOPS: usize = 4;

/// The API caps these; sending more returns an error rather than more data.
const CONTENT_LIMIT: usize = 100;
const VERSION_LIMIT: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum MarketplaceError {
    #[error(
        "no Mendix credential: set {} or {}",
        credentials::PAT_ENV,
        credentials::PAT_FILE_ENV
    )]
    NoCredential,

    #[error("Mendix PAT must not be empty")]
    EmptyPat,

    #[error("Mendix PAT file is empty: {0}")]
    EmptyPatFile(String),

    #[error(
        "cannot read a token from {path}: expected a bare token, a {env} line, or JSON with \"mendix_pat\"",
        env = credentials::PAT_ENV
    )]
    UnreadablePatFile { path: String },

    #[error("cannot read Mendix PAT file {path}: {source}")]
    PatFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("transport error for {url}: {message}")]
    Transport { url: String, message: String },

    #[error("Marketplace API returned HTTP {status} for {url}")]
    Status { status: u16, url: String },

    #[error("Marketplace API returned a response this client cannot parse ({url}): {source}")]
    Parse {
        url: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("{label} must be a positive integer, got {value:?}")]
    InvalidId { label: &'static str, value: String },

    #[error("version ID must be a UUID, got {0:?}")]
    InvalidVersionId(String),

    #[error("{0:?} is not a Mendix version (expected up to three dot-separated numbers)")]
    InvalidMendixVersion(String),

    #[error("Marketplace content not found: {0:?}")]
    NotFound(String),

    #[error("Marketplace content name {0:?} matches {1} entries; use the numeric content ID")]
    Ambiguous(String, usize),

    #[error(
        "{name} {version} supports Mendix {minimum} and later, which does not include {requested}"
    )]
    Incompatible {
        name: String,
        version: String,
        minimum: String,
        requested: String,
    },

    #[error("{name} has no version matching {0:?}", requested)]
    NoSuchVersion { name: String, requested: String },

    #[error("refusing to send credentials to {0}, which is not a Marketplace host")]
    UntrustedHost(String),

    #[error("refusing to download from {0}, which is not a Mendix file host")]
    UntrustedDownloadHost(String),

    #[error("download of {0} redirected more than {MAX_DOWNLOAD_HOPS} times")]
    TooManyRedirects(String),

    #[error("download of {name} produced an empty file; the server returned no content")]
    EmptyDownload { name: String },

    #[error("cannot write {path}: {source}")]
    Download {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid Mendix widget package {path}: {message}")]
    InvalidWidgetPackage { path: String, message: String },

    #[error("widget install refused: {0}")]
    WidgetInstall(String),

    #[error("package not found: {0}")]
    PackageNotFound(String),

    #[error("target .mpr not found: {0}")]
    TargetNotFound(String),

    #[error("target .mpr has no root unit")]
    TargetHasNoRoot,

    #[error("invalid Mendix module package {path}: {message}")]
    InvalidPackage { path: String, message: String },

    #[error("invalid module manifest: {message}")]
    InvalidManifest { message: String },

    #[error("unsupported Marketplace package type {0:?}; only modules can be installed")]
    UnsupportedPackageType(String),

    #[error("invalid module name in package: {0:?}")]
    InvalidModuleName(String),

    #[error("unsafe package path {0:?}: it would write outside the project")]
    UnsafePackagePath(String),

    #[error("protected package path {0:?}: a package may not write there")]
    ProtectedPackagePath(String),

    #[error("declared package file is missing from {path}: {file}")]
    MissingPackageFile { path: String, file: String },

    #[error("module {0:?} is absent from the package's own project")]
    ModuleAbsentFromPackage(String),

    #[error("module {0} is already installed in the target project")]
    ModuleAlreadyInstalled(String),

    #[error(
        "unit {0} already exists in the target project; installing would replace an unrelated document"
    )]
    UnitIdCollision(String),

    #[error("invalid marketplace lockfile {path}: {message}")]
    InvalidLock { path: String, message: String },

    #[error("Marketplace package {0:?} is not installed")]
    NotInstalled(String),

    #[error("Marketplace package {0:?} is not an imported module")]
    NotAModule(String),

    #[error("cached Marketplace package is missing: {0}")]
    MissingCachedPackage(String),

    #[error("Marketplace lifecycle plan is blocked: {0}")]
    PlanBlocked(String),

    #[error("cached marketplace package checksum mismatch")]
    CacheChecksumMismatch,

    #[error(
        "module manifest declares Mendix {declared} but its model is {actual}; the package is inconsistent"
    )]
    ManifestVersionMismatch { declared: String, actual: String },

    // Fields are `package`/`project` rather than `source`/`target`: thiserror
    // treats a field named `source` as the error cause.
    #[error(
        "module targets Mendix {package} and the project is {project}; this import performs no model migration (pass --allow-model-upgrade to import forward)"
    )]
    ModelVersionMismatch { package: String, project: String },

    #[error("asset destination is a symbolic link: {0}")]
    AssetIsSymlink(String),

    #[error("asset destination is a directory: {0}")]
    AssetIsDirectory(String),

    #[error("cannot access {path}: {source}")]
    PackageIo {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("MPR error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),
}

pub type Result<T> = std::result::Result<T, MarketplaceError>;

/// One published Marketplace component.
///
/// Only the fields this client acts on are modelled; the API returns more, and
/// unknown fields are ignored rather than rejected so a server-side addition
/// does not break the client.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    pub content_id: u64,
    #[serde(default)]
    pub publisher: String,
    #[serde(rename = "type", default)]
    pub content_type: String,
    #[serde(default)]
    pub is_private: bool,
    #[serde(default)]
    pub is_company_approved: bool,
    /// Absent for content with no published version.
    #[serde(default)]
    pub latest_version: Option<Version>,
}

impl Content {
    /// The component's display name. It lives on the version rather than on
    /// the content record, which is why searching by name matches versions.
    pub fn name(&self) -> Option<&str> {
        self.latest_version
            .as_ref()
            .map(|version| version.name.as_str())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    #[serde(default)]
    pub name: String,
    pub version_id: String,
    #[serde(default)]
    pub version_number: String,
    /// Oldest Mendix version this package supports; the API states no upper
    /// bound, so compatibility is a floor check.
    #[serde(default)]
    pub min_supported_mendix_version: Option<String>,
    #[serde(default)]
    pub publication_date: Option<String>,
    #[serde(default)]
    pub version_type: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
    /// Known-issue codes the API records for this exact version — used by
    /// `verify::audit`, not required for install/resolve.
    #[serde(default)]
    pub vulnerabilities: Vec<VersionIssue>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VersionIssue {
    #[serde(default)]
    pub code: Option<String>,
}

/// A resolved content + version pair, ready to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub content: Content,
    pub version: Version,
}

impl Package {
    pub fn name(&self) -> &str {
        &self.version.name
    }

    /// Prefers the URL the API supplied and falls back to the documented
    /// route, so a response that omits it is still usable.
    pub fn download_url(&self) -> String {
        self.version
            .download_url
            .clone()
            .unwrap_or_else(|| format!("{BASE_URL}/versions/{}/download", self.version.version_id))
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub name: Option<String>,
    pub private: Option<bool>,
    pub company_approved: Option<bool>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Deserialize)]
struct Items<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
}

pub struct ContentApi<T: Transport> {
    transport: T,
    pat: Pat,
}

impl<T: Transport> ContentApi<T> {
    pub fn new(transport: T, pat: Pat) -> Self {
        Self { transport, pat }
    }

    /// The underlying transport, so a test double can be inspected after a
    /// call. Exposed rather than made `pub(crate)` because the assertions that
    /// matter most here are about *which URL was requested*, not only about
    /// what came back.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn search(&self, query: &SearchQuery) -> Result<Vec<Content>> {
        let mut parameters: Vec<(&str, String)> = Vec::new();
        if let Some(name) = &query.name {
            parameters.push(("name", name.clone()));
        }
        if let Some(private) = query.private {
            parameters.push(("isPrivate", private.to_string()));
        }
        if let Some(approved) = query.company_approved {
            parameters.push(("isCompanyApproved", approved.to_string()));
        }
        parameters.push((
            "limit",
            clamp(query.limit.unwrap_or(10), 1, CONTENT_LIMIT).to_string(),
        ));
        parameters.push(("offset", query.offset.unwrap_or(0).to_string()));
        let url = url("/content", &parameters);
        Ok(self.get::<Items<Content>>(&url)?.items)
    }

    pub fn content(&self, content_id: &str) -> Result<Content> {
        let identifier = positive_integer(content_id, "content ID")?;
        self.get(&url(&format!("/content/{identifier}"), &[]))
    }

    pub fn versions(&self, content_id: &str, mendix_version: Option<&str>) -> Result<Vec<Version>> {
        let identifier = positive_integer(content_id, "content ID")?;
        let mut parameters: Vec<(&str, String)> = Vec::new();
        if let Some(version) = mendix_version {
            parameters.push(("supportedMendixVersion", mendix_version_filter(version)?));
        }
        parameters.push(("limit", VERSION_LIMIT.to_string()));
        let url = url(&format!("/content/{identifier}/versions"), &parameters);
        Ok(self.get::<Items<Version>>(&url)?.items)
    }

    /// Finds content by numeric ID or by exact name.
    ///
    /// A name matching several components is an error rather than a guess:
    /// picking the first would install something the user did not ask for.
    pub fn find(&self, identifier: &str) -> Result<Content> {
        if identifier
            .chars()
            .all(|character| character.is_ascii_digit())
            && !identifier.is_empty()
        {
            return self.content(identifier);
        }
        let matches = self.search(&SearchQuery {
            name: Some(identifier.to_string()),
            limit: Some(CONTENT_LIMIT),
            ..SearchQuery::default()
        })?;
        match matches.len() {
            0 => Err(MarketplaceError::NotFound(identifier.to_string())),
            1 => Ok(matches.into_iter().next().expect("length checked")),
            count => Err(MarketplaceError::Ambiguous(identifier.to_string(), count)),
        }
    }

    /// Resolves `identifier` to a downloadable package.
    ///
    /// `version` selects an exact version number; without it the newest
    /// compatible one is used. `mendix_version` is checked against the
    /// package's declared floor, so an incompatible pick fails here rather
    /// than inside Studio Pro.
    pub fn resolve(
        &self,
        identifier: &str,
        version: Option<&str>,
        mendix_version: Option<&str>,
    ) -> Result<Package> {
        let content = self.find(identifier)?;
        let candidates = self.versions(&content.content_id.to_string(), mendix_version)?;
        let selected = match version {
            Some(requested) => candidates
                .into_iter()
                .find(|candidate| candidate.version_number == requested)
                .ok_or_else(|| MarketplaceError::NoSuchVersion {
                    name: identifier.to_string(),
                    requested: requested.to_string(),
                })?,
            None => candidates
                .into_iter()
                .next()
                .or_else(|| content.latest_version.clone())
                .ok_or_else(|| MarketplaceError::NotFound(identifier.to_string()))?,
        };
        if let Some(requested) = mendix_version {
            ensure_compatible(&selected, requested)?;
        }
        Ok(Package {
            content,
            version: selected,
        })
    }

    /// Downloads a resolved package. Credentials are attached only for
    /// [`AUTHORIZED_HOSTS`]; a `downloadUrl` pointing elsewhere is refused
    /// outright rather than fetched anonymously, because a redirect to an
    /// unexpected host is more likely an attack than a CDN.
    pub fn download(&self, package: &Package, destination: &Path) -> Result<u64> {
        let mut url = package.download_url();
        for _ in 0..MAX_DOWNLOAD_HOPS {
            let host = host_of(&url).unwrap_or_default();
            if !DOWNLOAD_HOSTS.contains(&host.as_str()) {
                // The first URL comes from the API response and the rest from
                // `Location`; both are server-controlled, so each hop is
                // checked rather than only the first.
                return Err(if AUTHORIZED_HOSTS.contains(&host.as_str()) {
                    MarketplaceError::UntrustedDownloadHost(host)
                } else {
                    MarketplaceError::UntrustedHost(host)
                });
            }
            // The credential travels only to the hosts that authenticate it.
            // The file host uses a pre-signed URL and must not see the token.
            let authorization = AUTHORIZED_HOSTS
                .contains(&host.as_str())
                .then(|| self.pat.authorization());
            match self
                .transport
                .download(&url, authorization.as_deref(), destination)?
            {
                Download::Written(0) => {
                    return Err(MarketplaceError::EmptyDownload {
                        name: package.name().to_string(),
                    });
                }
                Download::Written(bytes) => return Ok(bytes),
                Download::Redirect(location) => url = absolute_location(&url, &location),
            }
        }
        Err(MarketplaceError::TooManyRedirects(package.download_url()))
    }

    fn get<D: serde::de::DeserializeOwned>(&self, url: &str) -> Result<D> {
        let body = self.transport.get(url, &self.pat.authorization())?;
        serde_json::from_str(&body).map_err(|source| MarketplaceError::Parse {
            url: url.to_string(),
            source,
        })
    }
}

fn url(path: &str, parameters: &[(&str, String)]) -> String {
    if parameters.is_empty() {
        return format!("{BASE_URL}{path}");
    }
    let query = parameters
        .iter()
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}{path}?{query}")
}

/// Percent-encodes everything outside the unreserved set. Hand-rolled because
/// this is the only escaping the crate needs and a URL-encoding dependency for
/// one function is not worth the supply chain.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Resolves a `Location` against the URL it came from. Servers are allowed to
/// answer with a relative location, and treating one as absolute would produce
/// a host of `""` and a confusing refusal instead of a working hop.
fn absolute_location(from: &str, location: &str) -> String {
    if location.starts_with("https://") || location.starts_with("http://") {
        return location.to_string();
    }
    let Some(rest) = from.strip_prefix("https://") else {
        return location.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let origin = &from[..("https://".len() + authority_end)];
    if let Some(absolute) = location.strip_prefix('/') {
        format!("{origin}/{absolute}")
    } else {
        format!("{origin}/{location}")
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // Strip any userinfo and port so `evil.com@marketplace.mendix.com` and
    // `marketplace.mendix.com:8443` are both judged on the real host.
    let host = authority.rsplit('@').next()?;
    Some(host.split(':').next()?.to_ascii_lowercase())
}

fn clamp(value: usize, low: usize, high: usize) -> usize {
    value.clamp(low, high)
}

fn positive_integer(value: &str, label: &'static str) -> Result<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .ok_or_else(|| MarketplaceError::InvalidId {
            label,
            value: value.to_string(),
        })
}

/// Up to three dot-separated numbers, mirroring mxrb's `MENDIX_VERSION`. The
/// API rejects anything else, so catching it here turns a 400 into a message
/// naming the offending value.
fn mendix_version_filter(value: &str) -> Result<String> {
    let segments: Vec<&str> = value.split('.').collect();
    let valid = (1..=3).contains(&segments.len())
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment.len() <= 4
                && segment.bytes().all(|byte| byte.is_ascii_digit())
        });
    if valid {
        Ok(value.to_string())
    } else {
        Err(MarketplaceError::InvalidMendixVersion(value.to_string()))
    }
}

/// Compares dotted numeric versions segment by segment. String comparison
/// would rank `10.24.0` below `9.0.0`.
fn ensure_compatible(version: &Version, requested: &str) -> Result<()> {
    let Some(minimum) = &version.min_supported_mendix_version else {
        return Ok(());
    };
    if numeric_version(requested) >= numeric_version(minimum) {
        return Ok(());
    }
    Err(MarketplaceError::Incompatible {
        name: version.name.clone(),
        version: version.version_number.clone(),
        minimum: minimum.clone(),
        requested: requested.to_string(),
    })
}

fn numeric_version(value: &str) -> Vec<u64> {
    let mut parts: Vec<u64> = value
        .split('.')
        .map(|segment| segment.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parameters_are_encoded_and_limits_are_clamped_to_what_the_api_accepts() {
        let built = url(
            "/content",
            &[
                ("name", "Community Commons".into()),
                ("limit", clamp(9999, 1, CONTENT_LIMIT).to_string()),
            ],
        );
        assert_eq!(
            built,
            "https://marketplace-api.mendix.com/v1/content?name=Community%20Commons&limit=100"
        );
        assert_eq!(clamp(0, 1, CONTENT_LIMIT), 1);
        // A name with characters that would otherwise break the query string.
        assert!(url("/content", &[("name", "a&b=c".into())]).ends_with("name=a%26b%3Dc"));
    }

    #[test]
    fn only_real_marketplace_hosts_are_trusted_with_a_credential() {
        assert_eq!(
            host_of("https://marketplace.mendix.com/v1/versions/x/download").as_deref(),
            Some("marketplace.mendix.com")
        );
        // Userinfo must not be mistaken for the host.
        assert_eq!(
            host_of("https://marketplace.mendix.com@evil.example/x").as_deref(),
            Some("evil.example")
        );
        assert_eq!(
            host_of("https://MARKETPLACE.mendix.com:443/x").as_deref(),
            Some("marketplace.mendix.com")
        );
        // Plain HTTP is not a Marketplace URL; sending a token over it is
        // exactly what must not happen.
        assert_eq!(host_of("http://marketplace.mendix.com/x"), None);
        for host in ["evil.example", "marketplace.mendix.com.evil.example"] {
            assert!(!AUTHORIZED_HOSTS.contains(&host));
        }
    }

    #[test]
    fn identifiers_and_versions_are_validated_before_a_request_is_made() {
        assert_eq!(positive_integer("170", "content ID").unwrap(), 170);
        for invalid in ["0", "-1", "", "17a", "1.5"] {
            assert!(
                matches!(
                    positive_integer(invalid, "content ID"),
                    Err(MarketplaceError::InvalidId { .. })
                ),
                "{invalid} was accepted"
            );
        }
        assert!(mendix_version_filter("11.12.1").is_ok());
        assert!(mendix_version_filter("10").is_ok());
        for invalid in ["", "11.12.1.4", "11.x", "11..1", "-1"] {
            assert!(
                mendix_version_filter(invalid).is_err(),
                "{invalid} was accepted"
            );
        }
    }

    #[test]
    fn compatibility_compares_numbers_rather_than_strings() {
        let version = |minimum: &str| Version {
            name: "Community Commons".into(),
            version_id: "id".into(),
            version_number: "11.5.1".into(),
            min_supported_mendix_version: Some(minimum.into()),
            publication_date: None,
            version_type: None,
            download_url: None,
            vulnerabilities: Vec::new(),
        };
        // The case string ordering gets wrong: "10.24.0" < "9.0.0" as text.
        assert!(ensure_compatible(&version("10.24.0"), "11.12.1").is_ok());
        assert!(ensure_compatible(&version("9.0.0"), "10.24.0").is_ok());
        assert!(matches!(
            ensure_compatible(&version("11.0.0"), "10.24.0"),
            Err(MarketplaceError::Incompatible { .. })
        ));
        // Equal floors are compatible.
        assert!(ensure_compatible(&version("11.12.1"), "11.12.1").is_ok());
    }
}
