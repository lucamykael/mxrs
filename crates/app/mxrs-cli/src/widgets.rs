//! Widget development commands — ports `Mxrb::WidgetDevelopment`
//! (`lib/mxrb/widget_development.rb`): a thin, argv-safe integration with
//! Mendix's official TypeScript widget generator and build tool. Like
//! mxrb, this consumes the resulting MPK through the widget-package
//! schema synchronizer (`mxrs-widget-package` + the writer's
//! `packages_root`) instead of inventing a second package format. The
//! `new`/`build` actions genuinely require Node's `npx`/`npm` — a missing
//! toolchain is an honest, named error, never a silent no-op.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Mendix's official widget generator, pinned like mxrb pins it.
pub const GENERATOR: &str = "@mendix/generator-widget@11.11.0";

/// `(environment, command argv, working directory) -> did it succeed`.
/// Injectable so tests exercise the full argument/environment contract
/// without a Node toolchain.
pub type Runner = dyn Fn(&HashMap<String, String>, &[String], &Path) -> std::io::Result<bool>;

pub struct WidgetDevelopment {
    runner: Box<Runner>,
}

impl Default for WidgetDevelopment {
    fn default() -> Self {
        Self::new()
    }
}

impl WidgetDevelopment {
    pub fn new() -> Self {
        Self::with_runner(Box::new(|environment, command, directory| {
            std::process::Command::new(&command[0])
                .args(&command[1..])
                .envs(environment)
                .current_dir(directory)
                .status()
                .map(|status| status.success())
        }))
    }

    pub fn with_runner(runner: Box<Runner>) -> Self {
        Self { runner }
    }

    /// `mxrs widgets new NAME [DIR]` — scaffolds a React + TypeScript
    /// pluggable widget with Mendix's official generator. Returns the
    /// widget project directory.
    pub fn create(&self, name: &str, directory: &Path) -> Result<PathBuf, String> {
        let name = validate_name(name)?;
        let root = std::path::absolute(directory).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("cannot create {}: {error}", root.display()))?;
        self.run(
            &HashMap::new(),
            &[
                "npx".to_string(),
                "--yes".to_string(),
                GENERATOR.to_string(),
                name.clone(),
            ],
            &root,
        )?;
        Ok(root.join(name))
    }

    /// `mxrs widgets build DIR [--project P]` — `npm ci` + `npm run
    /// release`, then the built `dist/*.mpk` packages in sorted order.
    pub fn build(&self, directory: &Path, project: Option<&Path>) -> Result<Vec<PathBuf>, String> {
        let root = std::path::absolute(directory).map_err(|error| error.to_string())?;
        if !root.join("package.json").is_file() {
            return Err(format!("widget package not found: {}", root.display()));
        }
        let mut environment = HashMap::new();
        if let Some(project) = project {
            let project = std::path::absolute(project).map_err(|error| error.to_string())?;
            environment.insert("MX_PROJECT_PATH".to_string(), project.display().to_string());
        }
        self.run(&environment, &["npm".to_string(), "ci".to_string()], &root)?;
        self.run(
            &environment,
            &["npm".to_string(), "run".to_string(), "release".to_string()],
            &root,
        )?;
        let mut packages: Vec<PathBuf> = match std::fs::read_dir(root.join("dist")) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("mpk"))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        packages.sort();
        if packages.is_empty() {
            return Err(format!(
                "Mendix widget build produced no MPK in {}/dist",
                root.display()
            ));
        }
        Ok(packages)
    }

    fn run(
        &self,
        environment: &HashMap<String, String>,
        command: &[String],
        directory: &Path,
    ) -> Result<(), String> {
        match (self.runner)(environment, command, directory) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("command failed: {}", command.join(" "))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(format!(
                "{} is not installed — Mendix's official widget tooling needs Node.js (npm)",
                command[0]
            )),
            Err(error) => Err(format!("cannot run {}: {error}", command.join(" "))),
        }
    }
}

fn validate_name(name: &str) -> Result<String, String> {
    let value = name.trim();
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(value.to_string())
    } else {
        Err("widget name must start with a letter and contain only letters, digits, _ or -".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Call = (HashMap<String, String>, Vec<String>, PathBuf);

    fn recording(calls: Arc<Mutex<Vec<Call>>>, succeed: bool) -> WidgetDevelopment {
        WidgetDevelopment::with_runner(Box::new(move |environment, command, directory| {
            calls.lock().unwrap().push((
                environment.clone(),
                command.to_vec(),
                directory.to_path_buf(),
            ));
            Ok(succeed)
        }))
    }

    #[test]
    fn create_runs_the_official_generator_in_the_destination() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let created = recording(calls.clone(), true)
            .create("rating-widget", directory.path())
            .unwrap();
        assert_eq!(created, directory.path().join("rating-widget"));
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, ["npx", "--yes", GENERATOR, "rating-widget"]);
        assert_eq!(calls[0].2, directory.path());
    }

    #[test]
    fn create_rejects_hostile_names_before_running_anything() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let error = recording(calls.clone(), true)
            .create("; rm -rf /", directory.path())
            .unwrap_err();
        assert!(error.contains("widget name"), "{error}");
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn build_requires_a_package_runs_npm_and_collects_sorted_mpks() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let development = recording(calls.clone(), true);
        let missing = development.build(directory.path(), None).unwrap_err();
        assert!(missing.contains("widget package not found"), "{missing}");

        std::fs::write(directory.path().join("package.json"), "{}").unwrap();
        std::fs::create_dir_all(directory.path().join("dist")).unwrap();
        std::fs::write(directory.path().join("dist/b.mpk"), "b").unwrap();
        std::fs::write(directory.path().join("dist/a.mpk"), "a").unwrap();
        let project = directory.path().join("app");
        let packages = development.build(directory.path(), Some(&project)).unwrap();
        assert_eq!(
            packages,
            [
                directory.path().join("dist/a.mpk"),
                directory.path().join("dist/b.mpk")
            ]
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, ["npm", "ci"]);
        assert_eq!(calls[1].1, ["npm", "run", "release"]);
        assert_eq!(
            calls[1].0.get("MX_PROJECT_PATH"),
            Some(&project.display().to_string())
        );
    }

    #[test]
    fn a_build_that_produces_no_mpk_is_a_loud_error() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("package.json"), "{}").unwrap();
        let error = recording(Arc::new(Mutex::new(Vec::new())), true)
            .build(directory.path(), None)
            .unwrap_err();
        assert!(error.contains("produced no MPK"), "{error}");
    }

    #[test]
    fn a_missing_node_toolchain_is_an_honest_named_error() {
        let directory = tempfile::tempdir().unwrap();
        let development = WidgetDevelopment::with_runner(Box::new(|_, _, _| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }));
        let error = development.create("rating", directory.path()).unwrap_err();
        assert!(error.contains("npx is not installed"), "{error}");
        assert!(error.contains("Node.js"), "{error}");
    }
}
