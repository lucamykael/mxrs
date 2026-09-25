//! SQLite migration for the runtime database.
//!
//! Ports the applying half of `lib/mxrb/runtime/schema_migrator.rb`: an
//! in-place migration that adds what is missing, rebuilds what changed, and
//! refuses destructive evolutions unless explicitly allowed. The schema being
//! applied is derived by `mxrs-relational-schema` and is the same one every
//! other backend sees; what lives here is SQLite's column types, its DDL, and
//! its table-rebuild dance.
//!
//! Table names and the `mxrb_schema_*` metadata catalog are byte-compatible
//! with mxrb's, so a database created by either tool migrates under the other.

use std::collections::{BTreeMap, BTreeSet};

use mxrs_model::AttributeType;
pub use mxrs_relational_schema::{
    AssociationSchema, EntitySchema, MigrationResult, RemovedItem, RuntimeSchema, SchemaColumn,
    derive, logical_type, physical_name,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::{Result, SqliteRuntimeError};

/// SQLite column types. The schema itself is dialect-independent and lives
/// in `mxrs-relational-schema`; only the mapping from a Mendix attribute to a
/// column type belongs to a backend.
const fn sql_type(kind: AttributeType) -> &'static str {
    match kind {
        AttributeType::Integer
        | AttributeType::Long
        | AttributeType::AutoNumber
        | AttributeType::Boolean => "INTEGER",
        AttributeType::Float | AttributeType::Decimal => "REAL",
        AttributeType::Binary => "BLOB",
        AttributeType::DateTime
        | AttributeType::Enum
        | AttributeType::HashString
        | AttributeType::String => "TEXT",
    }
}

/// Every system member is an identifier or an ISO-8601 timestamp, and mxrb
/// stores both as `TEXT`.
const SYSTEM_MEMBER_TYPE: &str = "TEXT";

/// Ports `SchemaMigrator#migrate!` against an open connection. The whole
/// migration is one immediate transaction: it either completes or leaves
/// the database untouched.
pub fn migrate(
    connection: &mut Connection,
    schema: &RuntimeSchema,
    allow_destructive: bool,
) -> Result<MigrationResult> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut result = MigrationResult::default();
    create_metadata_tables(&transaction)?;
    let plan = build_migration_plan(&transaction, schema)?;
    if !plan.is_empty() {
        if !allow_destructive {
            let descriptions: Vec<String> = plan
                .iter()
                .map(|change| format!("{} {}", change.kind, change.name))
                .collect();
            return Err(SqliteRuntimeError::UnsafeMigration {
                message: format!(
                    "schema migration would remove {}; rerun with allow_destructive after backing up the database",
                    descriptions.join(", ")
                ),
                changes: plan,
            });
        }
        apply_destructive_migration(&transaction, schema, &plan, &mut result.rebuilt_tables)?;
    }
    for entity in &schema.entities {
        migrate_entity(&transaction, entity, &mut result)?;
    }
    for association in &schema.associations {
        migrate_association(&transaction, association, &mut result)?;
    }
    synchronize_metadata(&transaction, schema)?;
    prune_obsolete_metadata(&transaction, schema)?;
    transaction.commit()?;
    Ok(result)
}

