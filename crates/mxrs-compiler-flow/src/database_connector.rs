//! Lowers `DatabaseConnector$ExecuteDatabaseQueryAction` into
//! `Microflows$JavaActionCallAction` calls against the
//! `ExternalDatabaseConnector` Java module — ports `lib/mxrb/compiler/
//! database_connector_action_compiler.rb` (280 lines) verbatim, including
//! every decision-table branch and the raw-JSON-splice mapping shapes.
//! Read directly as ground truth (not from a summary) given the file's
//! own risk profile — it's one of only two files in this crate's whole
//! scope with a dedicated RSpec file.
//!
//! **The one deliberate implementation difference**: the Ruby regex-
//! post-processes real `JSON.generate` output to splice unescaped
//! fragments in via a `{'__mxrb_raw__' => value}` sentinel
//! (`raw`/`raw_json`, lines 260-267) — fragile by construction (one wrong
//! regex match from corrupting adjacent JSON). [`RawJsonValue`] builds the
//! same byte output directly instead: an ordered (`Vec`, not `HashMap` —
//! Mendix/Ruby-hash key order is part of the byte-exact contract)
//! JSON-value tree with an explicit `Raw` variant for unescaped splices,
//! rendered by hand rather than through any JSON library's own
//! serialization choices.

use std::collections::HashMap;

use mxrs_bson::Document;

use crate::CompilerError;
use crate::support::{array_docs, database_derived_id, get_id_any, get_str_any};

/// A JSON value tree that preserves insertion order and can splice an
/// unescaped fragment verbatim — see this module's doc comment.
enum RawJsonValue {
    Str(String),
    /// Spliced byte-for-byte, no escaping — used for Mendix microflow
    /// expression fragments (`'+(...)+'`) that must appear unquoted
    /// inside the generated JSON text.
    Raw(String),
    Object(Vec<(String, RawJsonValue)>),
}

impl RawJsonValue {
    fn render(&self) -> String {
        match self {
            RawJsonValue::Str(s) => serde_json::to_string(s).expect("strings always serialize"),
            RawJsonValue::Raw(s) => s.clone(),
            RawJsonValue::Object(entries) => {
                let body = entries
                    .iter()
                    .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap(), v.render()))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{{body}}}")
            }
        }
    }
}

pub struct DatabaseConnectorCompiler<'a> {
    connections: &'a HashMap<String, Document>,
    constants: &'a HashMap<String, Document>,
}

impl<'a> DatabaseConnectorCompiler<'a> {
    pub fn new(
        connections: &'a HashMap<String, Document>,
        constants: &'a HashMap<String, Document>,
    ) -> Self {
        DatabaseConnectorCompiler {
            connections,
            constants,
        }
    }

    pub fn compile(&self, action: &Document) -> Result<Document, CompilerError> {
        let query_ref = get_str_any(action, &["Query"]).unwrap_or_default();
        let (connection, query) = self.resolve_query(&query_ref)?;
        let java_action = java_action_for(&query)?;
        let action_id = get_id_any(action, &["$ID"]).unwrap_or_default();
        let mut mappings = self.basic_mappings(action, &connection, &query, java_action)?;
        mappings.extend(self.result_mappings(&query, java_action)?);
        Ok(mxrs_bson::doc! {
            "$ID": action_id,
            "$Type": "Microflows$JavaActionCallAction",
            "QueueSettings": mxrs_bson::Bson::Null,
            "ParameterMappings": mappings,
            "ErrorHandlingType": get_str_any(action, &["ErrorHandlingType"]).unwrap_or_default(),
            "JavaAction": java_action,
            "ResultVariableName": get_str_any(action, &["OutputVariableName"]).unwrap_or_default(),
            "UseReturnVariable": true,
        })
    }

