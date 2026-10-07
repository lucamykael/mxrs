//! OQL as logical SQL: mxrb's `Oql::Translator`.
//!
//! Tables are named as the Mendix Runtime names them (`"sales$order"`, or
//! `"Sales.Order"` in ANSI), attributes lowercase, parameters `:name`. The
//! names are inferred, never read from a database, and the projection says
//! so. An association path needs storage metadata a model alone lacks, and
//! is refused.

use crate::lexer::{Kind, Token, tokenize};
use crate::{Dialect, Projection};

const TAIL_CLAUSES: &[&str] = &["ORDER", "LIMIT", "OFFSET", "UNION"];
const ALIAS_STOP_WORDS: &[&str] = &[
    "FULL", "GROUP", "HAVING", "INNER", "JOIN", "LEFT", "LIMIT", "OFFSET", "ON", "ORDER", "OUTER",
    "RIGHT", "SELECT", "UNION", "WHERE",
];
const PHYSICAL_MAPPING: &str =
    "logical table and column names are inferred; verify them against the Runtime database schema";

/// The names of the parameters `source` uses, each once, in order.
pub fn parameters(source: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for token in tokenize(source) {
        if token.kind == Kind::Parameter {
            let name = token.text.trim_start_matches('$').to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// `source` as logical SQL in `dialect`.
pub fn translate(source: &str, dialect: Dialect) -> Projection {
    let parameters = parameters(source);
    let unsupported = |warning: &str| Projection {
        sql: None,
        dialect,
        confidence: "unsupported".to_string(),
        warnings: vec![warning.to_string()],
        parameters: parameters.clone(),
    };
    let tokens = tokenize(source);
    if let Some(warning) = validation_warning(&tokens) {
        return unsupported(warning);
    }
    let mut tokens = reorder_from_first(tokens);
    if association_path(&tokens) {
        return unsupported("association-path JOINs require Mendix Runtime storage metadata");
    }
    let aliases = translate_entities(&mut tokens, dialect);
    translate_attributes(&mut tokens, &aliases, dialect);
    for token in &mut tokens {
        if token.kind == Kind::Parameter {
            token.text = format!(":{}", token.text.trim_start_matches('$'));
        }
    }
    Projection {
        sql: Some(
            tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<String>()
                .trim()
                .to_string(),
        ),
        dialect,
        confidence: "logical".to_string(),
        warnings: vec![PHYSICAL_MAPPING.to_string()],
        parameters,
    }
}

fn significant(token: &Token) -> bool {
    !matches!(token.kind, Kind::Space | Kind::Comment)
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
    if token.kind != Kind::QuotedIdentifier || text.len() < 2 {
        return text.clone();
    }
    // Only a closed quotation is unquoted: one the source left open keeps
    // its text, and its delimiters, one byte each, bound the slice.
    if text.starts_with('[') && text.ends_with(']') {
        return text[1..text.len() - 1].replace("]]", "]");
    }
    if text.starts_with('"') && text.ends_with('"') {
        return text[1..text.len() - 1].replace("\"\"", "\"");
    }
    text.clone()
}

fn validation_warning(tokens: &[Token]) -> Option<&'static str> {
    let indices = significant_indices(tokens);
    let first = indices
        .first()
        .map(|&index| tokens[index].text.to_ascii_uppercase());
    if !matches!(first.as_deref(), Some("SELECT" | "FROM")) {
        return Some("only read-only OQL SELECT queries can be visualized");
    }
    if indices.iter().any(|&index| tokens[index].text == ";") {
        return Some("multiple OQL statements are not accepted");
    }
    None
}

/// `FROM ... SELECT ...` as `SELECT ...` then its `FROM`, the clauses after
/// the projection kept last.
fn reorder_from_first(tokens: Vec<Token>) -> Vec<Token> {
    let Some(&first) = significant_indices(&tokens).first() else {
        return tokens;
    };
    if !tokens[first].text.eq_ignore_ascii_case("FROM") {
        return tokens;
    }
    let Some(select) = top_level_keyword(&tokens, "SELECT", 0) else {
        return tokens;
    };
    let tail = TAIL_CLAUSES
        .iter()
        .filter_map(|keyword| top_level_keyword(&tokens, keyword, select + 1))
        .min()
        .unwrap_or(tokens.len());
    let mut reordered = tokens[select..tail].to_vec();
    reordered.push(Token::new(Kind::Space, "\n"));
    reordered.extend_from_slice(&tokens[..select]);
    reordered.extend_from_slice(&tokens[tail..]);
    reordered
}

fn top_level_keyword(tokens: &[Token], keyword: &str, after: usize) -> Option<usize> {
    let mut depth = 0_i64;
    for (index, token) in tokens.iter().enumerate() {
        if token.text == "(" {
            depth += 1;
        }
        if token.text == ")" {
            depth -= 1;
        }
        if index < after || depth != 0 || token.kind != Kind::Word {
            continue;
        }
        if token.text.eq_ignore_ascii_case(keyword) {
            return Some(index);
        }
    }
    None
}

fn source_keyword(token: &Token) -> bool {
    token.kind == Kind::Word
        && (token.text.eq_ignore_ascii_case("FROM") || token.text.eq_ignore_ascii_case("JOIN"))
}

fn association_path(tokens: &[Token]) -> bool {
    significant_indices(tokens).into_iter().any(|index| {
        if !source_keyword(&tokens[index]) {
            return false;
        }
        let Some(following) = next_significant(tokens, index) else {
            return false;
        };
        if tokens[following].text == "(" {
            return false;
        }
        let Some(separator) = next_significant(tokens, following) else {
            return false;
        };
        if tokens[separator].text == "/" {
            return true;
        }
        // `Module.Entity/Path` is a path too: mxrb reads past it and writes
        // SQL no database runs, mxrs refuses it as one.
        tokens[separator].text == "."
            && next_significant(tokens, separator)
                .and_then(|entity| next_significant(tokens, entity))
                .is_some_and(|after| tokens[after].text == "/")
    })
}

fn translate_entities(tokens: &mut [Token], dialect: Dialect) -> Vec<String> {
    let mut aliases = Vec::new();
    for index in significant_indices(tokens) {
        if !source_keyword(&tokens[index]) {
            continue;
        }
        let Some(first) = next_significant(tokens, index) else {
            continue;
        };
        if tokens[first].text == "(" {
            continue;
        }
        let Some(dot) = next_significant(tokens, first) else {
            continue;
        };
        let Some(entity) = next_significant(tokens, dot) else {
            continue;
        };
        if tokens[dot].text != "." || !identifier(&tokens[first]) || !identifier(&tokens[entity]) {
            continue;
        }
        let module_name = identifier_text(&tokens[first]);
        let entity_name = identifier_text(&tokens[entity]);
        tokens[first] = Token::new(
            Kind::QuotedIdentifier,
            quote_table(&module_name, &entity_name, dialect),
        );
        tokens[dot] = Token::new(Kind::Symbol, "");
        tokens[entity] = Token::new(Kind::Word, "");
        let mut alias_index = next_significant(tokens, entity);
        if let Some(at) = alias_index
            && tokens[at].text.eq_ignore_ascii_case("AS")
        {
            alias_index = next_significant(tokens, at);
        }
        let alias = match alias_index {
            Some(at)
                if identifier(&tokens[at])
                    && !ALIAS_STOP_WORDS
                        .contains(&tokens[at].text.to_ascii_uppercase().as_str()) =>
            {
                identifier_text(&tokens[at])
            }
            _ => entity_name,
        };
        aliases.push(alias.to_lowercase());
    }
    aliases
}

fn translate_attributes(tokens: &mut [Token], aliases: &[String], dialect: Dialect) {
    for index in significant_indices(tokens) {
        if !identifier(&tokens[index])
            || !aliases.contains(&identifier_text(&tokens[index]).to_lowercase())
        {
            continue;
        }
        let Some(separator) = next_significant(tokens, index) else {
            continue;
        };
        let Some(attribute) = next_significant(tokens, separator) else {
            continue;
        };
        if !matches!(tokens[separator].text.as_str(), "." | "/") || !identifier(&tokens[attribute])
        {
            continue;
        }
        let name = identifier_text(&tokens[attribute]);
        tokens[separator] = Token::new(Kind::Symbol, ".");
        tokens[attribute] = Token::new(Kind::QuotedIdentifier, quote_attribute(&name, dialect));
    }
}

fn quote_table(module_name: &str, entity_name: &str, dialect: Dialect) -> String {
    let logical = match dialect {
        Dialect::Ansi => format!("{module_name}.{entity_name}"),
        _ => format!(
            "{}${}",
            module_name.to_lowercase(),
            entity_name.to_lowercase()
        ),
    };
    quote_identifier(&logical, dialect)
}

fn quote_attribute(value: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::Ansi => quote_identifier(value, dialect),
        _ => quote_identifier(&value.to_lowercase(), dialect),
    }
}

fn quote_identifier(value: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::SqlServer => format!("[{}]", value.replace(']', "]]")),
        _ => format!("\"{}\"", value.replace('"', "\"\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entities, attributes and parameters take the Runtime's logical
    /// names, a query that opens with its `FROM` is read as SQL writes it,
    /// and the names are said to be inferred.
    #[test]
    fn oql_becomes_logical_sql() {
        let projection = translate(
            "FROM Sales.Order AS o SELECT o/Number WHERE o/Total > $min ORDER BY o/Number",
            Dialect::PostgreSql,
        );
        assert_eq!(
            projection.sql.as_deref(),
            Some(
                "SELECT o.\"number\" WHERE o.\"total\" > :min \nFROM \"sales$order\" AS o ORDER BY o.\"number\""
            )
        );
        assert_eq!(projection.parameters, ["min"]);
        assert_eq!(projection.warnings, [PHYSICAL_MAPPING]);
        let server = translate("SELECT Order/Name FROM Sales.Order", Dialect::SqlServer);
        assert_eq!(
            server.sql.as_deref(),
            Some("SELECT Order.[name] FROM [sales$order]")
        );
        let ansi = translate("SELECT Order/Name FROM Sales.Order", Dialect::Ansi);
        assert_eq!(
            ansi.sql.as_deref(),
            Some("SELECT Order.\"Name\" FROM \"Sales.Order\"")
        );
    }

    /// An identifier the source leaves unquoted at its end, after a
    /// character of several bytes, is read as written.
    #[test]
    fn an_open_quotation_is_read_as_written() {
        let projection = translate(
            "SELECT o/Name FROM Sales.Order AS o WHERE \"Salé",
            Dialect::PostgreSql,
        );
        assert!(projection.sql.unwrap().ends_with("\"Salé"));
    }

    /// What a model alone cannot say is refused, with why.
    #[test]
    fn what_needs_storage_metadata_is_refused() {
        for (source, reason) in [
            ("DELETE FROM Sales.Order", "read-only"),
            ("SELECT 1; SELECT 2", "multiple"),
            (
                "SELECT l/Q FROM Sales.Order/Sales.Line AS l",
                "association-path",
            ),
        ] {
            let projection = translate(source, Dialect::PostgreSql);
            assert!(projection.sql.is_none(), "{source}");
            assert!(projection.warnings[0].contains(reason), "{source}");
        }
    }
}
