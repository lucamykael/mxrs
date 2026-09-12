use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluggableError {
    #[error("expected {expected} at {path}, got a different $Type")]
    UnexpectedType {
        expected: &'static str,
        path: String,
    },

    #[error("missing required field {field:?} at {path}")]
    MissingField { field: &'static str, path: String },

    #[error("unmapped storage field {field:?} at {path}")]
    UnmappedField { field: String, path: String },

    #[error("unknown pluggable widget {0:?}")]
    UnknownWidget(String),

    #[error("unknown pluggable widget property {0:?}")]
    UnknownProperty(String),

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),
}

pub type Result<T> = std::result::Result<T, PluggableError>;
