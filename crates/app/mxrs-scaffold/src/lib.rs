//! Transactional scaffolding for Cargo-native Mendix applications: whole new
//! projects ([`generate_project`]) and individual artifacts added to an
//! existing one ([`artifact::scaffold_artifact`], the mxrs counterpart of
//! mxrb's `mxrb entity new`/`mxrb page new`/… generators).

use std::path::{Path, PathBuf};

pub mod artifact;
pub mod lifecycle;
pub mod page_templates;
pub mod registry;
mod service;
mod templates;
mod transaction;

pub use artifact::{
    ArtifactKind, ArtifactScaffold, PageChain, ProjectInspection, SCAFFOLD_COMMANDS,
    ScaffoldCommand, ScaffoldOutcome, inspect_project, scaffold_artifact,
};
pub use page_templates::PageTemplate;
pub use registry::RegisteredScaffold;

#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("scaffold destination already exists: {0}")]
    DestinationExists(String),
    #[error("invalid project source {path}: {reason}")]
    InvalidProjectSource { path: String, reason: String },
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
    #[error("{0}: not a Cargo-native MXRS project (run `mxrs new` or `mxrs import` first)")]
    ProjectNotFound(String),
    #[error("{0}: module not found (run `mxrs module new <Module>` first)")]
    ModuleNotFound(String),
    #[error("{0}: module already exists")]
    ModuleExists(String),
    #[error("{0}: file already exists")]
    FileExists(String),
    #[error(
        "{0}: aggregator not found (for a project created before layered generation, run `mxrs upgrade` from its root, inspect the preview, then rerun with `--apply`)"
    )]
    AggregatorNotFound(String),
    #[error("{label} name must be a Mendix identifier: {value}")]
    InvalidIdentifier { label: &'static str, value: String },
    #[error("name must be qualified as Module.Artifact: {0}")]
    UnqualifiedName(String),
    #[error("unknown page template: {0} (run `mxrs page templates` for the catalog)")]
    UnknownPageTemplate(String),
    #[error(
        "unknown page chain: {0} (expected page:microflow, page:nanoflow or page:nanoflow:microflow)"
    )]
    UnknownPageChain(String),
    #[error("entity name is reserved by Mendix: {0}")]
    ReservedEntityName(String),
    #[error("{0} has no Rust module spelling; rename the artifact")]
    UnsupportedArtifactName(String),
    #[error("invalid scaffold registry: {0}")]
    RegistryInvalid(String),
    #[error("scaffold not registered: {0}")]
    ScaffoldNotRegistered(String),
    #[error("refusing to remove a changed or missing scaffold file: {0}")]
    ScaffoldFileChanged(String),
    #[error("unsafe scaffold path: {0}")]
    UnsafeScaffoldPath(String),
    #[error("Cargo-native project has no #[mxrs::application(version = \"…\")] declaration")]
    MissingVersionDeclaration,
    #[error(
        "{0}: generated project layout cannot be migrated safely; restore a complete pre-layered or layered source tree"
    )]
    UnsupportedLayerMigration(String),
    #[error(
        "{root}: this project uses the earlier {layout} layout; write its model out with `mxrs convert rust-to-mendix` and re-import it with `mxrs convert mendix-to-rust` to get the current one"
    )]
    OutdatedLayout { root: String, layout: &'static str },
    #[error("project version declarations disagree: {0:?}")]
    VersionMismatch(Vec<String>),
    #[error("{0}: project security is not initialized (run `mxrs security init <Module>` first)")]
    SecurityNotInitialized(String),
    #[error(
        "demo user references user role {0:?}, which src/domain/security declares nowhere; declare it with `security.role({0:?}, …)` or scaffold it via `mxrs security init`"
    )]
    UnknownDemoUserRole(String),
    #[error(
        "demo user entity {0:?} was not found in this project's domain layer (expected `Module.Entity` with src/domain/entities/<module>/<entity>.rs, or `System.User`)"
    )]
    UnknownDemoUserEntity(String),
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
    let count = files.len();
    for (relative, body) in files {
        let body = if relative.ends_with(".rs") {
            format_rust(body)
        } else {
            body
        };
        write_file(staging.path(), &relative, &body)?;
    }
    publish(staging.path(), &destination)?;
    Ok(ScaffoldReport {
        destination,
        package_name,
        files: count,
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
) -> Result<Vec<(String, String)>> {
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
    // The user interface is the frontend's: the same React + TypeScript
    // application an import writes, with the navigation it declares.
    let frontend = mxrs_materializers::frontend_source_files()
        .iter()
        .map(|(path, bytes)| {
            (
                format!("frontend/{path}"),
                String::from_utf8_lossy(bytes).into_owned(),
            )
        })
        .chain([(
            "frontend/src/navigation/index.ts".to_string(),
            templates::navigation(),
        )]);
    let rust: Vec<(&str, String)> = vec![
        (
            "Cargo.toml",
            format!(
                "[package]\nname = {package_name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nmxrs = {dependency}\n"
            ),
        ),
        (
            ".gitignore",
            "/build\n/target\n/frontend/node_modules\n/frontend/dist\n".to_string(),
        ),
        // The crate root is the layers and the application: every
        // declaration below registers itself, so nothing here composes.
        (
            "src/lib.rs",
            format!(
                "pub mod domain;\npub mod infrastructure;\npub mod services;\npub mod ui;\n\n#[mxrs::application(version = {version})]\npub struct Application;\n"
            ),
        ),
        ("src/domain/mod.rs", templates::domain_layer()),
        (
            "src/domain/modules/mod.rs",
            templates::module_registry("main"),
        ),
        (
            "src/domain/modules/main.rs",
            templates::module_declaration("Main"),
        ),
        ("src/services/mod.rs", templates::services_layer()),
        ("src/ui/mod.rs", templates::ui_layer()),
        (
            "src/ui/layouts/mod.rs",
            templates::registering_concept_index("layouts", "main"),
        ),
        (
            "src/ui/layouts/main/mod.rs",
            templates::registering_folder_index("Main", "layouts", "application_layout"),
        ),
        (
            "src/ui/layouts/main/application_layout.rs",
            templates::layouts("Main", "Main"),
        ),
        (
            "src/ui/pages/mod.rs",
            templates::registering_concept_index("pages", "main"),
        ),
        (
            "src/ui/pages/main/mod.rs",
            templates::registering_folder_index("Main", "pages", "home"),
        ),
        (
            "src/ui/pages/main/home.rs",
            templates::home_page(&escape_rust_string(&options.name)),
        ),
        (
            "src/infrastructure/mod.rs",
            templates::infrastructure_layer(),
        ),
        (
            "src/main.rs",
            format!(
                "fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    let output = std::env::args().nth(1).unwrap_or_else(|| \"build/{}.mpr\".to_string());\n    let output_path = std::path::Path::new(&output);\n    if let Some(parent) = output_path.parent() {{ std::fs::create_dir_all(parent)?; }}\n    let mut declaration = {crate_name}::Application::build();\n    mxrs::merge_frontend(&mut declaration, std::path::Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"frontend\"))?;\n    if output_path.exists() {{\n        mxrs::synchronize_project(output_path, &declaration)?;\n    }} else {{\n        mxrs::write_project(output_path, &declaration)?;\n    }}\n    let web = output_path.parent().unwrap_or_else(|| std::path::Path::new(\".\")).join(\"web\");\n    mxrs::materialize_mpr(output_path, web)?;\n    println!(\"built {{output}}\");\n    Ok(())\n}}\n",
                package_name
            ),
        ),
        (
            "README.md",
            format!(
                "# {}\n\nCargo-native Mendix application generated by `mxrs new`.\n\n```sh\ncargo check\ncargo test\ncargo run\n```\n\nThe build writes `build/{}.mpr` and the embedded web application under `build/web/`.\n\nThe user interface is the frontend's: `frontend/` is a React + TypeScript application, and `frontend/src/navigation/index.ts` declares the navigation every build reads into the model.\n",
                options.name, package_name
            ),
        ),
    ];
    Ok(rust
        .into_iter()
        .map(|(path, body)| (path.to_string(), body))
        .chain(frontend)
        .collect())
}

/// Generated Rust as rustfmt would leave it.
///
/// A template cannot know how long the names it is given are, and rustfmt's
/// line-breaking depends on exactly that, so the formatter itself has the
/// last word. It is best effort by design: without rustfmt on the path —
/// or if it rejects the source — the template's own layout is kept, which
/// is valid Rust either way.
pub(crate) fn format_rust(source: String) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let Ok(mut formatter) = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return source;
    };
    let written = formatter
        .stdin
        .take()
        .is_some_and(|mut input| input.write_all(source.as_bytes()).is_ok());
    let Ok(output) = formatter.wait_with_output() else {
        return source;
    };
    if !written || !output.status.success() {
        return source;
    }
    match String::from_utf8(output.stdout) {
        Ok(formatted) if !formatted.trim().is_empty() => formatted,
        _ => source,
    }
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
    if is_rust_keyword(&output) {
        output.insert_str(0, "app-");
    }
    output
}

