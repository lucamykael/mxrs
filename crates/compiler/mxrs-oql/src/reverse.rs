//! SQL back into logical OQL: mxrb's `Oql::ReverseTranslator`.
//!
//! A conservative, read-only subset converts: one `SELECT` whose sources
//! map to `Module.Entity` — logical names, or the Runtime's physical
//! `module$entity` — with the functions and operators OQL has. Anything
//! else is refused with the reason, never guessed. Given the project, the
//! entities and attributes take their canonical casing; without it, a
//! physical name's casing is inferred and said to be.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::Dialect;
use crate::lexer::{Kind, Token, tokenize};

const STOP_CLAUSES: &[&str] = &[
    "GROUP", "HAVING", "LIMIT", "OFFSET", "ORDER", "UNION", "WHERE",
];
const ALIAS_STOP_WORDS: &[&str] = &[
    "FULL", "GROUP", "HAVING", "INNER", "JOIN", "LEFT", "LIMIT", "OFFSET", "ON", "ORDER", "OUTER",
    "RIGHT", "SELECT", "UNION", "WHERE", "CROSS", "FETCH", "FOR",
];
const REJECTED_WORDS: &[&str] = &[
    "ALTER",
    "APPLY",
    "CALL",
    "COLLATE",
    "CREATE",
    "CROSS",
    "DELETE",
    "DROP",
    "EXEC",
    "EXECUTE",
    "FETCH",
    "FILTER",
    "FOR",
    "GRANT",
    "INSERT",
    "INTO",
    "LATERAL",
    "MERGE",
    "NATURAL",
    "NULLS",
    "OVER",
    "QUALIFY",
    "RETURNING",
    "REVOKE",
    "ROWS",
    "TOP",
    "TRUNCATE",
    "UNNEST",
    "UPDATE",
    "UPSERT",
    "VALUES",
    "WINDOW",
    "WITH",
];
const FUNCTIONS: &[&str] = &[
    "AVG",
    "CAST",
    "COALESCE",
    "COUNT",
    "DATEADD",
    "DATEDIFF",
    "DATEFORMAT",
    "DATEPARSE",
    "DATEPART",
    "DATETRUNC",
    "LENGTH",
    "LOCATE",
    "LOWER",
    "LPAD",
    "LTRIM",
    "MAX",
    "MIN",
    "RANGEBEGIN",
    "RANGEEND",
    "REPLACE",
    "ROUND",
    "RPAD",
    "RTRIM",
    "STRING_AGG",
    "SUBSTRING",
    "SUM",
    "TRIM",
    "UPPER",
];
const FUNCTION_LIKE_OPERATORS: &[&str] = &[
    "AS", "BOOLEAN", "CASE", "DATETIME", "DECIMAL", "EXISTS", "FLOAT", "FROM", "HAVING", "IN",
    "INTEGER", "JOIN", "LONG", "ON", "SELECT", "STRING", "WHEN", "WHERE",
];
const RESERVED: &[&str] = &[
    "ALL",
    "AND",
    "AS",
    "ASC",
    "AVG",
    "BOOLEAN",
    "BY",
    "CASE",
    "CAST",
    "COUNT",
    "DATETIME",
    "DAY",
    "DECIMAL",
    "DESC",
    "DISTINCT",
    "ELSE",
    "END",
    "EXISTS",
    "FALSE",
    "FLOAT",
    "FROM",
    "FULL",
    "GROUP",
    "HAVING",
    "HOUR",
    "IN",
    "INNER",
    "INTEGER",
    "IS",
    "JOIN",
    "KEY",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LONG",
    "MATCHED",
    "MAX",
    "MERGE",
    "MILLISECOND",
    "MIN",
    "MINUTE",
    "MONTH",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "QUARTER",
    "REPLACE",
    "RIGHT",
    "SECOND",
    "SELECT",
    "SET",
    "SOURCE",
    "STRING",
    "STRING_AGG",
    "SUM",
    "TARGET",
    "THEN",
    "TRUE",
    "UNION",
    "UPDATE",
    "UPSERT",
    "USING",
    "VALUES",
    "WEEK",
    "WEEKDAY",
    "WHEN",
    "WHERE",
    "WITH",
    "YEAR",
];
const SCHEMAS: &[&str] = &["dbo", "public"];

