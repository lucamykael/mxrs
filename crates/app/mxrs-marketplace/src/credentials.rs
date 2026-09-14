//! Personal Access Token resolution.
//!
//! A PAT is a bearer credential for the user's whole Mendix account, so this
//! module's job is as much about *not* spreading it as about finding it:
//!
//! - [`Pat`] has a hand-written [`std::fmt::Debug`] that prints a placeholder.
//!   Deriving `Debug` anywhere up the call chain would otherwise leak the token
//!   into a panic message, a test failure, or a `dbg!` left in by accident.
//! - Nothing here writes a token anywhere. mxrb offers to persist one into its
//!   own credentials file; this deliberately does not, because the one thing
//!   that made the earlier mxrb work safe was that the secret never landed on
//!   disk in a new place.
//! - The token is never part of an error message. A failure says which source
//!   was consulted, not what it contained.

use std::path::{Path, PathBuf};

use crate::{MarketplaceError, Result};

/// Environment variable holding the token directly.
pub const PAT_ENV: &str = "MXRS_MENDIX_PAT";
/// Environment variable holding a path to a file containing the token.
pub const PAT_FILE_ENV: &str = "MXRS_MENDIX_PAT_FILE";

/// A Mendix Personal Access Token.
///
/// Construct it through [`Credentials::resolve`] rather than directly wherever
/// possible, so the resolution order stays in one place.
#[derive(Clone, PartialEq, Eq)]
pub struct Pat(String);

impl Pat {
    /// Rejects an empty or whitespace-only token up front: an empty
    /// `Authorization` header produces a confusing 401 rather than an obvious
    /// "you have no credential" error.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into().trim().to_string();
        if value.is_empty() {
            return Err(MarketplaceError::EmptyPat);
        }
        Ok(Self(value))
    }

    /// The `Authorization` header value. `MxToken`, not `Bearer` — the
    /// Marketplace Content API uses its own scheme.
    pub(crate) fn authorization(&self) -> String {
        format!("MxToken {}", self.0)
    }
}

/// Prints a placeholder. See this module's doc comment for why this is not
/// derived.
impl std::fmt::Debug for Pat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Pat(<redacted>)")
    }
}

/// Where to look for a token, in order.
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    explicit: Option<String>,
    file: Option<PathBuf>,
}

impl Credentials {
    /// A token supplied directly by the caller, taking precedence over the
    /// environment.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.explicit = Some(token.into());
        self
    }

    pub fn with_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }

    /// Resolution order: explicit token, then [`PAT_ENV`], then an explicitly
    /// configured file, then [`PAT_FILE_ENV`].
    ///
    /// The environment beats a configured file so a one-off run can override
    /// a persisted setting without editing it, and both beat nothing at all —
    /// there is no implicit default path, because silently reading a file the
    /// user did not name is how a credential gets used by surprise.
    pub fn resolve(&self) -> Result<Pat> {
        if let Some(token) = &self.explicit {
            return Pat::new(token.clone());
        }
        if let Ok(token) = std::env::var(PAT_ENV)
            && !token.trim().is_empty()
        {
            return Pat::new(token);
        }
        if let Some(path) = &self.file {
            return read_pat_file(path);
        }
        if let Ok(path) = std::env::var(PAT_FILE_ENV)
            && !path.trim().is_empty()
        {
            return read_pat_file(Path::new(path.trim()));
        }
        Err(MarketplaceError::NoCredential)
    }
}

