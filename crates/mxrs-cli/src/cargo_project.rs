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
