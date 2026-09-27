//! Applies a model's relational schema to PostgreSQL.
//!
//! The schema itself is derived by `mxrs-relational-schema` and is the same
//! one the SQLite backend and the OQL projection see. What lives here is
//! PostgreSQL's half: column types, DDL, and the difference between the
//! schema a model declares and the one a database currently has.
//!
//! # Why the columns are typed
//!
//! The SQLite backend stores everything as `TEXT`/`INTEGER`/`REAL`/`BLOB`
//! because that layout is a contract with mxrb — a database written by either
//! tool is read by the other. PostgreSQL has no mxrb counterpart: mxrb's
//! `db sync` boots a Mendix Runtime and lets *it* synchronize the schema, so
//! there is no layout to match and no reason to inherit SQLite's.
//!
//! Inheriting it would be actively wrong. `mxrs serve` binds every query
//! parameter as a quoted literal, and PostgreSQL resolves an unknown-typed
//! literal against the column it is compared with. Against `bigint`,
//! `WHERE n > '99'` finds 100; against `text` the same predicate compares
//! lexicographically, finds nothing, and reports no error at all. Typed
//! columns turn that silent wrong answer into a correct one — and a parameter
//! that cannot be a number into a loud `invalid input syntax for type bigint`.
//!
//! # What a migration will not do
//!
//! Changing an attribute's default in the model rewrites the column default;
//! it does not backfill rows that already exist. Adding a *required* attribute
//! to a table that already has rows, with no default to give them, fails the
//! whole migration — PostgreSQL refuses the column, the transaction rolls
//! back, and nothing is half-applied.

use std::collections::{BTreeMap, BTreeSet};

use mxrs_model::AttributeType;
use mxrs_relational_schema::{
    AssociationSchema, EntitySchema, RemovedItem, RuntimeSchema, SchemaColumn, logical_type,
    physical_name,
};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum PostgresError {
    /// The database could not be reached or refused a statement. The message
    /// is the session's own, because it is the one that names the cause.
    #[error("{0}")]
    Session(String),
    #[error(
        "schema migration would remove {}; rerun with --allow-destructive-schema after backing up the database",
        summarize(.0)
    )]
    Destructive(Vec<RemovedItem>),
    #[error("PostgreSQL returned an unreadable catalog: {0}")]
    InvalidCatalog(String),
}

pub type Result<T> = std::result::Result<T, PostgresError>;

fn summarize(removed: &[RemovedItem]) -> String {
    removed
        .iter()
        .map(|item| match &item.column {
            Some(column) => format!("{} {} ({}.{column})", item.kind, item.name, item.table),
            None => format!("{} {} ({})", item.kind, item.name, item.table),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// How a caller runs SQL. Reads and writes are separate because they are
/// separate privileges: everything this module reads is a catalog query, and
/// everything it writes goes out as one script.
pub trait SqlSession {
    /// Runs one read-only statement and returns its rows as column maps.
    ///
    /// # Errors
    ///
    /// [`PostgresError::Session`] when the statement cannot be run.
    fn query(&self, sql: &str) -> Result<Vec<Map<String, Value>>>;

    /// Runs one script. psql processes a multi-statement command inside a
    /// single implicit transaction, which is what makes a migration atomic:
    /// it either lands whole or leaves the database untouched.
    ///
    /// # Errors
    ///
    /// [`PostgresError::Session`] when any statement in the script fails.
    fn execute(&self, script: &str) -> Result<()>;
}

/// The column type a Mendix attribute deploys to.
#[must_use]
pub const fn column_type(kind: AttributeType) -> &'static str {
    match kind {
        AttributeType::String | AttributeType::HashString | AttributeType::Enum => "text",
        // The model declares 32-bit and 64-bit ranges separately; keeping them
        // apart lets the database reject an overflow the model already forbids.
        AttributeType::Integer => "integer",
        AttributeType::Long | AttributeType::AutoNumber => "bigint",
        AttributeType::Float => "double precision",
        // SQLite maps Decimal to REAL and loses precision doing it. There is
        // no compatibility reason to repeat that here.
        AttributeType::Decimal => "numeric",
        AttributeType::Boolean => "boolean",
        AttributeType::DateTime => "timestamp with time zone",
        AttributeType::Binary => "bytea",
    }
}

/// Identity columns are text; the two timestamps are timestamps.
#[must_use]
pub fn system_member_type(name: &str) -> &'static str {
    if name.ends_with("_at") {
        "timestamp with time zone"
    } else {
        "text"
    }
}

/// What a sync did, or would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    pub created_tables: Vec<String>,
    pub added_columns: Vec<String>,
    pub retyped_columns: Vec<String>,
    pub renullified_columns: Vec<String>,
    pub created_indexes: Vec<String>,
    /// Empty unless the migration was allowed to remove things.
    pub removed: Vec<RemovedItem>,
    /// `true` when the model and the database already agree.
    pub unchanged: bool,
}

