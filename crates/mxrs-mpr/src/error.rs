use thiserror::Error;

#[derive(Debug, Error)]
pub enum MprError {
    #[error("{0}: not a valid SQLite file")]
    NotSqlite(String),

    #[error("{0}: Unit table missing — not a valid .mpr file")]
    MissingUnitTable(String),

    #[error("opened in read-only mode")]
    ReadOnly,

    #[error("{0}")]
    IncompletePackage(String),

    #[error("nested MPR v2 transactions are not supported")]
    NestedTransaction,

    #[error("unit not found: {0}")]
    UnitNotFound(String),

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("transaction journal error: {0}")]
    Journal(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, MprError>;
