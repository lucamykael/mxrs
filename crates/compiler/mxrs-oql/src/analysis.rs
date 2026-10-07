//! What the model's OQL is, and what reads badly in it: mxrb's
//! `Oql::Catalog` and `Oql::Analyzer`.
//!
//! The catalog finds every OQL string the model stores — a node whose type
//! names OQL and holds a `Query`, a view entity's source document, any
//! `OqlQuery` field — named for the nearest named document holding it. The
//! analyzer is positional: each finding keeps the fragment of the source it
//! is about, as an editor would highlight it.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use mxrs_bson::{Bson, Document};
use regex::Regex;
use serde::Serialize;

use crate::{Dialect, Query, Result, parameters};

/// What a query is: a data set's, a view entity's, or any other OQL.
#[derive(Debug, Clone, Copy, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Dataset,
    ViewEntity,
    Oql,
    /// A query given on the command line as SQL.
    Sql,
}

impl QueryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            QueryKind::Dataset => "dataset",
            QueryKind::ViewEntity => "view_entity",
            QueryKind::Oql => "oql",
            QueryKind::Sql => "sql",
        }
    }
}

/// Every OQL query the model stores, by qualified name.
pub fn catalog(project: &mxrs_model::Project) -> Result<Vec<Query>> {
    let units = project.all_units()?;
    let containers: HashMap<&str, &str> = units
        .iter()
        .map(|unit| (unit.unit_id.as_str(), unit.container_id.as_str()))
        .collect();
    let modules: HashMap<String, String> = project
        .modules()?
        .into_iter()
        .filter_map(|module| Some((module.id, module.name?)))
        .collect();
    let mut queries = Vec::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        let module = ancestor_module(&unit.unit_id, &containers, &modules);
        let mut found = Vec::new();
        walk(
            &document,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut |node, path, owners| {
                let source_type = node.get_str("$Type").unwrap_or_default();
                if source_type.to_ascii_lowercase().contains("oql")
                    && let Ok(query) = node.get_str("Query")
                {
                    found.push((
                        query.to_string(),
                        extend(path, "Query"),
                        owners.last().copied(),
                    ));
                }
                if source_type == "DomainModels$ViewEntitySourceDocument"
                    && let Ok(query) = node.get_str("Oql")
                {
                    found.push((
                        query.to_string(),
                        extend(path, "Oql"),
                        owners.last().copied(),
                    ));
                }
                for (key, value) in node {
                    if key.eq_ignore_ascii_case("OqlQuery")
                        && let Bson::String(query) = value
                    {
                        found.push((query.clone(), extend(path, key), owners.last().copied()));
                    }
                }
            },
        );
        let mut seen = BTreeSet::new();
        let unit_type = document.get_str("$Type").unwrap_or_default();
        for (oql, path, owner) in found {
            if !seen.insert(path) {
                continue;
            }
            let owner_type = owner
                .and_then(|owner| owner.get_str("$Type").ok())
                .unwrap_or_default();
            let name = owner
                .and_then(|owner| {
                    owner
                        .get_str("Name")
                        .or_else(|_| owner.get_str("name"))
                        .ok()
                })
                .map(str::to_string)
                .unwrap_or_else(|| format!("OQL_{}", &unit.unit_id.replace('-', "")[..8]));
            let kind = if unit_type == "DataSets$DataSet" {
                QueryKind::Dataset
            } else if unit_type == "DomainModels$ViewEntitySourceDocument"
                || owner_type.to_ascii_lowercase().contains("entity")
            {
                QueryKind::ViewEntity
            } else {
                QueryKind::Oql
            };
            let id = owner
                .and_then(|owner| owner.get("$ID"))
                .and_then(mxrs_bson::extract_id)
                .unwrap_or_else(|| unit.unit_id.clone());
            queries.push(Query {
                id,
                qualified_name: match &module {
                    Some(module) => format!("{module}.{name}"),
                    None => name.clone(),
                },
                parameters: parameters(&oql),
                oql,
                kind,
            });
        }
    }
    queries.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
    Ok(queries)
}

fn extend(path: &[String], key: &str) -> Vec<String> {
    let mut path = path.to_vec();
    path.push(key.to_string());
    path
}

/// Visits every document under `node`, depth first, with its path and the
/// named documents it is in — itself included when it is named.
fn walk<'a>(
    node: &'a Document,
    path: &mut Vec<String>,
    owners: &mut Vec<&'a Document>,
    visit: &mut impl FnMut(&'a Document, &[String], &[&'a Document]),
) {
    let named = node
        .get("Name")
        .or_else(|| node.get("name"))
        .is_some_and(|name| !matches!(name, Bson::Null | Bson::Boolean(false)));
    if named {
        owners.push(node);
    }
    visit(node, path, owners);
    for (key, value) in node {
        match value {
            Bson::Document(child) => {
                path.push(key.clone());
                walk(child, path, owners, visit);
                path.pop();
            }
            Bson::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    if let Bson::Document(child) = item {
                        path.push(key.clone());
                        path.push(index.to_string());
                        walk(child, path, owners, visit);
                        path.truncate(path.len() - 2);
                    }
                }
            }
            _ => {}
        }
    }
    if named {
        owners.pop();
    }
}

