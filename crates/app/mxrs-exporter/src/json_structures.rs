//! The JSON structures of the modules a project made, as the Rust that
//! declares each one: `src/domain/documents/<module>/<structure>.rs`, the
//! snippet and what differs from the elements Studio Pro derives from it.
//!
//! A structure is declared only when its declaration states the stored
//! document again; one whose elements are not the snippet's — a snippet
//! edited without refreshing them — stays in the imported model.

use std::fmt::Write as _;

use mxrs_ir::{ExportLevel, JsonElement, JsonStructureDecl};

/// One structure the importer declares: its module, its name, and the file
/// that states it.
pub(crate) struct DeclaredStructure {
    pub(crate) module: String,
    pub(crate) name: String,
    pub(crate) stem: String,
    pub(crate) source: String,
}

/// The JSON structures their declaration restates, of the modules
/// `authored` says the project made.
pub(crate) fn declare(
    project: &mxrs_model::Project,
    authored: impl Fn(&str) -> bool,
) -> crate::Result<Vec<DeclaredStructure>> {
    let units = project.all_units()?;
    let containers: std::collections::HashMap<&str, &str> = units
        .iter()
        .map(|unit| (unit.unit_id.as_str(), unit.container_id.as_str()))
        .collect();
    let modules: std::collections::HashMap<String, String> = project
        .modules()?
        .into_iter()
        .filter_map(|module| Some((module.id, module.name?)))
        .collect();
    let mut declared = Vec::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        if document.get_str("$Type").ok() != Some("JsonStructures$JsonStructure") {
            continue;
        }
        let Some(module) = crate::code_actions::module_of(&unit.unit_id, &containers, &modules)
        else {
            continue;
        };
        if !authored(&module) {
            continue;
        }
        let Some(stored) = mxrs_writer::stated_document(&document)
            .ok()
            .and_then(|stated| JsonStructureDecl::from_document(&stated))
        else {
            continue;
        };
        let Some(changes) = changes(&stored) else {
            continue;
        };
        let stem = crate::inner_file_stem(&stored.name);
        let source = render(&module, &stem, &stored, &changes);
        declared.push(DeclaredStructure {
            module,
            name: stored.name,
            stem,
            source,
        });
    }
    declared.sort_by(|left, right| (&left.module, &left.name).cmp(&(&right.module, &right.name)));
    Ok(declared)
}

/// What one element's declaration changes: each a call on its builder.
type Change = (String, Vec<String>);

/// The changes that make the snippet's derived elements `stored`'s, when
/// the two trees have the same elements in the same places.
fn changes(stored: &JsonStructureDecl) -> Option<Vec<Change>> {
    let derived = JsonStructureDecl::derive(stored.name.clone(), stored.snippet.clone()).ok()?;
    if derived.elements.len() != stored.elements.len() {
        return None;
    }
    let mut changes = Vec::new();
    for (derived, stored) in derived.elements.iter().zip(&stored.elements) {
        let (derived, stored) = (derived.walk(), stored.walk());
        if derived.len() != stored.len() {
            return None;
        }
        for (derived, stored) in derived.into_iter().zip(stored) {
            if derived.path != stored.path
                || derived.element_type != stored.element_type
                || derived.children.len() != stored.children.len()
            {
                return None;
            }
            let calls = element_changes(derived, stored);
            if !calls.is_empty() {
                changes.push((stored.path.clone(), calls));
            }
        }
    }
    Some(changes)
}