    /// Models "the connection isn't wired up in this environment, no-op
    /// the write instead of failing" — the node compiler
    /// (`node.rs`) checks this before delegating a
    /// `DatabaseConnector$ExecuteDatabaseQueryAction` here at all.
    /// **High stakes, deliberately not short-circuited past
    /// `resolve_query`**: like the Ruby, a bad `Query` reference on a
    /// write action still propagates `UnknownDatabaseConnection`/
    /// `UnknownDatabaseQuery` rather than silently reporting "not
    /// unconfigured" — only the type check below is a true fast path.
    pub fn is_unconfigured_write(&self, action: &Document) -> Result<bool, CompilerError> {
        if get_str_any(action, &["$Type"]).as_deref()
            != Some("DatabaseConnector$ExecuteDatabaseQueryAction")
        {
            return Ok(false);
        }
        let query_ref = get_str_any(action, &["Query"]).unwrap_or_default();
        let (connection, query) = self.resolve_query(&query_ref)?;
        if selecting_query(&query) {
            return Ok(false);
        }
        let override_value = array_docs(action, &["ConnectionParameterMappings"])
            .into_iter()
            .find(|mapping| {
                get_str_any(mapping, &["ParameterName"])
                    .is_some_and(|name| name.eq_ignore_ascii_case("DBSource"))
            });
        let override_is_blank = override_value
            .as_ref()
            .map(|mapping| {
                get_str_any(mapping, &["Value"])
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            })
            .unwrap_or(true);
        if !override_is_blank {
            return Ok(false);
        }
        let constant_name = get_str_any(&connection, &["ConnectionString"]).unwrap_or_default();
        let Some(constant) = self.constants.get(&constant_name) else {
            return Ok(false);
        };
        Ok(get_str_any(constant, &["DefaultValue"])
            .unwrap_or_default()
            .is_empty())
    }

    /// The connection name a `DatabaseConnector$ExecuteDatabaseQueryAction`
    /// targets, split off its `Query` field the same way [`Self::resolve_query`]
    /// does. Used only to label an [`crate::node::FlowDiagnostic::UnconfiguredWrite`]
    /// diagnostic after [`Self::is_unconfigured_write`] has already returned
    /// `true` — not itself part of that decision, so it never fails: an
    /// action with a malformed `Query` field would already have raised
    /// `UnknownDatabaseConnection`/`UnknownDatabaseQuery` earlier in the same
    /// call, per [`Self::is_unconfigured_write`]'s own doc comment.
    pub fn connection_name_for(&self, action: &Document) -> String {
        let query_ref = get_str_any(action, &["Query"]).unwrap_or_default();
        rpartition(&query_ref).0
    }

    fn resolve_query(&self, qualified: &str) -> Result<(Document, Document), CompilerError> {
        let (connection_name, query_name) = rpartition(qualified);
        let connection = self
            .connections
            .get(&connection_name)
            .cloned()
            .ok_or_else(|| CompilerError::UnknownDatabaseConnection {
                connection_name: connection_name.clone(),
            })?;
        let query = array_docs(&connection, &["Queries"])
            .into_iter()
            .find(|q| get_str_any(q, &["Name"]).as_deref() == Some(query_name.as_str()))
            .ok_or_else(|| CompilerError::UnknownDatabaseQuery {
                qualified_name: qualified.to_string(),
            })?;
        Ok((connection, query))
    }

    fn basic_mappings(
        &self,
        action: &Document,
        connection: &Document,
        query: &Document,
        java_action: &'static str,
    ) -> Result<Vec<Document>, CompilerError> {
        let action_id = get_id_any(action, &["$ID"]).unwrap_or_default();
        let entries = [
            (
                "connectionDetails",
                expression_literal(&self.connection_json(action, connection)?.render()),
            ),
            ("sql", expression_literal(&runtime_query(action, query)?)),
            (
                "queryParameters",
                expression_literal(&self.parameters_json(action, query)?.render()),
            ),
        ];
        Ok(entries
            .into_iter()
            .enumerate()
            .map(|(index, (name, value))| {
                parameter_mapping(&action_id, java_action, name, &value, index)
            })
            .collect())
    }

