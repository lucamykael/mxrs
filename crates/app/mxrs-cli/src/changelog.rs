//! Release-note retrieval for the mxrs project.
//!
//! This command deliberately talks only to the public GitHub Releases API for
//! `lucamykael/mxrs`. It sends no credentials, does not consult a user cache,
//! and keeps endpoint construction and response decoding independently
//! testable so malformed versions or API responses cannot become vague
//! "release not found" messages.

use serde::{Deserialize, Serialize};

const RELEASES_API: &str = "https://api.github.com/repos/lucamykael/mxrs/releases";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Release {
    pub version: String,
    pub title: String,
    pub published_at: Option<String>,
    pub body: Option<String>,
    pub url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangelogError {
    #[error("invalid release version {0:?}; use a tag such as 0.4.0 or v0.4.0")]
    InvalidVersion(String),
    #[error("GitHub Releases returned HTTP {status} for {url}")]
    Status { status: u16, url: String },
    #[error("could not reach GitHub Releases at {url}: {message}")]
    Transport { url: String, message: String },
    #[error("GitHub Releases returned invalid JSON for {url}: {message}")]
    Json { url: String, message: String },
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    published_at: Option<String>,
    body: Option<String>,
    html_url: String,
}

pub fn endpoint(version: Option<&str>) -> Result<String, ChangelogError> {
    endpoint_at(RELEASES_API, version)
}

/// The release endpoint under `api`, a GitHub Releases API root.
fn endpoint_at(api: &str, version: Option<&str>) -> Result<String, ChangelogError> {
    match version {
        None => Ok(format!("{api}/latest")),
        Some(raw) => {
            let version = raw.strip_prefix('v').unwrap_or(raw);
            if version.is_empty()
                || !version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
            {
                return Err(ChangelogError::InvalidVersion(raw.to_string()));
            }
            Ok(format!("{api}/tags/v{version}"))
        }
    }
}

pub fn fetch(version: Option<&str>) -> Result<Release, ChangelogError> {
    fetch_from(RELEASES_API, version)
}

/// [`fetch`] from the Releases API at `api` — a test's double of GitHub's.
pub(crate) fn fetch_from(api: &str, version: Option<&str>) -> Result<Release, ChangelogError> {
    let url = endpoint_at(api, version)?;
    let config = ureq::Agent::config_builder().max_redirects(0).build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("mxrs/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|error| match error {
            ureq::Error::StatusCode(status) => ChangelogError::Status {
                status,
                url: url.clone(),
            },
            other => ChangelogError::Transport {
                url: url.clone(),
                message: other.to_string(),
            },
        })?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| ChangelogError::Transport {
            url: url.clone(),
            message: error.to_string(),
        })?;
    decode(&url, &body)
}

fn decode(url: &str, body: &str) -> Result<Release, ChangelogError> {
    let raw: GithubRelease = serde_json::from_str(body).map_err(|error| ChangelogError::Json {
        url: url.to_string(),
        message: error.to_string(),
    })?;
    let title = raw
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| raw.tag_name.clone());
    let body = raw.body.filter(|body| !body.trim().is_empty());
    Ok(Release {
        version: raw.tag_name,
        title,
        published_at: raw.published_at,
        body,
        url: raw.html_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_fixed_and_versions_are_validated_before_network_access() {
        assert_eq!(
            endpoint(None).unwrap(),
            "https://api.github.com/repos/lucamykael/mxrs/releases/latest"
        );
        assert_eq!(
            endpoint(Some("v1.2.3-rc.1")).unwrap(),
            "https://api.github.com/repos/lucamykael/mxrs/releases/tags/v1.2.3-rc.1"
        );
        for invalid in ["", "../latest", "1/2", "release me", "v"] {
            assert!(matches!(
                endpoint(Some(invalid)),
                Err(ChangelogError::InvalidVersion(_))
            ));
        }
    }

    #[test]
    fn github_json_is_decoded_without_hiding_missing_optional_fields() {
        let release = decode(
            "https://example.invalid/release",
            r#"{
                "tag_name": "v0.4.0",
                "name": "",
                "published_at": null,
                "body": "   ",
                "html_url": "https://github.com/lucamykael/mxrs/releases/tag/v0.4.0"
            }"#,
        )
        .unwrap();
        assert_eq!(release.version, "v0.4.0");
        assert_eq!(release.title, "v0.4.0");
        assert_eq!(release.published_at, None);
        assert_eq!(release.body, None);
        assert!(decode("https://example.invalid/release", "{}").is_err());
    }
}
