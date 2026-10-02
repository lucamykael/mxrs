//! Where a scaffolded microflow goes: a method of the service of what it is
//! about, the way the importer places an imported one.
//!
//! A flow is about the module's entity its name names
//! (`ACT_AssetType_Edit` and `SUB_CheckAssetTypeInUse` are `edit` and
//! `check_in_use` of `AssetTypeService`), or about the word its name opens
//! with when the module already has a service of that word
//! (`MF_RubyCrud_Create` joins `RubyCrudService`). Anything else is a method
//! of the module's own service.

use std::path::Path;

use crate::templates::{pascal_case, snake_case};
use crate::transaction::Transaction;
use crate::{Result, ScaffoldError};

/// A microflow to scaffold as a service method.
pub(crate) struct ServiceMethod {
    /// The flow's Mendix name.
    pub(crate) name: String,
    /// Doc comment lines, without their `///`.
    pub(crate) docs: Vec<String>,
    /// `use` lines the body needs.
    pub(crate) imports: Vec<String>,
    /// The body's statements, one per line; none for an empty flow.
    pub(crate) body: Vec<String>,
}

/// Where a flow was placed: the service file's module path, under which
/// the type its declaration generates is named.
pub(crate) struct Placed {
    pub(crate) module_path: String,
}

/// Adds `method` to the service of its subject in `module_name`: creating
/// the service's file, or adding the method to its `impl`. `entities` are
/// the module's entities this scaffold creates besides the ones its project
/// already declares.
pub(crate) fn add_service_method(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    method: &ServiceMethod,
    entities: &[String],
    create_file: impl FnOnce(&mut Transaction, &str, String) -> Result<()>,
) -> Result<Placed> {
    let module_stem = snake_case(module_name);
    let folder = root.join("src/services").join(&module_stem);
    let mut known = declared_entities(root, &module_stem);
    known.extend(entities.iter().cloned());
    let (prefix, core) = split_prefix(&method.name);
    let first_word = core.split('_').next().unwrap_or(core);
    let subject = entity_subject(core, &known, first_word.len())
        .map(|(entity, action)| (Subject::Entity(entity), action))
        .or_else(|| {
            let (word, action) = core.split_once('_')?;
            let stem = format!("{}_service", snake_case(word));
            (!action.is_empty() && !is_verb(word) && (folder.join(format!("{stem}.rs")).is_file()))
                .then(|| (Subject::Word(word.to_string()), action.to_string()))
        })
        .or_else(|| {
            entity_subject(core, &known, core.len())
                .map(|(entity, action)| (Subject::Entity(entity), action))
        });
    let (subject, action) = match subject {
        Some((subject, action)) => (Some(subject), action),
        None => (None, core.to_string()),
    };
    let (file_stem, service) = match &subject {
        Some(subject) => {
            let text = subject.text();
            let stem = snake_case(text);
            let type_name = if text.starts_with(|c: char| c.is_ascii_uppercase())
                && text.chars().all(|c| c.is_ascii_alphanumeric())
            {
                text.to_string()
            } else {
                pascal_case(&stem)
            };
            (format!("{stem}_service"), format!("{type_name}Service"))
        }
        None => (
            format!("{module_stem}_service"),
            format!("{}Service", pascal_case(&module_stem)),
        ),
    };
    let path = folder.join(format!("{file_stem}.rs"));
    let existing = transaction.content(&path)?;
    let mut function = method_name(&action, &method.name);
    if let Some(existing) = &existing
        && existing.contains(&format!("pub fn {function}("))
    {
        function = match prefix {
            Some(prefix) => format!("{}_{function}", prefix.to_ascii_lowercase()),
            None => format!("{function}_flow"),
        };
    }
    let derived = prefix
        .map(str::to_string)
        .into_iter()
        .chain(subject.as_ref().map(|subject| subject.text().to_string()))
        .chain([pascal_case(&function)])
        .collect::<Vec<_>>()
        .join("_");
    let mut arguments = prefix.map(str::to_string).into_iter().collect::<Vec<_>>();
    if derived != method.name {
        arguments.push(format!("name = {:?}", method.name));
    }
    let attribute = if arguments.is_empty() {
        "#[microflow]".to_string()
    } else {
        format!("#[microflow({})]", arguments.join(", "))
    };
    let mut method_lines: Vec<String> = method
        .docs
        .iter()
        .map(|line| format!("    /// {line}").trim_end().to_string())
        .collect();
    method_lines.push(format!("    {attribute}"));
    if method.body.is_empty() {
        method_lines.push(format!(
            "    pub fn {function}(_flow: &mut FlowBuilder) {{}}"
        ));
    } else {
        method_lines.push(format!("    pub fn {function}(flow: &mut FlowBuilder) {{"));
        method_lines.extend(method.body.iter().map(|line| format!("        {line}")));
        method_lines.push("    }".to_string());
    }
    let module_path = format!("crate::services::{module_stem}::{file_stem}");
    match existing {
        Some(existing) => {
            let opening = format!("impl {service} {{");
            let start = existing
                .lines()
                .position(|line| line == opening)
                .ok_or_else(|| {
                    ScaffoldError::UnsupportedLayerMigration(path.display().to_string())
                })?;
            let lines: Vec<&str> = existing.lines().collect();
            let closing = lines[start..]
                .iter()
                .position(|line| *line == "}")
                .map(|offset| start + offset)
                .ok_or_else(|| {
                    ScaffoldError::UnsupportedLayerMigration(path.display().to_string())
                })?;
            let mut updated: Vec<String> = Vec::with_capacity(lines.len() + method_lines.len() + 4);
            // New imports join the file's own, after its last `use`.
            let last_use = lines[..start]
                .iter()
                .rposition(|line| line.starts_with("use "));
            for (index, line) in lines.iter().enumerate() {
                if index == closing {
                    updated.push(String::new());
                    updated.extend(method_lines.iter().cloned());
                }
                updated.push(line.to_string());
                if Some(index) == last_use {
                    updated.extend(
                        method
                            .imports
                            .iter()
                            .filter(|import| !existing.contains(import.as_str()))
                            .cloned(),
                    );
                }
            }
            let mut source = updated.join("\n");
            source.push('\n');
            transaction.write(&path, source)?;
        }
        None => {
            let subject_argument = match &subject {
                Some(Subject::Entity(entity)) => format!(", subject = {entity}"),
                Some(Subject::Word(word)) => format!(", subject = {word:?}"),
                None => String::new(),
            };
            let what = match &subject {
                Some(subject) => format!(
                    "What the {module_name} module does with {}.",
                    subject.text()
                ),
                None => format!("What the {module_name} module does that no one subject gathers."),
            };
            let mut imports = method.imports.clone();
            if let Some(Subject::Entity(entity)) = &subject
                && !imports
                    .iter()
                    .any(|import| import.ends_with(&format!("::{entity};")))
            {
                imports.push(format!(
                    "use crate::domain::entities::{module_stem}::{}::{entity};",
                    snake_case(entity)
                ));
            }
            imports.sort();
            imports.dedup();
            let mut source = String::from("use mxrs::prelude::*;\n\n");
            for import in &imports {
                source.push_str(import);
                source.push('\n');
            }
            if !imports.is_empty() {
                source.push('\n');
            }
            source.push_str(&format!(
                "/// {what}\npub struct {service};\n\n#[service(module = {module_name:?}{subject_argument})]\nimpl {service} {{\n"
            ));
            for line in &method_lines {
                source.push_str(line);
                source.push('\n');
            }
            source.push_str("}\n");
            create_file(transaction, &file_stem, source)?;
        }
    }
    Ok(Placed { module_path })
}

