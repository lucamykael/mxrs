//! Relational durability for the runtime store.
//!
//! Where the original adapter wrote one JSON snapshot table, this writes the
//! schema-migrated relational layout `schema.rs` derives — real entity
//! tables, association join tables, unique indexes — so the state database
//! is queryable with plain SQL and byte-compatible with mxrb's runtime
//! layout. The unit-of-work semantics stay in [`mxrs_runtime::Store`]; this
//! adapter persists committed state and restores it, with the same
//! generation-based conflict detection as the snapshot adapter. A version-1
//! snapshot database upgrades in place on open; objects of entities the model
//! has since dropped make that upgrade destructive, so it is refused by name
//! unless explicitly allowed.
//!
//! AutoNumber sequences are synchronized into `mxrb_schema_sequences` by the
//! migrator; allocation on create remains with the store layer and is
//! tracked in the backlog, not silently faked here.
//!
//! One value-level divergence from mxrb: a DateTime column holds the store's
//! tagged member (`mxrs_runtime::DATETIME_MEMBER_PREFIX` plus epoch seconds),
//! written through verbatim, because the store — not this adapter — owns the
//! typing of a member. The table *layout* stays byte-compatible with mxrb;
//! the datetime *values* in it are not mxrb text, and a reader outside the
//! runtime has to detag them.

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
        let (application_id, version, existing) = {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (application_id, version) = identity(&transaction)?;
            let existing = crate::schema_objects(&transaction)?;
            transaction.commit()?;
            (application_id, version, existing)
        };
        let legacy_objects = match (application_id, version) {
            // `application_id = 0` is SQLite's universal default, so it
            // identifies nothing on its own: a stock `.sqlite3` belonging to
            // some other program reads as (0, 0) too. Adopt it only when the
            // file is empty, or when it holds exactly the pre-versioned
            // snapshot layout this adapter shipped before it stamped an
            // identity — anything else is someone else's database, and
            // stamping pragmas plus `mxrb_*` tables into it would corrupt it.
            (0, 0) if existing.is_empty() => None,
            (0, 0) if crate::is_legacy_schema(&existing) => {
                Some(read_legacy_snapshot(&connection)?)
            }
            (0, 0) => {
                return Err(SqliteRuntimeError::UnrelatedDatabase {
                    application_id,
                    version,
                });
            }
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
        // Resolve the snapshot against the current model *before* migrating:
        // a refusal must leave the database exactly as it was, and
        // `schema::migrate` commits its own transaction.
        let legacy_objects = legacy_objects
            .map(|objects| retained_legacy_objects(&runtime_schema, objects, allow_destructive))
            .transpose()?;
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

/// Splits a version-1 snapshot into the objects the current model can still
/// store and the entities it has dropped.
///
/// The upgrade path used to hand the whole snapshot to `write_objects`, which
/// failed with `unknown entity X` on the first orphan — and because a
/// version-1 database has no `mxrb_schema_*` catalogue, `schema::migrate` had
/// nothing to detect the removal from, so `--allow-destructive-schema` could
/// not clear it either. Every subsequent boot hit the same error until the
/// state file was deleted by hand. Now the removal is named up front and
/// refused like any other destructive migration, or applied when allowed.
fn retained_legacy_objects(
    schema: &RuntimeSchema,
    objects: Vec<ObjectValue>,
    allow_destructive: bool,
) -> Result<Vec<ObjectValue>> {
    let mut retained = Vec::with_capacity(objects.len());
    let mut dropped: BTreeMap<String, usize> = BTreeMap::new();
    for object in objects {
        if schema.lookup(&object.entity)?.is_some() {
            retained.push(object);
        } else {
            *dropped.entry(object.entity).or_default() += 1;
        }
    }
    if !dropped.is_empty() && !allow_destructive {
        let descriptions: Vec<String> = dropped
            .iter()
            .map(|(entity, count)| format!("{entity} ({count})"))
            .collect();
        return Err(SqliteRuntimeError::UnsafeMigration {
            message: format!(
                "version-1 snapshot contains objects of entities absent from the current model: {}; \
                 rerun with --allow-destructive-schema after backing up the database",
                descriptions.join(", ")
            ),
            changes: Vec::new(),
        });
    }
    Ok(retained)
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
            values.push(member_to_sql(
                object.members.get(&column.name),
                column.kind,
                &entity.name,
                &column.name,
            )?);
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

/// Projects a member onto its column.
///
/// A number that does not fit its integer column (`2.5`, or a magnitude past
/// `i64`) is refused by name rather than written as NULL: this adapter is the
/// durability boundary, and a save that silently discards the one value the
/// caller changed is worse than a save that fails.
fn member_to_sql(
    value: Option<&Value>,
    kind: AttributeType,
    entity: &str,
    member: &str,
) -> Result<SqlValue> {
    let Some(value) = value else {
        return Ok(SqlValue::Null);
    };
    let lossy = || SqliteRuntimeError::LossyMember {
        entity: entity.to_string(),
        member: member.to_string(),
        value: value.to_string(),
        kind,
    };
    Ok(match (kind, value) {
        (_, Value::Null) => SqlValue::Null,
        (AttributeType::Boolean, Value::Bool(value)) => SqlValue::Integer(i64::from(*value)),
        (
            AttributeType::Integer | AttributeType::Long | AttributeType::AutoNumber,
            Value::Number(number),
        ) => SqlValue::Integer(number.as_i64().ok_or_else(lossy)?),
        (AttributeType::Float | AttributeType::Decimal, Value::Number(number)) => {
            SqlValue::Real(number.as_f64().ok_or_else(lossy)?)
        }
        (_, Value::String(value)) => SqlValue::Text(value.clone()),
        (_, other) => SqlValue::Text(other.to_string()),
    })
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
