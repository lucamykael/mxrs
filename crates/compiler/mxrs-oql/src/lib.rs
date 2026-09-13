//! Conservative OQL tooling. Unsupported syntax returns an explicit reason;
//! no translation result is ever presented as executable physical SQL.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum OqlError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error("unsupported SQL dialect {0}")]
    UnsupportedDialect(String),
}

pub type Result<T> = std::result::Result<T, OqlError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    Ansi,
    PostgreSql,
    SqlServer,
}

impl Dialect {
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "ansi" => Ok(Self::Ansi),
            "postgres" | "postgresql" => Ok(Self::PostgreSql),
            "sqlserver" | "sql_server" | "sql-server" => Ok(Self::SqlServer),
            _ => Err(OqlError::UnsupportedDialect(value.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Query {
    pub id: String,
    pub qualified_name: String,
    pub oql: String,
    pub parameters: Vec<String>,
}

pub fn catalog(project: &mxrs_model::Project) -> Result<Vec<Query>> {
    let mut queries = Vec::new();
    for module in project.modules()? {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        for entity in module.entities() {
            let Some(oql) = entity
                .oql_query
                .as_ref()
                .filter(|query| !query.trim().is_empty())
            else {
                continue;
            };
            let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
            queries.push(Query {
                id: entity
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("{module_name}.{entity_name}")),
                qualified_name: format!("{module_name}.{entity_name}"),
                oql: oql.clone(),
                parameters: parameters(oql),
            });
        }
    }
    queries.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
    Ok(queries)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Projection {
    pub sql: Option<String>,
    pub dialect: Dialect,
    pub confidence: String,
    pub warnings: Vec<String>,
    pub parameters: Vec<String>,
}

impl Projection {
    pub fn supported(&self) -> bool {
        self.sql.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    Parameter,
    Space,
    Comment,
    Quoted,
    Symbol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    text: String,
}

pub fn parameters(source: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    tokenize(source)
        .into_iter()
        .filter(|token| token.kind == TokenKind::Parameter)
        .filter_map(|token| {
            let value = token.text.trim_start_matches('$').to_string();
            seen.insert(value.clone()).then_some(value)
        })
        .collect()
}

pub fn translate(source: &str, dialect: Dialect) -> Projection {
    let original_parameters = parameters(source);
    let mut tokens = tokenize(source);
    if let Some(warning) = validate(&tokens) {
        return unsupported(dialect, original_parameters, warning);
    }
    if has_association_source(&tokens) {
        return unsupported(
            dialect,
            original_parameters,
            "association-path JOINs require physical runtime metadata",
        );
    }
    reorder_from_first(&mut tokens);
    let aliases = translate_entities(&mut tokens, dialect);
    translate_attributes(&mut tokens, dialect, &aliases);
    for token in &mut tokens {
        if token.kind == TokenKind::Parameter {
            let name = token.text.trim_start_matches('$');
            token.text = match dialect {
                Dialect::SqlServer => format!("@{name}"),
                Dialect::Ansi | Dialect::PostgreSql => format!(":{name}"),
            };
        }
    }
    Projection {
        sql: Some(
            tokens
                .into_iter()
                .map(|token| token.text)
                .collect::<String>()
                .trim()
                .to_string(),
        ),
        dialect,
        confidence: "logical".to_string(),
        warnings: vec![
            "Logical projection only; Mendix Runtime storage mappings are not applied.".to_string(),
        ],
        parameters: original_parameters,
    }
}

fn unsupported(dialect: Dialect, parameters: Vec<String>, warning: &str) -> Projection {
    Projection {
        sql: None,
        dialect,
        confidence: "unsupported".to_string(),
        warnings: vec![warning.to_string()],
        parameters,
    }
}

fn validate(tokens: &[Token]) -> Option<&'static str> {
    let significant = significant(tokens);
    let first = significant
        .first()
        .map(|index| tokens[*index].text.to_ascii_uppercase());
    if !matches!(first.as_deref(), Some("SELECT" | "FROM")) {
        return Some("only read-only OQL SELECT queries can be projected");
    }
    if significant.iter().any(|index| tokens[*index].text == ";") {
        return Some("multiple OQL statements are not accepted");
    }
    None
}

fn has_association_source(tokens: &[Token]) -> bool {
    let significant = significant(tokens);
    for (position, index) in significant.iter().enumerate() {
        if tokens[*index].kind != TokenKind::Word
            || !matches!(
                tokens[*index].text.to_ascii_uppercase().as_str(),
                "FROM" | "JOIN"
            )
        {
            continue;
        }
        let Some(first) = significant.get(position + 1) else {
            continue;
        };
        let Some(separator) = significant.get(position + 2) else {
            continue;
        };
        let association_after_entity = significant
            .get(position + 4)
            .is_some_and(|index| tokens[*index].text == "/");
        if tokens[*first].text != "("
            && (tokens[*separator].text == "/" || association_after_entity)
        {
            return true;
        }
    }
    false
}

fn reorder_from_first(tokens: &mut Vec<Token>) {
    let significant = significant(tokens);
    let Some(first) = significant.first().copied() else {
        return;
    };
    if !tokens[first].text.eq_ignore_ascii_case("FROM") {
        return;
    }
    let Some(select) = top_level_keyword(tokens, "SELECT", 0) else {
        return;
    };
    let tail = ["ORDER", "LIMIT", "OFFSET", "UNION"]
        .iter()
        .filter_map(|keyword| top_level_keyword(tokens, keyword, select + 1))
        .min()
        .unwrap_or(tokens.len());
    let mut reordered = tokens[select..tail].to_vec();
    reordered.push(Token {
        kind: TokenKind::Space,
        text: "\n".to_string(),
    });
    reordered.extend_from_slice(&tokens[..select]);
    reordered.extend_from_slice(&tokens[tail..]);
    *tokens = reordered;
}

fn translate_entities(tokens: &mut [Token], dialect: Dialect) -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    let indices = significant(tokens);
    for (position, index) in indices.iter().copied().enumerate() {
        if tokens[index].kind != TokenKind::Word
            || !matches!(
                tokens[index].text.to_ascii_uppercase().as_str(),
                "FROM" | "JOIN"
            )
        {
            continue;
        }
        let (Some(first), Some(dot), Some(entity)) = (
            indices.get(position + 1).copied(),
            indices.get(position + 2).copied(),
            indices.get(position + 3).copied(),
        ) else {
            continue;
        };
        if tokens[dot].text != "." || !identifier(&tokens[first]) || !identifier(&tokens[entity]) {
            continue;
        }
        let module = identifier_text(&tokens[first]);
        let entity_name = identifier_text(&tokens[entity]);
        tokens[first].text = quote(&format!("{module}${entity_name}"), dialect);
        tokens[dot].text.clear();
        tokens[entity].text.clear();
        let alias_index = indices.get(position + 4).copied().and_then(|next| {
            if tokens[next].text.eq_ignore_ascii_case("AS") {
                indices.get(position + 5).copied()
            } else {
                Some(next)
            }
        });
        let alias = alias_index
            .filter(|next| identifier(&tokens[*next]))
            .map(|next| identifier_text(&tokens[next]))
            .filter(|value| !stop_word(value))
            .unwrap_or_else(|| entity_name.clone());
        aliases.insert(alias.to_ascii_lowercase(), entity_name);
    }
    aliases
}

fn translate_attributes(
    tokens: &mut [Token],
    dialect: Dialect,
    aliases: &BTreeMap<String, String>,
) {
    let indices = significant(tokens);
    for window in indices.windows(3) {
        let [left, separator, attribute] = *window else {
            continue;
        };
        if tokens[separator].text != "/"
            || !identifier(&tokens[left])
            || !identifier(&tokens[attribute])
        {
            continue;
        }
        if aliases.contains_key(&identifier_text(&tokens[left]).to_ascii_lowercase()) {
            tokens[separator].text = ".".to_string();
            tokens[attribute].text = quote(&identifier_text(&tokens[attribute]), dialect);
        }
    }
}

fn top_level_keyword(tokens: &[Token], keyword: &str, after: usize) -> Option<usize> {
    let mut depth = 0_i32;
    for (index, token) in tokens.iter().enumerate() {
        if token.text == "(" {
            depth += 1;
        }
        if token.text == ")" {
            depth -= 1;
        }
        if index >= after
            && depth == 0
            && token.kind == TokenKind::Word
            && token.text.eq_ignore_ascii_case(keyword)
        {
            return Some(index);
        }
    }
    None
}

fn significant(tokens: &[Token]) -> Vec<usize> {
    tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| !matches!(token.kind, TokenKind::Space | TokenKind::Comment))
        .map(|(index, _)| index)
        .collect()
}

fn stop_word(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "FULL"
            | "GROUP"
            | "HAVING"
            | "INNER"
            | "JOIN"
            | "LEFT"
            | "LIMIT"
            | "OFFSET"
            | "ON"
            | "ORDER"
            | "OUTER"
            | "RIGHT"
            | "SELECT"
            | "UNION"
            | "WHERE"
    )
}

