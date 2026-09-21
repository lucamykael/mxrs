//! Relational durability for the runtime store.
//!
//! Where the original adapter wrote one JSON snapshot table, this writes the
//! schema-migrated relational layout `schema.rs` derives — real entity
//! tables, association join tables, unique indexes — so the state database
//! is queryable with plain SQL and byte-compatible with mxrb's runtime
//! layout. The unit-of-work semantics stay in [`mxrs_runtime::Store`]; this
//! adapter persists committed state and restores it, with the same
//! generation-based conflict detection as the snapshot adapter. A version-1
//! snapshot database upgrades in place on open.
//!
//! AutoNumber sequences are synchronized into `mxrb_schema_sequences` by the
//! migrator; allocation on create remains with the store layer and is
//! tracked in the backlog, not silently faked here.

use std::collections::BTreeMap;
use std::path::Path;

use mxrs_model::{AttributeType, Module};
use mxrs_runtime::{ObjectValue, Store};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::Value;

use crate::schema::{self, RuntimeSchema, quote};
use crate::{APPLICATION_ID, Result, SqliteRuntimeError};

const RELATIONAL_VERSION: i64 = 2;
const METADATA_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS mxrs_runtime_metadata (
    singleton INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
    generation INTEGER NOT NULL CHECK (generation >= 0)
) WITHOUT ROWID";

#[derive(Debug)]
pub struct RelationalRuntimeStore {
    connection: Connection,
    schema: RuntimeSchema,
    loaded_generation: Option<i64>,
}

impl RelationalRuntimeStore {
    pub fn open(
        path: impl AsRef<Path>,
        modules: &[Module],
        allow_destructive: bool,
    ) -> Result<Self> {
        Self::from_connection(Connection::open(path)?, modules, allow_destructive)
    }

    pub fn in_memory(modules: &[Module], allow_destructive: bool) -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?, modules, allow_destructive)
    }

    fn from_connection(
        mut connection: Connection,
        modules: &[Module],
        allow_destructive: bool,
    ) -> Result<Self> {
        let runtime_schema = schema::derive(modules);
        let (application_id, version) = {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let identity = identity(&transaction)?;
            transaction.commit()?;
            identity
        };
        let legacy_objects = match (application_id, version) {
            (0, 0) => None,
            (APPLICATION_ID, 1) => Some(read_legacy_snapshot(&connection)?),
            (APPLICATION_ID, RELATIONAL_VERSION) => None,
            (APPLICATION_ID, version) => {
                return Err(SqliteRuntimeError::UnsupportedVersion(version));
            }
            _ => {
                return Err(SqliteRuntimeError::UnrelatedDatabase {
                    application_id,
                    version,
                });
            }
        };
        schema::migrate(&mut connection, &runtime_schema, allow_destructive)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(METADATA_SCHEMA)?;
        transaction.execute(
            "INSERT OR IGNORE INTO mxrs_runtime_metadata VALUES (1, 0)",
            [],
        )?;
        if let Some(objects) = legacy_objects {
            write_objects(&transaction, &runtime_schema, &objects)?;
            transaction.execute("DROP TABLE IF EXISTS mxrs_runtime_objects", [])?;
        }
        transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
        transaction.pragma_update(None, "user_version", RELATIONAL_VERSION)?;
        let generation: i64 = transaction.query_row(
            "SELECT generation FROM mxrs_runtime_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let empty = is_empty(&transaction, &runtime_schema)?;
        transaction.commit()?;
        Ok(Self {
            connection,
            schema: runtime_schema,
            loaded_generation: (generation == 0 && empty).then_some(0),
        })
    }

    pub fn schema(&self) -> &RuntimeSchema {
        &self.schema
    }

    pub fn loaded_generation(&self) -> Option<i64> {
        self.loaded_generation
    }

    /// Loads every persisted object (attributes and associations) into the
    /// store's committed state.
    pub fn load(&mut self, store: &mut Store) -> Result<usize> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let objects = read_objects(&transaction, &self.schema)?;
        let generation: i64 = transaction.query_row(
            "SELECT generation FROM mxrs_runtime_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        let count = objects.len();
        store.restore_persistent(objects)?;
        self.loaded_generation = Some(generation);
        Ok(count)
    }

    /// Replaces the durable relational state atomically, guarded by the
    /// snapshot-generation conflict check.
    pub fn save(&mut self, store: &Store) -> Result<usize> {
        let expected = self
            .loaded_generation
            .ok_or(SqliteRuntimeError::SnapshotNotLoaded)?;
        let objects = store.persistent_objects();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actual: i64 = transaction.query_row(
            "SELECT generation FROM mxrs_runtime_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if actual != expected {
            return Err(SqliteRuntimeError::Conflict { expected, actual });
        }
        let next = expected
            .checked_add(1)
            .ok_or(SqliteRuntimeError::GenerationExhausted)?;
        let count = write_objects(&transaction, &self.schema, &objects)?;
        transaction.execute(
            "UPDATE mxrs_runtime_metadata SET generation = ?1 WHERE singleton = 1",
            params![next],
        )?;
        transaction.commit()?;
        self.loaded_generation = Some(next);
        Ok(count)
    }
}

fn identity(connection: &Connection) -> Result<(i64, i64)> {
    let application_id: i64 =
        connection.query_row("SELECT * FROM pragma_application_id", [], |row| row.get(0))?;
    let version: i64 =
        connection.query_row("SELECT * FROM pragma_user_version", [], |row| row.get(0))?;
    Ok((application_id, version))
}