/// The module a unit is in, through its containers; none for a unit outside
/// every module, or one whose containers loop.
fn ancestor_module(
    unit: &str,
    containers: &HashMap<&str, &str>,
    modules: &HashMap<String, String>,
) -> Option<String> {
    let mut current = unit;
    let mut visited = BTreeSet::new();
    loop {
        let container = *containers.get(current)?;
        if let Some(module) = modules.get(container) {
            return Some(module.clone());
        }
        if !visited.insert(container) {
            return None;
        }
        current = container;
    }
}

/// One thing that reads badly in a query: the rule, how bad, the fragment
/// it is about, why, and what to do instead on each dialect.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub severity: &'static str,
    pub fragment: String,
    pub message: &'static str,
    pub suggestions: Suggestions,
}

/// What to do instead, per dialect, in mxrb's order.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Suggestions {
    pub postgresql: &'static str,
    pub sql_server: &'static str,
    pub ansi: &'static str,
}

impl Suggestions {
    pub fn get(&self, dialect: Dialect) -> &'static str {
        match dialect {
            Dialect::PostgreSql => self.postgresql,
            Dialect::SqlServer => self.sql_server,
            Dialect::Ansi => self.ansi,
        }
    }
}

/// A query and what reads badly in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub name: String,
    pub kind: QueryKind,
    pub source: String,
    pub findings: Vec<Finding>,
}

impl Report {
    /// No finding is an error.
    pub fn clean(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.severity != "error")
    }

    /// Some finding is a warning.
    pub fn warnings(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == "warning")
    }
}

/// The report of a query of the model.
pub fn analyze_query(query: &Query) -> Report {
    Report {
        name: query.qualified_name.clone(),
        kind: query.kind,
        source: query.oql.clone(),
        findings: analyze(&query.oql),
    }
}

/// The report of a query given as text, OQL or SQL; it is named `AdHoc`.
pub fn analyze_source(source: &str, kind: QueryKind) -> Report {
    Report {
        name: "AdHoc".to_string(),
        kind,
        source: source.to_string(),
        findings: analyze(source),
    }
}

static LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bLIKE\s+'((?:''|[^'])*)'").expect("valid LIKE pattern"));
static WHERE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bWHERE\b").expect("valid WHERE pattern"));
static WHERE_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:GROUP\s+BY|ORDER\s+BY|HAVING|LIMIT|OFFSET|UNION)\b")
        .expect("valid WHERE stop pattern")
});
static FROM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bFROM\b").expect("valid FROM pattern"));
static FROM_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:WHERE|GROUP\s+BY|ORDER\s+BY|HAVING|LIMIT|OFFSET|UNION)\b")
        .expect("valid FROM stop pattern")
});
static FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:LOWER|UPPER|CAST)\s*\([^)]*\)").expect("valid function pattern")
});
static SELECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bSELECT\b(?:\s+DISTINCT\s+|\s+)").expect("valid SELECT pattern")
});
static JOIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bJOIN\b").expect("valid JOIN pattern"));
static STAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\b[A-Za-z_][A-Za-z0-9_$]*\s*/\s*)?\*").expect("valid star pattern")
});

/// What reads badly in `source`: wildcards a `LIKE` leads with, functions
/// over filtered columns, comma joins, and `SELECT *` — in that order, each
/// as often as it is written.
pub fn analyze(source: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for captures in LIKE.captures_iter(source) {
        let pattern = &captures[1];
        let fragment = &captures[0];
        let rule = match (pattern.starts_with('%'), pattern.ends_with('%')) {
            (true, true) => "like_both_wildcard",
            (true, false) => "like_leading_wildcard",
            (false, true) => "like_trailing_only",
            (false, false) => continue,
        };
        findings.push(finding(rule, fragment));
    }
    for body in clauses(source, &WHERE, &WHERE_END) {
        for function in FUNCTION.find_iter(&source[body.clone()]) {
            findings.push(finding("function_in_where", function.as_str()));
        }
    }
    let mut offset = 0;
    while let Some(keyword) = FROM.find_at(source, offset) {
        let end = FROM_END
            .find_at(source, keyword.end())
            .map_or(source.len(), |stop| stop.start());
        let body = &source[keyword.end()..end];
        if body.contains(',') && !JOIN.is_match(body) {
            findings.push(finding(
                "cartesian_join",
                source[keyword.start()..end].trim(),
            ));
        }
        offset = end.max(keyword.end());
    }
    let mut offset = 0;
    while let Some(keyword) = SELECT.find_at(source, offset) {
        // The list a `SELECT` projects ends where its `FROM` begins; one
        // with no `FROM` after it projects nothing to read.
        let Some(from) = FROM.find_at(source, keyword.end()) else {
            break;
        };
        let body = &source[keyword.end()..from.start()];
        let mut at = 0;
        while let Some(star) = STAR.find_at(body, at) {
            // A star a parenthesis opens on is an aggregate's, `COUNT(*)`.
            if body[..star.start()].ends_with('(') {
                at = star.start() + 1;
                continue;
            }
            findings.push(finding("select_star", star.as_str()));
            at = star.end();
        }
        offset = from.start();
    }
    findings
}

