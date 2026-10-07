//! Self-update for the mxrs CLI — ports `Mxrb::CLI::{Releases,Updater}`
//! (`lib/mxrb/cli/release.rb`) onto this project's release channel: the
//! public GitHub Releases of `lucamykael/mxrs`, the same confined endpoint
//! [`crate::changelog`] already talks to (no credentials, no cache — mxrb
//! itself forces a fresh check for both `update` and `update --check`).
//! mxrb installs via RubyGems (`gem update mxrb`); the Cargo-native
//! equivalent installs the released tag straight from the repository. Like
//! mxrb, a source checkout is refused rather than overwritten, and while
//! the repository has no published releases the check reports the actual
//! HTTP failure instead of pretending to be up to date.

use std::path::PathBuf;

use crate::changelog::{ChangelogError, fetch};

pub const REPOSITORY: &str = "https://github.com/lucamykael/mxrs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseStatus {
    pub installed: String,
    pub latest: String,
}

impl ReleaseStatus {
    pub fn available(&self) -> bool {
        newer(&self.latest, &self.installed)
    }
}

/// Fresh latest-release check (mxrb's `Releases#status(force: true)`).
/// The error carries the real cause — a repository with no releases yet
/// surfaces as GitHub's HTTP 404, never as "already up to date".
pub fn status(installed: &str) -> Result<ReleaseStatus, ChangelogError> {
    release_status(fetch(None)?, installed)
}

fn release_status(
    release: crate::changelog::Release,
    installed: &str,
) -> Result<ReleaseStatus, ChangelogError> {
    Ok(ReleaseStatus {
        installed: installed.to_string(),
        latest: release
            .version
            .strip_prefix('v')
            .unwrap_or(&release.version)
            .to_string(),
    })
}

/// `latest > installed`, comparing the numeric `MAJOR.MINOR.PATCH` core;
/// when the cores are equal a pre-release (`-…`) counts as older than the
/// plain release, mirroring how `Gem::Version` orders mxrb's versions.
/// Unparsable versions are never "newer" (mxrb rescues `ArgumentError` to
/// the same effect).
fn newer(latest: &str, installed: &str) -> bool {
    let (Some(latest), Some(installed)) = (parse(latest), parse(installed)) else {
        return false;
    };
    match latest.0.cmp(&installed.0) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => !latest.1 && installed.1,
    }
}

/// `(numeric core, has a pre-release suffix)`.
fn parse(version: &str) -> Option<(Vec<u64>, bool)> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) if !pre.is_empty() => (core, true),
        Some((core, _)) => (core, false),
        None => (version, false),
    };
    let numbers = core
        .split('.')
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<u64>>>()?;
    (!numbers.is_empty()).then_some((numbers, pre))
}

/// `argv -> did it succeed` — injectable so tests exercise the update
/// command contract without touching a real toolchain.
pub type Runner = dyn Fn(&[String]) -> std::io::Result<bool>;

/// Installs a published release while refusing to overwrite a source
/// checkout — ports `Mxrb::CLI::Updater`.
pub struct Updater {
    runner: Box<Runner>,
    executable: Option<PathBuf>,
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

impl Updater {
    pub fn new() -> Self {
        Self {
            runner: Box::new(|command| {
                std::process::Command::new(&command[0])
                    .args(&command[1..])
                    .status()
                    .map(|status| status.success())
            }),
            executable: std::env::current_exe().ok(),
        }
    }

    pub fn with_runner(runner: Box<Runner>, executable: Option<PathBuf>) -> Self {
        Self { runner, executable }
    }

    /// Runs the Cargo-native equivalent of mxrb's `gem update mxrb` for an
    /// available release. The caller decides whether one is available;
    /// this only refuses checkouts and reports command failures.
    pub fn install(&self, status: &ReleaseStatus) -> Result<(), String> {
        if self.source_checkout() {
            return Err(
                "This mxrs executable comes from a source checkout. Update it with `git pull` \
                 and `cargo build --release`; the CLI will not overwrite local source files."
                    .to_string(),
            );
        }
        let command: Vec<String> = [
            "cargo",
            "install",
            "--locked",
            "--git",
            REPOSITORY,
            "--tag",
            &format!("v{}", status.latest),
            "mxrs",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        match (self.runner)(&command) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("Update command failed: {}", command.join(" "))),
            Err(error) => Err(format!("cannot run {}: {error}", command.join(" "))),
        }
    }

