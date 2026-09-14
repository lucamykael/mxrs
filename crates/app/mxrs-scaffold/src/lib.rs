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
    if options.destination.symlink_metadata().is_ok() {
        return Err(ScaffoldError::DestinationExists(
            options.destination.display().to_string(),
        ));
    }
    let destination = std::path::absolute(&options.destination)
        .map_err(|source| io_error(&options.destination, source))?;
    let package_name = package_name(&options.name);
    let crate_name = package_name.replace('-', "_");
    let files = project_files(options, &package_name, &crate_name)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("application");
    let staging = tempfile::Builder::new()
        .prefix(&format!(".{file_name}.mxrs-"))
        .tempdir_in(parent)
        .map_err(|source| io_error(parent, source))?;
    for (relative, body) in &files {
        write_file(staging.path(), relative, body)?;
    }
    publish(staging.path(), &destination)?;
    Ok(ScaffoldReport {
        destination,
        package_name,
        files: files.len(),
    })
}

// Reserve an empty destination with create_dir (exclusive even for dangling
// symlinks). rename then replaces only our empty reservation; it cannot replace
// a raced-in nonempty project. Parent directories are trusted workspace paths.
fn publish(staging: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir(destination).map_err(|source| {
        if source.kind() == std::io::ErrorKind::AlreadyExists {
            ScaffoldError::DestinationExists(destination.display().to_string())
        } else {
            io_error(destination, source)
        }
    })?;
    if let Err(source) = std::fs::rename(staging, destination) {
        // Never recursively remove a destination: another actor may have added
        // data after our reservation. remove_dir only succeeds when still empty.
        let _ = std::fs::remove_dir(destination);
        return Err(io_error(destination, source));
    }
    Ok(())
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
            let absolute = absolute
                .to_str()
                .ok_or_else(|| ScaffoldError::InvalidDependencyPath(path.display().to_string()))?;
            format!("{{ path = {} }}", json_string(absolute))
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
                "pub fn build() -> mxrs::ProjectDecl {{\n    let mut project = mxrs::ProjectBuilder::new({version});\n    project.module(\"Main\", |module| {{\n        module.layout(\"ApplicationLayout\", |layout| {{\n            layout.placeholder(\"Main\");\n        }});\n        module.page(\"Home\", |page| {{\n            page.layout(\"Main.ApplicationLayout\", \"Main\");\n            page.text(\"Welcome to {}\");\n        }});\n    }});\n    project.navigation(|navigation| {{\n        navigation.profile(\"Responsive\", |profile| {{\n            profile.home_page(\"Main.Home\");\n        }});\n    }});\n    project.build()\n}}\n",
                escape_rust_string(&options.name)
            ),
        ),
        (
            "src/main.rs",
            format!(
                "fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    let output = std::env::args().nth(1).unwrap_or_else(|| \"build/{}.mpr\".to_string());\n    let output_path = std::path::Path::new(&output);\n    if let Some(parent) = output_path.parent() {{ std::fs::create_dir_all(parent)?; }}\n    let declaration = {crate_name}::Application::build();\n    if output_path.exists() {{\n        mxrs::synchronize_project(output_path, &declaration)?;\n    }} else {{\n        mxrs::write_project(output_path, &declaration)?;\n    }}\n    let web = output_path.parent().unwrap_or_else(|| std::path::Path::new(\".\")).join(\"web\");\n    mxrs::materialize_mpr(output_path, web)?;\n    println!(\"built {{output}}\");\n    Ok(())\n}}\n",
                package_name
            ),
        ),
        (
            "README.md",
            format!(
                "# {}\n\nCargo-native Mendix application generated by `mxrs new`.\n\n```sh\ncargo check\ncargo test\ncargo run\n```\n\nThe build writes `build/{}.mpr` and the embedded web application under `build/web/`.\n",
                options.name, package_name
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
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || part.parse::<u32>().is_err()
        })
    {
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
    if matches!(
        output.as_str(),
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "gen"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    ) {
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
#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_configuration_has_no_filesystem_side_effects() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("not-created/app");
        let options = ProjectScaffold::new("Demo", "11.12.1", &destination)
            .dependency(MxrsDependency::Git(" ".into()));
        assert!(matches!(
            generate_project(&options),
            Err(ScaffoldError::InvalidDependencyPath(_))
        ));
        assert!(!destination.parent().unwrap().exists());
        for version in [
            "+11.12.1",
            "11.12",
            "11..1",
            "11.12.1.0",
            "4294967296.0.0",
            "11.a.1",
        ] {
            assert!(validate_version(version).is_err());
        }
        for name in ["\nDemo", "Demo\0", "  "] {
            assert!(validate_name(name).is_err());
        }
    }

    #[test]
    fn predictable_old_staging_directories_are_never_deleted() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("demo");
        let old = directory
            .path()
            .join(format!(".demo.mxrs-{}.tmp", std::process::id()));
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("user-data"), "keep").unwrap();
        generate_project(&ProjectScaffold::new("Demo", "11.12.1", &destination)).unwrap();
        assert_eq!(
            std::fs::read_to_string(old.join("user-data")).unwrap(),
            "keep"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn publication_refuses_raced_destinations_and_cleans_only_its_empty_reservation() {
        let directory = tempfile::tempdir().unwrap();
        let staging = directory.path().join("staging");
        let destination = directory.path().join("destination");
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(&destination, "keep").unwrap();
        assert!(matches!(
            publish(&staging, &destination),
            Err(ScaffoldError::DestinationExists(_))
        ));
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "keep");
        assert!(staging.is_dir());
        let empty = directory.path().join("reservation");
        assert!(publish(&directory.path().join("missing"), &empty).is_err());
        assert!(!empty.exists());
        assert!(publish(&staging, &destination.join("child")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_destination_symlinks_and_non_unicode_dependencies_are_rejected() {
        use std::os::unix::{ffi::OsStringExt, fs::symlink};
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("demo");
        symlink(directory.path().join("missing"), &destination).unwrap();
        assert!(matches!(
            generate_project(&ProjectScaffold::new("Demo", "11.12.1", &destination)),
            Err(ScaffoldError::DestinationExists(_))
        ));
        assert!(destination.is_symlink());
        let invalid = PathBuf::from(std::ffi::OsString::from_vec(vec![b'a', 0xff]));
        let options = ProjectScaffold::new("Demo", "11.12.1", directory.path().join("new"))
            .dependency(MxrsDependency::Path(invalid));
        assert!(matches!(
            generate_project(&options),
            Err(ScaffoldError::InvalidDependencyPath(_))
        ));
    }

    #[test]
    fn display_names_cannot_escape_the_default_build_directory() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("demo");
        let report = generate_project(&ProjectScaffold::new(
            "../../Escape / App",
            "11.12.1",
            &destination,
        ))
        .unwrap();
        assert_eq!(report.package_name, "escape-app");
        let main = std::fs::read_to_string(destination.join("src/main.rs")).unwrap();
        assert!(main.contains("build/escape-app.mpr"));
        assert!(!main.contains("../"));
        let domain = std::fs::read_to_string(destination.join("src/domain/mod.rs")).unwrap();
        assert!(domain.contains("Welcome to ../../Escape / App"));
        assert_eq!(package_name("日本語"), "mendix-app");
        for name in ["async", "type", "self", "gen"] {
            assert_eq!(package_name(name), format!("app-{name}"));
        }
    }

    #[test]
    fn keyword_named_generated_applications_compile_with_real_cargo() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|path| path.join("xtask/Cargo.toml").is_file())
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("application");
        generate_project(
            &ProjectScaffold::new("async", "11.12.1", &destination)
                .dependency(MxrsDependency::Path(workspace.join("crates/app/mxrs"))),
        )
        .unwrap();
        let output = std::process::Command::new(env!("CARGO"))
            .args(["build", "--offline"])
            .current_dir(&destination)
            .env(
                "CARGO_TARGET_DIR",
                crate::nested_cargo::target_dir(workspace.join("target")),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

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