fn create_metadata_tables(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS mxrb_schema_entities (
            storage_key TEXT PRIMARY KEY, logical_name TEXT NOT NULL, table_name TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS mxrb_schema_attributes (
            entity_key TEXT NOT NULL, storage_key TEXT NOT NULL, logical_name TEXT NOT NULL,
            column_name TEXT NOT NULL, logical_type TEXT NOT NULL,
            required INTEGER NOT NULL DEFAULT 0, unique_value INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (entity_key, storage_key)
        );
        CREATE TABLE IF NOT EXISTS mxrb_schema_associations (
            storage_key TEXT PRIMARY KEY, logical_name TEXT NOT NULL, table_name TEXT NOT NULL,
            from_entity TEXT NOT NULL, to_entity TEXT NOT NULL, association_type TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS mxrb_schema_sequences (
            entity_key TEXT NOT NULL, attribute_key TEXT NOT NULL, next_value INTEGER NOT NULL,
            PRIMARY KEY (entity_key, attribute_key)
        );",
    )?;
    for (name, definition) in [
        ("required", "INTEGER NOT NULL DEFAULT 0"),
        ("unique_value", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !table_columns(connection, "mxrb_schema_attributes")?.contains_key(name) {
            connection.execute(
                &format!(
                    "ALTER TABLE \"mxrb_schema_attributes\" ADD COLUMN {} {definition}",
                    quote(name)
                ),
                [],
            )?;
        }
    }
    Ok(())
}

fn build_migration_plan(
    connection: &Connection,
    schema: &RuntimeSchema,
) -> Result<Vec<RemovedItem>> {
    let current_entities: BTreeMap<&str, &EntitySchema> = schema
        .entities
        .iter()
        .map(|entity| (entity.storage_key.as_str(), entity))
        .collect();
    let current_attributes: BTreeSet<(String, String)> = schema
        .entities
        .iter()
        .flat_map(|entity| {
            entity
                .columns
                .iter()
                .map(|column| (entity.storage_key.clone(), column.storage_key.clone()))
        })
        .collect();
    let current_associations: BTreeSet<&str> = schema
        .associations
        .iter()
        .map(|association| association.storage_key.as_str())
        .collect();

    let mut removed = Vec::new();
    let mut removed_entity_keys = BTreeSet::new();
    let mut statement = connection
        .prepare("SELECT storage_key, logical_name, table_name FROM mxrb_schema_entities")?;
    let entity_rows: Vec<(String, String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for (key, name, table) in entity_rows {
        if !current_entities.contains_key(key.as_str()) {
            removed_entity_keys.insert(key.clone());
            removed.push(RemovedItem {
                kind: "entity",
                name,
                table,
                storage_key: key,
                entity_key: None,
                column: None,
            });
        }
    }
    let mut statement = connection.prepare(
        "SELECT entity_key, storage_key, logical_name, column_name FROM mxrb_schema_attributes",
    )?;
    let attribute_rows: Vec<(String, String, String, String)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    for (entity_key, key, name, column) in attribute_rows {
        if removed_entity_keys.contains(&entity_key)
            || current_attributes.contains(&(entity_key.clone(), key.clone()))
        {
            continue;
        }
        let Some(entity) = current_entities.get(entity_key.as_str()) else {
            continue;
        };
        removed.push(RemovedItem {
            kind: "attribute",
            name,
            table: entity.table.clone(),
            storage_key: key,
            entity_key: Some(entity_key),
            column: Some(column),
        });
    }
    let mut statement = connection
        .prepare("SELECT storage_key, logical_name, table_name FROM mxrb_schema_associations")?;
    let association_rows: Vec<(String, String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for (key, name, table) in association_rows {
        if !current_associations.contains(key.as_str()) {
            removed.push(RemovedItem {
                kind: "association",
                name,
                table,
                storage_key: key,
                entity_key: None,
                column: None,
            });
        }
    }
    Ok(removed)
}

fn apply_destructive_migration(
    connection: &Connection,
    schema: &RuntimeSchema,
    plan: &[RemovedItem],
    rebuilt: &mut Vec<String>,
) -> Result<()> {
    for change in plan
        .iter()
        .filter(|change| change.kind == "association" || change.kind == "entity")
    {
        if table_exists(connection, &change.table)? {
            connection.execute(&format!("DROP TABLE {}", quote(&change.table)), [])?;
        }
    }
    let mut rebuilt_keys = BTreeSet::new();
    for change in plan.iter().filter(|change| change.kind == "attribute") {
        let Some(entity_key) = &change.entity_key else {
            continue;
        };
        if !rebuilt_keys.insert(entity_key.clone()) {
            continue;
        }
        let Some(entity) = schema
            .entities
            .iter()
            .find(|entity| &entity.storage_key == entity_key)
        else {
            continue;
        };
        if table_exists(connection, &entity.table)? {
            let actual = table_columns(connection, &entity.table)?;
            rebuild_entity(connection, entity, &actual)?;
            if !rebuilt.contains(&entity.table) {
                rebuilt.push(entity.table.clone());
            }
        }
    }
    Ok(())
}

fn migrate_entity(
    connection: &Connection,
    entity: &EntitySchema,
    result: &mut MigrationResult,
) -> Result<()> {
    if !table_exists(connection, &entity.table)? {
        connection.execute(&create_entity_sql(entity, &entity.table), [])?;
        create_entity_indexes(connection, entity)?;
        result.created_tables.push(entity.table.clone());
        return Ok(());
    }
    let actual = table_columns(connection, &entity.table)?;
    let expected = entity_columns(entity);
    let missing: Vec<&String> = expected
        .iter()
        .map(|(name, _)| name)
        .filter(|name| !actual.contains_key(*name))
        .collect();
    let incompatible = expected.iter().any(|(name, definition)| {
        actual.get(name).is_some_and(|column| {
            normalize_type(&column.kind) != normalize_type(definition)
                || (name != "id" && column.not_null != required_definition(definition))
        })
    });
    let required_missing = missing
        .iter()
        .any(|name| definition_of(&expected, name).is_some_and(required_definition));
    if incompatible || required_missing {
        validate_required_values(connection, entity, &actual, &missing)?;
        rebuild_entity(connection, entity, &actual)?;
        result.rebuilt_tables.push(entity.table.clone());
    } else {
        for name in missing {
            connection.execute(
                &format!(
                    "ALTER TABLE {} ADD COLUMN {} {}",
                    quote(&entity.table),
                    quote(name),
                    definition_of(&expected, name).unwrap_or("TEXT")
                ),
                [],
            )?;
            result
                .added_columns
                .push(format!("{}.{name}", entity.table));
        }
    }
    create_entity_indexes(connection, entity)?;
    Ok(())
}

fn migrate_association(
    connection: &Connection,
    association: &AssociationSchema,
    result: &mut MigrationResult,
) -> Result<()> {
    if !table_exists(connection, &association.table)? {
        connection.execute(&create_association_sql(association, &association.table), [])?;
        result.created_tables.push(association.table.clone());
        return Ok(());
    }
    let actual = table_columns(connection, &association.table)?;
    if actual.contains_key("source_id") && actual.contains_key("target_id") {
        return Ok(());
    }
    let temporary = format!("{}_new", association.table);
    connection.execute(&format!("DROP TABLE IF EXISTS {}", quote(&temporary)), [])?;
    connection.execute(&create_association_sql(association, &temporary), [])?;
    let source = ["source_id", "source", "owner_id"]
        .into_iter()
        .find(|name| actual.contains_key(*name));
    let target = ["target_id", "target", "child_id"]
        .into_iter()
        .find(|name| actual.contains_key(*name));
    if let (Some(source), Some(target)) = (source, target) {
        connection.execute(
            &format!(
                "INSERT OR IGNORE INTO {} (source_id, target_id) SELECT {}, {} FROM {} WHERE {} IS NOT NULL AND {} IS NOT NULL",
                quote(&temporary),
                quote(source),
                quote(target),
                quote(&association.table),
                quote(source),
                quote(target)
            ),
            [],
        )?;
    }
    connection.execute(&format!("DROP TABLE {}", quote(&association.table)), [])?;
    connection.execute(
        &format!(
            "ALTER TABLE {} RENAME TO {}",
            quote(&temporary),
            quote(&association.table)
        ),
        [],
    )?;
    result.rebuilt_tables.push(association.table.clone());
    Ok(())
}

fn rebuild_entity(
    connection: &Connection,
    entity: &EntitySchema,
    actual: &BTreeMap<String, ActualColumn>,
) -> Result<()> {
    let temporary = format!("{}_new", entity.table);
    connection.execute(&format!("DROP TABLE IF EXISTS {}", quote(&temporary)), [])?;
    connection.execute(&create_entity_sql(entity, &temporary), [])?;
    let copies = copy_expressions(entity, actual);
    if !copies.is_empty() {
        let fields: Vec<String> = copies.iter().map(|(name, _)| quote(name)).collect();
        let expressions: Vec<&str> = copies
            .iter()
            .map(|(_, expression)| expression.as_str())
            .collect();
        connection.execute(
            &format!(
                "INSERT INTO {} ({}) SELECT {} FROM {}",
                quote(&temporary),
                fields.join(", "),
                expressions.join(", "),
                quote(&entity.table)
            ),
            [],
        )?;
    }
    connection.execute(&format!("DROP TABLE {}", quote(&entity.table)), [])?;
    connection.execute(
        &format!(
            "ALTER TABLE {} RENAME TO {}",
            quote(&temporary),
            quote(&entity.table)
        ),
        [],
    )?;
    create_entity_indexes(connection, entity)?;
    Ok(())
}

fn create_entity_sql(entity: &EntitySchema, table: &str) -> String {
    let definitions: Vec<String> = entity_columns(entity)
        .iter()
        .map(|(name, definition)| {
            let suffix = if name == "id" { " PRIMARY KEY" } else { "" };
            format!("{} {definition}{suffix}", quote(name))
        })
        .collect();
    format!("CREATE TABLE {} ({})", quote(table), definitions.join(", "))
}

fn create_entity_indexes(connection: &Connection, entity: &EntitySchema) -> Result<()> {
    for column in entity.columns.iter().filter(|column| column.unique) {
        let index = physical_name(
            "unique",
            &format!("{}:{}", entity.storage_key, column.storage_key),
        );
        connection.execute(
            &format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS {} ON {} ({})",
                quote(&index),
                quote(&entity.table),
                quote(&column.sql_name)
            ),
            [],
        )?;
    }
    Ok(())
}

fn create_association_sql(association: &AssociationSchema, table: &str) -> String {
    let uniqueness = if association.reference {
        ", UNIQUE (source_id)"
    } else {
        ""
    };
    format!(
        "CREATE TABLE {} (\n  source_id TEXT NOT NULL, target_id TEXT NOT NULL,\n  PRIMARY KEY (source_id, target_id){uniqueness}\n)",
        quote(table)
    )
}

/// Ordered `(sql_name, definition)` pairs, `id` first like mxrb's.
fn entity_columns(entity: &EntitySchema) -> Vec<(String, String)> {
    let mut result = vec![("id".to_string(), "TEXT".to_string())];
    for column in &entity.columns {
        result.push((column.sql_name.clone(), column_definition(column)));
    }
    for name in &entity.system_members {
        result.push(((*name).to_string(), SYSTEM_MEMBER_TYPE.to_string()));
    }
    result
}

fn definition_of<'a>(columns: &'a [(String, String)], name: &str) -> Option<&'a str> {
    columns
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, definition)| definition.as_str())
}