/// A migration worked out but not yet applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationPlan {
    pub report: SyncReport,
    /// Removals the plan found, whether or not it is allowed to apply them.
    pub removals: Vec<RemovedItem>,
    pub statements: Vec<String>,
}

impl MigrationPlan {
    /// The statements as one script, or `None` when there is nothing to do.
    #[must_use]
    pub fn script(&self) -> Option<String> {
        (!self.statements.is_empty()).then(|| self.statements.join("\n"))
    }
}

/// The schema the database currently has: its columns, and the catalog rows
/// recording which model artifact each one belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurrentState {
    columns: BTreeMap<String, BTreeMap<String, ColumnState>>,
    indexes: BTreeSet<String>,
    entities: BTreeMap<String, CatalogEntity>,
    attributes: BTreeMap<(String, String), CatalogAttribute>,
    associations: BTreeMap<String, CatalogEntity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnState {
    data_type: String,
    nullable: bool,
    default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogEntity {
    logical_name: String,
    table: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogAttribute {
    logical_name: String,
    column: String,
    logical_type: String,
    required: bool,
    unique: bool,
}

const CATALOG_DDL: &str = "\
CREATE TABLE IF NOT EXISTS mxrb_schema_entities (
  storage_key text PRIMARY KEY, logical_name text NOT NULL, table_name text NOT NULL
);
CREATE TABLE IF NOT EXISTS mxrb_schema_attributes (
  entity_key text NOT NULL, storage_key text NOT NULL, logical_name text NOT NULL,
  column_name text NOT NULL, logical_type text NOT NULL,
  required boolean NOT NULL DEFAULT false, unique_value boolean NOT NULL DEFAULT false,
  PRIMARY KEY (entity_key, storage_key)
);
CREATE TABLE IF NOT EXISTS mxrb_schema_associations (
  storage_key text PRIMARY KEY, logical_name text NOT NULL, table_name text NOT NULL,
  from_entity text NOT NULL, to_entity text NOT NULL, association_type text NOT NULL
);
CREATE TABLE IF NOT EXISTS mxrb_schema_sequences (
  entity_key text NOT NULL, attribute_key text NOT NULL, next_value bigint NOT NULL,
  PRIMARY KEY (entity_key, attribute_key)
);";

const COLUMNS_SQL: &str = "SELECT table_name, column_name, data_type, is_nullable, column_default \
     FROM information_schema.columns WHERE table_schema = current_schema()";
const INDEXES_SQL: &str = "SELECT indexname FROM pg_indexes WHERE schemaname = current_schema()";
const CATALOG_ENTITIES_SQL: &str =
    "SELECT storage_key, logical_name, table_name FROM mxrb_schema_entities";
const CATALOG_ATTRIBUTES_SQL: &str = "SELECT entity_key, storage_key, logical_name, column_name, \
     logical_type, required, unique_value FROM mxrb_schema_attributes";
const CATALOG_ASSOCIATIONS_SQL: &str =
    "SELECT storage_key, logical_name, table_name FROM mxrb_schema_associations";
const CATALOG_PRESENT_SQL: &str = "SELECT table_name FROM information_schema.tables \
     WHERE table_schema = current_schema() AND table_name = 'mxrb_schema_entities'";

/// Whether this database was written by MXRS's own schema applier.
///
/// `mxrb_schema_entities` is created by [`migrate`] and by the SQLite
/// migrator, and by nothing else — a Mendix Runtime synchronizing the same
/// model never creates it. That makes its presence the fingerprint of which of
/// the two legitimate layouts a database has, which is what lets `mxrs serve`
/// choose between physical and Mendix Runtime naming for OQL instead of
/// assuming one.
///
/// The question is provenance, not content: a catalog with no rows yet still
/// says the database belongs to MXRS. It is asked through
/// `information_schema`, so an absent table is an empty result rather than a
/// failed statement.
///
/// # Errors
///
/// [`PostgresError::Session`] when the catalog cannot be read.
pub fn catalog_present(session: &impl SqlSession) -> Result<bool> {
    Ok(!session.query(CATALOG_PRESENT_SQL)?.is_empty())
}

/// Brings `session`'s database in line with `schema`.
///
/// The catalog tables are created first, because the plan is computed from
/// what they record. Everything else goes out as one script.
///
/// # Errors
///
/// [`PostgresError::Destructive`] when the model no longer declares something
/// the database still stores and `allow_destructive` is false — nothing is
/// applied in that case. [`PostgresError::Session`] when a statement fails.
pub fn migrate(
    session: &impl SqlSession,
    schema: &RuntimeSchema,
    allow_destructive: bool,
) -> Result<SyncReport> {
    session.execute(CATALOG_DDL)?;
    let current = read_state(session)?;
    let plan = plan(schema, &current, allow_destructive);
    if !allow_destructive && !plan.removals.is_empty() {
        return Err(PostgresError::Destructive(plan.removals));
    }
    if let Some(script) = plan.script() {
        session.execute(&script)?;
    }
    Ok(plan.report)
}

/// Reads the database's current schema. Public so a caller can show a plan
/// without applying it.
///
/// # Errors
///
/// [`PostgresError::Session`] or [`PostgresError::InvalidCatalog`].
pub fn read_state(session: &impl SqlSession) -> Result<CurrentState> {
    let mut state = CurrentState::default();
    for row in session.query(COLUMNS_SQL)? {
        let table = text(&row, "table_name")?;
        let column = text(&row, "column_name")?;
        state.columns.entry(table).or_default().insert(
            column,
            ColumnState {
                data_type: text(&row, "data_type")?,
                nullable: text(&row, "is_nullable")? != "NO",
                default: row
                    .get("column_default")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            },
        );
    }
    for row in session.query(INDEXES_SQL)? {
        state.indexes.insert(text(&row, "indexname")?);
    }
    for row in session.query(CATALOG_ENTITIES_SQL)? {
        state.entities.insert(
            text(&row, "storage_key")?,
            CatalogEntity {
                logical_name: text(&row, "logical_name")?,
                table: text(&row, "table_name")?,
            },
        );
    }
    for row in session.query(CATALOG_ATTRIBUTES_SQL)? {
        state.attributes.insert(
            (text(&row, "entity_key")?, text(&row, "storage_key")?),
            CatalogAttribute {
                logical_name: text(&row, "logical_name")?,
                column: text(&row, "column_name")?,
                logical_type: text(&row, "logical_type")?,
                required: flag(&row, "required"),
                unique: flag(&row, "unique_value"),
            },
        );
    }
    for row in session.query(CATALOG_ASSOCIATIONS_SQL)? {
        state.associations.insert(
            text(&row, "storage_key")?,
            CatalogEntity {
                logical_name: text(&row, "logical_name")?,
                table: text(&row, "table_name")?,
            },
        );
    }
    Ok(state)
}

/// Works out what has to change. Pure: the same model and the same database
/// state always produce the same script, and an unchanged pair produces none.
#[must_use]
pub fn plan(
    schema: &RuntimeSchema,
    current: &CurrentState,
    allow_destructive: bool,
) -> MigrationPlan {
    let mut plan = MigrationPlan::default();
    for entity in &schema.entities {
        plan_entity(&mut plan, entity, current);
    }
    for association in &schema.associations {
        plan_association(&mut plan, association, current);
    }
    plan.removals = removals(schema, current);
    if allow_destructive {
        apply_removals(&mut plan);
    }
    plan_catalog(&mut plan, schema, current);
    plan.report.unchanged = plan.statements.is_empty();
    plan
}

fn plan_entity(plan: &mut MigrationPlan, entity: &EntitySchema, current: &CurrentState) {
    let desired = desired_columns(entity);
    match current.columns.get(&entity.table) {
        None => {
            let definitions: Vec<String> = desired
                .iter()
                .map(|column| column.definition())
                .collect::<Vec<_>>();
            plan.statements.push(format!(
                "CREATE TABLE {} ({});",
                quote(&entity.table),
                definitions.join(", ")
            ));
            plan.report.created_tables.push(entity.table.clone());
        }
        Some(existing) => {
            for column in &desired {
                match existing.get(&column.name) {
                    None => {
                        plan.statements.push(format!(
                            "ALTER TABLE {} ADD COLUMN {};",
                            quote(&entity.table),
                            column.definition()
                        ));
                        plan.report
                            .added_columns
                            .push(format!("{}.{}", entity.table, column.name));
                    }
                    Some(state) => alter_column(plan, &entity.table, column, state),
                }
            }
        }
    }
    for column in entity.columns.iter().filter(|column| column.unique) {
        let index = physical_name(
            "unique",
            &format!("{}:{}", entity.storage_key, column.storage_key),
        );
        if current.indexes.contains(&index) {
            continue;
        }
        plan.statements.push(format!(
            "CREATE UNIQUE INDEX {} ON {} ({});",
            quote(&index),
            quote(&entity.table),
            quote(&column.sql_name)
        ));
        plan.report.created_indexes.push(index);
    }
}

fn alter_column(
    plan: &mut MigrationPlan,
    table: &str,
    column: &DesiredColumn,
    state: &ColumnState,
) {
    if state.data_type != column.data_type {
        // `USING` is what lets a column change shape at all: without it
        // PostgreSQL refuses anything but a widening cast.
        plan.statements.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} TYPE {} USING {}::{};",
            quote(table),
            quote(&column.name),
            column.data_type,
            quote(&column.name),
            column.data_type
        ));
        plan.report
            .retyped_columns
            .push(format!("{table}.{}", column.name));
    }
    // A primary key is not null by construction; asking again would churn.
    if !column.primary_key && state.nullable == column.not_null {
        plan.statements.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} {} NOT NULL;",
            quote(table),
            quote(&column.name),
            if column.not_null { "SET" } else { "DROP" }
        ));
        plan.report
            .renullified_columns
            .push(format!("{table}.{}", column.name));
    }
    if normalized_default(state.default.as_deref()) != column.default {
        plan.statements.push(match &column.default {
            Some(default) => format!(
                "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {default};",
                quote(table),
                quote(&column.name)
            ),
            None => format!(
                "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
                quote(table),
                quote(&column.name)
            ),
        });
    }
}