fn identifier(token: &Token) -> bool {
    matches!(token.kind, TokenKind::Word | TokenKind::Quoted)
}

fn identifier_text(token: &Token) -> String {
    token
        .text
        .trim_matches('"')
        .trim_matches('[')
        .trim_matches(']')
        .to_string()
}

fn quote(value: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::SqlServer => format!("[{}]", value.replace(']', "]]")),
        Dialect::Ansi | Dialect::PostgreSql => format!("\"{}\"", value.replace('"', "\"\"")),
    }
}

fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        let byte = bytes[index];
        let (kind, end) = if byte.is_ascii_whitespace() {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            (TokenKind::Space, index)
        } else if byte == b'\'' || byte == b'"' {
            index = quoted_end(bytes, index, byte);
            (TokenKind::Quoted, index)
        } else if byte == b'[' {
            index = bracket_end(bytes, index);
            (TokenKind::Quoted, index)
        } else if bytes.get(index..index + 2) == Some(b"--") {
            index = bytes[index..]
                .iter()
                .position(|value| *value == b'\n')
                .map_or(bytes.len(), |offset| index + offset);
            (TokenKind::Comment, index)
        } else if bytes.get(index..index + 2) == Some(b"/*") {
            index = bytes[index + 2..]
                .windows(2)
                .position(|value| value == b"*/")
                .map_or(bytes.len(), |offset| index + offset + 4);
            (TokenKind::Comment, index)
        } else if byte == b'$' && bytes.get(index + 1).is_some_and(u8::is_ascii_alphabetic) {
            index += 2;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'$'))
            {
                index += 1;
            }
            (TokenKind::Parameter, index)
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'$'))
            {
                index += 1;
            }
            (TokenKind::Word, index)
        } else {
            index += 1;
            (TokenKind::Symbol, index)
        };
        tokens.push(Token {
            kind,
            text: source[start..end].to_string(),
        });
    }
    tokens
}