/// SQL converted to OQL, or why it was not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OqlProjection {
    pub sql: String,
    pub oql: Option<String>,
    pub dialect: Dialect,
    /// `logical` when every name is the model's, `inferred` when a casing
    /// was guessed from a physical name, `unsupported` when nothing came.
    pub confidence: &'static str,
    pub warnings: Vec<String>,
    pub parameters: Vec<String>,
}

impl OqlProjection {
    pub fn supported(&self) -> bool {
        self.oql.is_some()
    }
}

/// An entity a source may name: its qualified name, parts, and its
/// attributes by their lowercase names.
#[derive(Debug, Clone)]
struct EntityInfo {
    qualified_name: String,
    entity_name: String,
    attributes: HashMap<String, String>,
}

/// The project's entities and attributes, for canonical casing.
#[derive(Debug, Default, Clone)]
pub struct EntityCatalog {
    entries: HashMap<String, EntityInfo>,
}

impl EntityCatalog {
    pub fn of(project: &mxrs_model::Project) -> crate::Result<Self> {
        let mut entries = HashMap::new();
        for module in project.modules()? {
            let module_name = module.name.clone().unwrap_or_default();
            for entity in module.entities() {
                let qualified = entity.qualified_name.clone().unwrap_or_else(|| {
                    format!(
                        "{module_name}.{}",
                        entity.name.as_deref().unwrap_or_default()
                    )
                });
                let attributes = entity
                    .attributes
                    .iter()
                    .filter_map(|attribute| attribute.name.clone())
                    .map(|name| (name.to_lowercase(), name))
                    .collect();
                let info = entity_info(&qualified, attributes);
                entries.insert(qualified.to_lowercase(), info.clone());
                entries.insert(qualified.replace('.', "$").to_lowercase(), info);
            }
        }
        Ok(Self { entries })
    }

    fn get(&self, value: &str) -> Option<&EntityInfo> {
        self.entries.get(&value.to_lowercase())
    }
}

fn entity_info(qualified: &str, attributes: HashMap<String, String>) -> EntityInfo {
    let (module, entity) = qualified.split_once('.').unwrap_or((qualified, ""));
    EntityInfo {
        qualified_name: format!("{module}.{entity}"),
        entity_name: entity.to_string(),
        attributes,
    }
}

/// `source` as logical OQL, with the canonical names `catalog` holds.
pub fn sql_to_oql(source: &str, dialect: Dialect, catalog: &EntityCatalog) -> OqlProjection {
    let unsupported = |warning: String| OqlProjection {
        sql: source.to_string(),
        oql: None,
        dialect,
        confidence: "unsupported",
        warnings: vec![warning],
        parameters: Vec::new(),
    };
    let mut tokens = tokenize(source);
    for token in &mut tokens {
        if token.kind == Kind::Word {
            match token.text.to_ascii_uppercase().as_str() {
                "CHAR_LENGTH" | "LEN" => token.text = "LENGTH".to_string(),
                _ => {}
            }
        }
    }
    if let Some(warning) = validation_warning(&tokens) {
        return unsupported(warning);
    }
    let mut warnings = Vec::new();
    let aliases = match translate_tables(&mut tokens, &mut warnings, catalog) {
        Ok(aliases) => aliases,
        Err(warning) => return unsupported(warning),
    };
    translate_attributes(&mut tokens, &aliases);
    let parameters = match translate_parameters(&mut tokens) {
        Ok(parameters) => parameters,
        Err(warning) => return unsupported(warning),
    };
    translate_operators(&mut tokens);
    let oql = tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>()
        .trim()
        .to_string();
    let mut unique = Vec::new();
    for warning in warnings {
        if !unique.contains(&warning) {
            unique.push(warning);
        }
    }
    OqlProjection {
        sql: source.to_string(),
        oql: Some(oql),
        dialect,
        confidence: if unique.is_empty() {
            "logical"
        } else {
            "inferred"
        },
        warnings: unique,
        parameters,
    }
}

