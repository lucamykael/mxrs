use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, thiserror::Error)]
pub enum CargoProjectError {
    #[error("Cargo manifest {0} does not exist")]
    MissingManifest(String),
    #[error("cannot run Cargo: {0}")]
    Cargo(#[from] std::io::Error),
    #[error("the application crate failed to build the Mendix artifact")]
    BuildFailed,
    #[error(transparent)]
    Validate(#[from] mxrs_mpr::MprError),
    #[error("the generated Mendix artifact is invalid: {0:?}")]
    InvalidArtifact(Vec<String>),
    #[error("the frontend's nanoflows name what the model does not have:\n  {}", .0.join("\n  "))]
    UnresolvedReferences(Vec<String>),
    #[error("cannot check what the built model refers to: {0}")]
    References(String),
    #[error(transparent)]
    Project(#[from] mxrs_project::ProjectError),
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error(transparent)]
    Materialize(#[from] mxrs_materializers::MaterializeError),
}

/// Builds the current Rust declaration and compares it with the exact
/// imported snapshot, including editable filesystem assets.
pub fn diff(
    manifest: impl AsRef<Path>,
    snapshot: impl AsRef<Path>,
    release: bool,
    offline: bool,
) -> Result<crate::compare::CompareResult> {
    let manifest = std::path::absolute(manifest.as_ref())?;
    let snapshot = std::path::absolute(snapshot.as_ref())?;
    let project_root = manifest.parent().unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::tempdir()?;
    let baseline_directory = temporary.path().join("baseline");
    let current_directory = temporary.path().join("current");
    std::fs::create_dir_all(&baseline_directory)?;
    std::fs::create_dir_all(&current_directory)?;
    let baseline = baseline_directory.join("Project.mpr");
    let current = current_directory.join("Project.mpr");

    mxrs_project::restore_imported_project(&snapshot, &baseline)?;
    mxrs_project::materialize_project_assets(project_root.join("assets"), &baseline)?;
    mxrs_project::materialize_java_sources(project_root.join("java"), &baseline)?;
    build(&manifest, &current, release, offline)?;
    Ok(crate::compare::compare(baseline, current)?)
}

pub type Result<T> = std::result::Result<T, CargoProjectError>;

/// Executes the application crate's build binary and validates the resulting
/// `.mpr`. This is the engine behind `cargo mxrs build`.
pub fn build(
    manifest: impl AsRef<Path>,
    output: impl AsRef<Path>,
    release: bool,
    offline: bool,
) -> Result<PathBuf> {
    let output = std::path::absolute(output.as_ref())?;
    let web_output = output
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("web");
    build_with_web_output(manifest, output, web_output, release, offline)
}

/// Builds and validates the MPR, then materializes the embedded React shell
/// and stable model contract into `web_output` without invoking Node.
pub fn build_with_web_output(
    manifest: impl AsRef<Path>,
    output: impl AsRef<Path>,
    web_output: impl AsRef<Path>,
    release: bool,
    offline: bool,
) -> Result<PathBuf> {
    let manifest = manifest.as_ref();
    if !manifest.is_file() {
        return Err(CargoProjectError::MissingManifest(
            manifest.display().to_string(),
        ));
    }
    let output = std::path::absolute(output.as_ref())?;
    let mut command = Command::new("cargo");
    command.args(["run", "--quiet", "--manifest-path"]);
    command.arg(manifest);
    if release {
        command.arg("--release");
    }
    if offline {
        command.arg("--offline");
    }
    // Application build binaries may print progress; keep that on stderr so
    // `cargo mxrs diff --json` remains one parseable machine-readable document.
    let mut child = command
        .arg("--")
        .arg(&output)
        .stdout(Stdio::piped())
        .spawn()?;
    let logs = std::io::copy(
        &mut child.stdout.take().expect("stdout was piped"),
        &mut std::io::stderr(),
    );
    let status = child.wait()?;
    logs?;
    if !status.success() {
        return Err(CargoProjectError::BuildFailed);
    }
    let report = crate::validate::validate(&output)?;
    if !report.is_valid() {
        return Err(CargoProjectError::InvalidArtifact(report.errors));
    }
    let dangling = unresolved_frontend_references(manifest, &output)?;
    if !dangling.is_empty() {
        return Err(CargoProjectError::UnresolvedReferences(dangling));
    }
    mxrs_materializers::materialize_mpr(&output, web_output)?;
    // The shell the project's frontend is drawn with is the one this
    // build knows: a project made before the shell changed is brought up
    // to it, its own files untouched.
    let frontend = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("frontend");
    if frontend.join("package.json").is_file() {
        let report = mxrs_materializers::materialize_frontend_sources(&frontend)?;
        if report.changed_files > 0 {
            eprintln!(
                "[mxrs] frontend shell brought up to this build: {} file(s) in {}",
                report.changed_files,
                frontend.display()
            );
        }
    }
    Ok(output)
}

/// What the nanoflows the frontend declares name — an entity, a member, a
/// flow, a page — that the built model does not have, each where its
/// nanoflow is declared. TypeScript names them as text, so nothing before
/// the build could tell.
fn unresolved_frontend_references(manifest: &Path, output: &Path) -> Result<Vec<String>> {
    let frontend = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("frontend");
    // A frontend the build could not read has already failed it.
    let Ok(declared) = mxrs_frontend::read_frontend(&frontend) else {
        return Ok(Vec::new());
    };
    if declared.nanoflow_origins.is_empty() && declared.form_origins.is_empty() {
        return Ok(Vec::new());
    }
    let project = mxrs_model::Project::open(output, true)?;
    let index = mxrs_semantic::SemanticIndex::build(&project)
        .map_err(|error| CargoProjectError::References(error.to_string()))?;
    let mut dangling = Vec::new();
    for diagnostic in index.diagnostics() {
        if diagnostic.code != "unresolved_reference" {
            continue;
        }
        // What a page, layout or snippet of the frontend names and the
        // model lacks is said, not refused: a model may be imported with
        // such a page, and the build must still be the model it was.
        let form = [
            ("page:", "Forms$Page"),
            ("layout:", "Forms$Layout"),
            ("snippet:", "Forms$Snippet"),
        ]
        .iter()
        .find_map(|(prefix, kind)| {
            let rest = diagnostic.message.strip_prefix(prefix)?;
            let (qualified, what) = rest.split_once(' ')?;
            let origin = declared.form_origins.get(&format!("{kind} {qualified}"))?;
            Some(format!(
                "{origin}: {}{qualified} {what}",
                prefix.replace(':', " ")
            ))
        });
        if let Some(warning) = form {
            eprintln!("[mxrs] warning: {warning}");
            continue;
        }
        let Some(rest) = diagnostic.message.strip_prefix("nanoflow:") else {
            continue;
        };
        let Some((qualified, what)) = rest.split_once(' ') else {
            continue;
        };
        if let Some(origin) = declared.nanoflow_origins.get(qualified) {
            dangling.push(format!("{origin}: nanoflow {qualified} {what}"));
        }
    }
    dangling.sort();
    Ok(dangling)
}

pub fn frontend_sources(output: impl AsRef<Path>) -> Result<PathBuf> {
    let output = std::path::absolute(output.as_ref())?;
    mxrs_materializers::materialize_frontend_sources(&output)?;
    Ok(output)
}
