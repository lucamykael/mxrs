//! Team Server, as mxrb reaches it: the official App Repository and
//! Projects APIs, and Git over HTTPS.
//!
//! Credentials store only a path to a user-managed PAT file. Every Git
//! operation validates the official Mendix remote first, and a PAT reaches
//! Git only through a short-lived `GIT_ASKPASS` helper — never in a command
//! line, URL, config value, or report.

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
    #[error("{0}: destination already exists")]
    DestinationExists(String),
    #[error("clone depth must be a positive integer")]
    InvalidDepth,
    #[error("clone contains no MPR")]
    NoMpr,
    #[error("cloned MPR is invalid: {0}")]
    InvalidMpr(String),
    #[error("invalid Team Server app ID")]
    InvalidAppId,
    #[error("{0} requires --pat-file or MXRS_TEAM_SERVER_PAT_FILE")]
    MissingCredentials(&'static str),
    #[error("{0}")]
    Api(String),
    #[error("unsafe Projects API pagination URL {0:?}")]
    UnsafePagination(String),
    #[error("Projects API pagination exceeded its safety limit")]
    PaginationLimit,
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

/// Where the PAT is: the file `login` recorded, or one a command names.
#[derive(Debug, Clone)]
pub struct Credentials {
    path: PathBuf,
    pat_file: Option<PathBuf>,
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
        Self {
            path: path.into(),
            pat_file: None,
        }
    }

    /// These credentials, reading the PAT from `pat_file` — a command's
    /// `--pat-file`, else `MXRS_TEAM_SERVER_PAT_FILE` — before the file
    /// `login` recorded.
    pub fn with_pat_file(mut self, pat_file: Option<impl AsRef<Path>>) -> Self {
        self.pat_file = pat_file
            .map(|path| path.as_ref().to_path_buf())
            .or_else(|| std::env::var_os("MXRS_TEAM_SERVER_PAT_FILE").map(PathBuf::from))
            .filter(|path| !path.as_os_str().is_empty());
        self
    }

    /// The PAT, when one is configured.
    pub fn pat(&self) -> Result<Option<String>, TeamServerError> {
        let source = match &self.pat_file {
            Some(path) => Some(absolute(path)?),
            None if self.path.is_file() => {
                let bytes = std::fs::read(&self.path).map_err(|source| io(&self.path, source))?;
                let config: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|error| TeamServerError::InvalidCredentials(error.to_string()))?;
                config
                    .get("team_server_pat_file")
                    .and_then(serde_json::Value::as_str)
                    .filter(|path| !path.trim().is_empty())
                    .map(PathBuf::from)
            }
            None => None,
        };
        source.map(|path| read_pat(&path)).transpose()
    }

    pub fn configure_pat_file(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<LoginReport, TeamServerError> {
        let source = absolute(source.as_ref())?;
        read_pat(&source)?;
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

/// What `status` says of a local Team Server repository.
pub fn status(root: impl AsRef<Path>) -> Result<RepositoryStatus, TeamServerError> {
    let repository = Repository::new(Credentials::default().with_pat_file(None::<PathBuf>));
    repository.status(root)
}

/// What runs Git: `(environment, arguments, directory) -> (succeeded,
/// stdout and stderr together)` — injectable, so the transport is tested
/// without a network.
pub type GitRunner =
    dyn Fn(&[(String, String)], &[String], Option<&Path>) -> std::io::Result<(bool, String)>;

/// The repository a clone made: where, from where, and its valid MPRs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloneReport {
    pub root: PathBuf,
    pub repository_url: String,
    pub branch: Option<String>,
    pub mpr_files: Vec<PathBuf>,
}

/// Git transport for Team Server repositories.
pub struct Repository {
    credentials: Credentials,
    runner: Box<GitRunner>,
}

impl Repository {
    pub fn new(credentials: Credentials) -> Self {
        Self::with_runner(
            credentials,
            Box::new(|environment, arguments, directory| {
                let mut command = Command::new(&arguments[0]);
                command.args(&arguments[1..]);
                command.envs(environment.iter().map(|(key, value)| (key, value)));
                if let Some(directory) = directory {
                    command.current_dir(directory);
                }
                let output = command.output()?;
                let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&output.stderr));
                Ok((output.status.success(), text))
            }),
        )
    }

    pub fn with_runner(credentials: Credentials, runner: Box<GitRunner>) -> Self {
        Self {
            credentials,
            runner,
        }
    }

    /// Clones the repository of an app (`APP_ID` or its Team Server URL)
    /// into `target`, which must not exist, and checks every MPR at its
    /// root. A clone that fails leaves nothing behind.
    pub fn clone(
        &self,
        source: &str,
        target: impl AsRef<Path>,
        branch: Option<&str>,
        depth: Option<&str>,
    ) -> Result<CloneReport, TeamServerError> {
        let url = repository_url(source)?;
        let root = absolute(target.as_ref())?;
        if root.exists() {
            return Err(TeamServerError::DestinationExists(
                root.display().to_string(),
            ));
        }
        let mut arguments = vec!["git".to_string(), "clone".to_string()];
        if let Some(branch) = branch {
            arguments.extend(["--branch".to_string(), branch.to_string()]);
        }
        if let Some(depth) = depth {
            let depth = depth
                .parse::<u64>()
                .ok()
                .filter(|depth| *depth > 0)
                .ok_or(TeamServerError::InvalidDepth)?;
            arguments.extend(["--depth".to_string(), depth.to_string()]);
        }
        arguments.extend(["--".to_string(), url.clone(), root.display().to_string()]);
        let cloned = self.git(&arguments, None).and_then(|_| {
            let mprs = valid_mprs(&root)?;
            if mprs.is_empty() {
                return Err(TeamServerError::NoMpr);
            }
            Ok(mprs)
        });
        match cloned {
            Ok(mpr_files) => Ok(CloneReport {
                root,
                repository_url: url,
                branch: branch.map(str::to_string),
                mpr_files,
            }),
            Err(error) => {
                if root.is_dir() && !root.join(".git").exists() {
                    let _ = std::fs::remove_dir_all(&root);
                }
                Err(error)
            }
        }
    }

    /// `git status --short --branch` of a Team Server repository.
    pub fn status(&self, root: impl AsRef<Path>) -> Result<RepositoryStatus, TeamServerError> {
        let root = self.repository_root(root.as_ref())?;
        let repository_url = self.remote_url(&root, "origin")?;
        let status = self.git(
            &command(&["git", "status", "--short", "--branch"]),
            Some(&root),
        )?;
        Ok(RepositoryStatus {
            root,
            repository_url,
            status,
        })
    }

    /// `git fetch --prune`.
    pub fn fetch(&self, root: impl AsRef<Path>) -> Result<String, TeamServerError> {
        let root = self.repository_root(root.as_ref())?;
        self.git(&command(&["git", "fetch", "--prune"]), Some(&root))
    }

    /// `git pull --ff-only`, then every MPR at the root checked again.
    pub fn pull(&self, root: impl AsRef<Path>) -> Result<String, TeamServerError> {
        let root = self.repository_root(root.as_ref())?;
        let output = self.git(&command(&["git", "pull", "--ff-only"]), Some(&root))?;
        valid_mprs(&root)?;
        Ok(output)
    }

    /// `git push -- REMOTE [BRANCH]` to the Team Server remote.
    pub fn push(
        &self,
        root: impl AsRef<Path>,
        remote: &str,
        branch: Option<&str>,
    ) -> Result<String, TeamServerError> {
        let root = self.repository_root(root.as_ref())?;
        self.remote_url(&root, remote)?;
        // Git pushes to the push URLs, which a configuration may set apart
        // from the one it fetches from: each must be Team Server's.
        let push_urls = self.git(
            &command(&["git", "remote", "get-url", "--push", "--all", remote]),
            Some(&root),
        )?;
        for url in push_urls.lines().filter(|line| !line.trim().is_empty()) {
            repository_url(url.trim())?;
        }
        let mut arguments = command(&["git", "push", "--", remote]);
        arguments.extend(branch.map(str::to_string));
        self.git(&arguments, Some(&root))
    }

    fn repository_root(&self, root: &Path) -> Result<PathBuf, TeamServerError> {
        let root = absolute(root)?;
        if !root.join(".git").is_dir() {
            return Err(TeamServerError::NotRepository(root.display().to_string()));
        }
        self.remote_url(&root, "origin")?;
        Ok(root)
    }

    fn remote_url(&self, root: &Path, remote: &str) -> Result<String, TeamServerError> {
        let output = self.git(&command(&["git", "remote", "get-url", remote]), Some(root))?;
        repository_url(output.trim())
    }

    /// Runs Git with the PAT, when there is one, behind an askpass helper
    /// that lives only as long as the command.
    fn git(
        &self,
        arguments: &[String],
        directory: Option<&Path>,
    ) -> Result<String, TeamServerError> {
        // No credential helper is asked to remember what authenticated: a
        // PAT stays in its file, not in the user's keychain or
        // `.git-credentials`.
        let mut environment = vec![
            ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
            ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
            (
                "GIT_CONFIG_KEY_0".to_string(),
                "credential.helper".to_string(),
            ),
            ("GIT_CONFIG_VALUE_0".to_string(), String::new()),
        ];
        let helper = match self.credentials.pat()? {
            Some(token) => {
                let helper = askpass_helper()?;
                environment.extend([
                    (
                        "GIT_ASKPASS".to_string(),
                        helper.path().join("askpass").display().to_string(),
                    ),
                    ("MXRS_TEAM_SERVER_PAT".to_string(), token),
                ]);
                Some(helper)
            }
            None => None,
        };
        let (succeeded, output) = (self.runner)(&environment, arguments, directory)
            .map_err(|error| TeamServerError::Git(error.to_string()))?;
        drop(helper);
        if succeeded {
            Ok(output)
        } else {
            Err(TeamServerError::Git(output.trim().to_string()))
        }
    }
}