fn significant(token: &Token) -> bool {
    !matches!(token.kind, Kind::Space | Kind::Comment) && !token.text.is_empty()
}

fn significant_indices(tokens: &[Token]) -> Vec<usize> {
    (0..tokens.len())
        .filter(|&index| significant(&tokens[index]))
        .collect()
}

fn next_significant(tokens: &[Token], index: usize) -> Option<usize> {
    (index + 1..tokens.len()).find(|&candidate| significant(&tokens[candidate]))
}

fn identifier(token: &Token) -> bool {
    matches!(token.kind, Kind::Word | Kind::QuotedIdentifier)
}

fn identifier_text(token: &Token) -> String {
    let text = &token.text;
    if text.len() >= 2 && text.starts_with('[') && text.ends_with(']') {
        return text[1..text.len() - 1].replace("]]", "]");
    }
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        return text[1..text.len() - 1].replace("\"\"", "\"");
    }
    text.clone()
}

fn pairs(tokens: &[Token]) -> Vec<(&Token, &Token)> {
    significant_indices(tokens)
        .windows(2)
        .map(|pair| (&tokens[pair[0]], &tokens[pair[1]]))
        .collect()
}

fn validation_warning(tokens: &[Token]) -> Option<String> {
    let indices = significant_indices(tokens);
    let first = indices
        .first()
        .map(|&index| tokens[index].text.to_ascii_uppercase());
    if first.as_deref() == Some("WITH") {
        return Some("SQL keyword WITH has no safe OQL conversion".into());
    }
    if first.as_deref() != Some("SELECT") {
        return Some("only read-only SQL SELECT queries can be converted to OQL".into());
    }
    if indices.iter().any(|&index| tokens[index].text == ";") {
        return Some("multiple SQL statements are not accepted".into());
    }
    if let Some(&rejected) = indices.iter().find(|&&index| {
        tokens[index].kind == Kind::Word
            && REJECTED_WORDS.contains(&tokens[index].text.to_ascii_uppercase().as_str())
    }) {
        return Some(format!(
            "SQL keyword {} has no safe OQL conversion",
            tokens[rejected].text.to_ascii_uppercase()
        ));
    }
    let pairs = pairs(tokens);
    let adjacent_symbols = |first: &str, second: &str| {
        pairs
            .iter()
            .any(|(left, right)| left.text == first && right.text == second)
    };
    if adjacent_symbols(":", ":") {
        return Some(
            "PostgreSQL casts using :: are not supported; use CAST(expression AS type)".into(),
        );
    }
    if indices.iter().any(|&index| {
        tokens[index].kind == Kind::Word && tokens[index].text.eq_ignore_ascii_case("ILIKE")
    }) {
        return Some("SQL ILIKE has no database-independent OQL equivalent".into());
    }
    if pairs.iter().any(|(left, right)| {
        left.kind == Kind::Word
            && right.kind == Kind::Word
            && left.text.eq_ignore_ascii_case("DISTINCT")
            && right.text.eq_ignore_ascii_case("ON")
    }) {
        return Some("PostgreSQL DISTINCT ON has no OQL equivalent".into());
    }
    if pairs.iter().any(|(left, right)| {
        left.kind == Kind::Word
            && matches!(left.text.to_ascii_uppercase().as_str(), "FROM" | "JOIN")
            && right.text == "("
    }) {
        return Some("SQL table subqueries are outside the safe OQL conversion subset".into());
    }
    if let Some(function) = pairs.iter().find_map(|(name, opening)| {
        if !identifier(name) || opening.text != "(" {
            return None;
        }
        let function = identifier_text(name).to_uppercase();
        (!FUNCTIONS.contains(&function.as_str())
            && !FUNCTION_LIKE_OPERATORS.contains(&function.as_str()))
        .then_some(function)
    }) {
        return Some(format!(
            "SQL function {function} has no supported OQL equivalent"
        ));
    }
    if adjacent_symbols("|", "|")
        || adjacent_symbols("-", ">")
        || adjacent_symbols("#", ">")
        || indices
            .iter()
            .any(|&index| matches!(tokens[index].text.as_str(), "&" | "^"))
    {
        return Some("SQL concatenation and bitwise operators have no safe OQL conversion".into());
    }
    let positional = pairs.iter().any(|(left, right)| {
        left.text == "$" && right.text.len() == 1 && right.text.as_bytes()[0].is_ascii_digit()
    });
    if positional || indices.iter().any(|&index| tokens[index].text == "?") {
        return Some("positional SQL parameters are not supported; use named parameters".into());
    }
    None
}