    fn result_mappings(
        &self,
        query: &Document,
        java_action: &'static str,
    ) -> Result<Vec<Document>, CompilerError> {
        if !matches!(
            java_action,
            "ExternalDatabaseConnector.ExecuteQuery"
                | "ExternalDatabaseConnector.ExecuteCallableQuery"
                | "ExternalDatabaseConnector.ExecuteCallable"
        ) {
            return Ok(vec![]);
        }
        let Some(table) = array_docs(query, &["TableMappings"]).into_iter().next() else {
            return Ok(vec![]);
        };
        let entity = get_str_any(&table, &["Entity"]).unwrap_or_default();
        let column_mapping = RawJsonValue::Object(vec![
            ("EntityName".to_string(), RawJsonValue::Str(entity.clone())),
            (
                "ColumnAttributeMapping".to_string(),
                RawJsonValue::Object(
                    array_docs(&table, &["Columns"])
                        .iter()
                        .map(|column| {
                            let column_name =
                                get_str_any(column, &["ColumnName"]).unwrap_or_default();
                            let attribute = get_str_any(column, &["Attribute"])
                                .unwrap_or_default()
                                .rsplit('.')
                                .next()
                                .unwrap_or_default()
                                .to_string();
                            (column_name, RawJsonValue::Str(attribute))
                        })
                        .collect(),
                ),
            ),
        ]);
        let query_id = get_id_any(query, &["$ID"]).unwrap_or_default();
        let offset = 10;
        Ok(vec![
            parameter_mapping(
                &query_id,
                java_action,
                "columnMapping",
                &expression_literal(&column_mapping.render()),
                offset,
            ),
            entity_parameter_mapping(&query_id, java_action, "OutputEntity", &entity, offset + 1),
        ])
    }

    fn connection_json(
        &self,
        action: &Document,
        connection: &Document,
    ) -> Result<RawJsonValue, CompilerError> {
        let overrides = connection_parameter_overrides(action);
        Ok(RawJsonValue::Object(vec![
            (
                "UserName".to_string(),
                RawJsonValue::Raw(connection_value(
                    &overrides,
                    connection,
                    "dbusername",
                    "UserName",
                )),
            ),
            (
                "Password".to_string(),
                RawJsonValue::Raw(connection_value(
                    &overrides,
                    connection,
                    "dbpassword",
                    "Password",
                )),
            ),
            (
                "ConnectionString".to_string(),
                RawJsonValue::Raw(connection_value(
                    &overrides,
                    connection,
                    "dbsource",
                    "ConnectionString",
                )),
            ),
            (
                "DatabaseType".to_string(),
                RawJsonValue::Str(get_str_any(connection, &["DatabaseType"]).unwrap_or_default()),
            ),
            (
                "AdditionalProperties".to_string(),
                additional_properties(&overrides, connection),
            ),
        ]))
    }

