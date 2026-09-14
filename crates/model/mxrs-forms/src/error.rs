use thiserror::Error;

#[derive(Debug, Error)]
pub enum FormsError {
    #[error("unsupported Forms schema version {0:?}; only 11.12.1 is embedded")]
    UnsupportedSchemaVersion(String),

    #[error("Forms schema version mismatch: expected {expected}, got {actual:?}")]
    SchemaVersionMismatch { expected: String, actual: String },

    #[error("malformed Forms schema: missing or invalid {0}")]
    MalformedSchema(&'static str),

    #[error("unknown Forms type {type_name:?} for Mendix {version}")]
    UnknownType { version: String, type_name: String },

    #[error("unknown {type_name} property {property:?}")]
    UnknownProperty { type_name: String, property: String },

    #[error("{type_name} is an enum, not an element")]
    TypeIsEnum { type_name: String },

    #[error("{type_name}.{property} is not a collection")]
    NotACollection { type_name: String, property: String },

    #[error("{type_name}.{property} cannot be nil")]
    UnexpectedNil { type_name: String, property: String },

    #[error("{type_name}.{property} requires an array")]
    ExpectedArray { type_name: String, property: String },

    #[error("{type_name}.{property} expects {expected}, got a different type")]
    TypeMismatch {
        type_name: String,
        property: String,
        expected: String,
    },

    #[error("invalid {enum_name} value {value:?}")]
    InvalidEnumValue { enum_name: String, value: String },

    #[error("{type_name}.{property} expects {target} element")]
    IncompatibleElement {
        type_name: String,
        property: String,
        target: String,
    },

    #[error("not a Forms storage type: {0:?}")]
    NotAFormsStorageType(String),

    #[error("unmapped {type_name} storage field(s) at {path}: {fields}")]
    UnsupportedStorageProperty {
        type_name: String,
        path: String,
        fields: String,
    },

    #[error("unresolved storage reference at {path}: requires a semantic resolver")]
    UnresolvedStorageReference { path: String },

    #[error("unsupported external Forms type {type_name} at {path}")]
    UnsupportedExternalType { type_name: String, path: String },

    #[error("invalid {shape} at {path}")]
    InvalidShape { shape: &'static str, path: String },

    #[error("translated Placeholder at {path} cannot store template parameters or fallback text")]
    UnrepresentablePlaceholder { path: String },

    #[error("custom (pluggable) widget at {path} could not be decoded: {source}")]
    PluggableDecodeFailed {
        path: String,
        #[source]
        source: mxrs_pluggable::PluggableError,
    },

    #[error("custom (pluggable) widget at {path} could not be encoded: {source}")]
    PluggableEncodeFailed {
        path: String,
        #[source]
        source: mxrs_pluggable::PluggableError,
    },

    #[error("{0}")]
    Other(String),

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("fragment store error: {0}")]
    FragmentStore(#[from] mxrs_fragment_store::FragmentStoreError),

    #[error("schema JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, FormsError>;
