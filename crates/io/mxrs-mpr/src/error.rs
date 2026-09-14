use thiserror::Error;

#[derive(Debug, Error)]
pub enum MprError {
    #[error("{0}: not a valid SQLite file")]
    NotSqlite(String),

    #[error("{0}: Unit table missing — not a valid .mpr file")]
    MissingUnitTable(String),

    #[error("opened in read-only mode")]
    ReadOnly,

    #[error(
        "MPR handle requires transaction recovery; drop it and reopen writable before further queries or writes"
    )]
    RecoveryRequired,

    #[error("{operation} is not allowed inside a managed MPR transaction")]
    ManagedTransactionOperation { operation: &'static str },

    #[error("{0}")]
    IncompletePackage(String),

    #[error("nested MPR transactions are not supported")]
    NestedTransaction,

    #[error(
        "transaction committed, but file-journal cleanup failed ({0}); reopen writable to finish cleanup before retrying any writes"
    )]
    CommittedTransactionCleanup(Box<MprError>),

    #[error(
        "transaction failed ({original}); recovery also failed ({recovery}); preserve the journal and reopen writable to recover"
    )]
    TransactionRecovery {
        original: Box<MprError>,
        recovery: Box<MprError>,
    },

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
