//! Conservative OQL tooling. Unsupported syntax returns an explicit reason;
//! no translation result is ever presented as executable physical SQL.

pub mod plan;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

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

/// The relational names used by the MXRS runtime for one association. They
/// are derived from storage identities, never from display names, so renaming
/// a model artifact does not change a query's physical target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociationRelation {
    pub qualified_name: String,
    pub table: String,
    pub from_entity: String,
    pub from_table: String,
    pub to_entity: String,
    pub to_table: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntityRelation {
    qualified_name: String,
    table: String,
    columns: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RuntimeCatalog {
    entities: BTreeMap<String, EntityRelation>,
    associations: Vec<AssociationRelation>,
}

/// Derives the association tables using the same stable SHA-256 naming
/// contract as `mxrs-runtime-sqlite`. Keeping this data explicit is the
/// prerequisite for translating OQL association paths without guessing.
pub fn association_relations(project: &mxrs_model::Project) -> Result<Vec<AssociationRelation>> {
    Ok(runtime_catalog(project)?.associations)
}

fn runtime_catalog(project: &mxrs_model::Project) -> Result<RuntimeCatalog> {
    let modules = project.modules()?;
    let mut entity_names = BTreeMap::new();
    let mut entities = BTreeMap::new();
    for module in &modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        for entity in module.entities() {
            let Some(name) = entity.qualified_name.clone().or_else(|| {
                entity
                    .name
                    .as_ref()
                    .map(|name| format!("{module_name}.{name}"))
            }) else {
                continue;
            };
            if let Some(id) = entity.id.as_ref() {
                entity_names.insert(id.clone(), name.clone());
            }
            entity_names.insert(name.clone(), name.clone());
            if !entity.persistable || entity.oql_view() {
                continue;
            }
            let storage_key = entity
                .data_storage_guid
                .as_ref()
                .filter(|key| !key.is_empty())
                .cloned()
                .or_else(|| entity.id.as_ref().filter(|key| !key.is_empty()).cloned())
                .unwrap_or_else(|| name.clone());
            let columns = entity
                .attributes
                .iter()
                .filter_map(|attribute| {
                    let name = attribute.name.as_ref()?.clone();
                    let key = attribute
                        .data_storage_guid
                        .as_ref()
                        .filter(|key| !key.is_empty())
                        .cloned()
                        .or_else(|| attribute.id.as_ref().filter(|key| !key.is_empty()).cloned())
                        .unwrap_or_else(|| format!("{storage_key}:{name}"));
                    Some((name.to_ascii_lowercase(), physical_name("attribute", &key)))
                })
                .collect();
            entities.insert(
                name.clone(),
                EntityRelation {
                    qualified_name: name,
                    table: physical_name("entity", &storage_key),
                    columns,
                },
            );
        }
    }

    let mut relations = Vec::new();
    for module in &modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        for association in module.associations() {
            let Some(name) = association.name.as_deref() else {
                continue;
            };
            let Some(from) = association
                .from_entity_id
                .as_ref()
                .and_then(|id| entity_names.get(id))
            else {
                continue;
            };
            let Some(to) = association
                .to_entity_id
                .as_ref()
                .and_then(|target| entity_names.get(target))
            else {
                continue;
            };
            let (Some(from_relation), Some(to_relation)) = (entities.get(from), entities.get(to))
            else {
                continue;
            };
            let qualified_name = format!("{module_name}.{name}");
            let storage_key = association
                .id
                .clone()
                .unwrap_or_else(|| qualified_name.clone());
            relations.push(AssociationRelation {
                qualified_name,
                table: physical_name("association", &storage_key),
                from_entity: from.clone(),
                from_table: from_relation.table.clone(),
                to_entity: to.clone(),
                to_table: to_relation.table.clone(),
            });
        }
    }
    relations.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
    Ok(RuntimeCatalog {
        entities,
        associations: relations,
    })
}

fn physical_name(kind: &str, key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("mxrb_{kind}_{}", &hex[..20])
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
    translate_with_catalog(source, dialect, None)
}

/// Projects OQL stored in an MPR onto the physical tables created by MXRS's
/// SQLite runtime. Unlike [`translate`], this mode has the storage identities
/// needed to expand an association-path `JOIN` without guessing.
pub fn translate_project(
    source: &str,
    dialect: Dialect,
    project: &mxrs_model::Project,
) -> Result<Projection> {
    let catalog = runtime_catalog(project)?;
    Ok(translate_with_catalog(source, dialect, Some(&catalog)))
}

type Aliases = BTreeMap<String, Option<EntityRelation>>;