fn translate_tables(
    tokens: &mut [Token],
    warnings: &mut Vec<String>,
    catalog: &EntityCatalog,
) -> Result<HashMap<String, EntityInfo>, String> {
    let mut aliases = HashMap::new();
    for start in table_start_indices(tokens) {
        let Some((info, finish, original, table_warnings)) = parse_table(tokens, start, catalog)
        else {
            return Err(format!(
                "SQL table {} cannot be mapped to Module.Entity",
                ruby_inspect(&tokens[start].text)
            ));
        };
        warnings.extend(table_warnings);
        tokens[start] = Token::new(Kind::Word, render_qualified(&info.qualified_name));
        for token in &mut tokens[start + 1..=finish] {
            *token = Token::new(Kind::Word, "");
        }
        let mut alias_index = next_significant(tokens, finish);
        if let Some(index) = alias_index
            && tokens[index].text.eq_ignore_ascii_case("AS")
        {
            alias_index = next_significant(tokens, index);
        }
        let explicit = alias_index.filter(|&index| {
            identifier(&tokens[index])
                && !ALIAS_STOP_WORDS.contains(&tokens[index].text.to_ascii_uppercase().as_str())
        });
        let alias = match explicit {
            Some(index) => identifier_text(&tokens[index]),
            None => info.entity_name.clone(),
        };
        aliases.insert(alias.to_lowercase(), info.clone());
        aliases.insert(original.to_lowercase(), info.clone());
        aliases.insert(info.qualified_name.to_lowercase(), info);
    }
    if aliases.is_empty() {
        return Err("SQL query does not reference a convertible Module.Entity source".into());
    }
    Ok(aliases)
}

fn table_start_indices(tokens: &[Token]) -> Vec<usize> {
    let mut starts: Vec<usize> = Vec::new();
    let mut active_from: BTreeMap<i64, bool> = BTreeMap::new();
    let mut join_constraint: BTreeMap<i64, bool> = BTreeMap::new();
    let mut depth: i64 = 0;
    let push = |starts: &mut Vec<usize>, index: Option<usize>| {
        if let Some(index) = index
            && !starts.contains(&index)
        {
            starts.push(index);
        }
    };
    for index in significant_indices(tokens) {
        let token = &tokens[index];
        if token.text == ")" {
            active_from.remove(&depth);
            depth -= 1;
            continue;
        }
        if token.kind == Kind::Word {
            match token.text.to_ascii_uppercase().as_str() {
                "FROM" => {
                    active_from.insert(depth, true);
                    join_constraint.insert(depth, false);
                    push(&mut starts, next_significant(tokens, index));
                }
                "JOIN" => {
                    join_constraint.insert(depth, false);
                    push(&mut starts, next_significant(tokens, index));
                }
                "ON" => {
                    join_constraint.insert(depth, true);
                }
                word if STOP_CLAUSES.contains(&word) => {
                    active_from.remove(&depth);
                    join_constraint.remove(&depth);
                }
                _ => {}
            }
        } else if token.text == ","
            && active_from.get(&depth).copied().unwrap_or(false)
            && !join_constraint.get(&depth).copied().unwrap_or(false)
        {
            push(&mut starts, next_significant(tokens, index));
        }
        if token.text == "(" {
            depth += 1;
        }
    }
    starts
}

