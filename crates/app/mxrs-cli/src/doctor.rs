//! Read-only diagnostics for Cargo-native MXRS application workspaces.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    pub root: PathBuf,
    pub valid: bool,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn errors(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Warning)
            .count()
    }
}

pub fn diagnose(target: impl AsRef<Path>) -> DoctorReport {
    diagnose_with(target, command_version)
}

fn diagnose_with(
    target: impl AsRef<Path>,
    runner: impl Fn(&str) -> Option<String>,
) -> DoctorReport {
    let target = target.as_ref();
    let expanded = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    let root = if expanded.is_file() {
        expanded
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        expanded
    };
    let mut checks = vec![
        file_check("manifest", &root.join("Cargo.toml"), "Cargo.toml"),
        file_check("application", &root.join("src/lib.rs"), "src/lib.rs"),
        file_check(
            "domain",
            &root.join("src/domain/mod.rs"),
            "src/domain/mod.rs",
        ),
        executable_check("cargo", false, &runner),
        executable_check("rustc", false, &runner),
        executable_check("java", true, &runner),
        executable_check("docker", true, &runner),
    ];
    checks.push(mpr_check(&root));
    let valid = checks
        .iter()
        .all(|check| check.status != CheckStatus::Error);
    DoctorReport {
        root,
        valid,
        checks,
    }
}

fn file_check(name: &'static str, path: &Path, label: &str) -> DoctorCheck {
    if path.is_file() {
        DoctorCheck {
            name,
            status: CheckStatus::Ok,
            message: format!("{label} found"),
        }
    } else {
        DoctorCheck {
            name,
            status: CheckStatus::Error,
            message: format!("{label} missing"),
        }
    }
}

fn executable_check(
    name: &'static str,
    optional: bool,
    runner: &impl Fn(&str) -> Option<String>,
) -> DoctorCheck {
    match runner(name) {
        Some(version) => DoctorCheck {
            name,
            status: CheckStatus::Ok,
            message: version,
        },
        None => DoctorCheck {
            name,
            status: if optional {
                CheckStatus::Warning
            } else {
                CheckStatus::Error
            },
            message: "not available".to_string(),
        },
    }
}

fn command_version(name: &str) -> Option<String> {
    let output = Command::new(name).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .or_else(|| Some("available".to_string()))
}

fn mpr_check(root: &Path) -> DoctorCheck {
    let mut candidates = Vec::new();
    for directory in [root.to_path_buf(), root.join("build")] {
        if let Ok(entries) = std::fs::read_dir(directory) {
            candidates.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mpr"))
            }));
        }
    }
    candidates.sort();
    let Some(path) = candidates.first() else {
        return DoctorCheck {
            name: "mpr",
            status: CheckStatus::Warning,
            message: "no generated MPR found".to_string(),
        };
    };
    match crate::validate::validate(path) {
        Ok(report) => DoctorCheck {
            name: "mpr",
            status: if report.is_valid() {
                CheckStatus::Ok
            } else {
                CheckStatus::Error
            },
            message: format!("{} validation error(s)", report.errors.len()),
        },
        Err(error) => DoctorCheck {
            name: "mpr",
            status: CheckStatus::Error,
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root: &Path) {
        for relative in ["Cargo.toml", "src/lib.rs", "src/domain/mod.rs"] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
    }

    #[test]
    fn required_tools_and_project_files_control_validity() {
        let directory = tempfile::tempdir().unwrap();
        project(directory.path());
        let report = diagnose_with(directory.path(), |name| {
            matches!(name, "cargo" | "rustc").then(|| format!("{name} test"))
        });
        assert!(report.valid);
        assert_eq!(report.errors(), 0);
        assert_eq!(report.warnings(), 3);
        assert_eq!(report.checks[3].message, "cargo test");

        std::fs::remove_file(directory.path().join("src/lib.rs")).unwrap();
        let report = diagnose_with(directory.path(), |_| None);
        assert!(!report.valid);
        assert_eq!(report.errors(), 3);
    }

    #[test]
    fn a_file_target_uses_its_parent_and_an_invalid_mpr_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        project(directory.path());
        std::fs::write(directory.path().join("broken.mpr"), "not sqlite").unwrap();
        let report = diagnose_with(directory.path().join("Cargo.toml"), |name| {
            matches!(name, "cargo" | "rustc").then(|| "available".to_string())
        });
        assert_eq!(report.root, directory.path());
        assert!(!report.valid);
        let check = report
            .checks
            .iter()
            .find(|check| check.name == "mpr")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Error);
    }
}