/// Shared by [`package_name`] and `artifact`'s Rust module naming: both have
/// to avoid emitting a bare keyword where an identifier is required.
pub(crate) fn is_rust_keyword(value: &str) -> bool {
    matches!(
        value,
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
    )
}

fn escape_rust_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string is always serializable")
}

pub(crate) fn io_error(path: &Path, source: std::io::Error) -> ScaffoldError {
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
        // The display name reaches generated source through the home page's
        // text, not through `src/domain/mod.rs`.
        let home = std::fs::read_to_string(destination.join("src/ui/pages/main/home.rs")).unwrap();
        assert!(home.contains("Welcome to ../../Escape / App"));
        let domain = std::fs::read_to_string(destination.join("src/domain/mod.rs")).unwrap();
        assert!(!domain.contains("Welcome to"));
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

    /// `mxrs new` and `mxrs import` have to agree on the layered shape, or a
    /// scaffolded project and an imported one would be different projects to
    /// work in. This mirrors `mxrs-exporter`'s
    /// `generated_layers_only_list_what_they_hold`.
    #[test]
    fn a_fresh_project_is_its_layers_and_self_registering_declarations() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("demo");
        generate_project(&ProjectScaffold::new("Demo", "11.12.1", &destination)).unwrap();

        let crate_root = std::fs::read_to_string(destination.join("src/lib.rs")).unwrap();
        for layer in ["services", "domain", "infrastructure", "ui"] {
            assert!(
                crate_root.contains(&format!("pub mod {layer};")),
                "{layer} is not a public top-level module: {crate_root}"
            );
        }
        // The crate root is the layers and the application. Nothing composes:
        // every declaration below registers itself, so no layer can reach
        // outward to apply another.
        assert_eq!(
            crate_root,
            "pub mod domain;\npub mod infrastructure;\npub mod services;\npub mod ui;\n\n#[mxrs::application(version = \"11.12.1\")]\npub struct Application;\n"
        );
        for layer in ["domain", "services", "infrastructure", "ui"] {
            let source =
                std::fs::read_to_string(destination.join(format!("src/{layer}/mod.rs"))).unwrap();
            for composing in ["fn build", "fn apply", "ProjectDecl", "crate::"] {
                assert!(
                    !source.contains(composing),
                    "{layer}: {composing}\n{source}"
                );
            }
        }
        // A fresh project is the same shape an import produces: the module
        // it declares, a layout, a home page and the navigation opening it,
        // each the annotated item that declares it.
        let read = |relative: &str| std::fs::read_to_string(destination.join(relative)).unwrap();
        assert!(read("src/domain/modules/main.rs").contains("#[declaration(module = \"Main\")]"));
        assert!(
            read("src/ui/layouts/main/application_layout.rs")
                .contains("#[layout(module = \"Main\")]")
        );
        assert!(read("src/ui/pages/main/home.rs").contains("#[page(module = \"Main\")]"));
        // The navigation is the frontend's, beside the application that
        // renders it.
        assert!(read("frontend/src/navigation/index.ts").contains("homePage: \"Main.Home\""));
        assert!(!destination.join("src/ui/navigation.rs").exists());
        assert!(!read("src/ui/mod.rs").contains("pub mod navigation;"));

        // There is no module tree at all: the authored tree is layer-first,
        // and a concept grows a folder for a module when the first artifact
        // of that concept is scaffolded into it.
        assert!(!destination.join("src/modules").exists());
        assert!(!crate_root.contains("pub mod modules;"), "{crate_root}");
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
        // A bare count would not notice a layer going missing, so name every
        // file `mxrs new` owes a fresh project: each architectural layer, its
        // scaffolded-module aggregator, and the crate plumbing around them.
        let expected = [
            ".gitignore",
            "Cargo.toml",
            "README.md",
            "src/services/mod.rs",
            "src/domain/mod.rs",
            "src/domain/modules/main.rs",
            "src/domain/modules/mod.rs",
            "src/infrastructure/mod.rs",
            "src/lib.rs",
            "src/main.rs",
            "src/ui/layouts/main/application_layout.rs",
            "src/ui/layouts/main/mod.rs",
            "src/ui/layouts/mod.rs",
            "src/ui/mod.rs",
            "src/ui/pages/main/home.rs",
            "src/ui/pages/main/mod.rs",
            "src/ui/pages/mod.rs",
        ];
        let frontend = mxrs_materializers::frontend_source_files();
        for relative in expected
            .iter()
            .map(|relative| relative.to_string())
            .chain(frontend.iter().map(|(path, _)| format!("frontend/{path}")))
            .chain(["frontend/src/navigation/index.ts".to_string()])
        {
            assert!(
                destination.join(&relative).is_file(),
                "{relative} is missing from a fresh scaffold"
            );
        }
        assert_eq!(report.files, expected.len() + frontend.len() + 1);
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
