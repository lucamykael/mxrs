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

    #[error("unresolved widget property pointer at {path}")]
    UnresolvedPropertyPointer { path: String },

    #[error(
        "value kind {kind:?} at {path} needs mxrs-forms to expose more of its private codec \
         surface first (see mxrs-pluggable::mpr_codec's module doc) — not a silent gap, a known \
         follow-up"
    )]
    NeedsFormsIntegration { kind: String, path: String },

    #[error("invalid {kind} value {value:?} at {path}: {source}")]
    InvalidPrimitive {
        kind: &'static str,
        value: String,
        path: String,
        #[source]
        source: std::num::ParseIntError,
    },

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("invalid domain-model reference: {0}")]
    InvalidReference(#[from] mxrs_forms_refs::RefError),

    #[error("embedded Forms element decode failed at {path}: {source}")]
    EmbeddedDecodeFailed {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    #[error("expected an embedded Forms element document at {path}, got a non-document value")]
    ExpectedEmbeddedDocument { path: String },
}

pub type Result<T> = std::result::Result<T, PluggableError>;
