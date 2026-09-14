//! Safe, offline Team Server primitives.
//!
//! Credentials store only a path to a user-managed PAT file. Repository
//! status validates the official Mendix remote before invoking Git and never
//! places the token in a command line, URL, config value, or report.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

const HOST: &str = "git.api.mendix.com";

#[derive(Debug, thiserror::Error)]
pub enum TeamServerError {
    #[error("{0}: not a Git repository")]
    NotRepository(String),
    #[error("invalid Team Server Git URL: {0:?}")]
    InvalidUrl(String),
    #[error("Team Server PAT file not found: {0}")]
    MissingPat(String),
    #[error("Team Server PAT file is empty: {0}")]
    EmptyPat(String),
    #[error("Team Server PAT variable not found in {0}")]
    MissingPatVariable(String),
    #[error("Team Server Git operation failed: {0}")]
    Git(String),
    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid credentials file: {0}")]
    InvalidCredentials(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoginReport {
    pub credentials_file: PathBuf,
    pub pat_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryStatus {
    pub root: PathBuf,
    pub repository_url: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    path: PathBuf,
}

impl Default for Credentials {
    fn default() -> Self {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(std::env::temp_dir);
        Self::new(base.join("mxrs/credentials"))
    }
}

impl Credentials {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn configure_pat_file(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<LoginReport, TeamServerError> {
        let source = absolute(source.as_ref())?;
        validate_pat_file(&source)?;
        let mut config = if self.path.is_file() {
            let bytes = std::fs::read(&self.path).map_err(|source| io(&self.path, source))?;
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)
                .map_err(|error| TeamServerError::InvalidCredentials(error.to_string()))?
        } else {
            serde_json::Map::new()
        };
        config.remove("team_server_pat");
        config.insert(
            "team_server_pat_file".to_string(),
            serde_json::Value::String(source.display().to_string()),
        );
        let mut bytes = serde_json::to_vec(&config)
            .map_err(|error| TeamServerError::InvalidCredentials(error.to_string()))?;
        bytes.push(b'\n');
        write_private_atomic(&self.path, &bytes)?;
        Ok(LoginReport {
            credentials_file: self.path.clone(),
            pat_file: source,
        })
    }
}

pub fn repository_url(source: &str) -> Result<String, TeamServerError> {
    let value = source.trim();
    let value = if valid_app_id(value) {
        format!("https://{HOST}/{value}.git")
    } else {
        value.to_string()
    };
    let prefix = format!("https://{HOST}/");
    let Some(identifier) = value
        .strip_prefix(&prefix)
        .and_then(|path| path.strip_suffix(".git"))
    else {
        return Err(TeamServerError::InvalidUrl(source.to_string()));
    };
    if !valid_app_id(identifier) || value.contains('@') || identifier.contains(['/', '?', '#']) {
        return Err(TeamServerError::InvalidUrl(source.to_string()));
    }
    Ok(value)
}

pub fn status(root: impl AsRef<Path>) -> Result<RepositoryStatus, TeamServerError> {
    let root = absolute(root.as_ref())?;
    if !root.join(".git").is_dir() {
        return Err(TeamServerError::NotRepository(root.display().to_string()));
    }
    let remote = git(&root, &["remote", "get-url", "origin"])?;
    let repository_url = repository_url(remote.trim())?;
    let status = git(&root, &["status", "--short", "--branch"])?;
    Ok(RepositoryStatus {
        root,
        repository_url,
        status,
    })
}

fn git(root: &Path, arguments: &[&str]) -> Result<String, TeamServerError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| TeamServerError::Git(error.to_string()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(TeamServerError::Git(message));
    }
    String::from_utf8(output.stdout).map_err(|error| TeamServerError::Git(error.to_string()))
}

fn valid_app_id(value: &str) -> bool {
    [8, 4, 4, 4, 12]
        .into_iter()
        .zip(value.split('-'))
        .all(|(length, part)| {
            part.len() == length && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        && value.matches('-').count() == 4
}

fn validate_pat_file(path: &Path) -> Result<(), TeamServerError> {
    if !path.is_file() {
        return Err(TeamServerError::MissingPat(path.display().to_string()));
    }
    let raw = std::fs::read_to_string(path).map_err(|source| io(path, source))?;
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(TeamServerError::EmptyPat(path.display().to_string()));
    }
    let is_env = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".env"))
        || raw.lines().any(|line| pat_assignment(line).is_some());
    let token = if is_env {
        raw.lines()
            .rev()
            .find_map(pat_assignment)
            .ok_or_else(|| TeamServerError::MissingPatVariable(path.display().to_string()))?
            .trim_matches(|character| character == '\'' || character == '"')
            .trim()
            .to_string()
    } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        value
            .as_object()
            .and_then(|object| {
                object
                    .get("team_server_pat")
                    .or_else(|| object.get("mendix_pat"))
            })
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        raw.to_string()
    };
    if token.is_empty() {
        return Err(TeamServerError::EmptyPat(path.display().to_string()));
    }
    Ok(())
}

