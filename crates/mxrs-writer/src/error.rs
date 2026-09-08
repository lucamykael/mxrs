use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriterError {
    #[error("unsupported Mendix version {0:?} (mxrs-schema has no embedded schema hash for it)")]
    UnsupportedVersion(String),

    #[error("newly created .mpr has no root unit")]
    MissingRootUnit,

    #[error("association {0:?} targets an entity in a different module — cross-module associations aren't supported yet")]
    UnsupportedCrossModuleAssociation(String),

    #[error("association target {0:?} does not match any entity declared in this module")]
    UnknownAssociationTarget(String),

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("MPR I/O error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),
}

pub type Result<T> = std::result::Result<T, WriterError>;
