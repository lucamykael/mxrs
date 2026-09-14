use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("cannot save a unit without a container_id")]
    MissingContainer,

    #[error("invalid model structure at {path}: expected {expected}")]
    InvalidStructure {
        path: String,
        expected: &'static str,
    },

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("MPR I/O error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),
}

pub type Result<T> = std::result::Result<T, ModelError>;