fn pat_assignment(line: &str) -> Option<&str> {
    let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
    [
        "MXRS_TEAM_SERVER_PAT=",
        "MXRS_MENDIX_PAT=",
        "MXRB_TEAM_SERVER_PAT=",
        "MXRB_MENDIX_PAT=",
    ]
    .into_iter()
    .find_map(|prefix| line.strip_prefix(prefix))
}

fn absolute(path: &Path) -> Result<PathBuf, TeamServerError> {
    std::path::absolute(path).map_err(|source| io(path, source))
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), TeamServerError> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| io(parent, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|source| io(parent, source))?;
    }
    let mut staging = tempfile::Builder::new()
        .prefix(".mxrs-credentials-")
        .tempfile_in(parent)
        .map_err(|source| io(parent, source))?;
    let staging_path = staging.path().to_path_buf();
    {
        let file = staging.as_file_mut();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|source| io(&staging_path, source))?;
        }
        file.write_all(bytes)
            .map_err(|source| io(&staging_path, source))?;
        file.sync_all()
            .map_err(|source| io(&staging_path, source))?;
    }
    staging
        .persist(path)
        .map_err(|error| io(path, error.error))?;
    Ok(())
}

fn io(path: &Path, source: std::io::Error) -> TeamServerError {
    TeamServerError::Io {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_ID: &str = "12345678-1234-abcd-9876-1234567890ab";

    #[test]
    fn credentials_store_only_a_private_pointer() {
        let directory = tempfile::tempdir().unwrap();
        let default = Credentials::default();
        assert!(default.path.ends_with("mxrs/credentials"));
        let pat = directory.path().join(".env.team-server");
        std::fs::write(&pat, "MXRS_TEAM_SERVER_PAT='do-not-copy-me'\n").unwrap();
        let path = directory.path().join("config/mxrs/credentials");
        let report = Credentials::new(&path).configure_pat_file(&pat).unwrap();
        assert_eq!(report.credentials_file, path);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("team_server_pat_file"));
        assert!(!contents.contains("do-not-copy-me"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn repository_urls_are_restricted_to_the_official_host_and_app_ids() {
        assert_eq!(
            repository_url(APP_ID).unwrap(),
            format!("https://{HOST}/{APP_ID}.git")
        );
        assert!(repository_url("https://example.com/project.git").is_err());
        assert!(repository_url("https://pat@git.api.mendix.com/123.git").is_err());
        assert!(repository_url("../project").is_err());
    }

    #[test]
    fn malformed_or_empty_credential_inputs_fail_without_writing_config() {
        let directory = tempfile::tempdir().unwrap();
        let credentials_path = directory.path().join("config/credentials");
        let credentials = Credentials::new(&credentials_path);
        assert!(
            io(Path::new("credentials"), std::io::Error::other("denied"))
                .to_string()
                .contains("credentials")
        );
        assert!(matches!(
            credentials.configure_pat_file(directory.path().join("missing")),
            Err(TeamServerError::MissingPat(_))
        ));
        for (name, contents) in [
            ("empty", ""),
            (".env", "UNRELATED=value\n"),
            ("empty.json", r#"{"team_server_pat":""}"#),
        ] {
            let path = directory.path().join(name);
            std::fs::write(&path, contents).unwrap();
            assert!(credentials.configure_pat_file(path).is_err());
        }
        assert!(!credentials_path.exists());

        let pat = directory.path().join("pat");
        std::fs::write(&pat, "valid").unwrap();
        std::fs::create_dir_all(credentials_path.parent().unwrap()).unwrap();
        std::fs::write(&credentials_path, "not json").unwrap();
        assert!(matches!(
            credentials.configure_pat_file(pat),
            Err(TeamServerError::InvalidCredentials(_))
        ));
    }

    #[test]
    fn status_reports_a_validated_local_repository() {
        let directory = tempfile::tempdir().unwrap();
        let result = Command::new("git")
            .args(["init", "-q"])
            .current_dir(directory.path())
            .status()
            .unwrap();
        assert!(result.success());
        let url = format!("https://{HOST}/{APP_ID}.git");
        let result = Command::new("git")
            .args(["remote", "add", "origin", &url])
            .current_dir(directory.path())
            .status()
            .unwrap();
        assert!(result.success());
        std::fs::write(directory.path().join("untracked.txt"), "test").unwrap();
        let report = status(directory.path()).unwrap();
        assert_eq!(
            report.repository_url,
            format!("https://{HOST}/{APP_ID}.git")
        );
        assert!(report.status.contains("untracked.txt"));
        let invalid = tempfile::tempdir().unwrap();
        assert!(matches!(
            status(invalid.path()),
            Err(TeamServerError::NotRepository(_))
        ));
    }
}