fn column_definition(column: &SchemaColumn) -> String {
    let mut definition = sql_type(column.kind).to_string();
    if column.required {
        definition.push_str(" NOT NULL");
    }
    if let Some(default) = &column.default {
        definition.push_str(&format!(" DEFAULT {}", sql_literal(default, column.kind)));
    }
    definition
}

fn required_definition(definition: &str) -> bool {
    definition.to_ascii_uppercase().contains("NOT NULL")
}

fn validate_required_values(
    connection: &Connection,
    entity: &EntitySchema,
    actual: &BTreeMap<String, ActualColumn>,
    missing: &[&String],
) -> Result<()> {
    let rows: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM {}", quote(&entity.table)),
        [],
        |row| row.get(0),
    )?;
    if rows == 0 {
        return Ok(());
    }
    for column in entity.columns.iter().filter(|column| column.required) {
        if missing.iter().any(|name| **name == column.sql_name) {
            if column.default.is_none() {
                return Err(SqliteRuntimeError::UnsafeMigration {
                    message: format!(
                        "cannot add required attribute {}.{} to a populated table without a default",
                        entity.name, column.name
                    ),
                    changes: Vec::new(),
                });
            }
        } else if actual.contains_key(&column.sql_name)
            && column.default.is_none()
            && has_null_values(connection, &entity.table, &column.sql_name)?
        {
            return Err(SqliteRuntimeError::UnsafeMigration {
                message: format!(
                    "cannot make {}.{} required while NULL values exist",
                    entity.name, column.name
                ),
                changes: Vec::new(),
            });
        }
    }
    Ok(())
}