fn command(words: &[&str]) -> Vec<String> {
    words.iter().map(ToString::to_string).collect()
}

/// A private folder holding the helper Git asks for the credentials: the
/// user `pat`, the password the PAT the environment holds — only for a
/// prompt naming Team Server's host, so no other host is answered.
fn askpass_helper() -> Result<tempfile::TempDir, TeamServerError> {
    let directory = tempfile::Builder::new()
        .prefix("mxrs-askpass-")
        .tempdir()
        .map_err(|source| io(Path::new("askpass"), source))?;
    let helper = directory.path().join("askpass");
    std::fs::write(
        &helper,
        "#!/bin/sh\ncase \"$1\" in\n  *Username*git.api.mendix.com*) printf '%s\\n' pat ;;\n  *Password*git.api.mendix.com*) printf '%s\\n' \"$MXRS_TEAM_SERVER_PAT\" ;;\n  *) exit 1 ;;\nesac\n",
    )
    .map_err(|source| io(&helper, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700))
            .map_err(|source| io(&helper, source))?;
    }
    Ok(directory)
}

/// The MPRs at a repository's root, each checked as `mxrs validate` checks
/// it.
fn valid_mprs(root: &Path) -> Result<Vec<PathBuf>, TeamServerError> {
    let mut mprs: Vec<PathBuf> = std::fs::read_dir(root)
        .map_err(|source| io(root, source))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "mpr"))
        .collect();
    mprs.sort();
    for mpr in &mprs {
        let report = crate::validate::validate(mpr)
            .map_err(|error| TeamServerError::InvalidMpr(error.to_string()))?;
        if let Some(error) = report.errors.first() {
            return Err(TeamServerError::InvalidMpr(error.clone()));
        }
    }
    Ok(mprs)
}

