use thiserror::Error;

/// Errors produced by the Mendix-specific BSON codec.
#[derive(Debug, Error)]
pub enum BsonCodecError {
    #[error("BSON parse/serialize error: {0}")]
    Bson(#[from] bson::error::Error),

    #[error("invalid UUID string: {0:?}")]
    InvalidUuid(String),

    #[error("invalid base64 payload: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
}

pub type Result<T> = std::result::Result<T, BsonCodecError>;