fn translate_with_catalog(
    source: &str,
    dialect: Dialect,
    catalog: Option<&RuntimeCatalog>,
) -> Projection {
    let original_parameters = parameters(source);
    let mut tokens = tokenize(source);
    if let Some(warning) = validate(&tokens) {
        return unsupported(dialect, original_parameters, warning);
    }
    if catalog.is_none() && has_association_source(&tokens) {
        return unsupported(
            dialect,
            original_parameters,
            "association-path JOINs require physical runtime metadata",
        );
    }
    reorder_from_first(&mut tokens);
    let mut aliases = match translate_entities(&mut tokens, dialect, catalog) {
        Ok(aliases) => aliases,
        Err(warning) => return unsupported(dialect, original_parameters, &warning),
    };
    if let Some(catalog) = catalog {
        if let Err(warning) =
            translate_association_joins(&mut tokens, dialect, catalog, &mut aliases)
        {
            return unsupported(dialect, original_parameters, &warning);
        }
        if has_association_source(&tokens) {
            return unsupported(
                dialect,
                original_parameters,
                "association paths in FROM sources are not supported; use an aliased JOIN path",
            );
        }
    }
    if let Err(warning) = translate_attributes(&mut tokens, dialect, &aliases) {
        return unsupported(dialect, original_parameters, &warning);
    }
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
        confidence: if catalog.is_some() {
            "physical".to_string()
        } else {
            "logical".to_string()
        },
        warnings: if catalog.is_some() {
            Vec::new()
        } else {
            vec![
                "Logical projection only; Mendix Runtime storage mappings are not applied."
                    .to_string(),
            ]
        },
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

fn translate_entities(
    tokens: &mut [Token],
    dialect: Dialect,
    catalog: Option<&RuntimeCatalog>,
) -> std::result::Result<Aliases, String> {
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
        let relation = catalog
            .map(|catalog| {
                catalog
                    .entities
                    .get(&format!("{module}.{entity_name}"))
                    .cloned()
                    .ok_or_else(|| {
                        format!("no physical runtime table for entity {module}.{entity_name}")
                    })
            })
            .transpose()?;
        let table = relation
            .as_ref()
            .map(|relation| relation.table.clone())
            .unwrap_or_else(|| format!("{module}${entity_name}"));
        tokens[first].text = quote(&table, dialect);
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
        aliases.insert(alias.to_ascii_lowercase(), relation);
    }
    Ok(aliases)
}

fn translate_attributes(
    tokens: &mut [Token],
    dialect: Dialect,
    aliases: &Aliases,
) -> std::result::Result<(), String> {
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
        if let Some(relation) = aliases.get(&identifier_text(&tokens[left]).to_ascii_lowercase()) {
            tokens[separator].text = ".".to_string();
            let attribute_name = identifier_text(&tokens[attribute]);
            let column = relation
                .as_ref()
                .map(|relation| {
                    if attribute_name.eq_ignore_ascii_case("id") {
                        Ok("id".to_string())
                    } else {
                        relation
                            .columns
                            .get(&attribute_name.to_ascii_lowercase())
                            .cloned()
                            .ok_or_else(|| {
                                format!(
                                    "unknown attribute {} on entity {}",
                                    attribute_name, relation.qualified_name
                                )
                            })
                    }
                })
                .transpose()?
                .unwrap_or(attribute_name);
            tokens[attribute].text = quote(&column, dialect);
        }
    }
    Ok(())
}