enum Subject {
    Entity(String),
    Word(String),
}

impl Subject {
    fn text(&self) -> &str {
        match self {
            Subject::Entity(text) | Subject::Word(text) => text,
        }
    }
}

/// The entities the project already declares for the module: the structs
/// of its entity and DTO files.
fn declared_entities(root: &Path, module_stem: &str) -> Vec<String> {
    let mut entities = Vec::new();
    for concept in ["domain/entities", "domain/dtos"] {
        let Ok(files) = std::fs::read_dir(root.join("src").join(concept).join(module_stem)) else {
            continue;
        };
        for file in files.flatten() {
            let Ok(source) = std::fs::read_to_string(file.path()) else {
                continue;
            };
            entities.extend(source.lines().filter_map(|line| {
                let name = line.strip_prefix("pub struct ")?;
                let name = name.split([' ', '{', ';', '(']).next()?;
                (!name.is_empty()).then(|| name.to_string())
            }));
        }
    }
    entities
}

/// The method declaring a flow, from what its name says besides its subject.
fn method_name(action: &str, name: &str) -> String {
    let mut function = snake_case(action);
    if function.is_empty() {
        function = "flow".to_string();
    }
    if crate::is_rust_keyword(&function)
        || matches!(
            function.as_str(),
            "string"
                | "integer"
                | "long"
                | "float"
                | "decimal"
                | "boolean"
                | "attribute"
                | "association"
        )
    {
        function.push('_');
    }
    if function == name {
        function.push_str("_flow");
    }
    function
}