/// Reads a token from a file, accepting the three shapes a user is likely to
/// already have: a bare token, a `KEY=value` line, or a JSON object with a
/// `mendix_pat` key (mxrb's own credentials-file format).
fn read_pat_file(path: &Path) -> Result<Pat> {
    let raw = std::fs::read_to_string(path).map_err(|source| MarketplaceError::PatFile {
        path: path.display().to_string(),
        source,
    })?;
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(MarketplaceError::EmptyPatFile(path.display().to_string()));
    }
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw)
        && let Some(serde_json::Value::String(token)) = map.get("mendix_pat")
    {
        return Pat::new(token.clone());
    }
    // A `KEY=value` line, with or without `export` and quotes. The last such
    // line wins, matching how a shell would evaluate the file.
    if let Some(value) = raw
        .lines()
        .rev()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            let (key, value) = line.split_once('=')?;
            (key.trim() == PAT_ENV).then(|| value.trim())
        })
        .next()
    {
        let unquoted = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        return Pat::new(unquoted);
    }
    // A bare token: reject anything with whitespace rather than sending a
    // truncated header and getting an opaque 401.
    if raw.lines().count() > 1 || raw.split_whitespace().count() > 1 {
        return Err(MarketplaceError::UnreadablePatFile {
            path: path.display().to_string(),
        });
    }
    Pat::new(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(directory: &Path, name: &str, contents: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn a_token_never_appears_in_debug_output() {
        let pat = Pat::new("secret-value-do-not-print").unwrap();
        let rendered = format!("{pat:?}");
        assert_eq!(rendered, "Pat(<redacted>)");
        assert!(!rendered.contains("secret"));
        // The same must hold when nested inside another derived Debug.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Holder {
            pat: Pat,
        }
        assert!(!format!("{:?}", Holder { pat }).contains("secret"));
    }

    #[test]
    fn the_header_uses_the_marketplace_scheme() {
        assert_eq!(
            Pat::new(" token ").unwrap().authorization(),
            "MxToken token"
        );
        assert!(matches!(Pat::new("   "), Err(MarketplaceError::EmptyPat)));
    }

    #[test]
    fn a_pat_file_may_be_bare_an_env_line_or_mxrbs_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();

        assert_eq!(
            read_pat_file(&write(path, "bare", "  abc123\n"))
                .unwrap()
                .authorization(),
            "MxToken abc123"
        );
        assert_eq!(
            read_pat_file(&write(
                path,
                "env",
                "# comment\nexport MXRS_MENDIX_PAT=\"from-env-line\"\n"
            ))
            .unwrap()
            .authorization(),
            "MxToken from-env-line"
        );
        assert_eq!(
            read_pat_file(&write(path, "json", r#"{"mendix_pat": "from-json"}"#))
                .unwrap()
                .authorization(),
            "MxToken from-json"
        );
        // A later assignment wins, as it would in a shell.
        assert_eq!(
            read_pat_file(&write(
                path,
                "twice",
                "MXRS_MENDIX_PAT=first\nMXRS_MENDIX_PAT=second\n"
            ))
            .unwrap()
            .authorization(),
            "MxToken second"
        );
    }

    #[test]
    fn an_unusable_pat_file_is_an_error_rather_than_a_truncated_header() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        assert!(matches!(
            read_pat_file(&write(path, "empty", "   \n")),
            Err(MarketplaceError::EmptyPatFile(_))
        ));
        // Prose, or an SSH key, or anything else multi-word: refusing beats
        // sending the first word as a credential.
        assert!(matches!(
            read_pat_file(&write(path, "prose", "this is not a token")),
            Err(MarketplaceError::UnreadablePatFile { .. })
        ));
        assert!(matches!(
            read_pat_file(Path::new("/nonexistent/pat")),
            Err(MarketplaceError::PatFile { .. })
        ));
    }

    #[test]
    fn an_explicit_token_beats_a_configured_file_and_absence_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let file = write(directory.path(), "pat", "from-file");
        assert_eq!(
            Credentials::default()
                .with_token("explicit")
                .with_file(&file)
                .resolve()
                .unwrap()
                .authorization(),
            "MxToken explicit"
        );
        assert_eq!(
            Credentials::default()
                .with_file(&file)
                .resolve()
                .unwrap()
                .authorization(),
            "MxToken from-file"
        );
        // No source configured and no environment set is a named error, not a
        // silent anonymous request.
        if std::env::var(PAT_ENV).is_err() && std::env::var(PAT_FILE_ENV).is_err() {
            assert!(matches!(
                Credentials::default().resolve(),
                Err(MarketplaceError::NoCredential)
            ));
        }
    }
}
