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
    mxrs_materializers::materialize_mpr(&output, web_output)?;
    Ok(output)
}

pub fn frontend_sources(output: impl AsRef<Path>) -> Result<PathBuf> {
    let output = std::path::absolute(output.as_ref())?;
    mxrs_materializers::materialize_frontend_sources(&output)?;
    Ok(output)
}
