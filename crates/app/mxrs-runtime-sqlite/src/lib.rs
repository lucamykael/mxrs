//! Durable SQLite storage for the runtime's committed object state.
//!
//! The adapter writes a complete snapshot inside one SQLite transaction and
//! validates every row before replacing live state. Transient entities never
//! cross this boundary. An application identifier and exact versioned schema
//! prevent accidentally adopting another application's SQLite file. Only the
//! exact, unversioned runtime schema shipped before version 1 is migrated.
//!
//! Snapshot generations prevent a second connection from overwriting a newer
//! commit. Published snapshots must be loaded before saving; conflicts require an
//! explicit reload/reapply by the caller, never a silent last-writer-wins retry.
//! This remains a snapshot adapter, not an incremental query engine or an
//! automatic persistence transaction around runtime action execution.

pub mod relational;
pub mod schema;

pub use relational::RelationalRuntimeStore;
pub use schema::{MigrationResult, RemovedItem, RuntimeSchema};

use std::collections::BTreeMap;
use std::path::Path;

use mxrs_model::AttributeType;
use mxrs_runtime::{ObjectValue, PersistentObjectValidator, RuntimeError, Store};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

pub(crate) const APPLICATION_ID: i64 = 0x4d58_5253;
const SCHEMA_VERSION: i64 = 1;
const OBJECTS_SCHEMA: &str = "CREATE TABLE mxrs_runtime_objects (
    entity TEXT NOT NULL,
    object_id TEXT NOT NULL,
    members_json TEXT NOT NULL,
    PRIMARY KEY (entity, object_id)
) WITHOUT ROWID";
const METADATA_SCHEMA: &str = "CREATE TABLE mxrs_runtime_metadata (
    singleton INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
    generation INTEGER NOT NULL CHECK (generation >= 0)
) WITHOUT ROWID";

#[derive(Debug, thiserror::Error)]
pub enum SqliteRuntimeError {
    #[error("SQLite runtime storage failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A question the schema itself answers — an unknown or ambiguous name —
    /// which is invalid regardless of the storage behind it.
    #[error("{0}")]
    Schema(#[from] mxrs_relational_schema::SchemaError),
    #[error("persistent members for {entity}/{id} are invalid JSON: {source}")]
    Json {
        entity: String,
        id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(
        "refusing unrelated SQLite database (application_id={application_id}, user_version={version})"
    )]
    UnrelatedDatabase { application_id: i64, version: i64 },
    #[error("unsupported runtime SQLite schema version {0}")]
    UnsupportedVersion(i64),
    #[error("runtime SQLite schema is invalid: {0}")]
    InvalidSchema(String),
    #[error("load the existing durable snapshot successfully before saving")]
    SnapshotNotLoaded,
    #[error(
        "runtime snapshot changed concurrently (loaded generation {expected}, current {actual}); reload and reapply changes"
    )]
    Conflict { expected: i64, actual: i64 },
    #[error("runtime snapshot generation is exhausted")]
    GenerationExhausted,
    /// A schema evolution that would drop data, refused unless explicitly
    /// allowed. Ports mxrb's `UnsafeSchemaMigrationError`.
    #[error("{message}")]
    UnsafeMigration {
        message: String,
        changes: Vec<schema::RemovedItem>,
    },
    /// A member whose JSON value cannot be stored in its column without
    /// losing information. Refused rather than written as NULL.
    #[error("{entity}/{member} holds {value}, which does not fit its {kind:?} column without loss")]
    LossyMember {
        entity: String,
        member: String,
        value: String,
        kind: AttributeType,
    },
}

pub type Result<T> = std::result::Result<T, SqliteRuntimeError>;

pub struct SqliteRuntimeStore {
    connection: Connection,
    loaded_generation: Option<i64>,
}