/// Expands `JOIN source_alias/Module.Association [AS] target_alias` into the
/// two concrete runtime joins. The grammar intentionally excludes an explicit
/// `ON` clause: association paths already carry the only valid join condition.
fn translate_association_joins(
    tokens: &mut [Token],
    dialect: Dialect,
    catalog: &RuntimeCatalog,
    aliases: &mut Aliases,
) -> std::result::Result<(), String> {
    let indices = significant(tokens);
    let mut generated = 0_usize;
    for (position, join) in indices.iter().copied().enumerate() {
        if tokens[join].kind != TokenKind::Word || !tokens[join].text.eq_ignore_ascii_case("JOIN") {
            continue;
        }
        let Some(source) = indices.get(position + 1).copied() else {
            continue;
        };
        let Some(slash) = indices.get(position + 2).copied() else {
            continue;
        };
        if tokens[slash].text != "/" || !identifier(&tokens[source]) {
            continue;
        }
        let (Some(module), Some(dot), Some(association)) = (
            indices.get(position + 3).copied(),
            indices.get(position + 4).copied(),
            indices.get(position + 5).copied(),
        ) else {
            return Err("association JOIN is missing its qualified association name".to_string());
        };
        if tokens[dot].text != "."
            || !identifier(&tokens[module])
            || !identifier(&tokens[association])
        {
            return Err("association JOIN must name Module.Association".to_string());
        }
        let source_alias = identifier_text(&tokens[source]);
        let Some(Some(source_entity)) = aliases.get(&source_alias.to_ascii_lowercase()) else {
            return Err(format!(
                "association JOIN source alias {source_alias} is unknown"
            ));
        };
        let association_name = format!(
            "{}.{}",
            identifier_text(&tokens[module]),
            identifier_text(&tokens[association])
        );
        let Some(relation) = catalog
            .associations
            .iter()
            .find(|relation| relation.qualified_name == association_name)
        else {
            return Err(format!("unknown association {association_name}"));
        };
        let (target_entity, target_table, source_column, target_column) =
            if source_entity.qualified_name == relation.from_entity {
                (
                    relation.to_entity.clone(),
                    relation.to_table.clone(),
                    "source_id",
                    "target_id",
                )
            } else if source_entity.qualified_name == relation.to_entity {
                (
                    relation.from_entity.clone(),
                    relation.from_table.clone(),
                    "target_id",
                    "source_id",
                )
            } else {
                return Err(format!(
                    "association {association_name} does not connect source entity {}",
                    source_entity.qualified_name
                ));
            };
        let target_relation = catalog
            .entities
            .get(&target_entity)
            .cloned()
            .ok_or_else(|| format!("no physical runtime table for entity {target_entity}"))?;
        let mut last = association;
        let alias_candidate = indices.get(position + 6).copied();
        let alias_index = alias_candidate.and_then(|candidate| {
            if tokens[candidate].text.eq_ignore_ascii_case("AS") {
                let next = indices.get(position + 7).copied();
                last = next.unwrap_or(candidate);
                next
            } else {
                Some(candidate)
            }
        });
        let target_alias = alias_index
            .filter(|candidate| {
                identifier(&tokens[*candidate]) && !stop_word(&identifier_text(&tokens[*candidate]))
            })
            .map(|candidate| {
                last = candidate;
                identifier_text(&tokens[candidate])
            })
            .unwrap_or_else(|| {
                target_entity
                    .rsplit('.')
                    .next()
                    .unwrap_or(&target_entity)
                    .to_string()
            });
        if aliases.contains_key(&target_alias.to_ascii_lowercase()) {
            return Err(format!(
                "association JOIN alias {target_alias} is already in use"
            ));
        }
        if let Some(next) = indices
            .iter()
            .copied()
            .skip_while(|index| *index != last)
            .nth(1)
            && tokens[next].text.eq_ignore_ascii_case("ON")
        {
            return Err("association JOIN paths cannot include an explicit ON clause".to_string());
        }
        let bridge = format!("_mxrs_assoc_{generated}");
        generated += 1;
        tokens[join].text = format!(
            "JOIN {} AS {} ON {}.{} = {}.{} JOIN {} AS {} ON {}.{} = {}.{}",
            quote(&relation.table, dialect),
            quote(&bridge, dialect),
            source_alias,
            quote("id", dialect),
            quote(&bridge, dialect),
            quote(source_column, dialect),
            quote(&target_table, dialect),
            target_alias,
            quote(&bridge, dialect),
            quote(target_column, dialect),
            target_alias,
            quote("id", dialect),
        );
        for index in indices.iter().copied().skip(position + 1) {
            tokens[index].text.clear();
            if index == last {
                break;
            }
        }
        aliases.insert(target_alias.to_ascii_lowercase(), Some(target_relation));
    }
    Ok(())
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
    pub suggestions: BTreeMap<String, String>,
}