/// `ACT_CreateOrder` → (`ACT`, `CreateOrder`). The importer's rule.
fn split_prefix(name: &str) -> (Option<&str>, &str) {
    match name.split_once('_') {
        Some((prefix, rest))
            if (2..=5).contains(&prefix.len())
                && !rest.is_empty()
                && prefix.starts_with(|c: char| c.is_ascii_uppercase())
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) =>
        {
            (Some(prefix), rest)
        }
        _ => (None, name),
    }
}

/// Whether a flow name's word says what the flow does. The importer's list.
fn is_verb(word: &str) -> bool {
    const VERBS: [&str; 34] = [
        "Act",
        "Add",
        "Calculate",
        "Cancel",
        "Change",
        "Check",
        "Clear",
        "Close",
        "Commit",
        "Create",
        "Delete",
        "Do",
        "Edit",
        "Export",
        "Find",
        "Generate",
        "Get",
        "Import",
        "List",
        "Load",
        "New",
        "Open",
        "Process",
        "Refresh",
        "Remove",
        "Retrieve",
        "Save",
        "Search",
        "Select",
        "Send",
        "Set",
        "Show",
        "Update",
        "Validate",
    ];
    VERBS.contains(&word)
}

/// The longest entity `core` names as a word of its own, starting before
/// `within`, and what the name says besides. The importer's rule.
fn entity_subject(core: &str, entities: &[String], within: usize) -> Option<(String, String)> {
    let mut best: Option<(&String, usize, usize)> = None;
    for entity in entities {
        for (start, _) in core.match_indices(entity.as_str()) {
            if start >= within {
                continue;
            }
            let mut end = start + entity.len();
            let rest = &core[end..];
            if let Some(plural) = ["es", "s"].iter().find(|plural| {
                rest.strip_prefix(**plural).is_some_and(|after| {
                    after.is_empty()
                        || after.starts_with('_')
                        || after.starts_with(|c: char| c.is_ascii_uppercase())
                })
            }) {
                end += plural.len();
            }
            let before = core[..start].chars().next_back();
            let after = core[end..].chars().next();
            let opens =
                before.is_none_or(|c| c == '_' || c.is_ascii_lowercase() || c.is_ascii_digit());
            let closes =
                after.is_none_or(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit());
            if opens && closes && best.is_none_or(|(chosen, _, _)| entity.len() > chosen.len()) {
                best = Some((entity, start, end));
            }
        }
    }
    let (entity, start, end) = best?;
    let action = format!("{}{}", &core[..start], &core[end..])
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    (!action.is_empty()).then(|| (entity.clone(), action))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flow_is_placed_the_way_the_importer_places_it() {
        let entities = vec!["AssetType".to_string(), "AssetReactionPlan".to_string()];
        assert_eq!(
            entity_subject("AssetType_Edit", &entities, "AssetType".len()),
            Some(("AssetType".to_string(), "Edit".to_string()))
        );
        assert_eq!(
            entity_subject("List_AssetReactionPlan", &entities, 4),
            None,
            "a verb is what the flow does, not what it is about"
        );
        assert_eq!(
            entity_subject("List_AssetReactionPlan", &entities, 30),
            Some(("AssetReactionPlan".to_string(), "List".to_string()))
        );
        assert_eq!(
            entity_subject("CheckAssetTypeInUse", &entities, 30),
            Some(("AssetType".to_string(), "CheckInUse".to_string()))
        );
        assert!(is_verb("List") && !is_verb("RubyCrud"));
        assert_eq!(method_name("CheckInUse", "SUB_X"), "check_in_use");
    }
}
