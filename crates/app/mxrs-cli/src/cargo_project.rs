use std::path::{Path, PathBuf};
use std::process::Command;

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
    let status = command.arg("--").arg(&output).status()?;
    if !status.success() {
        return Err(CargoProjectError::BuildFailed);
    }
    let report = crate::validate::validate(&output)?;
    if !report.is_valid() {
        return Err(CargoProjectError::InvalidArtifact(report.errors));
    }
    Ok(output)
}