fn plan_association(
    plan: &mut MigrationPlan,
    association: &AssociationSchema,
    current: &CurrentState,
) {
    if current.columns.contains_key(&association.table) {
        return;
    }
    let uniqueness = if association.reference {
        ", UNIQUE (source_id)"
    } else {
        ""
    };
    plan.statements.push(format!(
        "CREATE TABLE {} (source_id text NOT NULL, target_id text NOT NULL, \
         PRIMARY KEY (source_id, target_id){uniqueness});",
        quote(&association.table)
    ));
    plan.report.created_tables.push(association.table.clone());
}

/// Everything the database still records that the model no longer declares.
fn removals(schema: &RuntimeSchema, current: &CurrentState) -> Vec<RemovedItem> {
    let entities: BTreeSet<&str> = schema
        .entities
        .iter()
        .map(|entity| entity.storage_key.as_str())
        .collect();
    let attributes: BTreeSet<(&str, &str)> = schema
        .entities
        .iter()
        .flat_map(|entity| {
            entity
                .columns
                .iter()
                .map(move |column| (entity.storage_key.as_str(), column.storage_key.as_str()))
        })
        .collect();
    let associations: BTreeSet<&str> = schema
        .associations
        .iter()
        .map(|association| association.storage_key.as_str())
        .collect();

    let mut removed = Vec::new();
    for (key, entity) in &current.entities {
        if !entities.contains(key.as_str()) {
            removed.push(RemovedItem {
                kind: "entity",
                name: entity.logical_name.clone(),
                table: entity.table.clone(),
                storage_key: key.clone(),
                entity_key: None,
                column: None,
            });
        }
    }
    for ((entity_key, key), attribute) in &current.attributes {
        // An attribute of an entity that is going away is covered by the
        // table going away; reporting both would double-count the same loss.
        if !entities.contains(entity_key.as_str())
            || attributes.contains(&(entity_key.as_str(), key.as_str()))
        {
            continue;
        }
        let table = current
            .entities
            .get(entity_key)
            .map(|entity| entity.table.clone())
            .unwrap_or_default();
        removed.push(RemovedItem {
            kind: "attribute",
            name: attribute.logical_name.clone(),
            table,
            storage_key: key.clone(),
            entity_key: Some(entity_key.clone()),
            column: Some(attribute.column.clone()),
        });
    }
    for (key, association) in &current.associations {
        if !associations.contains(key.as_str()) {
            removed.push(RemovedItem {
                kind: "association",
                name: association.logical_name.clone(),
                table: association.table.clone(),
                storage_key: key.clone(),
                entity_key: None,
                column: None,
            });
        }
    }
    removed
}