type ParsedTable = (EntityInfo, usize, String, Vec<String>);

fn parse_table(tokens: &[Token], start: usize, catalog: &EntityCatalog) -> Option<ParsedTable> {
    let first = &tokens[start];
    if !identifier(first) {
        return None;
    }
    let first_text = identifier_text(first);
    let dot = next_significant(tokens, start);
    let second = dot.and_then(|dot| next_significant(tokens, dot));
    if let (Some(dot), Some(second)) = (dot, second)
        && tokens[dot].text == "."
        && identifier(&tokens[second])
    {
        let second_text = identifier_text(&tokens[second]);
        if second_text.contains('$')
            && (SCHEMAS.contains(&first_text.to_lowercase().as_str())
                || catalog.get(&second_text).is_some())
        {
            let (info, mut warnings) = resolve_table(&second_text, catalog);
            let info = info?;
            warnings.push(format!(
                "SQL schema {first_text} was removed from the logical OQL entity name"
            ));
            return Some((info, second, second_text, warnings));
        }
        let qualified = format!("{first_text}.{second_text}");
        let (info, warnings) = resolve_table(&qualified, catalog);
        return info.map(|info| (info, second, qualified, warnings));
    }
    let (info, warnings) = resolve_table(&first_text, catalog);
    info.map(|info| (info, start, first_text, warnings))
}

fn resolve_table(value: &str, catalog: &EntityCatalog) -> (Option<EntityInfo>, Vec<String>) {
    if let Some(canonical) = catalog.get(value) {
        return (Some(canonical.clone()), Vec::new());
    }
    let parts = if value.contains('$') {
        value.split_once('$')
    } else if value.contains('.') {
        value.split_once('.')
    } else {
        None
    };
    let Some((module, entity)) =
        parts.filter(|(module, entity)| !module.is_empty() && !entity.is_empty())
    else {
        return (None, Vec::new());
    };
    let info = entity_info(&format!("{module}.{entity}"), HashMap::new());
    let warnings = if value.contains('$') {
        vec![format!(
            "logical entity casing was inferred from physical table {value}; use --project for canonical names"
        )]
    } else {
        Vec::new()
    };
    (Some(info), warnings)
}

fn translate_attributes(tokens: &mut [Token], aliases: &HashMap<String, EntityInfo>) {
    for index in significant_indices(tokens) {
        if !identifier(&tokens[index]) {
            continue;
        }
        let Some(info) = aliases.get(&identifier_text(&tokens[index]).to_lowercase()) else {
            continue;
        };
        let Some(separator) = next_significant(tokens, index) else {
            continue;
        };
        let Some(attribute) = next_significant(tokens, separator) else {
            continue;
        };
        if tokens[separator].text != "."
            || !(identifier(&tokens[attribute]) || tokens[attribute].text == "*")
        {
            continue;
        }
        let name = if tokens[attribute].text == "*" {
            "*".to_string()
        } else {
            let written = identifier_text(&tokens[attribute]);
            let canonical = info
                .attributes
                .get(&written.to_lowercase())
                .cloned()
                .unwrap_or(written);
            quote_identifier(&canonical)
        };
        tokens[separator] = Token::new(Kind::PathSeparator, "/");
        tokens[attribute] = Token::new(Kind::Word, name);
    }
}