/// The official App Repository API.
pub const REPOSITORY_API: &str = "https://repository.api.mendix.com/v1";
/// The official Projects API's first page.
pub const PROJECTS_API: &str = "https://projects-api.home.mendix.com/v2/projects?limit=100";
const PROJECTS_HOST: &str = "projects-api.home.mendix.com";
const MAX_PAGES: usize = 100;

/// A read-only client of Mendix's App Repository and Projects APIs.
pub struct Api {
    credentials: Credentials,
    repository_api: String,
    projects_api: String,
    /// Whether a pagination link may name a host other than Mendix's: only
    /// for a test's double of the API.
    any_host: bool,
}

impl Api {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            credentials,
            repository_api: REPOSITORY_API.to_string(),
            projects_api: PROJECTS_API.to_string(),
            any_host: false,
        }
    }

    #[cfg(test)]
    fn at(credentials: Credentials, base: &str) -> Self {
        Self {
            credentials,
            repository_api: format!("{base}/v1"),
            projects_api: format!("{base}/v2/projects?limit=100"),
            any_host: true,
        }
    }

    pub fn info(&self, app_id: &str) -> Result<serde_json::Value, TeamServerError> {
        self.repository(&format!("/repositories/{}/info", app(app_id)?), &[])
    }

    pub fn branches(&self, app_id: &str) -> Result<serde_json::Value, TeamServerError> {
        self.repository(
            &format!("/repositories/{}/branches", app(app_id)?),
            &[("limit", "100")],
        )
    }

    pub fn commits(
        &self,
        app_id: &str,
        branch: &str,
    ) -> Result<serde_json::Value, TeamServerError> {
        self.repository(
            &format!(
                "/repositories/{}/branches/{}/commits",
                app(app_id)?,
                escape(branch)
            ),
            &[("limit", "100")],
        )
    }

    /// Every company-owned project, page by page.
    pub fn projects(&self) -> Result<Vec<serde_json::Value>, TeamServerError> {
        let authorization = self.authorization("Projects API")?;
        let mut projects = Vec::new();
        let mut url = self.projects_api.clone();
        for _ in 0..MAX_PAGES {
            let page = get_json(&self.safe_page(&url)?, &authorization)?;
            if let Some(items) = page.get("items").and_then(serde_json::Value::as_array) {
                projects.extend(items.iter().cloned());
            }
            match page
                .pointer("/links/next")
                .and_then(serde_json::Value::as_str)
            {
                Some(next) => url = next.to_string(),
                None => return Ok(projects),
            }
        }
        Err(TeamServerError::PaginationLimit)
    }

    fn repository(
        &self,
        path: &str,
        parameters: &[(&str, &str)],
    ) -> Result<serde_json::Value, TeamServerError> {
        let authorization = self.authorization("Team Server API")?;
        let query = parameters
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let url = if query.is_empty() {
            format!("{}{path}", self.repository_api)
        } else {
            format!("{}{path}?{query}", self.repository_api)
        };
        get_json(&url, &authorization)
    }

    fn authorization(&self, what: &'static str) -> Result<String, TeamServerError> {
        match self.credentials.pat()? {
            Some(token) if !token.trim().is_empty() => Ok(format!("MxToken {}", token.trim())),
            _ => Err(TeamServerError::MissingCredentials(what)),
        }
    }

    /// A page of the Projects API, only on its own host and paths: a
    /// pagination link elsewhere would carry the PAT there.
    fn safe_page(&self, url: &str) -> Result<String, TeamServerError> {
        let unsafe_url = || TeamServerError::UnsafePagination(url.to_string());
        let rest = if self.any_host {
            url.split_once("://")
                .map(|(_, rest)| rest)
                .ok_or_else(unsafe_url)?
        } else {
            url.strip_prefix("https://").ok_or_else(unsafe_url)?
        };
        let (host, path) = rest.split_once('/').ok_or_else(unsafe_url)?;
        if !self.any_host && host != PROJECTS_HOST {
            return Err(unsafe_url());
        }
        let path = path.split(['?', '#']).next().unwrap_or_default();
        let segments: Vec<&str> = path.split('/').collect();
        let allowed = matches!(segments.as_slice(), ["v2", "projects"])
            || matches!(segments.as_slice(), ["v2", "accounts", account, "projects"] if !account.is_empty());
        if allowed && !url.contains('#') {
            Ok(url.to_string())
        } else {
            Err(unsafe_url())
        }
    }
}