fn read_legacy_snapshot(connection: &Connection) -> Result<Vec<ObjectValue>> {
    let mut statement =
        connection.prepare("SELECT entity, object_id, members_json FROM mxrs_runtime_objects")?;
    let rows: Vec<(String, String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut objects = Vec::new();
    for (entity, id, members_json) in rows {
        let members: BTreeMap<String, Value> =
            serde_json::from_str(&members_json).map_err(|source| SqliteRuntimeError::Json {
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
    Ok(objects)
}

fn is_empty(connection: &Connection, schema: &RuntimeSchema) -> Result<bool> {
    for entity in &schema.entities {
        let count: i64 = connection.query_row(
            &format!("SELECT COUNT(*) FROM {}", quote(&entity.table)),
            [],
            |row| row.get(0),
        )?;
        if count > 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn write_objects(
    connection: &Connection,
    schema: &RuntimeSchema,
    objects: &[ObjectValue],
) -> Result<usize> {
    for entity in &schema.entities {
        connection.execute(&format!("DELETE FROM {}", quote(&entity.table)), [])?;
    }
    for association in &schema.associations {
        connection.execute(&format!("DELETE FROM {}", quote(&association.table)), [])?;
    }
    let mut written = 0usize;
    for object in objects {
        let entity = schema.entity(&object.entity)?;
        let mut columns = vec!["id".to_string()];
        let mut values: Vec<SqlValue> = vec![SqlValue::Text(object.id.clone())];
        for column in &entity.columns {
            columns.push(column.sql_name.clone());
            values.push(member_to_sql(object.members.get(&column.name), column.kind));
        }
        let placeholders: Vec<String> = (1..=columns.len())
            .map(|index| format!("?{index}"))
            .collect();
        let quoted: Vec<String> = columns.iter().map(|name| quote(name)).collect();
        let parameters: Vec<&dyn rusqlite::ToSql> = values
            .iter()
            .map(|value| value as &dyn rusqlite::ToSql)
            .collect();
        connection.execute(
            &format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote(&entity.table),
                quoted.join(", "),
                placeholders.join(", ")
            ),
            parameters.as_slice(),
        )?;
        written += 1;
        for association in schema
            .associations
            .iter()
            .filter(|association| association.from_entity == entity.name)
        {
            let member = object
                .members
                .get(&association.name)
                .or_else(|| object.members.get(&association.qualified_name));
            let targets: Vec<&str> = match member {
                Some(Value::String(target)) => vec![target.as_str()],
                Some(Value::Array(targets)) => targets.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            for target in targets {
                connection.execute(
                    &format!(
                        "INSERT OR IGNORE INTO {} (source_id, target_id) VALUES (?1, ?2)",
                        quote(&association.table)
                    ),
                    params![object.id, target],
                )?;
            }
        }
    }
    Ok(written)
}

fn read_objects(connection: &Connection, schema: &RuntimeSchema) -> Result<Vec<ObjectValue>> {
    let mut objects = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for entity in &schema.entities {
        let column_list: Vec<String> = std::iter::once("id".to_string())
            .chain(entity.columns.iter().map(|column| column.sql_name.clone()))
            .map(|name| quote(&name))
            .collect();
        let mut statement = connection.prepare(&format!(
            "SELECT {} FROM {} ORDER BY rowid",
            column_list.join(", "),
            quote(&entity.table)
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let mut members = BTreeMap::new();
            for (position, column) in entity.columns.iter().enumerate() {
                let value: SqlValue = row.get(position + 1)?;
                if let Some(value) = sql_to_member(value, column.kind) {
                    members.insert(column.name.clone(), value);
                }
            }
            index.insert(id.clone(), objects.len());
            objects.push(ObjectValue {
                entity: entity.name.clone(),
                id,
                members,
            });
        }
    }
    for association in &schema.associations {
        let mut statement = connection.prepare(&format!(
            "SELECT source_id, target_id FROM {} ORDER BY rowid",
            quote(&association.table)
        ))?;
        let pairs: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        for (source, target) in pairs {
            let Some(position) = index.get(&source) else {
                continue;
            };
            let object = &mut objects[*position];
            if association.reference {
                object
                    .members
                    .insert(association.name.clone(), Value::String(target));
            } else {
                let entry = object
                    .members
                    .entry(association.name.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(targets) = entry {
                    targets.push(Value::String(target));
                }
            }
        }
    }
    Ok(objects)
}

fn member_to_sql(value: Option<&Value>, kind: AttributeType) -> SqlValue {
    let Some(value) = value else {
        return SqlValue::Null;
    };
    match (kind, value) {
        (_, Value::Null) => SqlValue::Null,
        (AttributeType::Boolean, Value::Bool(value)) => SqlValue::Integer(i64::from(*value)),
        (
            AttributeType::Integer | AttributeType::Long | AttributeType::AutoNumber,
            Value::Number(value),
        ) => value
            .as_i64()
            .map(SqlValue::Integer)
            .unwrap_or(SqlValue::Null),
        (AttributeType::Float | AttributeType::Decimal, Value::Number(value)) => {
            value.as_f64().map(SqlValue::Real).unwrap_or(SqlValue::Null)
        }
        (_, Value::String(value)) => SqlValue::Text(value.clone()),
        (_, other) => SqlValue::Text(other.to_string()),
    }
}

fn sql_to_member(value: SqlValue, kind: AttributeType) -> Option<Value> {
    match (kind, value) {
        (_, SqlValue::Null) => None,
        (AttributeType::Boolean, SqlValue::Integer(value)) => Some(Value::Bool(value != 0)),
        (_, SqlValue::Integer(value)) => Some(Value::from(value)),
        (_, SqlValue::Real(value)) => serde_json::Number::from_f64(value).map(Value::Number),
        (_, SqlValue::Text(value)) => Some(Value::String(value)),
        (_, SqlValue::Blob(_)) => None,
    }
}