fn translate_parameters(tokens: &mut [Token]) -> Result<Vec<String>, String> {
    let mut parameters: Vec<String> = Vec::new();
    for index in 0..tokens.len() {
        if tokens[index].kind == Kind::Parameter {
            let name = tokens[index].text.trim_start_matches('$').to_string();
            if !parameters.contains(&name) {
                parameters.push(name);
            }
            continue;
        }
        if !matches!(tokens[index].text.as_str(), ":" | "@") {
            continue;
        }
        let Some(name_index) =
            next_significant(tokens, index).filter(|&at| tokens[at].kind == Kind::Word)
        else {
            return Err("named SQL parameter is missing its name".into());
        };
        let name = tokens[name_index].text.clone();
        tokens[index] = Token::new(Kind::Parameter, format!("${name}"));
        tokens[name_index] = Token::new(Kind::Word, "");
        if !parameters.contains(&name) {
            parameters.push(name);
        }
    }
    Ok(parameters)
}

fn translate_operators(tokens: &mut [Token]) {
    for token in tokens.iter_mut() {
        if token.kind == Kind::Symbol && token.text == "/" {
            *token = Token::new(Kind::Symbol, ":");
        }
    }
    let indices = significant_indices(tokens);
    for pair in indices.windows(2) {
        if tokens[pair[0]].text == "<" && tokens[pair[1]].text == ">" {
            tokens[pair[0]] = Token::new(Kind::Symbol, "!");
            tokens[pair[1]] = Token::new(Kind::Symbol, "=");
        }
    }
}

fn render_qualified(value: &str) -> String {
    match value.split_once('.') {
        Some((module, entity)) => {
            format!("{}.{}", quote_identifier(module), quote_identifier(entity))
        }
        None => quote_identifier(value),
    }
}

fn quote_identifier(value: &str) -> String {
    let plain = value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'));
    if plain && !RESERVED.contains(&value.to_ascii_uppercase().as_str()) {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('"', "\"\""))
    }
}

/// A string as Ruby's `inspect` writes it, as mxrb's messages quote one.
fn ruby_inspect(value: &str) -> String {
    let mut text = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => text.push_str("\\\""),
            '\\' => text.push_str("\\\\"),
            '\n' => text.push_str("\\n"),
            '\t' => text.push_str("\\t"),
            other => text.push(other),
        }
    }
    text.push('"');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oql(source: &str) -> OqlProjection {
        sql_to_oql(source, Dialect::PostgreSql, &EntityCatalog::default())
    }

    /// Names, paths, parameters and operators become OQL's; a physical
    /// name's casing is inferred and said to be.
    #[test]
    fn sql_becomes_logical_oql() {
        let projection = oql("SELECT o.Number, COUNT(*) FROM public.sales$order o \
             WHERE o.Total / 2 <> :limit AND o.Name = @name");
        assert_eq!(
            projection.oql.as_deref(),
            Some(
                "SELECT o/Number, COUNT(*) FROM sales.\"order\" o WHERE o/Total : 2 != $limit AND o/Name = $name"
            )
        );
        assert_eq!(projection.confidence, "inferred");
        assert_eq!(projection.parameters, ["limit", "name"]);
        assert_eq!(projection.warnings.len(), 2);
    }

    /// What has no safe OQL form is refused with why.
    #[test]
    fn what_oql_cannot_say_is_refused() {
        for (source, reason) in [
            ("DELETE FROM Sales.Order", "only read-only"),
            ("SELECT * FROM Sales.Order; SELECT 1", "multiple"),
            ("SELECT x::int FROM Sales.Order", "::"),
            ("SELECT * FROM (SELECT 1) t", "subqueries"),
            ("SELECT md5(Name) FROM Sales.Order", "function MD5"),
            ("SELECT Name || 'x' FROM Sales.Order", "concatenation"),
            ("SELECT * FROM Sales.Order WHERE Id = ?", "positional"),
            ("SELECT * FROM orders", "cannot be mapped"),
        ] {
            let projection = oql(source);
            assert!(!projection.supported(), "{source}");
            assert!(
                projection.warnings[0].contains(reason),
                "{source}: {:?}",
                projection.warnings
            );
        }
    }
}
