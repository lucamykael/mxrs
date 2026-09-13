//! Durable SQLite storage for the runtime's committed object state.
//!
//! The adapter writes a complete snapshot inside one SQLite transaction and
//! validates every row before replacing live state. Transient entities never
//! cross this boundary.

use std::path::Path;

use mxrs_runtime::{ObjectValue, RuntimeError, Store};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

#[derive(Debug, thiserror::Error)]
pub enum SqliteRuntimeError {
    #[error("SQLite runtime storage failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("persistent members for {entity}/{id} are invalid JSON: {source}")]
    Json {
        entity: String,
        id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

pub type Result<T> = std::result::Result<T, SqliteRuntimeError>;

pub struct SqliteRuntimeStore {
    connection: Connection,
}

impl SqliteRuntimeStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS mxrs_runtime_objects (
               entity TEXT NOT NULL,
               object_id TEXT NOT NULL,
               members_json TEXT NOT NULL,
               PRIMARY KEY (entity, object_id)
             ) WITHOUT ROWID;",
        )?;
        Ok(Self { connection })
    }

    /// Replaces the durable snapshot atomically. A process crash can expose
    /// either the previous snapshot or the complete new one, never a prefix.
    pub fn save(&mut self, store: &Store) -> Result<usize> {
        let objects = store.persistent_objects();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM mxrs_runtime_objects", [])?;
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO mxrs_runtime_objects (entity, object_id, members_json)
                 VALUES (?1, ?2, ?3)",
            )?;
            for object in &objects {
                let members = serde_json::to_string(&object.members).map_err(|source| {
                    SqliteRuntimeError::Json {
                        entity: object.entity.clone(),
                        id: object.id.clone(),
                        source,
                    }
                })?;
                insert.execute(params![object.entity, object.id, members])?;
            }
        }
        transaction.commit()?;
        Ok(objects.len())
    }

    /// Validates the full durable snapshot before atomically installing it in
    /// the runtime store.
    pub fn load(&self, store: &mut Store) -> Result<usize> {
        let mut statement = self.connection.prepare(
            "SELECT entity, object_id, members_json
             FROM mxrs_runtime_objects ORDER BY entity, object_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut objects = Vec::new();
        for row in rows {
            let (entity, id, members) = row?;
            let members =
                serde_json::from_str(&members).map_err(|source| SqliteRuntimeError::Json {
                    entity: entity.clone(),
                    id: id.clone(),
                    source,
                })?;
            objects.push(ObjectValue {
                entity,
                id,
                members,
            });
        }
        let count = objects.len();
        store.restore_persistent(objects)?;
        Ok(count)
    }

    pub fn object_count(&self) -> Result<usize> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM mxrs_runtime_objects", [], |row| {
                row.get::<_, i64>(0)
            })?
            .try_into()
            .map_err(|_| RuntimeError::InvalidPersistence("negative object count".into()))?)
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
}