    /// mxrb checks whether the library root is a git checkout; the Cargo
    /// analog is the running executable living inside one (a `target/`
    /// build product under the repository).
    fn source_checkout(&self) -> bool {
        self.executable
            .as_deref()
            .is_some_and(|exe| exe.ancestors().any(|dir| dir.join(".git").is_dir()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn release(installed: &str, latest: &str) -> ReleaseStatus {
        ReleaseStatus {
            installed: installed.to_string(),
            latest: latest.to_string(),
        }
    }

    #[test]
    fn availability_follows_numeric_version_order() {
        assert!(release("0.1.0", "0.2.0").available());
        assert!(release("0.1.0", "1.0.0").available());
        assert!(!release("0.2.0", "0.2.0").available());
        assert!(!release("0.2.0", "0.1.9").available());
        // Pre-releases sit below their release, and garbage is never newer.
        assert!(release("0.2.0-rc.1", "0.2.0").available());
        assert!(!release("0.2.0", "0.2.0-rc.1").available());
        assert!(!release("0.1.0", "not-a-version").available());
    }

    #[test]
    fn install_runs_the_pinned_cargo_command_for_the_released_tag() {
        let calls: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let updater = Updater::with_runner(
            Box::new(move |command| {
                recorded.lock().unwrap().push(command.to_vec());
                Ok(true)
            }),
            Some(PathBuf::from("/usr/local/bin/mxrs")),
        );
        updater.install(&release("0.1.0", "0.2.0")).unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls[0],
            [
                "cargo", "install", "--locked", "--git", REPOSITORY, "--tag", "v0.2.0", "mxrs"
            ]
        );
    }

    #[test]
    fn a_source_checkout_is_refused_before_running_anything() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join(".git")).unwrap();
        let executable = directory.path().join("target/debug/mxrs");
        let calls: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let updater = Updater::with_runner(
            Box::new(move |command| {
                recorded.lock().unwrap().push(command.to_vec());
                Ok(true)
            }),
            Some(executable),
        );
        let error = updater.install(&release("0.1.0", "0.2.0")).unwrap_err();
        assert!(error.contains("source checkout"), "{error}");
        assert!(error.contains("git pull"), "{error}");
        assert!(calls.lock().unwrap().is_empty());
    }

    /// One HTTP answer to one request, from a local stand-in for GitHub's
    /// Releases API: the path asked for, and the answer's status and body.
    fn releases_api(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!(
            "http://{}/repos/lucamykael/mxrs/releases",
            listener.local_addr().unwrap()
        );
        let served = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
            }
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            request
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string()
        });
        (api, served)
    }

    /// The whole update: the latest release read from the Releases API, its
    /// version compared with the installed one, and the released tag
    /// installed.
    #[test]
    fn a_published_release_newer_than_the_installed_one_is_installed() {
        let (api, served) = releases_api(
            200,
            r#"{"tag_name":"v0.3.0","name":"0.3.0","published_at":"2026-10-07T00:00:00Z","body":"Notes","html_url":"https://github.com/lucamykael/mxrs/releases/tag/v0.3.0"}"#,
        );
        let release = crate::changelog::fetch_from(&api, None).unwrap();
        assert_eq!(
            served.join().unwrap(),
            "/repos/lucamykael/mxrs/releases/latest"
        );
        let status = release_status(release, "0.2.0").unwrap();
        assert!(status.available());
        let calls: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        Updater::with_runner(
            Box::new(move |command| {
                recorded.lock().unwrap().push(command.to_vec());
                Ok(true)
            }),
            Some(PathBuf::from("/usr/local/bin/mxrs")),
        )
        .install(&status)
        .unwrap();
        assert_eq!(calls.lock().unwrap()[0][6], "v0.3.0");
        // The installed release is up to date with itself.
        let (api, served) = releases_api(
            200,
            r#"{"tag_name":"v0.2.0","name":null,"published_at":null,"body":null,"html_url":"https://github.com/lucamykael/mxrs/releases/tag/v0.2.0"}"#,
        );
        let release = crate::changelog::fetch_from(&api, None).unwrap();
        served.join().unwrap();
        assert!(!release_status(release, "0.2.0").unwrap().available());
    }

    /// A repository without releases answers 404, which is said as such.
    #[test]
    fn a_missing_release_is_the_http_failure_it_is() {
        let (api, served) = releases_api(404, r#"{"message":"Not Found"}"#);
        let error = crate::changelog::fetch_from(&api, None).unwrap_err();
        served.join().unwrap();
        assert!(
            matches!(error, ChangelogError::Status { status: 404, .. }),
            "{error}"
        );
    }

    #[test]
    fn a_failing_update_command_is_a_loud_error() {
        let updater = Updater::with_runner(
            Box::new(|_| Ok(false)),
            Some(PathBuf::from("/usr/local/bin/mxrs")),
        );
        let error = updater.install(&release("0.1.0", "0.2.0")).unwrap_err();
        assert!(error.contains("Update command failed"), "{error}");
    }
}
