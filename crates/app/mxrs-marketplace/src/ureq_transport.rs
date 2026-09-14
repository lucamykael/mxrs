//! The live HTTPS transport. The only module in the workspace that opens an
//! outbound connection.
//!
//! Redirects are **not** followed here. The Marketplace download endpoint
//! answers 303 towards `files.appstore.mendix.com`, and following that while
//! still holding an `Authorization` header is how a credential reaches a host
//! that was never authorized. The target is returned to the caller, which
//! re-checks it against the allow-lists and decides whether the credential
//! travels with it — see `ContentApi::download`.

use std::io::Write;
use std::path::Path;

use crate::{Download, MarketplaceError, Result, Transport};

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            // See the module doc comment: a followed redirect would carry the
            // token to an unvetted host.
            .max_redirects(0)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn transport_error(url: &str, error: &ureq::Error) -> MarketplaceError {
        MarketplaceError::Transport {
            url: url.to_string(),
            message: error.to_string(),
        }
    }
}

impl Transport for UreqTransport {
    fn get(&self, url: &str, authorization: &str) -> Result<String> {
        let mut response = self
            .agent
            .get(url)
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            .call()
            .map_err(|error| match error {
                ureq::Error::StatusCode(status) => MarketplaceError::Status {
                    status,
                    url: url.to_string(),
                },
                other => Self::transport_error(url, &other),
            })?;
        response
            .body_mut()
            .read_to_string()
            .map_err(|error| Self::transport_error(url, &error))
    }

    fn download(
        &self,
        url: &str,
        authorization: Option<&str>,
        destination: &Path,
    ) -> Result<Download> {
        let mut request = self.agent.get(url);
        if let Some(authorization) = authorization {
            request = request.header("Authorization", authorization);
        }
        let mut response = request.call().map_err(|error| match error {
            ureq::Error::StatusCode(status) => MarketplaceError::Status {
                status,
                url: url.to_string(),
            },
            other => Self::transport_error(url, &other),
        })?;

        // With redirects disabled a 3xx arrives as an ordinary response. It
        // carries no body, so writing it out would produce a zero-byte file
        // that looks like a successful download — the bug this branch exists
        // to prevent.
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
                .ok_or_else(|| MarketplaceError::Transport {
                    url: url.to_string(),
                    message: format!("HTTP {status} without a Location header"),
                })?;
            return Ok(Download::Redirect(location));
        }

        // Write to a sibling temporary file and rename, so an interrupted
        // download cannot leave a truncated `.mpk` that looks complete.
        let temporary = destination.with_extension("partial");
        let mut file =
            std::fs::File::create(&temporary).map_err(|source| MarketplaceError::Download {
                path: temporary.display().to_string(),
                source,
            })?;
        let mut reader = response.body_mut().as_reader();
        let written =
            std::io::copy(&mut reader, &mut file).map_err(|source| MarketplaceError::Download {
                path: temporary.display().to_string(),
                source,
            })?;
        file.flush().map_err(|source| MarketplaceError::Download {
            path: temporary.display().to_string(),
            source,
        })?;
        drop(file);
        std::fs::rename(&temporary, destination).map_err(|source| MarketplaceError::Download {
            path: destination.display().to_string(),
            source,
        })?;
        Ok(Download::Written(written))
    }
}
