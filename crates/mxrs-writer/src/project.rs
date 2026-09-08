//! Top-level entry point: creates a fresh `.mpr` and persists an
//! `mxrs_ir::ProjectDecl` into it. Mirrors `Writer#create_project!` +
//! iterating `Writer#module_doc` per declared module.

use std::path::Path;

use mxrs_ir::declaration::ProjectDecl;
use mxrs_mpr::MprFile;

use crate::error::{Result, WriterError};
use crate::module;

pub fn write_project(path: impl AsRef<Path>, project: &ProjectDecl) -> Result<()> {
    let schema_hash = mxrs_schema::schema_hash(&project.mendix_version)
        .ok_or_else(|| WriterError::UnsupportedVersion(project.mendix_version.clone()))?;
    let mut mpr = MprFile::create(path, &project.mendix_version, schema_hash)?;
    let root_id = mpr.root_unit()?.ok_or(WriterError::MissingRootUnit)?.unit_id;

    for decl in &project.modules {
        module::write_module(&mut mpr, &root_id, decl)?;
    }
    Ok(())
}
