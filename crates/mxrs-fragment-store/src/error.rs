use thiserror::Error;

#[derive(Debug, Error)]
pub enum FragmentStoreError {
    #[error("invalid native fragment digest {0:?}")]
    InvalidDigest(String),

    #[error("native fragment {0} does not exist")]
    NotFound(String),

    #[error("native fragment digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },

    #[error("native fragment {digest} is missing declared type(s): {}", missing.join(", "))]
    MissingTypes { digest: String, missing: Vec<String> },

    #[error("native fragment {digest} is missing declared hint(s): {}", missing.join(", "))]
    MissingHints { digest: String, missing: Vec<String> },

    #[error("invalid native fragment override {key:?} for {digest}")]
    InvalidOverride { digest: String, key: String },

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, FragmentStoreError>;