fn quoted_end(bytes: &[u8], mut index: usize, delimiter: u8) -> usize {
    index += 1;
    while index < bytes.len() {
        if bytes[index] == delimiter {
            index += 1;
            if bytes.get(index) == Some(&delimiter) {
                index += 1;
            } else {
                break;
            }
        } else {
            index += 1;
        }
    }
    index
}

fn bracket_end(bytes: &[u8], mut index: usize) -> usize {
    index += 1;
    while index < bytes.len() {
        if bytes[index] == b']' {
            index += 1;
            if bytes.get(index) == Some(&b']') {
                index += 1;
            } else {
                break;
            }
        } else {
            index += 1;
        }
    }
    index
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finding {
    pub rule: String,
    pub severity: String,
    pub fragment: String,
    pub message: String,
}

pub fn analyze(source: &str) -> Vec<Finding> {
    let upper = source.to_ascii_uppercase();
    let mut findings = Vec::new();
    if upper.contains("SELECT *") {
        findings.push(finding(
            "select_star",
            "hint",
            "*",
            "Project only columns required by the caller.",
        ));
    }
    if let Some(position) = upper.find(" LIKE '") {
        let pattern = &source[position + 7..];
        if pattern.starts_with('%') {
            findings.push(finding(
                "like_leading_wildcard",
                "warning",
                "LIKE",
                "A leading wildcard usually prevents ordinary index seeks.",
            ));
        }
    }
    if let Some(from) = upper.find(" FROM ") {
        let tail = &upper[from + 6..];
        let end = [" WHERE ", " GROUP ", " ORDER ", " LIMIT "]
            .iter()
            .filter_map(|clause| tail.find(clause))
            .min()
            .unwrap_or(tail.len());
        if tail[..end].contains(',') && !tail[..end].contains(" JOIN ") {
            findings.push(finding(
                "cartesian_join",
                "error",
                "FROM",
                "Comma-separated entities without JOIN risk a Cartesian product.",
            ));
        }
    }
    findings
}

fn finding(rule: &str, severity: &str, fragment: &str, message: &str) -> Finding {
    Finding {
        rule: rule.to_string(),
        severity: severity.to_string(),
        fragment: fragment.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_ignore_strings_comments_and_deduplicate() {
        assert_eq!(
            parameters("SELECT '$ignored', $real -- $ignored\n, $real"),
            ["real"]
        );
    }

    #[test]
    fn select_first_translates_entity_attribute_and_parameter() {
        let projected = translate(
            "SELECT o/Number FROM Sales.Order AS o WHERE o/Number = $number",
            Dialect::PostgreSql,
        );
        assert_eq!(
            projected.sql.as_deref(),
            Some("SELECT o.\"Number\" FROM \"Sales$Order\" AS o WHERE o.\"Number\" = :number")
        );
    }

    #[test]
    fn from_first_oql_is_reordered_and_sql_server_quoted() {
        let projected = translate("FROM Sales.Order o SELECT o/Number", Dialect::SqlServer);
        assert_eq!(
            projected.sql.as_deref(),
            Some("SELECT o.[Number]\nFROM [Sales$Order] o")
        );
    }

    #[test]
    fn mutation_multiple_statements_and_association_sources_fail_closed() {
        assert!(!translate("DELETE FROM Sales.Order", Dialect::Ansi).supported());
        assert!(!translate("SELECT * FROM Sales.Order; SELECT 1", Dialect::Ansi).supported());
        assert!(!translate("SELECT * FROM Sales.Order/Customer", Dialect::Ansi).supported());
    }

    #[test]
    fn analyzer_reports_scan_and_projection_risks() {
        let findings =
            analyze("SELECT * FROM Sales.Order o, CRM.Customer c WHERE o/Name LIKE '%x'");
        assert_eq!(findings.len(), 3);
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "cartesian_join")
        );
    }

    #[test]
    fn dialect_parser_accepts_aliases_and_rejects_unknown_values() {
        assert_eq!(Dialect::parse("postgres").unwrap(), Dialect::PostgreSql);
        assert!(Dialect::parse("oracle").is_err());
    }
}