    fn parameters_json(
        &self,
        action: &Document,
        query: &Document,
    ) -> Result<RawJsonValue, CompilerError> {
        let mappings: HashMap<String, String> = array_docs(action, &["ParameterMappings"])
            .iter()
            .map(|mapping| {
                (
                    get_str_any(mapping, &["ParameterName"])
                        .unwrap_or_default()
                        .to_lowercase(),
                    get_str_any(mapping, &["Value"]).unwrap_or_default(),
                )
            })
            .collect();
        let mut entries = Vec::new();
        for (index, parameter) in array_docs(query, &["Parameters"]).iter().enumerate() {
            let data_type_name = crate::support::get_doc_any(parameter, &["DataType"])
                .and_then(|d| get_str_any(&d, &["$Type"]))
                .unwrap_or_default();
            let scalar_type = data_type_name
                .strip_prefix("DataTypes$")
                .unwrap_or(&data_type_name)
                .strip_suffix("Type")
                .unwrap_or(&data_type_name)
                .to_uppercase();
            let parameter_name = get_str_any(parameter, &["ParameterName"]).unwrap_or_default();
            let expression = mappings
                .get(&parameter_name.to_lowercase())
                .cloned()
                .unwrap_or_default();
            let value = parameter_value(&scalar_type, &expression)?;
            let sql_data_type = crate::support::get_doc_any(parameter, &["SqlDataType"])
                .and_then(|d| get_str_any(&d, &["DataTypeName"]))
                .unwrap_or_default();
            entries.push((
                (index + 1).to_string(),
                RawJsonValue::Object(vec![
                    ("Name".to_string(), RawJsonValue::Str(parameter_name)),
                    ("DataType".to_string(), RawJsonValue::Str(scalar_type)),
                    ("SqlDataType".to_string(), RawJsonValue::Str(sql_data_type)),
                    ("Value".to_string(), RawJsonValue::Raw(value)),
                    (
                        "ParameterMode".to_string(),
                        RawJsonValue::Str(get_str_any(parameter, &["Mode"]).unwrap_or_default()),
                    ),
                    (
                        "DatabaseParameterName".to_string(),
                        RawJsonValue::Str(
                            get_str_any(parameter, &["DatabaseParameterName"]).unwrap_or_default(),
                        ),
                    ),
                    (
                        "ParameterEntityMapping".to_string(),
                        RawJsonValue::Raw("null".to_string()),
                    ),
                ]),
            ));
        }
        Ok(RawJsonValue::Object(entries))
    }
}

fn connection_parameter_overrides(action: &Document) -> HashMap<String, String> {
    array_docs(action, &["ConnectionParameterMappings"])
        .iter()
        .map(|mapping| {
            (
                get_str_any(mapping, &["ParameterName"])
                    .unwrap_or_default()
                    .to_lowercase(),
                get_str_any(mapping, &["Value"]).unwrap_or_default(),
            )
        })
        .collect()
}

/// `parameter`/`field` are already lower/PascalCase respectively — mirrors
/// `connection_value(overrides, connection, parameter, field)`.
fn connection_value(
    overrides: &HashMap<String, String>,
    connection: &Document,
    override_key: &str,
    field: &str,
) -> String {
    let expression = overrides.get(override_key).cloned().or_else(|| {
        let value = get_str_any(connection, &[field]).unwrap_or_default();
        (!value.is_empty()).then(|| format!("@{value}"))
    });
    let expression = expression.unwrap_or_else(|| "''".to_string());
    format!("'+({expression})+'")
}

fn additional_properties(
    overrides: &HashMap<String, String>,
    connection: &Document,
) -> RawJsonValue {
    let entries = array_docs(connection, &["AdditionalProperties"])
        .iter()
        .map(|property| {
            let key = get_str_any(property, &["Key"]).unwrap_or_default();
            let value_doc = crate::support::get_doc_any(property, &["Value"]).unwrap_or_default();
            let rendered = if let Some(expression) = overrides.get(&key.to_lowercase()) {
                RawJsonValue::Raw(format!("'+({expression})+'"))
            } else if get_str_any(&value_doc, &["$Type"]).as_deref()
                == Some("DatabaseConnector$ValueAsConstant")
            {
                let value = get_str_any(&value_doc, &["Value"]).unwrap_or_default();
                RawJsonValue::Raw(format!("'+(@{value})+'"))
            } else {
                RawJsonValue::Str(get_str_any(&value_doc, &["Value"]).unwrap_or_default())
            };
            (key, rendered)
        })
        .collect();
    RawJsonValue::Object(entries)
}

fn runtime_query(action: &Document, query: &Document) -> Result<String, CompilerError> {
    let dynamic = get_str_any(action, &["DynamicQuery"]).unwrap_or_default();
    if !dynamic.trim().is_empty() {
        return Ok(format!("'+({dynamic})+'"));
    }
    let sql = get_str_any(query, &["Query"]).unwrap_or_default();
    let sql = if sql.trim().is_empty() {
        query_builder_select(query)?
    } else {
        sql
    };
    Ok(sql.replace('\'', "''"))
}