pub fn analyze(source: &str) -> Vec<Finding> {
    let upper = source.to_ascii_uppercase();
    let mut findings = Vec::new();
    if let Some(body) = clause_body(&upper, "SELECT", &["FROM"])
        && body.contains('*')
    {
        findings.push(finding(
            "select_star",
            "hint",
            "*",
            "Selecting every column increases transfer and couples callers to schema changes.",
        ));
    }
    let mut offset = 0;
    while let Some(found) = upper[offset..].find(" LIKE '") {
        let start = offset + found;
        let pattern_start = start + " LIKE '".len();
        let Some(end) = source[pattern_start..].find('\'') else {
            break;
        };
        let pattern = &source[pattern_start..pattern_start + end];
        let fragment = &source[start..pattern_start + end + 1];
        if pattern.starts_with('%') && pattern.ends_with('%') {
            findings.push(finding(
                "like_both_wildcard",
                "warning",
                fragment,
                "Wildcards on both sides usually force a full scan.",
            ));
        } else if pattern.starts_with('%') {
            findings.push(finding(
                "like_leading_wildcard",
                "error",
                fragment,
                "A leading wildcard prevents ordinary index seeks.",
            ));
        } else if pattern.ends_with('%') {
            findings.push(finding(
                "like_trailing_only",
                "hint",
                fragment,
                "A trailing-only wildcard can use an index in many configurations.",
            ));
        }
        offset = pattern_start + end + 1;
    }
    if let Some(body) = clause_body(
        &upper,
        "WHERE",
        &["GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION"],
    ) {
        for function in ["LOWER", "UPPER", "CAST"] {
            if body.contains(&format!("{function}(")) || body.contains(&format!("{function} (")) {
                findings.push(finding(
                    "function_in_where",
                    "warning",
                    function,
                    "Applying a function to a filtered column can make the predicate non-sargable.",
                ));
            }
        }
    }
    if let Some(body) = clause_body(
        &upper,
        "FROM",
        &[
            "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION",
        ],
    ) && body.contains(',')
        && !body.contains("JOIN")
    {
        findings.push(finding(
            "cartesian_join",
            "error",
            "FROM",
            "Comma-separated entities without JOIN risk a Cartesian product.",
        ));
    }
    findings
}

fn clause_body<'a>(source: &'a str, keyword: &str, stops: &[&str]) -> Option<&'a str> {
    let start = source.find(&format!("{keyword} "))? + keyword.len() + 1;
    let tail = &source[start..];
    let end = stops
        .iter()
        .filter_map(|stop| tail.find(&format!(" {stop} ")))
        .min()
        .unwrap_or(tail.len());
    Some(&tail[..end])
}

fn finding(rule: &str, severity: &str, fragment: &str, message: &str) -> Finding {
    Finding {
        rule: rule.to_string(),
        severity: severity.to_string(),
        fragment: fragment.to_string(),
        message: message.to_string(),
        suggestions: suggestions(rule),
    }
}