fn apply_removals(plan: &mut MigrationPlan) {
    for item in &plan.removals {
        match (&item.column, item.kind) {
            (Some(column), _) => plan.statements.push(format!(
                "ALTER TABLE {} DROP COLUMN IF EXISTS {};",
                quote(&item.table),
                quote(column)
            )),
            (None, _) => plan
                .statements
                .push(format!("DROP TABLE IF EXISTS {};", quote(&item.table))),
        }
    }
    plan.report.removed.clone_from(&plan.removals);
}

/// Catalog rows are written only where they differ, so a database that
/// already matches the model produces an empty script.
fn plan_catalog(plan: &mut MigrationPlan, schema: &RuntimeSchema, current: &CurrentState) {
    for entity in &schema.entities {
        let desired = CatalogEntity {
            logical_name: entity.name.clone(),
            table: entity.table.clone(),
        };
        if current.entities.get(&entity.storage_key) != Some(&desired) {
            plan.statements.push(format!(
                "INSERT INTO mxrb_schema_entities (storage_key, logical_name, table_name) \
                 VALUES ({}, {}, {}) ON CONFLICT (storage_key) DO UPDATE SET \
                 logical_name = EXCLUDED.logical_name, table_name = EXCLUDED.table_name;",
                literal(&entity.storage_key),
                literal(&entity.name),
                literal(&entity.table)
            ));
        }
        for column in &entity.columns {
            plan_catalog_attribute(plan, entity, column, current);
        }
    }
    for association in &schema.associations {
        let desired = CatalogEntity {
            logical_name: association.qualified_name.clone(),
            table: association.table.clone(),
        };
        if current.associations.get(&association.storage_key) != Some(&desired) {
            plan.statements.push(format!(
                "INSERT INTO mxrb_schema_associations (storage_key, logical_name, table_name, \
                 from_entity, to_entity, association_type) VALUES ({}, {}, {}, {}, {}, {}) \
                 ON CONFLICT (storage_key) DO UPDATE SET logical_name = EXCLUDED.logical_name, \
                 table_name = EXCLUDED.table_name, from_entity = EXCLUDED.from_entity, \
                 to_entity = EXCLUDED.to_entity, association_type = EXCLUDED.association_type;",
                literal(&association.storage_key),
                literal(&association.qualified_name),
                literal(&association.table),
                literal(&association.from_entity),
                literal(&association.to_entity),
                literal(if association.reference {
                    "Reference"
                } else {
                    "ReferenceSet"
                })
            ));
        }
    }
    for item in &plan.report.removed.clone() {
        plan.statements.push(match &item.entity_key {
            Some(entity_key) => format!(
                "DELETE FROM mxrb_schema_attributes WHERE entity_key = {} AND storage_key = {};",
                literal(entity_key),
                literal(&item.storage_key)
            ),
            None if item.kind == "association" => format!(
                "DELETE FROM mxrb_schema_associations WHERE storage_key = {};",
                literal(&item.storage_key)
            ),
            None => format!(
                "DELETE FROM mxrb_schema_entities WHERE storage_key = {};\n\
                 DELETE FROM mxrb_schema_attributes WHERE entity_key = {};",
                literal(&item.storage_key),
                literal(&item.storage_key)
            ),
        });
    }
}