fn query_builder_select(query: &Document) -> Result<String, CompilerError> {
    let Some(table) = array_docs(query, &["TableMappings"]).into_iter().next() else {
        return Err(CompilerError::UnbuildableDatabaseQuery);
    };
    let columns: Vec<String> = array_docs(&table, &["Columns"])
        .iter()
        .filter_map(|column| get_str_any(column, &["ColumnName"]))
        .map(|name| valid_identifier(&name))
        .collect::<Result<_, _>>()?;
    let name = get_str_any(&table, &["TableName"]).unwrap_or_default();
    let name = valid_identifier(&name)?;
    if columns.is_empty() {
        return Err(CompilerError::UnbuildableDatabaseQuery);
    }
    Ok(format!("SELECT {} FROM {name}", columns.join(", ")))
}

fn valid_identifier(value: &str) -> Result<String, CompilerError> {
    let is_valid = !value.is_empty()
        && value.split('.').all(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        });
    if is_valid {
        Ok(value.to_string())
    } else {
        Err(CompilerError::UnsafeDatabaseIdentifier {
            identifier: value.to_string(),
        })
    }
}

fn parameter_value(scalar_type: &str, expression: &str) -> Result<String, CompilerError> {
    Ok(match scalar_type {
        "DATETIME" => {
            format!("'+(if ({expression}) = NULL then NULL else dateTimeToEpoch({expression}))+'")
        }
        "BOOLEAN" => format!("'+toString({expression})+'"),
        "STRING" => format!(
            "'+(if ({expression}) = NULL then NULL else (if ({expression}) = 'null' then \
             '903be4ca-ed8b-ddcb-a339-741c4088484a' else (urlEncode({expression}))))+'"
        ),
        "INTEGER" | "DECIMAL" => format!("'+({expression})+'"),
        other => {
            return Err(CompilerError::UnsupportedDatabaseParameterType {
                type_name: other.to_string(),
            });
        }
    })
}

fn expression_literal(value: &str) -> String {
    format!("'{value}'")
}

fn parameter_mapping(
    seed_id: &str,
    java_action: &str,
    name: &str,
    value: &str,
    index: usize,
) -> Document {
    mxrs_bson::doc! {
        "$ID": database_derived_id(seed_id, &format!("mapping-{index}")),
        "$Type": "Microflows$JavaActionParameterMapping",
        "Parameter": format!("{java_action}.{name}"),
        "Value": mxrs_bson::doc! {
            "$ID": database_derived_id(seed_id, &format!("value-{index}")),
            "$Type": "Microflows$BasicCodeActionParameterValue",
            "ValueExpression": value,
        },
    }
}

fn entity_parameter_mapping(
    seed_id: &str,
    java_action: &str,
    name: &str,
    entity: &str,
    index: usize,
) -> Document {
    mxrs_bson::doc! {
        "$ID": database_derived_id(seed_id, &format!("mapping-{index}")),
        "$Type": "Microflows$JavaActionParameterMapping",
        "Parameter": format!("{java_action}.{name}"),
        "Value": mxrs_bson::doc! {
            "$ID": database_derived_id(seed_id, &format!("value-{index}")),
            "$Type": "Microflows$EntityTypeCodeActionParameterValue",
            "Entity": entity,
            "ValueExpression": format!("'{entity}'"),
        },
    }
}

/// `java_action_for`'s full decision table
/// (`database_connector_action_compiler.rb:74-108`).
fn java_action_for(query: &Document) -> Result<&'static str, CompilerError> {
    let raw_sql = get_str_any(query, &["Query"]).unwrap_or_default();
    let sql = remove_comments(&raw_sql).to_lowercase();
    let mapped = !array_docs(query, &["TableMappings"]).is_empty();
    if selecting_sql(&sql) {
        return Ok(statement_action(mapped));
    }
    if !is_callable(query, &sql) {
        return Ok(statement_action(mapped));
    }
    if query_type(query) == 3 {
        return Ok("ExternalDatabaseConnector.ExecuteCallable");
    }
    Ok(if mapped {
        "ExternalDatabaseConnector.ExecuteCallableQuery"
    } else {
        "ExternalDatabaseConnector.ExecuteCallableStatement"
    })
}