fn app(value: &str) -> Result<&str, TeamServerError> {
    if valid_app_id(value) {
        Ok(value)
    } else {
        Err(TeamServerError::InvalidAppId)
    }
}

/// A path segment as `URI.encode_www_form_component` writes one, a space
/// as `%20`.
fn escape(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn get_json(url: &str, authorization: &str) -> Result<serde_json::Value, TeamServerError> {
    let config = ureq::Agent::config_builder().max_redirects(0).build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .get(url)
        .header("Accept", "application/json")
        .header("Authorization", authorization)
        .header("User-Agent", concat!("mxrs/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|error| match error {
            ureq::Error::StatusCode(status) => {
                TeamServerError::Api(format!("{url} returned HTTP {status}"))
            }
            other => TeamServerError::Api(format!("cannot reach {url}: {other}")),
        })?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| TeamServerError::Api(format!("cannot read {url}: {error}")))?;
    serde_json::from_str(&body)
        .map_err(|error| TeamServerError::Api(format!("{url} returned invalid JSON: {error}")))
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

fn read_pat(path: &Path) -> Result<String, TeamServerError> {
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
    Ok(token)
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
    fn pat_file(directory: &Path) -> PathBuf {
        let pat = directory.join("pat.env");
        std::fs::write(&pat, "MXRS_TEAM_SERVER_PAT=secret-token\n").unwrap();
        pat
    }

    fn credentials(directory: &Path) -> Credentials {
        Credentials::new(directory.join("credentials")).with_pat_file(Some(pat_file(directory)))
    }

    fn write_mpr(path: &Path) {
        mxrs_writer::write_project(path, &mxrs_dsl::ProjectBuilder::new("11.12.1").build())
            .unwrap();
    }

    /// A clone hands Git the PAT through its askpass helper and the
    /// environment, never an argument, and checks the MPR it brings.
    #[test]
    fn a_clone_authenticates_through_askpass_and_validates_its_mpr() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("app");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = seen.clone();
        let helpers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let helper_paths = helpers.clone();
        let repository = Repository::with_runner(
            credentials(directory.path()),
            Box::new(move |environment, arguments, _| {
                let variables: std::collections::HashMap<_, _> =
                    environment.iter().cloned().collect();
                let helper = &variables["GIT_ASKPASS"];
                helper_paths.lock().unwrap().push(PathBuf::from(helper));
                assert_eq!(variables["GIT_CONFIG_KEY_0"], "credential.helper");
                assert_eq!(variables["GIT_CONFIG_VALUE_0"], "");
                let answer = |prompt: &str| {
                    let output = Command::new("sh")
                        .arg(helper)
                        .arg(prompt)
                        .env("MXRS_TEAM_SERVER_PAT", &variables["MXRS_TEAM_SERVER_PAT"])
                        .output()
                        .unwrap();
                    String::from_utf8(output.stdout).unwrap()
                };
                assert_eq!(
                    answer("Username for 'https://git.api.mendix.com':"),
                    "pat\n"
                );
                assert_eq!(
                    answer("Password for 'https://pat@git.api.mendix.com':"),
                    "secret-token\n"
                );
                // Another host is not answered.
                assert_eq!(answer("Password for 'https://pat@evil.example':"), "");
                recorded.lock().unwrap().push(arguments.to_vec());
                let root = PathBuf::from(arguments.last().unwrap());
                std::fs::create_dir_all(root.join(".git")).unwrap();
                write_mpr(&root.join("App.mpr"));
                Ok((true, String::new()))
            }),
        );
        let report = repository
            .clone(APP_ID, &target, Some("main"), Some("1"))
            .unwrap();
        assert_eq!(report.mpr_files, [target.join("App.mpr")]);
        let arguments = &seen.lock().unwrap()[0];
        assert_eq!(
            arguments[..6],
            ["git", "clone", "--branch", "main", "--depth", "1"]
        );
        assert!(
            arguments
                .iter()
                .all(|argument| !argument.contains("secret-token"))
        );
        // The helper is gone once Git is done.
        let helper = helpers.lock().unwrap()[0].clone();
        assert!(!helper.exists());
        assert!(!helper.parent().unwrap().exists());
        assert!(matches!(
            repository.clone(APP_ID, &target, None, None),
            Err(TeamServerError::DestinationExists(_))
        ));
        assert!(matches!(
            repository.clone(APP_ID, directory.path().join("other"), None, Some("0")),
            Err(TeamServerError::InvalidDepth)
        ));
        assert!(matches!(
            repository.clone(
                "https://example.com/x.git",
                directory.path().join("x"),
                None,
                None
            ),
            Err(TeamServerError::InvalidUrl(_))
        ));
    }

    /// A failed clone that made no repository leaves nothing behind; one
    /// without an MPR is refused.
    #[test]
    fn a_failed_clone_leaves_nothing_behind() {
        let directory = tempfile::tempdir().unwrap();
        let failing = Repository::with_runner(
            credentials(directory.path()),
            Box::new(|_, arguments, _| {
                std::fs::create_dir_all(arguments.last().unwrap()).unwrap();
                Ok((false, "fatal: Authentication failed".to_string()))
            }),
        );
        let target = directory.path().join("app");
        let error = failing.clone(APP_ID, &target, None, None).unwrap_err();
        assert!(
            error.to_string().contains("Authentication failed"),
            "{error}"
        );
        assert!(!target.exists());
        let empty = Repository::with_runner(
            credentials(directory.path()),
            Box::new(|_, arguments, _| {
                std::fs::create_dir_all(PathBuf::from(arguments.last().unwrap()).join(".git"))
                    .unwrap();
                Ok((true, String::new()))
            }),
        );
        assert!(matches!(
            empty.clone(APP_ID, directory.path().join("empty"), None, None),
            Err(TeamServerError::NoMpr)
        ));
    }

    /// The whole Git flow with Git itself: a local repository stands in for
    /// Team Server's through `url.<local>.insteadOf`, so clone, fetch, pull
    /// and push run for real, offline.
    #[test]
    fn clone_fetch_pull_and_push_run_through_git() {
        let directory = tempfile::tempdir().unwrap();
        let git = |arguments: &[&str], directory: &Path| {
            let output = Command::new("git")
                .args(arguments)
                .current_dir(directory)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(output.status.success(), "{arguments:?}: {output:?}");
        };
        let origin = directory.path().join("origin.git");
        std::fs::create_dir(&origin).unwrap();
        git(&["init", "-q", "--bare", "-b", "main"], &origin);
        let seed = directory.path().join("seed");
        std::fs::create_dir(&seed).unwrap();
        git(&["init", "-q", "-b", "main"], &seed);
        write_mpr(&seed.join("App.mpr"));
        git(&["add", "."], &seed);
        git(
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "seed",
            ],
            &seed,
        );
        git(&["push", "-q", origin.to_str().unwrap(), "main"], &seed);
        let config = directory.path().join("gitconfig");
        std::fs::write(
            &config,
            format!(
                "[url \"{}\"]\n\tinsteadOf = https://{HOST}/{APP_ID}.git\n[user]\n\tname = t\n\temail = t@t\n",
                origin.display()
            ),
        )
        .unwrap();
        let config = config.display().to_string();
        let real = Repository::new(credentials(directory.path()));
        let repository = Repository::with_runner(
            credentials(directory.path()),
            Box::new(move |environment, arguments, directory| {
                // Git shows the remote as rewritten, which mxrs rightly
                // refuses; the remote stands for Team Server's.
                if arguments[1..3] == ["remote", "get-url"] {
                    return Ok((true, format!("https://{HOST}/{APP_ID}.git\n")));
                }
                let mut environment = environment.to_vec();
                environment.push(("GIT_CONFIG_GLOBAL".to_string(), config.clone()));
                environment.push(("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()));
                (real.runner)(&environment, arguments, directory)
            }),
        );
        let target = directory.path().join("app");
        let report = repository.clone(APP_ID, &target, None, None).unwrap();
        assert_eq!(
            report.repository_url,
            format!("https://{HOST}/{APP_ID}.git")
        );
        assert_eq!(report.mpr_files, [target.join("App.mpr")]);
        repository.fetch(&target).unwrap();
        repository.pull(&target).unwrap();
        std::fs::write(target.join("notes.txt"), "change").unwrap();
        git(&["add", "."], &target);
        git(
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "change",
            ],
            &target,
        );
        repository.push(&target, "origin", Some("main")).unwrap();
        let status = repository.status(&target).unwrap();
        assert!(status.status.starts_with("## main"), "{}", status.status);
    }

    /// One local stand-in for Mendix's APIs: answers each request in turn,
    /// recording its path and authorization.
    fn api_double(
        answers: impl FnOnce(&str) -> Vec<String>,
    ) -> (String, std::thread::JoinHandle<Vec<(String, String)>>) {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let answers = answers(&base);
        let served = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for body in answers {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let mut authorization = String::new();
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = header
                        .strip_prefix("authorization: ")
                        .or_else(|| header.strip_prefix("Authorization: "))
                    {
                        authorization = value.trim().to_string();
                    }
                }
                let mut stream = stream;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                requests.push((path, authorization));
            }
            requests
        });
        (base, served)
    }

    /// The App Repository API is asked with the PAT as `MxToken`, at the
    /// app's own paths; a branch name is escaped.
    #[test]
    fn the_app_repository_api_is_asked_with_the_pat() {
        let directory = tempfile::tempdir().unwrap();
        let (base, served) = api_double(|_| {
            vec![
                r#"{"name":"App"}"#.to_string(),
                r#"{"items":[]}"#.to_string(),
                r#"{"items":[]}"#.to_string(),
            ]
        });
        let api = Api::at(credentials(directory.path()), &base);
        assert_eq!(api.info(APP_ID).unwrap()["name"], "App");
        api.branches(APP_ID).unwrap();
        api.commits(APP_ID, "release 1/x").unwrap();
        let requests = served.join().unwrap();
        assert_eq!(requests[0].0, format!("/v1/repositories/{APP_ID}/info"));
        assert_eq!(requests[0].1, "MxToken secret-token");
        assert_eq!(
            requests[1].0,
            format!("/v1/repositories/{APP_ID}/branches?limit=100")
        );
        assert_eq!(
            requests[2].0,
            format!("/v1/repositories/{APP_ID}/branches/release%201%2Fx/commits?limit=100")
        );
        assert!(matches!(
            api.info("not-an-app"),
            Err(TeamServerError::InvalidAppId)
        ));
        let anonymous = Api::at(Credentials::new(directory.path().join("none")), &base);
        assert!(matches!(
            anonymous.info(APP_ID),
            Err(TeamServerError::MissingCredentials(_))
        ));
    }

    /// The Projects API is read page by page, and a link off its paths is
    /// refused rather than followed with the PAT.
    #[test]
    fn projects_are_read_page_by_page_within_the_api() {
        let directory = tempfile::tempdir().unwrap();
        let (base, served) = api_double(|base| {
            vec![
                format!(
                    r#"{{"items":[{{"id":1}}],"links":{{"next":"{base}/v2/accounts/a/projects?cursor=2"}}}}"#
                ),
                r#"{"items":[{"id":2}]}"#.to_string(),
            ]
        });
        let api = Api::at(credentials(directory.path()), &base);
        let projects = api.projects().unwrap();
        assert_eq!(projects.len(), 2);
        let requests = served.join().unwrap();
        assert_eq!(requests[1].0, "/v2/accounts/a/projects?cursor=2");
        let strict = Api::new(credentials(directory.path()));
        assert!(matches!(
            strict.safe_page("https://evil.example/v2/projects"),
            Err(TeamServerError::UnsafePagination(_))
        ));
        assert!(
            strict
                .safe_page("https://projects-api.home.mendix.com/v2/accounts/x/projects?cursor=y")
                .is_ok()
        );
    }
    /// A push URL set apart from the fetch URL must be Team Server's too:
    /// one elsewhere is refused before anything is pushed.
    #[test]
    fn a_push_url_elsewhere_is_refused_before_pushing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("app");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = commands.clone();
        let repository = Repository::with_runner(
            credentials(directory.path()),
            Box::new(move |_, arguments, _| {
                recorded.lock().unwrap().push(arguments.join(" "));
                let answer = if arguments.iter().any(|argument| argument == "--push") {
                    "https://evil.example/x.git\n".to_string()
                } else {
                    format!("https://{HOST}/{APP_ID}.git\n")
                };
                Ok((true, answer))
            }),
        );
        assert!(matches!(
            repository.push(&root, "origin", None),
            Err(TeamServerError::InvalidUrl(_))
        ));
        assert!(
            commands
                .lock()
                .unwrap()
                .iter()
                .all(|command| !command.starts_with("git push"))
        );
    }
}