/// The bodies of the clauses `keyword` opens, each up to the first `end`
/// after it or the end of the source.
fn clauses(source: &str, keyword: &Regex, end: &Regex) -> Vec<std::ops::Range<usize>> {
    let mut bodies = Vec::new();
    let mut offset = 0;
    while let Some(found) = keyword.find_at(source, offset) {
        let stop = end
            .find_at(source, found.end())
            .map_or(source.len(), |stop| stop.start());
        bodies.push(found.end()..stop);
        offset = stop.max(found.end());
    }
    bodies
}

fn finding(rule: &'static str, fragment: &str) -> Finding {
    let (severity, message, suggestions) = definition(rule);
    Finding {
        rule,
        severity,
        fragment: fragment.to_string(),
        message,
        suggestions,
    }
}

/// Each rule's severity, message and per-dialect suggestion, kept together
/// so no rule is said without what to do instead.
fn definition(rule: &str) -> (&'static str, &'static str, Suggestions) {
    let (severity, message, postgresql, sql_server, ansi) = match rule {
        "like_leading_wildcard" => (
            "error",
            "A leading wildcard prevents ordinary index seeks.",
            "Use pg_trgm with a GIN/GiST index or full-text search.",
            "Use CONTAINS with a full-text index.",
            "Redesign the predicate or add a dedicated search index.",
        ),
        "like_both_wildcard" => (
            "warning",
            "Wildcards on both sides usually force a full scan.",
            "Use column % 'term' with the pg_trgm extension and an index.",
            "Use CONTAINS(column, 'term') with a full-text index.",
            "Use a search-specific index or redesign the lookup.",
        ),
        "like_trailing_only" => (
            "hint",
            "A trailing-only wildcard can use an index in many configurations.",
            "Keep the prefix search; confirm operator class, collation, and index use.",
            "Keep the prefix search; confirm collation and the execution plan.",
            "Keep the prefix search, but verify collation and index behavior.",
        ),
        "function_in_where" => (
            "warning",
            "Applying a function to a filtered column can make the predicate non-sargable.",
            "Prefer ILIKE where appropriate or create a matching functional index.",
            "Prefer a case-insensitive collation or an indexed computed column.",
            "Normalize data or compare against a separately indexed normalized column.",
        ),
        "cartesian_join" => (
            "error",
            "Comma-separated entities without an explicit JOIN risk a Cartesian product.",
            "Use an explicit JOIN with an ON predicate.",
            "Use an explicit JOIN with an ON predicate.",
            "Use an explicit JOIN with an ON predicate.",
        ),
        _ => (
            "hint",
            "Selecting every column increases transfer and couples callers to schema changes.",
            "Project only the columns required by the caller.",
            "Project only the columns required by the caller.",
            "Project only the columns required by the caller.",
        ),
    };
    (
        severity,
        message,
        Suggestions {
            postgresql,
            sql_server,
            ansi,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(source: &str) -> Vec<(&'static str, String)> {
        analyze(source)
            .into_iter()
            .map(|finding| (finding.rule, finding.fragment))
            .collect()
    }

    /// Each rule keeps the fragment it is about, in mxrb's order.
    #[test]
    fn findings_keep_the_fragment_they_are_about() {
        assert_eq!(
            rules(
                "SELECT *, o/Number FROM Sales.Order o, Sales.Line l \
                 WHERE lower(o/Name) LIKE '%a%' AND o/Code like 'x%' ORDER BY o/Number"
            ),
            [
                ("like_both_wildcard", "LIKE '%a%'".to_string()),
                ("like_trailing_only", "like 'x%'".to_string()),
                ("function_in_where", "lower(o/Name)".to_string()),
                (
                    "cartesian_join",
                    "FROM Sales.Order o, Sales.Line l".to_string()
                ),
                ("select_star", "*".to_string()),
            ]
        );
        assert_eq!(
            rules("SELECT COUNT(*), o/* FROM Sales.Order AS o JOIN o/Sales.Line AS l"),
            [("select_star", "o/*".to_string())]
        );
        assert_eq!(
            rules("SELECT Name FROM Sales.Order WHERE Name LIKE '%it''s'"),
            [("like_leading_wildcard", "LIKE '%it''s'".to_string())]
        );
        assert!(rules("SELECT * WHERE x").is_empty());
    }
}
