//! Transactional scaffolding for new Cargo-native Mendix applications.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("scaffold destination already exists: {0}")]
    DestinationExists(String),
    #[error("invalid application name: {0}")]
    InvalidName(String),
    #[error("invalid Mendix version: {0}")]
    InvalidVersion(String),
    #[error("invalid dependency path: {0}")]
    InvalidDependencyPath(String),
    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, ScaffoldError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MxrsDependency {
    Git(String),
    Path(PathBuf),
}

impl Default for MxrsDependency {
    fn default() -> Self {
        Self::Git("https://github.com/lucamykael/mxrs".to_string())
    }
}

#[derive(Debug, Clone)]
pub struct ProjectScaffold {
    pub name: String,
    pub mendix_version: String,
    pub destination: PathBuf,
    pub dependency: MxrsDependency,
}

impl ProjectScaffold {
    pub fn new(
        name: impl Into<String>,
        mendix_version: impl Into<String>,
        destination: impl Into<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            mendix_version: mendix_version.into(),
            destination: destination.into(),
            dependency: MxrsDependency::default(),
        }
    }

    pub fn dependency(mut self, dependency: MxrsDependency) -> Self {
        self.dependency = dependency;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldReport {
    pub destination: PathBuf,
    pub package_name: String,
    pub files: usize,
}

pub fn generate_project(options: &ProjectScaffold) -> Result<ScaffoldReport> {
    validate_name(&options.name)?;
    validate_version(&options.mendix_version)?;
    if options.destination.exists() {
        return Err(ScaffoldError::DestinationExists(
            options.destination.display().to_string(),
        ));
    }
    let destination = std::path::absolute(&options.destination)
        .map_err(|source| io_error(&options.destination, source))?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("application");
    let staging = parent.join(format!(".{file_name}.mxrs-{}.tmp", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|source| io_error(&staging, source))?;
    }
    std::fs::create_dir_all(&staging).map_err(|source| io_error(&staging, source))?;

    let package_name = package_name(&options.name);
    let crate_name = package_name.replace('-', "_");
    let files = project_files(options, &package_name, &crate_name)?;
    let result = (|| {
        for (relative, body) in &files {
            write_file(&staging, relative, body)?;
        }
        std::fs::rename(&staging, &destination).map_err(|source| io_error(&destination, source))?;
        Ok(ScaffoldReport {
            destination,
            package_name,
            files: files.len(),
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn project_files(
    options: &ProjectScaffold,
    package_name: &str,
    crate_name: &str,
) -> Result<Vec<(&'static str, String)>> {
    let dependency = match &options.dependency {
        MxrsDependency::Git(url) if !url.trim().is_empty() => {
            format!("{{ git = {} }}", json_string(url))
        }
        MxrsDependency::Path(path) => {
            let absolute = std::path::absolute(path).map_err(|source| io_error(path, source))?;
            format!(
                "{{ path = {} }}",
                json_string(&absolute.display().to_string())
            )
        }
        MxrsDependency::Git(url) => return Err(ScaffoldError::InvalidDependencyPath(url.clone())),
    };
    let version = json_string(&options.mendix_version);
    Ok(vec![
        (
            "Cargo.toml",
            format!(
                "[package]\nname = {package_name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nmxrs = {dependency}\n"
            ),
        ),
        (".gitignore", "/build\n/target\n".to_string()),
        (
            "src/lib.rs",
            format!(
                "mod domain;\n\n#[mxrs::application(version = {version})]\npub struct Application;\n"
            ),
        ),
        (
            "src/domain/mod.rs",
            format!(
                "pub fn build() -> mxrs::ProjectDecl {{\n    let mut project = mxrs::ProjectBuilder::new({version});\n    project.module(\"Main\", |module| {{\n        module.page(\"Home\", |page| {{\n            page.layout(\"Atlas_Core.ApplicationLayout\", \"Main\");\n            page.text(\"Welcome to {}\");\n        }});\n    }});\n    project.build()\n}}\n",
                escape_rust_string(&options.name)
            ),
        ),
        (
            "src/main.rs",
            format!(
                "fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    let output = std::env::args().nth(1).unwrap_or_else(|| \"build/{}.mpr\".to_string());\n    let output_path = std::path::Path::new(&output);\n    if let Some(parent) = output_path.parent() {{ std::fs::create_dir_all(parent)?; }}\n    let declaration = {crate_name}::Application::build();\n    if output_path.exists() {{\n        mxrs::synchronize_project(output_path, &declaration)?;\n    }} else {{\n        mxrs::write_project(output_path, &declaration)?;\n    }}\n    let web = output_path.parent().unwrap_or_else(|| std::path::Path::new(\".\")).join(\"web\");\n    mxrs::materialize_mpr(output_path, web)?;\n    println!(\"built {{output}}\");\n    Ok(())\n}}\n",
                escape_rust_string(&options.name)
            ),
        ),
        (
            "README.md",
            format!(
                "# {}\n\nCargo-native Mendix application generated by `mxrs new`.\n\n```sh\ncargo check\ncargo test\ncargo run\n```\n\nThe build writes `build/{}.mpr` and the embedded web application under `build/web/`.\n",
                options.name, options.name
            ),
        ),
    ])
}

fn write_file(root: &Path, relative: &str, body: &str) -> Result<()> {
    let target = root.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    std::fs::write(&target, body).map_err(|source| io_error(&target, source))
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(ScaffoldError::InvalidName(name.to_string()));
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<()> {
    let parts = version.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.parse::<u32>().is_err()) {
        return Err(ScaffoldError::InvalidVersion(version.to_string()));
    }
    Ok(())
}

fn package_name(name: &str) -> String {
    let mut output = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while output.contains("--") {
        output = output.replace("--", "-");
    }
    output = output.trim_matches('-').to_string();
    if output.is_empty() {
        output = "mendix-app".to_string();
    }
    if output.starts_with(|character: char| character.is_ascii_digit()) {
        output.insert_str(0, "app-");
    }
    output
}

fn escape_rust_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string is always serializable")
}

fn io_error(path: &Path, source: std::io::Error) -> ScaffoldError {
    ScaffoldError::Io {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_is_complete_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("orders");
        let report = generate_project(&ProjectScaffold::new(
            "Order Portal",
            "11.12.1",
            &destination,
        ))
        .unwrap();
        assert_eq!(report.package_name, "order-portal");
        assert_eq!(report.files, 6);
        assert!(destination.join("src/domain/mod.rs").is_file());
        assert!(matches!(
            generate_project(&ProjectScaffold::new(
                "Order Portal",
                "11.12.1",
                &destination
            )),
            Err(ScaffoldError::DestinationExists(_))
        ));
    }

    #[test]
    fn validates_names_versions_and_package_identifiers() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            generate_project(&ProjectScaffold::new(
                "",
                "11.12.1",
                directory.path().join("a")
            ))
            .is_err()
        );
        assert!(
            generate_project(&ProjectScaffold::new(
                "App",
                "11",
                directory.path().join("b")
            ))
            .is_err()
        );
        assert_eq!(package_name(" 2026 / Orders "), "app-2026-orders");
    }

    #[test]
    fn local_dependency_is_absolute_and_generated_source_is_valid_rust_shape() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("demo");
        generate_project(
            &ProjectScaffold::new("Demo", "11.12.1", &destination)
                .dependency(MxrsDependency::Path(PathBuf::from("../mxrs"))),
        )
        .unwrap();
        let manifest = std::fs::read_to_string(destination.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("mxrs = { path = \""));
        let source = std::fs::read_to_string(destination.join("src/main.rs")).unwrap();
        assert!(source.contains("mxrs::write_project"));
        assert!(source.contains("mxrs::materialize_mpr"));
    }
}