fn plan_catalog_attribute(
    plan: &mut MigrationPlan,
    entity: &EntitySchema,
    column: &SchemaColumn,
    current: &CurrentState,
) {
    let desired = CatalogAttribute {
        logical_name: column.name.clone(),
        column: column.sql_name.clone(),
        logical_type: logical_type(column.kind).to_string(),
        required: column.required,
        unique: column.unique,
    };
    let key = (entity.storage_key.clone(), column.storage_key.clone());
    if current.attributes.get(&key) != Some(&desired) {
        plan.statements.push(format!(
            "INSERT INTO mxrb_schema_attributes (entity_key, storage_key, logical_name, \
             column_name, logical_type, required, unique_value) VALUES ({}, {}, {}, {}, {}, {}, {}) \
             ON CONFLICT (entity_key, storage_key) DO UPDATE SET \
             logical_name = EXCLUDED.logical_name, column_name = EXCLUDED.column_name, \
             logical_type = EXCLUDED.logical_type, required = EXCLUDED.required, \
             unique_value = EXCLUDED.unique_value;",
            literal(&entity.storage_key),
            literal(&column.storage_key),
            literal(&column.name),
            literal(&column.sql_name),
            literal(logical_type(column.kind)),
            column.required,
            column.unique
        ));
    }
    // An AutoNumber allocates from the catalog, not from a database sequence:
    // the allocation semantics belong to the model, and every backend has to
    // agree on them.
    if column.kind == AttributeType::AutoNumber {
        plan.statements.push(format!(
            "INSERT INTO mxrb_schema_sequences (entity_key, attribute_key, next_value) \
             SELECT {}, {}, COALESCE(MAX({}), 0) + 1 FROM {} \
             ON CONFLICT (entity_key, attribute_key) DO NOTHING;",
            literal(&entity.storage_key),
            literal(&column.storage_key),
            quote(&column.sql_name),
            quote(&entity.table)
        ));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesiredColumn {
    name: String,
    data_type: &'static str,
    not_null: bool,
    default: Option<String>,
    primary_key: bool,
}

impl DesiredColumn {
    fn definition(&self) -> String {
        let mut definition = format!("{} {}", quote(&self.name), self.data_type);
        if self.primary_key {
            definition.push_str(" PRIMARY KEY");
        } else if self.not_null {
            definition.push_str(" NOT NULL");
        }
        if let Some(default) = &self.default {
            definition.push_str(&format!(" DEFAULT {default}"));
        }
        definition
    }
}

/// `id` first, then the model's attributes, then the system members — the
/// order mxrb writes and the SQLite backend keeps.
fn desired_columns(entity: &EntitySchema) -> Vec<DesiredColumn> {
    let mut columns = vec![DesiredColumn {
        name: "id".to_string(),
        data_type: "text",
        not_null: true,
        default: None,
        primary_key: true,
    }];
    for column in &entity.columns {
        columns.push(DesiredColumn {
            name: column.sql_name.clone(),
            data_type: column_type(column.kind),
            not_null: column.required,
            default: column
                .default
                .as_ref()
                .map(|default| sql_literal(default, column.kind)),
            primary_key: false,
        });
    }
    for name in &entity.system_members {
        columns.push(DesiredColumn {
            name: (*name).to_string(),
            data_type: system_member_type(name),
            not_null: false,
            default: None,
            primary_key: false,
        });
    }
    columns
}

/// A model default as a PostgreSQL literal.
fn sql_literal(value: &str, kind: AttributeType) -> String {
    match kind {
        AttributeType::Boolean => {
            if value.eq_ignore_ascii_case("true") {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        AttributeType::Integer | AttributeType::Long | AttributeType::AutoNumber => {
            value.trim().parse::<i64>().unwrap_or_default().to_string()
        }
        AttributeType::Float | AttributeType::Decimal => {
            value.trim().parse::<f64>().unwrap_or_default().to_string()
        }
        _ => literal(value),
    }
}

/// PostgreSQL echoes a default back with its type spelled out — `'x'::text`
/// for what was written as `'x'`. Comparing the raw forms would rewrite every
/// default on every run; comparing without the cast compares what was meant.
fn normalized_default(stored: Option<&str>) -> Option<String> {
    let stored = stored?.trim();
    let base = match stored.rfind("::") {
        Some(cast) if !stored[cast + 2..].contains('\'') => &stored[..cast],
        _ => stored,
    };
    Some(base.trim().to_string())
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn text(row: &Map<String, Value>, key: &str) -> Result<String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| PostgresError::InvalidCatalog(format!("row has no {key}")))
}

fn flag(row: &Map<String, Value>, key: &str) -> bool {
    matches!(
        row.get(key).and_then(Value::as_str).unwrap_or_default(),
        "t" | "true" | "1"
    )
}

#[cfg(test)]
mod tests {
    use mxrs_relational_schema::{AssociationSchema, EntitySchema, SchemaColumn};
    use serde_json::json;

    use super::*;

    fn column(name: &str, kind: AttributeType) -> SchemaColumn {
        SchemaColumn {
            name: name.to_string(),
            storage_key: format!("{name}-key"),
            sql_name: physical_name("attribute", &format!("{name}-key")),
            kind,
            default: None,
            required: false,
            unique: false,
        }
    }

    fn entity(name: &str, columns: Vec<SchemaColumn>) -> EntitySchema {
        EntitySchema {
            name: name.to_string(),
            storage_key: format!("{name}-key"),
            table: physical_name("entity", &format!("{name}-key")),
            columns,
            system_members: vec!["__owner_id", "__created_at"],
        }
    }

    fn schema(entities: Vec<EntitySchema>, associations: Vec<AssociationSchema>) -> RuntimeSchema {
        RuntimeSchema {
            entities,
            associations,
        }
    }

    /// A session that answers the catalog queries from fixtures and records
    /// what it was asked to run.
    struct MockSession {
        rows: BTreeMap<&'static str, Value>,
        scripts: std::cell::RefCell<Vec<String>>,
    }

    impl MockSession {
        fn empty() -> Self {
            Self {
                rows: BTreeMap::new(),
                scripts: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl SqlSession for MockSession {
        fn query(&self, sql: &str) -> Result<Vec<Map<String, Value>>> {
            let key = if sql.contains("information_schema") {
                "columns"
            } else if sql.contains("pg_indexes") {
                "indexes"
            } else if sql.contains("mxrb_schema_entities") {
                "entities"
            } else if sql.contains("mxrb_schema_attributes") {
                "attributes"
            } else {
                "associations"
            };
            Ok(self
                .rows
                .get(key)
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .map(|row| row.as_object().expect("a row object").clone())
                        .collect()
                })
                .unwrap_or_default())
        }

        fn execute(&self, script: &str) -> Result<()> {
            self.scripts.borrow_mut().push(script.to_string());
            Ok(())
        }
    }

    #[test]
    fn a_first_migration_creates_typed_tables_and_records_them_in_the_catalog() {
        let model = schema(
            vec![entity(
                "Sales.Order",
                vec![
                    column("Amount", AttributeType::Decimal),
                    column("Placed", AttributeType::DateTime),
                ],
            )],
            Vec::new(),
        );
        let session = MockSession::empty();

        let report = migrate(&session, &model, false).unwrap();

        let scripts = session.scripts.borrow();
        assert_eq!(scripts.len(), 2, "the catalog DDL, then the migration");
        let script = &scripts[1];
        assert!(script.contains("\"id\" text PRIMARY KEY"), "{script}");
        assert!(script.contains("numeric"), "{script}");
        assert!(script.contains("timestamp with time zone"), "{script}");
        // The types are the point: a text column would compare lexicographically.
        assert!(!script.contains("REAL"), "{script}");
        assert!(
            script.contains("INSERT INTO mxrb_schema_entities"),
            "{script}"
        );
        assert_eq!(report.created_tables.len(), 1);
        assert!(!report.unchanged);
        assert!(report.removed.is_empty());
    }

    /// The property that makes the command safe to rerun: a database that
    /// already matches the model produces no statements at all.
    #[test]
    fn a_database_that_already_matches_the_model_produces_no_script() {
        let attribute = column("Amount", AttributeType::Decimal);
        let subject = entity("Sales.Order", vec![attribute.clone()]);
        let model = schema(vec![subject.clone()], Vec::new());
        let current = CurrentState {
            columns: BTreeMap::from([(
                subject.table.clone(),
                BTreeMap::from([
                    (
                        "id".to_string(),
                        ColumnState {
                            data_type: "text".to_string(),
                            nullable: false,
                            default: None,
                        },
                    ),
                    (
                        attribute.sql_name.clone(),
                        ColumnState {
                            data_type: "numeric".to_string(),
                            nullable: true,
                            default: None,
                        },
                    ),
                    (
                        "__owner_id".to_string(),
                        ColumnState {
                            data_type: "text".to_string(),
                            nullable: true,
                            default: None,
                        },
                    ),
                    (
                        "__created_at".to_string(),
                        ColumnState {
                            data_type: "timestamp with time zone".to_string(),
                            nullable: true,
                            default: None,
                        },
                    ),
                ]),
            )]),
            indexes: BTreeSet::new(),
            entities: BTreeMap::from([(
                subject.storage_key.clone(),
                CatalogEntity {
                    logical_name: subject.name.clone(),
                    table: subject.table.clone(),
                },
            )]),
            attributes: BTreeMap::from([(
                (subject.storage_key.clone(), attribute.storage_key.clone()),
                CatalogAttribute {
                    logical_name: attribute.name.clone(),
                    column: attribute.sql_name.clone(),
                    logical_type: "decimal".to_string(),
                    required: false,
                    unique: false,
                },
            )]),
            associations: BTreeMap::new(),
        };

        let plan = plan(&model, &current, false);

        assert_eq!(plan.statements, Vec::<String>::new());
        assert_eq!(plan.script(), None);
        assert!(plan.report.unchanged);
    }

    #[test]
    fn a_new_attribute_becomes_a_column_and_a_changed_type_becomes_an_alter() {
        let widened = column("Amount", AttributeType::Decimal);
        let added = column("Note", AttributeType::String);
        let subject = entity("Sales.Order", vec![widened.clone(), added.clone()]);
        let current = CurrentState {
            columns: BTreeMap::from([(
                subject.table.clone(),
                BTreeMap::from([(
                    widened.sql_name.clone(),
                    ColumnState {
                        data_type: "text".to_string(),
                        nullable: true,
                        default: None,
                    },
                )]),
            )]),
            ..CurrentState::default()
        };

        let plan = plan(&schema(vec![subject.clone()], Vec::new()), &current, false);
        let script = plan.script().unwrap();

        assert!(
            script.contains(&format!(
                "ALTER TABLE {} ALTER COLUMN {} TYPE numeric USING {}::numeric;",
                quote(&subject.table),
                quote(&widened.sql_name),
                quote(&widened.sql_name)
            )),
            "{script}"
        );
        assert!(
            script.contains(&format!(
                "ALTER TABLE {} ADD COLUMN {} text;",
                quote(&subject.table),
                quote(&added.sql_name)
            )),
            "{script}"
        );
        assert_eq!(plan.report.retyped_columns.len(), 1);
        // A table that exists but is missing `id` and the system members gets
        // them too: the migration reconciles the whole column set, not just
        // the attributes that happened to change.
        assert_eq!(
            plan.report.added_columns,
            [
                format!("{}.id", subject.table),
                format!("{}.{}", subject.table, added.sql_name),
                format!("{}.__owner_id", subject.table),
                format!("{}.__created_at", subject.table),
            ]
        );
    }

    #[test]
    fn removing_an_attribute_is_refused_until_it_is_asked_for() {
        let subject = entity("Sales.Order", Vec::new());
        let current = CurrentState {
            entities: BTreeMap::from([(
                subject.storage_key.clone(),
                CatalogEntity {
                    logical_name: subject.name.clone(),
                    table: subject.table.clone(),
                },
            )]),
            attributes: BTreeMap::from([(
                (subject.storage_key.clone(), "gone-key".to_string()),
                CatalogAttribute {
                    logical_name: "Gone".to_string(),
                    column: "mxrb_attribute_gone".to_string(),
                    logical_type: "string".to_string(),
                    required: false,
                    unique: false,
                },
            )]),
            ..CurrentState::default()
        };
        let model = schema(vec![subject.clone()], Vec::new());

        let refused = plan(&model, &current, false);
        assert_eq!(refused.removals.len(), 1);
        assert_eq!(refused.removals[0].kind, "attribute");
        assert!(
            refused.report.removed.is_empty(),
            "a refused removal is not reported as done"
        );
        assert!(
            !refused.script().unwrap_or_default().contains("DROP COLUMN"),
            "nothing is dropped without permission"
        );

        let allowed = plan(&model, &current, true);
        let script = allowed.script().unwrap();
        assert!(script.contains("DROP COLUMN IF EXISTS"), "{script}");
        assert!(
            script.contains("DELETE FROM mxrb_schema_attributes"),
            "{script}"
        );
        assert_eq!(allowed.report.removed.len(), 1);
    }

    #[test]
    fn a_refused_migration_reaches_the_caller_with_what_would_be_lost() {
        let session = MockSession {
            rows: BTreeMap::from([(
                "entities",
                json!([{"storage_key": "old", "logical_name": "Sales.Gone",
                        "table_name": "mxrb_entity_old"}]),
            )]),
            scripts: std::cell::RefCell::new(Vec::new()),
        };

        let error = migrate(&session, &schema(Vec::new(), Vec::new()), false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "schema migration would remove entity Sales.Gone (mxrb_entity_old); \
             rerun with --allow-destructive-schema after backing up the database"
        );
        assert_eq!(
            session.scripts.borrow().len(),
            1,
            "only the catalog DDL ran; nothing was applied"
        );
    }

    #[test]
    fn an_association_table_carries_its_cardinality() {
        let reference = AssociationSchema {
            name: "Order_Customer".to_string(),
            qualified_name: "Sales.Order_Customer".to_string(),
            storage_key: "assoc-1".to_string(),
            table: physical_name("association", "assoc-1"),
            from_entity: "Sales.Order".to_string(),
            to_entity: "Sales.Customer".to_string(),
            reference: true,
        };
        let set = AssociationSchema {
            reference: false,
            storage_key: "assoc-2".to_string(),
            table: physical_name("association", "assoc-2"),
            ..reference.clone()
        };

        let script = plan(
            &schema(Vec::new(), vec![reference, set]),
            &CurrentState::default(),
            false,
        )
        .script()
        .unwrap();

        assert_eq!(
            script.matches("PRIMARY KEY (source_id, target_id)").count(),
            2,
            "{script}"
        );
        assert_eq!(
            script.matches("UNIQUE (source_id)").count(),
            1,
            "only a Reference is at most one target: {script}"
        );
    }

    #[test]
    fn a_default_is_compared_without_the_type_postgresql_adds_to_it() {
        assert_eq!(
            normalized_default(Some("'x'::text")).as_deref(),
            Some("'x'")
        );
        assert_eq!(normalized_default(Some("0")).as_deref(), Some("0"));
        assert_eq!(normalized_default(Some("true")).as_deref(), Some("true"));
        assert_eq!(normalized_default(None), None);
        // A literal that itself contains `::` is not a cast.
        assert_eq!(
            normalized_default(Some("'a::b'")).as_deref(),
            Some("'a::b'")
        );
    }

    #[test]
    fn model_defaults_become_postgresql_literals_not_sqlite_ones() {
        assert_eq!(sql_literal("true", AttributeType::Boolean), "true");
        assert_eq!(sql_literal("TRUE", AttributeType::Boolean), "true");
        assert_eq!(sql_literal("no", AttributeType::Boolean), "false");
        assert_eq!(sql_literal(" 42 ", AttributeType::Integer), "42");
        assert_eq!(sql_literal("O'Hara", AttributeType::String), "'O''Hara'");
    }

    /// Answers exactly one statement — the probe — so the test also pins that
    /// the probe asks `information_schema` about `mxrb_schema_entities` and
    /// nothing else.
    struct ProbeSession(Vec<Map<String, Value>>);

    impl SqlSession for ProbeSession {
        fn query(&self, sql: &str) -> Result<Vec<Map<String, Value>>> {
            assert_eq!(sql, CATALOG_PRESENT_SQL);
            Ok(self.0.clone())
        }

        fn execute(&self, script: &str) -> Result<()> {
            panic!("the probe must not write: {script}");
        }
    }

    #[test]
    fn the_catalog_probe_asks_for_the_table_not_its_rows() {
        let row = json!({"table_name": "mxrb_schema_entities"});
        let present = ProbeSession(vec![row.as_object().expect("a row object").clone()]);
        assert!(catalog_present(&present).unwrap());
        // An empty catalog is still MXRS's: the question is provenance.
        let absent = ProbeSession(Vec::new());
        assert!(!catalog_present(&absent).unwrap());
    }

    #[test]
    fn every_attribute_type_has_a_column_type_that_is_not_text() {
        for (kind, expected) in [
            (AttributeType::String, "text"),
            (AttributeType::HashString, "text"),
            (AttributeType::Enum, "text"),
            (AttributeType::Integer, "integer"),
            (AttributeType::Long, "bigint"),
            (AttributeType::AutoNumber, "bigint"),
            (AttributeType::Float, "double precision"),
            (AttributeType::Decimal, "numeric"),
            (AttributeType::Boolean, "boolean"),
            (AttributeType::DateTime, "timestamp with time zone"),
            (AttributeType::Binary, "bytea"),
        ] {
            assert_eq!(column_type(kind), expected, "{kind:?}");
        }
        assert_eq!(
            system_member_type("__created_at"),
            "timestamp with time zone"
        );
        assert_eq!(system_member_type("__owner_id"), "text");
    }
}