fn suggestions(rule: &str) -> BTreeMap<String, String> {
    let (postgresql, sql_server, ansi) = match rule {
        "like_leading_wildcard" => (
            "Use pg_trgm with a GIN/GiST index or full-text search.",
            "Use CONTAINS with a full-text index.",
            "Redesign the predicate or add a dedicated search index.",
        ),
        "like_both_wildcard" => (
            "Use column % 'term' with the pg_trgm extension and an index.",
            "Use CONTAINS(column, 'term') with a full-text index.",
            "Use a search-specific index or redesign the lookup.",
        ),
        "like_trailing_only" => (
            "Keep the prefix search; confirm operator class, collation, and index use.",
            "Keep the prefix search; confirm collation and the execution plan.",
            "Keep the prefix search, but verify collation and index behavior.",
        ),
        "function_in_where" => (
            "Prefer ILIKE where appropriate or create a matching functional index.",
            "Prefer a case-insensitive collation or an indexed computed column.",
            "Normalize data or compare against a separately indexed normalized column.",
        ),
        "cartesian_join" => (
            "Use an explicit JOIN with an ON predicate.",
            "Use an explicit JOIN with an ON predicate.",
            "Use an explicit JOIN with an ON predicate.",
        ),
        _ => (
            "Project only the columns required by the caller.",
            "Project only the columns required by the caller.",
            "Project only the columns required by the caller.",
        ),
    };
    BTreeMap::from([
        ("postgresql".to_string(), postgresql.to_string()),
        ("sql_server".to_string(), sql_server.to_string()),
        ("ansi".to_string(), ansi.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_dsl::ProjectBuilder;
    use mxrs_model::Project;

    #[allow(non_camel_case_types)]
    mod model {
        pub struct Order;
        impl mxrs_ir::EntityMarker for Order {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Order";
        }

        pub struct Customer;
        impl mxrs_ir::EntityMarker for Customer {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Customer";
        }

        pub struct Order_Customer;
        impl mxrs_ir::AssociationMarker for Order_Customer {
            type From = Order;
            type To = Customer;
            const NAME: &'static str = "Order_Customer";
            const ASSOCIATION_TYPE: mxrs_ir::AssociationType = mxrs_ir::AssociationType::Reference;
        }
    }

    fn physical_catalog() -> RuntimeCatalog {
        let order = EntityRelation {
            qualified_name: "Sales.Order".to_string(),
            table: "mxrb_entity_order".to_string(),
            columns: BTreeMap::from([("number".to_string(), "mxrb_attribute_number".to_string())]),
        };
        let customer = EntityRelation {
            qualified_name: "Sales.Customer".to_string(),
            table: "mxrb_entity_customer".to_string(),
            columns: BTreeMap::from([("name".to_string(), "mxrb_attribute_name".to_string())]),
        };
        RuntimeCatalog {
            entities: BTreeMap::from([
                (order.qualified_name.clone(), order.clone()),
                (customer.qualified_name.clone(), customer.clone()),
            ]),
            associations: vec![AssociationRelation {
                qualified_name: "Sales.Order_Customer".to_string(),
                table: "mxrb_association_order_customer".to_string(),
                from_entity: order.qualified_name.clone(),
                from_table: order.table.clone(),
                to_entity: customer.qualified_name.clone(),
                to_table: customer.table.clone(),
            }],
        }
    }

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
    fn project_projection_uses_physical_storage_and_expands_association_joins() {
        let projection = translate_with_catalog(
            "SELECT c/Name FROM Sales.Order o JOIN o/Sales.Order_Customer c",
            Dialect::PostgreSql,
            Some(&physical_catalog()),
        );
        assert_eq!(projection.confidence, "physical");
        assert_eq!(projection.warnings, Vec::<String>::new());
        let sql = projection.sql.unwrap();
        assert!(sql.contains("FROM \"mxrb_entity_order\" o"), "{sql}");
        assert!(
            sql.contains("JOIN \"mxrb_association_order_customer\" AS \"_mxrs_assoc_0\""),
            "{sql}"
        );
        assert!(sql.contains("JOIN \"mxrb_entity_customer\" AS c"), "{sql}");
        assert!(sql.contains("c.\"mxrb_attribute_name\""), "{sql}");
    }

    #[test]
    fn project_projection_rejects_unknown_physical_attributes_and_explicit_path_conditions() {
        let unknown = translate_with_catalog(
            "SELECT o/Missing FROM Sales.Order o",
            Dialect::Ansi,
            Some(&physical_catalog()),
        );
        assert!(!unknown.supported());
        assert!(unknown.warnings[0].contains("unknown attribute Missing"));
        let explicit_condition = translate_with_catalog(
            "SELECT c/Name FROM Sales.Order o JOIN o/Sales.Order_Customer c ON c/id = o/id",
            Dialect::Ansi,
            Some(&physical_catalog()),
        );
        assert!(!explicit_condition.supported());
        assert!(explicit_condition.warnings[0].contains("explicit ON"));
    }

    #[test]
    fn project_projection_matches_runtime_storage_derived_from_a_real_mpr() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Oql.mpr");
        let mut source = ProjectBuilder::new("11.12.1");
        source.module("Sales", |module| {
            module.entity("Customer", |entity| {
                entity.string("Name");
            });
            module.entity("Order", |entity| {
                entity.string("Number");
                entity.association::<model::Order_Customer>();
            });
        });
        mxrs_writer::write_project(&path, &source.build()).unwrap();
        let project = Project::open(&path, true).unwrap();
        let relation = association_relations(&project).unwrap().pop().unwrap();
        let projection = translate_project(
            "SELECT c/Name FROM Sales.Order o JOIN o/Sales.Order_Customer c",
            Dialect::PostgreSql,
            &project,
        )
        .unwrap();
        let sql = projection.sql.unwrap();
        assert_eq!(projection.confidence, "physical");
        assert!(
            sql.contains(&quote(&relation.table, Dialect::PostgreSql)),
            "{sql}"
        );
        assert!(
            sql.contains(&quote(&relation.from_table, Dialect::PostgreSql)),
            "{sql}"
        );
        assert!(
            sql.contains(&quote(&relation.to_table, Dialect::PostgreSql)),
            "{sql}"
        );
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
    fn analyzer_classifies_all_like_forms_and_where_functions_with_dialect_advice() {
        let findings = analyze(
            "SELECT o/Name FROM Sales.Order o WHERE LOWER(o/Name) LIKE '%both%' OR o/Name LIKE 'prefix%'",
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "like_both_wildcard")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "like_trailing_only")
        );
        let function = findings
            .iter()
            .find(|finding| finding.rule == "function_in_where")
            .unwrap();
        assert!(function.suggestions.contains_key("postgresql"));
        assert!(function.suggestions.contains_key("sql_server"));
        assert!(function.suggestions.contains_key("ansi"));
    }

    #[test]
    fn dialect_parser_accepts_aliases_and_rejects_unknown_values() {
        assert_eq!(Dialect::parse("postgres").unwrap(), Dialect::PostgreSql);
        assert!(Dialect::parse("oracle").is_err());
    }
}
