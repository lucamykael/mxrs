use thiserror::Error;

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("unsupported project-settings type {0:?}")]
    UnsupportedType(String),

    #[error("unsupported project-settings component {0:?}")]
    UnsupportedComponent(String),

    #[error("unknown {method} property for {storage_type}")]
    UnknownField { storage_type: String, method: String },

    #[error("field {storage_type}.{field} not set")]
    FieldNotSet { storage_type: String, field: String },

    #[error("settings collection marker must be 1, 2, or 3")]
    InvalidMarker,

    #[error("{storage_type}.{field} expects {expected}, got a different type")]
    TypeMismatch { storage_type: String, field: String, expected: String },

    #[error("{storage_type}.{field}[{index}] has an incompatible value")]
    InvalidCollectionItem { storage_type: String, field: String, index: usize },

    #[error("project settings root must be Settings$ProjectSettings")]
    InvalidRoot,

    #[error("untyped maps are not valid project-settings values")]
    UntypedMap,

    #[error("unsupported BSON value in project settings: {0}")]
    UnsupportedBsonValue(String),

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),
}

pub type Result<T> = std::result::Result<T, SettingsError>;