fn has_null_values(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let found: Option<i64> = connection
        .query_row(
            &format!(
                "SELECT 1 FROM {} WHERE {} IS NULL LIMIT 1",
                quote(table),
                quote(column)
            ),
            [],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    Ok(found == Some(1))
}

fn copy_expressions(
    entity: &EntitySchema,
    actual: &BTreeMap<String, ActualColumn>,
) -> Vec<(String, String)> {
    let columns: BTreeMap<&str, &SchemaColumn> = entity
        .columns
        .iter()
        .map(|column| (column.sql_name.as_str(), column))
        .collect();
    entity_columns(entity)
        .iter()
        .map(|(name, _)| name)
        .filter_map(|name| {
            if actual.contains_key(name) {
                let expression = match columns.get(name.as_str()) {
                    Some(column) if column.required && column.default.is_some() => format!(
                        "COALESCE({}, {})",
                        quote(name),
                        sql_literal(column.default.as_deref().unwrap_or_default(), column.kind)
                    ),
                    _ => quote(name),
                };
                Some((name.clone(), expression))
            } else {
                match columns.get(name.as_str()) {
                    Some(column) if column.required && column.default.is_some() => Some((
                        name.clone(),
                        sql_literal(column.default.as_deref().unwrap_or_default(), column.kind),
                    )),
                    _ => None,
                }
            }
        })
        .collect()
}

/// Renders an attribute's modelled default as a SQL literal.
///
/// Two deliberate divergences from mxrb, both in mxrs's favour, both visible
/// in emitted `DEFAULT` clauses:
///
/// - **Boolean:** mxrb applies Ruby truthiness to the raw default string, so
///   the string `"false"` is truthy there and emits `DEFAULT 1`. This
///   compares against `"true"` instead, so `"false"` emits `DEFAULT 0`. The
///   table *layout* stays byte-compatible; a boolean default of `"false"`
///   is the one value whose literal differs.
/// - **Integer:** Ruby's `String#to_i` truncates at the first non-digit
///   (`"12abc"` → `12`); `str::parse` refuses the whole string and falls back
///   to `0`. Well-formed models are unaffected — only malformed defaults
///   diverge, and both sides pick a number rather than failing.
fn sql_literal(value: &str, kind: AttributeType) -> String {
    match kind {
        AttributeType::Boolean => {
            if value.eq_ignore_ascii_case("true") {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        AttributeType::Integer | AttributeType::Long | AttributeType::AutoNumber => {
            value.trim().parse::<i64>().unwrap_or_default().to_string()
        }
        AttributeType::Float | AttributeType::Decimal => {
            value.trim().parse::<f64>().unwrap_or_default().to_string()
        }
        _ => format!("'{}'", value.replace('\'', "''")),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ActualColumn {
    pub kind: String,
    pub not_null: bool,
}

pub(crate) fn table_exists(connection: &Connection, name: &str) -> Result<bool> {
    let found: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

pub(crate) fn table_columns(
    connection: &Connection,
    name: &str,
) -> Result<BTreeMap<String, ActualColumn>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({})", quote(name)))?;
    let rows: Vec<(String, String, i64)> = statement
        .query_map([], |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?)))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows
        .into_iter()
        .map(|(column, kind, not_null)| {
            (
                column,
                ActualColumn {
                    kind,
                    not_null: not_null == 1,
                },
            )
        })
        .collect())
}

fn normalize_type(kind: &str) -> String {
    kind.to_ascii_uppercase()
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

fn synchronize_metadata(connection: &Connection, schema: &RuntimeSchema) -> Result<()> {
    for entity in &schema.entities {
        connection.execute(
            "INSERT INTO mxrb_schema_entities VALUES (?1, ?2, ?3) \
             ON CONFLICT(storage_key) DO UPDATE SET logical_name=excluded.logical_name, table_name=excluded.table_name",
            params![entity.storage_key, entity.name, entity.table],
        )?;
        for column in &entity.columns {
            connection.execute(
                "INSERT INTO mxrb_schema_attributes \
                 (entity_key, storage_key, logical_name, column_name, logical_type, required, unique_value) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(entity_key, storage_key) DO UPDATE SET \
                 logical_name=excluded.logical_name, column_name=excluded.column_name, \
                 logical_type=excluded.logical_type, required=excluded.required, \
                 unique_value=excluded.unique_value",
                params![
                    entity.storage_key,
                    column.storage_key,
                    column.name,
                    column.sql_name,
                    logical_type(column.kind),
                    column.required as i64,
                    column.unique as i64
                ],
            )?;
            if column.kind == AttributeType::AutoNumber {
                let maximum: i64 = connection.query_row(
                    &format!(
                        "SELECT COALESCE(MAX({}), 0) FROM {}",
                        quote(&column.sql_name),
                        quote(&entity.table)
                    ),
                    [],
                    |row| row.get(0),
                )?;
                connection.execute(
                    "INSERT OR IGNORE INTO mxrb_schema_sequences VALUES (?1, ?2, ?3)",
                    params![entity.storage_key, column.storage_key, maximum + 1],
                )?;
            }
        }
    }
    for association in &schema.associations {
        connection.execute(
            "INSERT INTO mxrb_schema_associations VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(storage_key) DO UPDATE SET logical_name=excluded.logical_name, \
             table_name=excluded.table_name, from_entity=excluded.from_entity, \
             to_entity=excluded.to_entity, association_type=excluded.association_type",
            params![
                association.storage_key,
                association.qualified_name,
                association.table,
                association.from_entity,
                association.to_entity,
                if association.reference {
                    "Reference"
                } else {
                    "ReferenceSet"
                }
            ],
        )?;
    }
    Ok(())
}

fn prune_obsolete_metadata(connection: &Connection, schema: &RuntimeSchema) -> Result<()> {
    let entity_keys: BTreeSet<&str> = schema
        .entities
        .iter()
        .map(|entity| entity.storage_key.as_str())
        .collect();
    let association_keys: BTreeSet<&str> = schema
        .associations
        .iter()
        .map(|association| association.storage_key.as_str())
        .collect();
    let attribute_keys: BTreeSet<(String, String)> = schema
        .entities
        .iter()
        .flat_map(|entity| {
            entity
                .columns
                .iter()
                .map(|column| (entity.storage_key.clone(), column.storage_key.clone()))
        })
        .collect();
    let sequence_keys: BTreeSet<(String, String)> = schema
        .entities
        .iter()
        .flat_map(|entity| {
            entity
                .columns
                .iter()
                .filter(|column| column.kind == AttributeType::AutoNumber)
                .map(|column| (entity.storage_key.clone(), column.storage_key.clone()))
        })
        .collect();
    delete_missing(
        connection,
        "mxrb_schema_associations",
        "storage_key",
        &association_keys,
    )?;
    delete_missing_pairs(
        connection,
        "mxrb_schema_attributes",
        ("entity_key", "storage_key"),
        &attribute_keys,
    )?;
    delete_missing(
        connection,
        "mxrb_schema_entities",
        "storage_key",
        &entity_keys,
    )?;
    delete_missing_pairs(
        connection,
        "mxrb_schema_sequences",
        ("entity_key", "attribute_key"),
        &sequence_keys,
    )?;
    Ok(())
}

fn delete_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    keys: &BTreeSet<&str>,
) -> Result<()> {
    if keys.is_empty() {
        connection.execute(&format!("DELETE FROM {}", quote(table)), [])?;
        return Ok(());
    }
    let placeholders = vec!["?"; keys.len()].join(", ");
    let parameters: Vec<&dyn rusqlite::ToSql> =
        keys.iter().map(|key| key as &dyn rusqlite::ToSql).collect();
    connection.execute(
        &format!(
            "DELETE FROM {} WHERE {} NOT IN ({placeholders})",
            quote(table),
            quote(column)
        ),
        parameters.as_slice(),
    )?;
    Ok(())
}

fn delete_missing_pairs(
    connection: &Connection,
    table: &str,
    columns: (&str, &str),
    keys: &BTreeSet<(String, String)>,
) -> Result<()> {
    let mut statement = connection.prepare(&format!(
        "SELECT {}, {} FROM {}",
        quote(columns.0),
        quote(columns.1),
        quote(table)
    ))?;
    let rows: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for pair in rows {
        if !keys.contains(&pair) {
            connection.execute(
                &format!(
                    "DELETE FROM {} WHERE {} = ?1 AND {} = ?2",
                    quote(table),
                    quote(columns.0),
                    quote(columns.1)
                ),
                params![pair.0, pair.1],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