fn statement_action(mapped: bool) -> &'static str {
    if mapped {
        "ExternalDatabaseConnector.ExecuteQuery"
    } else {
        "ExternalDatabaseConnector.ExecuteStatement"
    }
}

fn selecting_query(query: &Document) -> bool {
    let raw_sql = get_str_any(query, &["Query"]).unwrap_or_default();
    selecting_sql(&remove_comments(&raw_sql).to_lowercase())
}

fn selecting_sql(sql: &str) -> bool {
    (sql.starts_with("select ") && !sql.contains(" into ")) || sql.starts_with("with ")
}

fn is_callable(query: &Document, sql: &str) -> bool {
    let query_type = query_type(query);
    if query_type == 3 {
        return true;
    }
    if query_type != 0 {
        return false;
    }
    let trimmed = sql.trim_start();
    let trimmed = trimmed
        .strip_prefix('{')
        .map(str::trim_start)
        .unwrap_or(trimmed);
    trimmed.starts_with("call") || trimmed.starts_with("exec")
}

fn query_type(query: &Document) -> i64 {
    match crate::support::get_any(query, &["QueryType"]) {
        Some(mxrs_bson::Bson::Int32(i)) => *i as i64,
        Some(mxrs_bson::Bson::Int64(i)) => *i,
        Some(mxrs_bson::Bson::Double(d)) => *d as i64,
        Some(mxrs_bson::Bson::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Strips `/* ... */` block comments (across lines) and `-- ...` line
/// comments (to end of line only, no dotall) — mirrors
/// `remove_comments`'s exact regex, ported without relying on a global
/// dotall flag that would (wrongly) let a line comment eat everything
/// after it.
fn remove_comments(sql: &str) -> String {
    let pattern = regex::Regex::new(r"/\*[\s\S]*?\*/|--[^\n]*").expect("valid regex");
    pattern.replace_all(sql, "").trim().to_string()
}

fn rpartition(qualified: &str) -> (String, String) {
    match qualified.rfind('.') {
        Some(index) => (
            qualified[..index].to_string(),
            qualified[index + 1..].to_string(),
        ),
        None => (String::new(), qualified.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::{Bson, doc};

    fn connection_with_query(query: Document) -> Document {
        doc! {
            "$ID": "conn-1",
            "Name": "MainDb",
            "DatabaseType": "PostgreSQL",
            "UserName": "Sales.DbUser",
            "Password": "Sales.DbPassword",
            "ConnectionString": "Sales.DbConnectionString",
            "Queries": mxrs_bson::build_array(vec![Bson::Document(query)], 3),
        }
    }

    fn compiler_with<'a>(
        connections: &'a HashMap<String, Document>,
        constants: &'a HashMap<String, Document>,
    ) -> DatabaseConnectorCompiler<'a> {
        DatabaseConnectorCompiler::new(connections, constants)
    }

    #[test]
    fn resolve_query_finds_the_connection_and_query_by_qualified_name() {
        let query = doc! { "$ID": "q1", "Name": "GetOrders", "Query": "SELECT 1" };
        let mut connections = HashMap::new();
        connections.insert("Sales.MainDb".to_string(), connection_with_query(query));
        let constants = HashMap::new();
        let compiler = compiler_with(&connections, &constants);
        let (_conn, query) = compiler.resolve_query("Sales.MainDb.GetOrders").unwrap();
        assert_eq!(query.get_str("Name").unwrap(), "GetOrders");
    }

    #[test]
    fn resolve_query_errors_loudly_on_an_unknown_connection() {
        let connections = HashMap::new();
        let constants = HashMap::new();
        let compiler = compiler_with(&connections, &constants);
        let err = compiler.resolve_query("Sales.Missing.Q").unwrap_err();
        assert!(matches!(
            err,
            CompilerError::UnknownDatabaseConnection { .. }
        ));
    }

    #[test]
    fn resolve_query_errors_loudly_on_an_unknown_query() {
        let query = doc! { "$ID": "q1", "Name": "GetOrders", "Query": "SELECT 1" };
        let mut connections = HashMap::new();
        connections.insert("Sales.MainDb".to_string(), connection_with_query(query));
        let constants = HashMap::new();
        let compiler = compiler_with(&connections, &constants);
        let err = compiler.resolve_query("Sales.MainDb.Bogus").unwrap_err();
        assert!(matches!(err, CompilerError::UnknownDatabaseQuery { .. }));
    }

    #[test]
    fn java_action_for_picks_execute_statement_for_an_unmapped_select() {
        let query = doc! { "Query": "select * from orders" };
        assert_eq!(
            java_action_for(&query).unwrap(),
            "ExternalDatabaseConnector.ExecuteStatement"
        );
    }

    #[test]
    fn java_action_for_picks_execute_query_for_a_mapped_select() {
        let query = doc! {
            "Query": "select * from orders",
            "TableMappings": mxrs_bson::build_array(vec![Bson::Document(doc! { "Entity": "Sales.Order" })], 3),
        };
        assert_eq!(
            java_action_for(&query).unwrap(),
            "ExternalDatabaseConnector.ExecuteQuery"
        );
    }

    #[test]
    fn java_action_for_detects_a_callable_statement_via_query_type() {
        let query = doc! { "Query": "sp_do_something", "QueryType": 3_i32 };
        assert_eq!(
            java_action_for(&query).unwrap(),
            "ExternalDatabaseConnector.ExecuteCallable"
        );
    }

    #[test]
    fn java_action_for_detects_a_callable_statement_via_the_call_keyword() {
        let query = doc! { "Query": "{ call sp_do_something() }", "QueryType": 0_i32 };
        assert_eq!(
            java_action_for(&query).unwrap(),
            "ExternalDatabaseConnector.ExecuteCallableStatement"
        );
    }

    #[test]
    fn selecting_query_ignores_sql_comments() {
        let query = doc! { "Query": "-- a comment\nselect * from orders" };
        assert!(selecting_query(&query));
    }

    #[test]
    fn valid_identifier_accepts_dotted_names_and_rejects_unsafe_ones() {
        assert_eq!(valid_identifier("orders").unwrap(), "orders");
        assert_eq!(valid_identifier("schema.orders").unwrap(), "schema.orders");
        assert!(valid_identifier("orders; DROP TABLE x").is_err());
    }

    #[test]
    fn parameter_value_encodes_string_with_the_null_sentinel() {
        let value = parameter_value("STRING", "$Param").unwrap();
        assert!(value.contains("903be4ca-ed8b-ddcb-a339-741c4088484a"));
    }

    #[test]
    fn parameter_value_rejects_an_unsupported_type() {
        let err = parameter_value("BLOB", "$Param").unwrap_err();
        assert!(matches!(
            err,
            CompilerError::UnsupportedDatabaseParameterType { .. }
        ));
    }

    #[test]
    fn is_unconfigured_write_is_false_for_a_selecting_query() {
        let query = doc! { "$ID": "q1", "Name": "GetOrders", "Query": "select * from orders" };
        let mut connections = HashMap::new();
        connections.insert("Sales.MainDb".to_string(), connection_with_query(query));
        let constants = HashMap::new();
        let compiler = compiler_with(&connections, &constants);
        let action = doc! {
            "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
            "Query": "Sales.MainDb.GetOrders",
        };
        assert!(!compiler.is_unconfigured_write(&action).unwrap());
    }

    #[test]
    fn is_unconfigured_write_is_true_for_a_write_with_no_override_and_an_empty_constant() {
        let query =
            doc! { "$ID": "q1", "Name": "DoInsert", "Query": "insert into orders values (1)" };
        let mut connections = HashMap::new();
        connections.insert("Sales.MainDb".to_string(), connection_with_query(query));
        let mut constants = HashMap::new();
        constants.insert(
            "Sales.DbConnectionString".to_string(),
            doc! { "Name": "DbConnectionString", "DefaultValue": "" },
        );
        let compiler = compiler_with(&connections, &constants);
        let action = doc! {
            "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
            "Query": "Sales.MainDb.DoInsert",
        };
        assert!(compiler.is_unconfigured_write(&action).unwrap());
    }

    #[test]
    fn is_unconfigured_write_is_false_when_the_constant_has_a_default_value() {
        let query =
            doc! { "$ID": "q1", "Name": "DoInsert", "Query": "insert into orders values (1)" };
        let mut connections = HashMap::new();
        connections.insert("Sales.MainDb".to_string(), connection_with_query(query));
        let mut constants = HashMap::new();
        constants.insert(
            "Sales.DbConnectionString".to_string(),
            doc! { "Name": "DbConnectionString", "DefaultValue": "jdbc:postgresql://host/db" },
        );
        let compiler = compiler_with(&connections, &constants);
        let action = doc! {
            "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
            "Query": "Sales.MainDb.DoInsert",
        };
        assert!(!compiler.is_unconfigured_write(&action).unwrap());
    }

    #[test]
    fn is_unconfigured_write_is_false_when_an_override_provides_a_connection_string() {
        let query =
            doc! { "$ID": "q1", "Name": "DoInsert", "Query": "insert into orders values (1)" };
        let mut connections = HashMap::new();
        connections.insert("Sales.MainDb".to_string(), connection_with_query(query));
        let mut constants = HashMap::new();
        constants.insert(
            "Sales.DbConnectionString".to_string(),
            doc! { "Name": "DbConnectionString", "DefaultValue": "" },
        );
        let compiler = compiler_with(&connections, &constants);
        let action = doc! {
            "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
            "Query": "Sales.MainDb.DoInsert",
            "ConnectionParameterMappings": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "ParameterName": "DBSource", "Value": "'jdbc:postgresql://override/db'",
            })], 3),
        };
        assert!(!compiler.is_unconfigured_write(&action).unwrap());
    }

    #[test]
    fn is_unconfigured_write_is_false_for_a_non_database_action() {
        let connections = HashMap::new();
        let constants = HashMap::new();
        let compiler = compiler_with(&connections, &constants);
        let action = doc! { "$Type": "Microflows$LogMessageAction" };
        assert!(!compiler.is_unconfigured_write(&action).unwrap());
    }

    #[test]
    fn connection_name_for_splits_off_the_query_name() {
        let connections = HashMap::new();
        let constants = HashMap::new();
        let compiler = compiler_with(&connections, &constants);
        let action = doc! {
            "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
            "Query": "Sales.MainDb.DoInsert",
        };
        assert_eq!(compiler.connection_name_for(&action), "Sales.MainDb");
    }

    #[test]
    fn compiles_a_full_query_action_with_result_mappings() {
        let query = doc! {
            "$ID": "q1", "Name": "GetOrders", "Query": "select * from orders",
            "TableMappings": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "Entity": "Sales.Order",
                "Columns": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "ColumnName": "id", "Attribute": "Sales.Order.Number",
                })], 3),
            })], 3),
            "Parameters": mxrs_bson::build_array(vec![], 3),
        };
        let mut connections = HashMap::new();
        connections.insert("Sales.MainDb".to_string(), connection_with_query(query));
        let constants = HashMap::new();
        let compiler = compiler_with(&connections, &constants);
        let action = doc! {
            "$ID": "act-1",
            "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
            "Query": "Sales.MainDb.GetOrders",
            "ErrorHandlingType": "Rollback",
            "OutputVariableName": "orders",
        };
        let compiled = compiler.compile(&action).unwrap();
        assert_eq!(
            compiled.get_str("JavaAction").unwrap(),
            "ExternalDatabaseConnector.ExecuteQuery"
        );
        assert_eq!(compiled.get_str("ResultVariableName").unwrap(), "orders");
        let mappings = compiled.get_array("ParameterMappings").unwrap();
        assert_eq!(mappings.len(), 5, "3 basic + columnMapping + OutputEntity");
    }
}