fn element_changes(derived: &JsonElement, stored: &JsonElement) -> Vec<String> {
    let mut calls = Vec::new();
    let text = |calls: &mut Vec<String>, method: &str, left: &str, right: &str| {
        if left != right {
            calls.push(format!(".{method}({})", crate::rust_string(right)));
        }
    };
    let number = |calls: &mut Vec<String>, method: &str, left: i32, right: i32| {
        if left != right {
            calls.push(format!(".{method}({right})"));
        }
    };
    let flag = |calls: &mut Vec<String>, method: &str, left: bool, right: bool| {
        if left != right {
            calls.push(format!(".{method}({right})"));
        }
    };
    text(
        &mut calls,
        "exposed_name",
        &derived.exposed_name,
        &stored.exposed_name,
    );
    text(
        &mut calls,
        "exposed_item_name",
        &derived.exposed_item_name,
        &stored.exposed_item_name,
    );
    if derived.primitive_type != stored.primitive_type {
        calls.push(format!(
            ".primitive_type(JsonPrimitiveType::{})",
            stored.primitive_type.as_str()
        ));
    }
    number(
        &mut calls,
        "max_length",
        derived.max_length,
        stored.max_length,
    );
    number(
        &mut calls,
        "min_occurs",
        derived.min_occurs,
        stored.min_occurs,
    );
    number(
        &mut calls,
        "max_occurs",
        derived.max_occurs,
        stored.max_occurs,
    );
    flag(&mut calls, "nillable", derived.nillable, stored.nillable);
    flag(
        &mut calls,
        "is_default_type",
        derived.is_default_type,
        stored.is_default_type,
    );
    number(
        &mut calls,
        "fraction_digits",
        derived.fraction_digits,
        stored.fraction_digits,
    );
    number(
        &mut calls,
        "total_digits",
        derived.total_digits,
        stored.total_digits,
    );
    text(
        &mut calls,
        "original_value",
        &derived.original_value,
        &stored.original_value,
    );
    text(
        &mut calls,
        "error_message",
        &derived.error_message,
        &stored.error_message,
    );
    text(
        &mut calls,
        "warning_message",
        &derived.warning_message,
        &stored.warning_message,
    );
    calls
}

/// The file declaring `structure`.
fn render(module: &str, stem: &str, structure: &JsonStructureDecl, changes: &[Change]) -> String {
    let mut calls: Vec<String> = Vec::new();
    if !structure.documentation.is_empty() {
        calls.push(format!(
            "json.documentation({});",
            crate::rust_string(&structure.documentation)
        ));
    }
    if structure.excluded {
        calls.push("json.excluded(true);".to_string());
    }
    if structure.export_level == ExportLevel::Published {
        calls.push("json.export_level(ExportLevel::Published);".to_string());
    }
    for (path, element_calls) in changes {
        let mut call = format!(
            "json.element({}, |element| {{\n            element",
            crate::rust_string(path)
        );
        for element_call in element_calls {
            call.push_str(element_call);
        }
        call.push_str(";\n        });");
        calls.push(call);
    }
    let mut source = format!(
        "//! JSON structure `{module}.{name}`: the JSON its mappings read and write,\n//! and what differs from the elements Studio Pro derives from its snippet.\n\nuse mxrs::prelude::*;\n\nconst SNIPPET: &str = {snippet};\n\n#[declaration(module = {module_literal})]\npub fn {stem}(module: &mut ModuleBuilder) {{\n",
        name = structure.name,
        snippet = snippet_literal(&structure.snippet),
        module_literal = crate::rust_string(module),
    );
    let name = crate::rust_string(&structure.name);
    if calls.is_empty() {
        let _ = writeln!(
            source,
            "    module.json_structure({name}, SNIPPET, |_json| {{}});"
        );
    } else {
        let _ = writeln!(
            source,
            "    module.json_structure({name}, SNIPPET, |json| {{"
        );
        for call in &calls {
            let _ = writeln!(source, "        {call}");
        }
        let _ = writeln!(source, "    }});");
    }
    source.push_str("}\n");
    source
}

/// The snippet as Rust writes it: as it reads, in a raw string, unless it
/// holds what a raw string cannot.
pub(crate) fn snippet_literal(snippet: &str) -> String {
    let unwritable = |text: &str| {
        text.chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    };
    // Lines ending as Windows ends them are each their own literal: Rust
    // reads a raw string's line breaks as `\n` alone.
    if snippet.contains("\r\n") && !unwritable(&snippet.replace("\r\n", "\n")) {
        let lines: Vec<String> = snippet
            .split_inclusive("\r\n")
            .map(crate::rust_string)
            .collect();
        return format!("concat!({})", lines.join(", "));
    }
    if unwritable(snippet) {
        return crate::rust_string(snippet);
    }
    let mut hashes = String::from("#");
    while snippet.contains(&format!("\"{hashes}")) {
        hashes.push('#');
    }
    format!("r{hashes}\"{snippet}\"{hashes}")
}
