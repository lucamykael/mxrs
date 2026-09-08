use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriterError {
    #[error("unsupported Mendix version {0:?} (mxrs-schema has no embedded schema hash for it)")]
    UnsupportedVersion(String),

    #[error("newly created .mpr has no root unit")]
    MissingRootUnit,

    #[error("association target {0:?} does not match any entity declared in this module")]
    UnknownAssociationTarget(String),

    #[error("cross-module association target {0:?} does not match any entity declared anywhere in the project (expected \"Module.Entity\")")]
    UnknownCrossModuleAssociationTarget(String),

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("MPR I/O error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),

    #[error("project scaffolding error: {0}")]
    ProjectTemplate(#[from] mxrs_schema::ProjectTemplateError),
}

pub type Result<T> = std::result::Result<T, WriterError>;