impl SqliteRuntimeStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut connection: Connection) -> Result<Self> {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (application_id, version) = identity(&transaction)?;
        let schema = schema_objects(&transaction)?;
        match (application_id, version) {
            (0, 0) if schema.is_empty() || is_legacy_schema(&schema) => {
                if schema.is_empty() {
                    transaction.execute_batch(OBJECTS_SCHEMA)?;
                } else {
                    read_objects(&transaction)?;
                }
                transaction.execute_batch(METADATA_SCHEMA)?;
                transaction.execute("INSERT INTO mxrs_runtime_metadata VALUES (1, 0)", [])?;
                transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
                transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            (APPLICATION_ID, SCHEMA_VERSION) => {}
            (APPLICATION_ID, version) => {
                return Err(SqliteRuntimeError::UnsupportedVersion(version));
            }
            _ => {
                return Err(SqliteRuntimeError::UnrelatedDatabase {
                    application_id,
                    version,
                });
            }
        }
        let generation = validate_schema(&transaction)?;
        let empty = transaction.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM mxrs_runtime_objects)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        transaction.commit()?;
        Ok(Self {
            connection,
            loaded_generation: (generation == 0 && empty).then_some(0),
        })
    }

    /// Replaces the durable snapshot atomically. A process crash can expose
    /// either the previous snapshot or the complete new one, never a prefix.
    /// Members are already JSON values with string keys; serialization has no
    /// user-defined serializers, non-string map keys or other fallible values.
    pub fn save(&mut self, store: &Store) -> Result<usize> {
        let expected = self
            .loaded_generation
            .ok_or(SqliteRuntimeError::SnapshotNotLoaded)?;
        let objects = store.persistent_objects();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actual = validate_schema(&transaction)?;
        if actual != expected {
            return Err(SqliteRuntimeError::Conflict { expected, actual });
        }
        let next_generation = actual
            .checked_add(1)
            .ok_or(SqliteRuntimeError::GenerationExhausted)?;
        transaction.execute("DELETE FROM mxrs_runtime_objects", [])?;
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO mxrs_runtime_objects (entity, object_id, members_json)
                 VALUES (?1, ?2, ?3)",
            )?;
            for object in &objects {
                let members = serde_json::to_string(&object.members)
                    .expect("string-keyed maps of JSON values are serializable");
                insert.execute(params![object.entity, object.id, members])?;
            }
        }
        transaction.execute(
            "UPDATE mxrs_runtime_metadata SET generation = ?1 WHERE singleton = 1",
            [next_generation],
        )?;
        transaction.commit()?;
        self.loaded_generation = Some(next_generation);
        Ok(objects.len())
    }

    /// Validates the full durable snapshot before atomically installing it in
    /// the runtime store.
    pub fn load(&mut self, store: &mut Store) -> Result<usize> {
        let transaction = self.connection.transaction()?;
        let generation = validate_schema(&transaction)?;
        let objects = read_objects(&transaction)?;
        let count = objects.len();
        transaction.commit()?;
        store.restore_persistent(objects)?;
        self.loaded_generation = Some(generation);
        Ok(count)
    }

    pub fn loaded_generation(&self) -> Option<i64> {
        self.loaded_generation
    }

    pub fn object_count(&self) -> Result<usize> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM mxrs_runtime_objects", [], |row| {
                row.get(0)
            })?)
    }

    pub fn contains(&self, entity: &str, id: &str) -> Result<bool> {
        Ok(self
            .connection
            .query_row(
                "SELECT 1 FROM mxrs_runtime_objects WHERE entity = ?1 AND object_id = ?2",
                params![entity, id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

fn identity(connection: &Connection) -> Result<(i64, i64)> {
    Ok((
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?,
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?,
    ))
}

pub(crate) fn schema_objects(connection: &Connection) -> Result<BTreeMap<String, String>> {
    let mut statement = connection.prepare(
        "SELECT name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut schema = BTreeMap::new();
    for row in rows {
        let (name, sql) = row?;
        if schema.insert(name.clone(), sql).is_some() {
            return Err(SqliteRuntimeError::InvalidSchema(format!(
                "ambiguous schema object name {name}"
            )));
        }
    }
    Ok(schema)
}

/// Whitespace may separate keywords: `TEXTNOTNULL` is a nullable SQLite type,
/// not `TEXT NOT NULL`. Quoted content is kept verbatim, never case-folded.
fn schema_tokens(sql: &str) -> Vec<String> {
    let mut characters = sql.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(character) = characters.next() {
        if character.is_ascii_whitespace() {
            continue;
        }
        let mut token = character.to_string();
        if character.is_ascii_alphanumeric() || character == '_' {
            while characters
                .peek()
                .is_some_and(|next| next.is_ascii_alphanumeric() || *next == '_')
            {
                token.push(characters.next().unwrap());
            }
            token.make_ascii_uppercase();
        } else if matches!(character, '\'' | '"' | '`' | '[') {
            let closing = if character == '[' { ']' } else { character };
            while let Some(next) = characters.next() {
                token.push(next);
                if next == closing {
                    if characters.peek() == Some(&closing) {
                        token.push(characters.next().unwrap());
                    } else {
                        break;
                    }
                }
            }
        }
        tokens.push(token);
    }
    tokens
}

pub(crate) fn is_legacy_schema(schema: &BTreeMap<String, String>) -> bool {
    schema.len() == 1
        && schema
            .get("mxrs_runtime_objects")
            .is_some_and(|sql| schema_tokens(sql) == schema_tokens(OBJECTS_SCHEMA))
}

fn validate_schema(connection: &Connection) -> Result<i64> {
    let (application_id, version) = identity(connection)?;
    if application_id != APPLICATION_ID {
        return Err(SqliteRuntimeError::UnrelatedDatabase {
            application_id,
            version,
        });
    }
    if version != SCHEMA_VERSION {
        return Err(SqliteRuntimeError::UnsupportedVersion(version));
    }
    let mut schema = schema_objects(connection)?;
    let metadata = schema.remove("mxrs_runtime_metadata");
    if metadata.as_deref().map(schema_tokens) != Some(schema_tokens(METADATA_SCHEMA))
        || !is_legacy_schema(&schema)
    {
        return Err(SqliteRuntimeError::InvalidSchema(
            "unexpected tables, indexes, triggers or column definitions".into(),
        ));
    }
    let (rows, singleton, generation) = connection.query_row(
        "SELECT COUNT(*), MIN(singleton), MIN(generation) FROM mxrs_runtime_metadata",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        },
    )?;
    match (rows, singleton, generation) {
        (1, Some(1), Some(generation)) if generation >= 0 => Ok(generation),
        _ => Err(SqliteRuntimeError::InvalidSchema(
            "missing or invalid snapshot generation".into(),
        )),
    }
}

fn read_objects(connection: &Connection) -> Result<Vec<ObjectValue>> {
    let mut statement = connection.prepare("SELECT entity, object_id, members_json FROM mxrs_runtime_objects ORDER BY entity, object_id")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut objects = Vec::new();
    let mut validator = PersistentObjectValidator::default();
    for row in rows {
        let (entity, id, members) = row?;
        let members =
            serde_json::from_str(&members).map_err(|source| SqliteRuntimeError::Json {
                entity: entity.clone(),
                id: id.clone(),
                source,
            })?;
        let object = ObjectValue {
            entity,
            id,
            members,
        };
        validator.validate(&object)?;
        objects.push(object);
    }
    Ok(objects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_runtime::StoreSchema;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn schema() -> StoreSchema {
        StoreSchema::default()
            .entity("Sales.Order", BTreeMap::new(), false)
            .entity("Session.Filter", BTreeMap::new(), true)
    }

    #[test]
    fn committed_objects_survive_a_sqlite_round_trip_but_transients_do_not() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("runtime.sqlite");
        let mut source = Store::new(schema());
        let order = source.create("Sales.Order").unwrap();
        source
            .set_member("Sales.Order", &order.id, "Number", json!("A-1"))
            .unwrap();
        source.commit("Sales.Order", &order.id).unwrap();
        source.create("Session.Filter").unwrap();

        let mut durable = SqliteRuntimeStore::open(&database).unwrap();
        assert_eq!(durable.save(&source).unwrap(), 1);
        assert!(durable.contains("Sales.Order", &order.id).unwrap());

        let mut restored = Store::new(schema());
        assert_eq!(durable.load(&mut restored).unwrap(), 1);
        assert_eq!(
            restored
                .find("Sales.Order", &order.id)
                .unwrap()
                .unwrap()
                .members["Number"],
            "A-1"
        );
        assert!(restored.retrieve("Session.Filter").unwrap().is_empty());
    }

    #[test]
    fn loading_an_unknown_entity_fails_without_replacing_live_state() {
        let mut durable = SqliteRuntimeStore::in_memory().unwrap();
        let mut source =
            Store::new(StoreSchema::default().entity("Legacy.Unknown", BTreeMap::new(), false));
        let legacy = source.create("Legacy.Unknown").unwrap();
        source.commit("Legacy.Unknown", &legacy.id).unwrap();
        durable.save(&source).unwrap();

        let mut live = Store::new(schema());
        let order = live.create("Sales.Order").unwrap();
        live.commit("Sales.Order", &order.id).unwrap();
        assert!(durable.load(&mut live).is_err());
        assert!(live.find("Sales.Order", &order.id).unwrap().is_some());
    }

    fn committed_order(number: i64) -> (Store, String) {
        let mut store = Store::new(schema());
        let order = store.create("Sales.Order").unwrap();
        store
            .set_member("Sales.Order", &order.id, "Number", json!(number))
            .unwrap();
        store.commit("Sales.Order", &order.id).unwrap();
        (store, order.id)
    }

    #[test]
    fn unrelated_databases_are_rejected_without_changing_any_file_bytes() {
        for setup in [
            "CREATE TABLE customer_records (secret TEXT); INSERT INTO customer_records VALUES ('keep');",
            "CREATE TABLE sqliteXprivate (secret TEXT); INSERT INTO sqliteXprivate VALUES ('keep');",
            "PRAGMA application_id = 1234;",
            "PRAGMA user_version = 12;",
            "CREATE TABLE mxrs_runtime_objects (entity TEXT);",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("unrelated.sqlite");
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(setup).unwrap();
            drop(connection);
            let before = std::fs::read(&path).unwrap();
            assert!(matches!(
                SqliteRuntimeStore::open(&path),
                Err(SqliteRuntimeError::UnrelatedDatabase { .. })
            ));
            assert_eq!(
                std::fs::read(path).unwrap(),
                before,
                "modified database for {setup}"
            );
        }
    }

    #[test]
    fn schema_comparison_preserves_keyword_boundaries_and_quoted_content() {
        assert_eq!(
            schema_tokens("_custom NOT NULL"),
            vec!["_CUSTOM", "NOT", "NULL"]
        );
        assert_eq!(
            schema_tokens("  text\n NOT\tNULL "),
            vec!["TEXT", "NOT", "NULL"]
        );
        assert_ne!(schema_tokens("TEXTNOTNULL"), schema_tokens("TEXT NOT NULL"));
        assert_eq!(
            schema_tokens("CHECK ( generation >= 0 )"),
            schema_tokens("check(generation>=0)")
        );
        assert_ne!(schema_tokens("'a b'"), schema_tokens("'ab'"));
        assert_ne!(schema_tokens("'lower'"), schema_tokens("'LOWER'"));
        assert_eq!(
            schema_tokens("'a'' b' \"a\"\" b\" `a`` b` [a b]"),
            vec!["'a'' b'", "\"a\"\" b\"", "`a`` b`", "[a b]"]
        );
        assert_ne!(
            schema_tokens("'unterminated"),
            schema_tokens("unterminated")
        );
    }

    #[test]
    fn nullable_concatenated_type_names_cannot_impersonate_legacy_or_owned_schemas() {
        for owned in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("malformed.sqlite");
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    &OBJECTS_SCHEMA
                        .replace("members_json TEXT NOT NULL", "members_json TEXTNOTNULL"),
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO mxrs_runtime_objects VALUES ('Sales.Order', 'keep', '{}')",
                    [],
                )
                .unwrap();
            let not_null: bool = connection.query_row("SELECT \"notnull\" FROM pragma_table_info('mxrs_runtime_objects') WHERE name = 'members_json'", [], |row| row.get(0)).unwrap();
            assert!(!not_null);
            if owned {
                connection.execute_batch(METADATA_SCHEMA).unwrap();
                connection
                    .execute("INSERT INTO mxrs_runtime_metadata VALUES (1, 0)", [])
                    .unwrap();
                connection
                    .pragma_update(None, "application_id", APPLICATION_ID)
                    .unwrap();
                connection
                    .pragma_update(None, "user_version", SCHEMA_VERSION)
                    .unwrap();
            }
            drop(connection);
            let before = std::fs::read(&path).unwrap();
            let result = SqliteRuntimeStore::open(&path);
            if owned {
                assert!(matches!(result, Err(SqliteRuntimeError::InvalidSchema(_))));
            } else {
                assert!(matches!(
                    result,
                    Err(SqliteRuntimeError::UnrelatedDatabase { .. })
                ));
            }
            assert_eq!(std::fs::read(path).unwrap(), before);
        }
    }

    #[test]
    fn exact_legacy_runtime_databases_migrate_atomically_and_require_loading_existing_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&OBJECTS_SCHEMA.replacen(
                "CREATE TABLE",
                "CREATE TABLE IF NOT EXISTS",
                1,
            ))
            .unwrap();
        connection.execute("INSERT INTO mxrs_runtime_objects VALUES ('Sales.Order', 'legacy', '{\"Number\": 42}')", []).unwrap();
        drop(connection);
        let mut durable = SqliteRuntimeStore::open(&path).unwrap();
        assert_eq!(
            identity(&durable.connection).unwrap(),
            (APPLICATION_ID, SCHEMA_VERSION)
        );
        assert_eq!(durable.loaded_generation(), None);
        assert_eq!(durable.object_count().unwrap(), 1);
        let mut store = Store::new(schema());
        assert!(matches!(
            durable.save(&store),
            Err(SqliteRuntimeError::SnapshotNotLoaded)
        ));
        assert_eq!(durable.load(&mut store).unwrap(), 1);
        assert_eq!(
            store
                .find("Sales.Order", "legacy")
                .unwrap()
                .unwrap()
                .members["Number"],
            42
        );
        assert_eq!(durable.loaded_generation(), Some(0));
        durable.save(&store).unwrap();
        assert_eq!(durable.loaded_generation(), Some(1));
        drop(durable);
        let mut reopened = SqliteRuntimeStore::open(&path).unwrap();
        assert_eq!(reopened.loaded_generation(), None);
        assert!(matches!(
            reopened.save(&store),
            Err(SqliteRuntimeError::SnapshotNotLoaded)
        ));
        reopened.load(&mut store).unwrap();
        assert_eq!(reopened.loaded_generation(), Some(1));
    }

    #[test]
    fn corrupt_legacy_rows_cannot_partially_migrate_the_database() {
        for (entity, id, members) in [
            ("Sales.Order", "id", "not JSON"),
            ("Sales.Order", "id", "[]"),
            ("", "id", "{}"),
            ("Sales.Order", "", "{}"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("legacy.sqlite");
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(OBJECTS_SCHEMA).unwrap();
            connection
                .execute(
                    "INSERT INTO mxrs_runtime_objects VALUES (?1, ?2, ?3)",
                    params![entity, id, members],
                )
                .unwrap();
            drop(connection);
            let before = std::fs::read(&path).unwrap();
            assert!(SqliteRuntimeStore::open(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            let connection = Connection::open(path).unwrap();
            assert_eq!(identity(&connection).unwrap(), (0, 0));
            assert!(is_legacy_schema(&schema_objects(&connection).unwrap()));
        }
    }

    #[test]
    fn malformed_names_and_globally_duplicate_ids_cannot_be_migrated_into_owned_databases() {
        for rows in [
            vec![(" ", "id", "{}")],
            vec![("Sales.\nOrder", "id", "{}")],
            vec![("Sales.Order", " \t", "{}")],
            vec![("Sales.Order", "id\n", "{}")],
            vec![("Sales.Order", "id", r#"{"":null}"#)],
            vec![("Sales.Order", "id", r#"{" \t":null}"#)],
            vec![("Sales.Order", "id", r#"{"Name\n":null}"#)],
            vec![
                ("Sales.Order", "duplicate", "{}"),
                ("Sales.Other", "duplicate", "{}"),
            ],
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("legacy.sqlite");
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(OBJECTS_SCHEMA).unwrap();
            for (entity, id, members) in rows {
                connection
                    .execute(
                        "INSERT INTO mxrs_runtime_objects VALUES (?1, ?2, ?3)",
                        params![entity, id, members],
                    )
                    .unwrap();
            }
            drop(connection);
            let before = std::fs::read(&path).unwrap();
            assert!(matches!(
                SqliteRuntimeStore::open(&path),
                Err(SqliteRuntimeError::Runtime(_))
            ));
            assert_eq!(std::fs::read(&path).unwrap(), before);
            let connection = Connection::open(path).unwrap();
            assert_eq!(identity(&connection).unwrap(), (0, 0));
            assert!(is_legacy_schema(&schema_objects(&connection).unwrap()));
        }
    }

    #[test]
    fn migration_failure_while_creating_metadata_rolls_back_schema_and_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(OBJECTS_SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO mxrs_runtime_objects VALUES ('Sales.Order', 'legacy', '{}')",
                [],
            )
            .unwrap();
        let pages: i64 = connection
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap();
        connection
            .pragma_update(None, "max_page_count", pages)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            SqliteRuntimeStore::from_connection(connection),
            Err(SqliteRuntimeError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::DiskFull,
                    ..
                },
                _
            )))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let connection = Connection::open(path).unwrap();
        assert_eq!(identity(&connection).unwrap(), (0, 0));
        assert!(is_legacy_schema(&schema_objects(&connection).unwrap()));
    }

    #[test]
    fn a_driver_denial_after_initial_schema_creation_rolls_back_the_entire_initialization() {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("empty.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .authorizer(Some(|context: AuthContext<'_>| match context.action {
                AuthAction::Read {
                    table_name: "mxrs_runtime_objects",
                    column_name: "",
                    ..
                } => Authorization::Deny,
                _ => Authorization::Allow,
            }))
            .unwrap();
        assert!(matches!(
            SqliteRuntimeStore::from_connection(connection),
            Err(SqliteRuntimeError::Sqlite(_))
        ));
        let connection = Connection::open(path).unwrap();
        assert_eq!(identity(&connection).unwrap(), (0, 0));
        assert!(schema_objects(&connection).unwrap().is_empty());
    }

    #[test]
    fn corrupt_schema_and_generation_types_return_errors_without_replacing_live_state() {
        let mut durable = SqliteRuntimeStore::in_memory().unwrap();
        let (mut live, id) = committed_order(1);
        durable.save(&live).unwrap();
        durable
            .connection
            .execute(
                "UPDATE mxrs_runtime_metadata SET generation = 'invalid'",
                [],
            )
            .unwrap();
        assert!(matches!(
            durable.load(&mut live),
            Err(SqliteRuntimeError::Sqlite(_))
        ));
        assert!(live.find("Sales.Order", &id).unwrap().is_some());
        assert_eq!(durable.loaded_generation(), Some(1));
        durable
            .connection
            .execute_batch("DROP TABLE mxrs_runtime_objects")
            .unwrap();
        assert!(matches!(
            durable.object_count(),
            Err(SqliteRuntimeError::Sqlite(_))
        ));
        assert!(matches!(
            durable.contains("Sales.Order", &id),
            Err(SqliteRuntimeError::Sqlite(_))
        ));
    }

    #[test]
    fn unsupported_owned_versions_and_modified_owned_schemas_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime.sqlite");
        let durable = SqliteRuntimeStore::open(&path).unwrap();
        durable
            .connection
            .pragma_update(None, "user_version", 2)
            .unwrap();
        drop(durable);
        assert!(matches!(
            SqliteRuntimeStore::open(&path),
            Err(SqliteRuntimeError::UnsupportedVersion(2))
        ));
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute_batch("CREATE INDEX custom_index ON mxrs_runtime_objects (members_json)")
            .unwrap();
        drop(connection);
        assert!(matches!(
            SqliteRuntimeStore::open(&path),
            Err(SqliteRuntimeError::InvalidSchema(_))
        ));
    }

    #[test]
    fn stale_writers_cannot_delete_a_newer_snapshot_and_may_reload_to_reapply_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime.sqlite");
        let mut first = SqliteRuntimeStore::open(&path).unwrap();
        let mut second = SqliteRuntimeStore::open(&path).unwrap();
        let (first_store, id) = committed_order(1);
        first.save(&first_store).unwrap();
        let mut stale_store = Store::new(schema());
        assert!(matches!(
            second.save(&stale_store),
            Err(SqliteRuntimeError::Conflict {
                expected: 0,
                actual: 1
            })
        ));
        assert_eq!(second.loaded_generation(), Some(0));
        assert!(second.contains("Sales.Order", &id).unwrap());
        second.load(&mut stale_store).unwrap();
        assert_eq!(second.loaded_generation(), Some(1));
        stale_store
            .set_member("Sales.Order", &id, "Number", json!(2))
            .unwrap();
        stale_store.commit("Sales.Order", &id).unwrap();
        second.save(&stale_store).unwrap();
        assert!(matches!(
            first.save(&first_store),
            Err(SqliteRuntimeError::Conflict {
                expected: 1,
                actual: 2
            })
        ));
        let mut restored = Store::new(schema());
        first.load(&mut restored).unwrap();
        assert_eq!(
            restored.find("Sales.Order", &id).unwrap().unwrap().members["Number"],
            2
        );
    }

    #[test]
    fn loading_bad_json_or_transient_rows_preserves_both_live_state_and_observed_generation() {
        for (entity, id, members) in [
            ("Sales.Order", "bad", "{invalid"),
            ("Sales.Order", "bad", "null"),
            ("Session.Filter", "bad", "{}"),
            ("Sales.Order", "", "{}"),
        ] {
            let mut durable = SqliteRuntimeStore::in_memory().unwrap();
            let (mut live, id_to_preserve) = committed_order(7);
            durable.save(&live).unwrap();
            durable
                .connection
                .execute(
                    "INSERT INTO mxrs_runtime_objects VALUES (?1, ?2, ?3)",
                    params![entity, id, members],
                )
                .unwrap();
            durable
                .connection
                .execute("UPDATE mxrs_runtime_metadata SET generation = 2", [])
                .unwrap();
            assert!(durable.load(&mut live).is_err());
            assert_eq!(live.persistent_objects().len(), 1);
            assert!(live.find("Sales.Order", &id_to_preserve).unwrap().is_some());
            assert_eq!(durable.loaded_generation(), Some(1));
            assert!(matches!(
                durable.save(&live),
                Err(SqliteRuntimeError::Conflict {
                    expected: 1,
                    actual: 2
                })
            ));
        }
    }

    #[test]
    fn saving_an_empty_snapshot_publishes_deletions_and_advances_the_generation() {
        let mut durable = SqliteRuntimeStore::in_memory().unwrap();
        let (store, id) = committed_order(1);
        durable.save(&store).unwrap();
        assert_eq!(durable.save(&Store::new(schema())).unwrap(), 0);
        assert_eq!(durable.object_count().unwrap(), 0);
        assert!(!durable.contains("Sales.Order", &id).unwrap());
        assert_eq!(durable.loaded_generation(), Some(2));
    }

    #[test]
    fn read_only_failures_preserve_durable_objects_and_the_observed_generation() {
        let mut durable = SqliteRuntimeStore::in_memory().unwrap();
        let (store, id) = committed_order(1);
        durable.save(&store).unwrap();
        durable
            .connection
            .pragma_update(None, "query_only", true)
            .unwrap();
        assert!(matches!(
            durable.save(&Store::new(schema())),
            Err(SqliteRuntimeError::Sqlite(_))
        ));
        assert!(durable.contains("Sales.Order", &id).unwrap());
        assert_eq!(durable.loaded_generation(), Some(1));
        assert_eq!(validate_schema(&durable.connection).unwrap(), 1);
        durable
            .connection
            .pragma_update(None, "query_only", false)
            .unwrap();
        durable.save(&store).unwrap();
        assert_eq!(durable.loaded_generation(), Some(2));
    }

    #[test]
    fn running_out_of_sqlite_pages_after_deleting_the_old_snapshot_rolls_back_every_change() {
        let mut durable = SqliteRuntimeStore::in_memory().unwrap();
        let (mut store, id) = committed_order(1);
        durable.save(&store).unwrap();
        let page_count: i64 = durable
            .connection
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap();
        durable
            .connection
            .pragma_update(None, "max_page_count", page_count)
            .unwrap();
        store
            .set_member("Sales.Order", &id, "LargeValue", json!("x".repeat(100_000)))
            .unwrap();
        store.commit("Sales.Order", &id).unwrap();
        assert!(matches!(
            durable.save(&store),
            Err(SqliteRuntimeError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::DiskFull,
                    ..
                },
                _
            )))
        ));
        assert_eq!(durable.loaded_generation(), Some(1));
        assert_eq!(validate_schema(&durable.connection).unwrap(), 1);
        let objects = read_objects(&durable.connection).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].id, id);
        assert!(!objects[0].members.contains_key("LargeValue"));
        durable
            .connection
            .pragma_update(None, "max_page_count", 1000)
            .unwrap();
        durable.save(&store).unwrap();
        assert_eq!(durable.loaded_generation(), Some(2));
    }

    #[test]
    fn generation_exhaustion_cannot_wrap_or_replace_durable_objects() {
        let mut durable = SqliteRuntimeStore::in_memory().unwrap();
        let (mut store, id) = committed_order(1);
        durable.save(&store).unwrap();
        durable
            .connection
            .execute(
                "UPDATE mxrs_runtime_metadata SET generation = ?1",
                [i64::MAX],
            )
            .unwrap();
        durable.load(&mut store).unwrap();
        assert!(matches!(
            durable.save(&Store::new(schema())),
            Err(SqliteRuntimeError::GenerationExhausted)
        ));
        assert!(durable.contains("Sales.Order", &id).unwrap());
        assert_eq!(durable.loaded_generation(), Some(i64::MAX));
    }

    #[test]
    fn changed_ownership_missing_generation_and_schema_triggers_are_rejected_before_writes() {
        for change in [
            "PRAGMA application_id = 1",
            "PRAGMA user_version = 2",
            "DELETE FROM mxrs_runtime_metadata",
            "PRAGMA ignore_check_constraints = ON; UPDATE mxrs_runtime_metadata SET generation = -1",
            "PRAGMA ignore_check_constraints = ON; INSERT INTO mxrs_runtime_metadata VALUES (2, 0)",
            "CREATE TRIGGER extra AFTER DELETE ON mxrs_runtime_objects BEGIN SELECT 1; END",
            "CREATE TRIGGER mxrs_runtime_objects AFTER DELETE ON mxrs_runtime_objects BEGIN SELECT 1; END",
            "DROP TABLE mxrs_runtime_metadata",
        ] {
            let mut durable = SqliteRuntimeStore::in_memory().unwrap();
            let (store, id) = committed_order(1);
            durable.save(&store).unwrap();
            durable.connection.execute_batch(change).unwrap();
            assert!(
                durable.save(&Store::new(schema())).is_err(),
                "accepted {change}"
            );
            assert!(durable.contains("Sales.Order", &id).unwrap());
            assert_eq!(durable.loaded_generation(), Some(1));
        }
    }

    #[test]
    fn concurrent_snapshot_loads_pair_objects_with_their_exact_persisted_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime.sqlite");
        let mut writer = SqliteRuntimeStore::open(&path).unwrap();
        writer
            .connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        let (mut store, id) = committed_order(1);
        writer.save(&store).unwrap();
        let mut reader = SqliteRuntimeStore::open(path).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let writer_barrier = barrier.clone();
        let writer_id = id.clone();
        let writer_thread = std::thread::spawn(move || {
            for generation in 2..=101 {
                writer_barrier.wait();
                store
                    .set_member("Sales.Order", &writer_id, "Number", json!(generation))
                    .unwrap();
                store.commit("Sales.Order", &writer_id).unwrap();
                writer.save(&store).unwrap();
            }
        });
        let mut live = Store::new(schema());
        for _ in 2..=101 {
            barrier.wait();
            reader.load(&mut live).unwrap();
            let number = live.find("Sales.Order", &id).unwrap().unwrap().members["Number"]
                .as_i64()
                .unwrap();
            assert_eq!(reader.loaded_generation(), Some(number));
        }
        writer_thread.join().unwrap();
        reader.load(&mut live).unwrap();
        assert_eq!(reader.loaded_generation(), Some(101));
    }

    #[test]
    fn sqlite_blob_values_in_text_columns_cannot_replace_the_live_snapshot() {
        for column in ["entity", "object_id", "members_json"] {
            let mut durable = SqliteRuntimeStore::in_memory().unwrap();
            let (mut store, _) = committed_order(7);
            durable.save(&store).unwrap();
            let before = store.persistent_objects();
            durable
                .connection
                .execute(
                    &format!("UPDATE mxrs_runtime_objects SET {column} = CAST('invalid' AS BLOB)"),
                    [],
                )
                .unwrap();
            assert!(
                matches!(
                    durable.load(&mut store),
                    Err(SqliteRuntimeError::Sqlite(
                        rusqlite::Error::InvalidColumnType(..)
                    ))
                ),
                "accepted BLOB in {column}"
            );
            assert_eq!(store.persistent_objects(), before);
            assert_eq!(durable.loaded_generation(), Some(1));
            assert_eq!(validate_schema(&durable.connection).unwrap(), 1);
        }
    }

    #[test]
    fn invalid_database_paths_report_driver_errors_without_creating_parent_directories() {
        let directory = tempfile::tempdir().unwrap();
        let missing_parent = directory.path().join("missing/runtime.sqlite");
        assert!(matches!(
            SqliteRuntimeStore::open(&missing_parent),
            Err(SqliteRuntimeError::Sqlite(_))
        ));
        assert!(!directory.path().join("missing").exists());
        assert!(matches!(
            SqliteRuntimeStore::open(directory.path()),
            Err(SqliteRuntimeError::Sqlite(_))
        ));
        assert!(directory.path().is_dir());
    }

    #[test]
    fn driver_denials_while_loading_leave_live_state_and_observed_generation_unchanged() {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};

        // rusqlite 0.40 maps SQLite's COMMIT operation to Unknown; BEGIN and
        // ROLLBACK have distinct variants and must remain independently usable.
        for phase in [
            "begin",
            "application_id",
            "user_version",
            "schema",
            "objects",
            "commit",
        ] {
            let mut durable = SqliteRuntimeStore::in_memory().unwrap();
            let (mut store, _) = committed_order(7);
            durable.save(&store).unwrap();
            let before = store.persistent_objects();
            durable
                .connection
                .authorizer(Some(move |context: AuthContext<'_>| {
                    let denied = matches!(
                        (phase, context.action),
                        (
                            "begin",
                            AuthAction::Transaction {
                                operation: TransactionOperation::Begin
                            }
                        ) | (
                            "application_id",
                            AuthAction::Pragma {
                                pragma_name: "application_id",
                                pragma_value: None
                            }
                        ) | (
                            "user_version",
                            AuthAction::Pragma {
                                pragma_name: "user_version",
                                pragma_value: None
                            }
                        ) | (
                            "schema",
                            AuthAction::Read {
                                table_name: "sqlite_master" | "sqlite_schema",
                                ..
                            }
                        ) | (
                            "objects",
                            AuthAction::Read {
                                table_name: "mxrs_runtime_objects",
                                ..
                            }
                        ) | (
                            "commit",
                            AuthAction::Transaction {
                                operation: TransactionOperation::Unknown
                            }
                        )
                    );
                    if denied {
                        Authorization::Deny
                    } else {
                        Authorization::Allow
                    }
                }))
                .unwrap();
            let result = durable.load(&mut store);
            durable
                .connection
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
                .unwrap();
            assert!(
                matches!(result, Err(SqliteRuntimeError::Sqlite(_))),
                "accepted denied {phase}"
            );
            assert!(
                durable.connection.is_autocommit(),
                "left a transaction open after {phase}"
            );
            assert_eq!(store.persistent_objects(), before);
            assert_eq!(durable.loaded_generation(), Some(1));
            assert_eq!(read_objects(&durable.connection).unwrap(), before);
        }
    }

    #[test]
    fn driver_denials_after_deleting_old_rows_roll_back_data_and_generation_together() {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};

        for phase in ["insert", "generation", "commit"] {
            let mut durable = SqliteRuntimeStore::in_memory().unwrap();
            let (mut store, id) = committed_order(7);
            durable.save(&store).unwrap();
            let before = read_objects(&durable.connection).unwrap();
            store
                .set_member("Sales.Order", &id, "Number", json!(8))
                .unwrap();
            store.commit("Sales.Order", &id).unwrap();
            durable
                .connection
                .authorizer(Some(move |context: AuthContext<'_>| {
                    let denied = matches!(
                        (phase, context.action),
                        (
                            "insert",
                            AuthAction::Insert {
                                table_name: "mxrs_runtime_objects"
                            }
                        ) | (
                            "generation",
                            AuthAction::Update {
                                table_name: "mxrs_runtime_metadata",
                                column_name: "generation"
                            }
                        ) | (
                            "commit",
                            AuthAction::Transaction {
                                operation: TransactionOperation::Unknown
                            }
                        )
                    );
                    if denied {
                        Authorization::Deny
                    } else {
                        Authorization::Allow
                    }
                }))
                .unwrap();
            let result = durable.save(&store);
            durable
                .connection
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
                .unwrap();
            assert!(
                matches!(result, Err(SqliteRuntimeError::Sqlite(_))),
                "accepted denied {phase}"
            );
            assert!(
                durable.connection.is_autocommit(),
                "left a transaction open after {phase}"
            );
            assert_eq!(durable.loaded_generation(), Some(1));
            assert_eq!(validate_schema(&durable.connection).unwrap(), 1);
            assert_eq!(read_objects(&durable.connection).unwrap(), before);
            durable.save(&store).unwrap();
            assert_eq!(durable.loaded_generation(), Some(2));
        }
    }
}
