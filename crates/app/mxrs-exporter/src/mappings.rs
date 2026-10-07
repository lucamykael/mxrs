//! The import and export mappings of the modules a project made, as the
//! Rust that declares each one: `src/domain/documents/<module>/<mapping>.rs`,
//! the entity each object of its JSON structure is and the attribute each
//! value fills, in the shape of the structure.
//!
//! A mapping is declared only when its declaration, written against the
//! structure the model stores, states the stored document again; one of an
//! XML schema or a message definition, or one made of an older structure,
//! stays in the imported model.

use std::collections::HashMap;
use std::fmt::Write as _;

use mxrs_ir::{
    ExportLevel, JsonStructureDecl, MappingAssociation, MappingDecl, MappingDirection,
    MappingElement, MappingElementKind, MappingValueType, NullValueOption, ObjectMapping,
};

/// One mapping the importer declares: its module, its type and name, and
/// the file that states it.
pub(crate) struct DeclaredMapping {
    pub(crate) module: String,
    pub(crate) native_type: &'static str,
    pub(crate) name: String,
    pub(crate) stem: String,
    pub(crate) source: String,
}

/// The mappings their declaration restates, of the modules `authored` says
/// the project made.
pub(crate) fn declare(
    project: &mxrs_model::Project,
    authored: impl Fn(&str) -> bool,
) -> crate::Result<Vec<DeclaredMapping>> {
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
    let mut structures: HashMap<String, JsonStructureDecl> = HashMap::new();
    let mut mappings = Vec::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        let native_type = match document.get_str("$Type").ok() {
            Some("JsonStructures$JsonStructure") => "JsonStructures$JsonStructure",
            Some("ImportMappings$ImportMapping") => "ImportMappings$ImportMapping",
            Some("ExportMappings$ExportMapping") => "ExportMappings$ExportMapping",
            _ => continue,
        };
        let Some(module) = crate::code_actions::module_of(&unit.unit_id, &containers, &modules)
        else {
            continue;
        };
        let Ok(stated) = mxrs_writer::stated_document(&document) else {
            continue;
        };
        if native_type == "JsonStructures$JsonStructure" {
            if let (Some(name), Some(structure)) =
                (stated.text("Name"), JsonStructureDecl::read(&stated))
            {
                structures.insert(format!("{module}.{name}"), structure);
            }
        } else if authored(&module) {
            mappings.push((module, native_type, stated));
        }
    }
    let mut declared = Vec::new();
    for (module, native_type, stated) in mappings {
        let Some(structure) = stated
            .text("JsonStructure")
            .and_then(|name| structures.get(name))
        else {
            continue;
        };
        let Some(mapping) = MappingDecl::from_document(&stated, structure) else {
            continue;
        };
        let mapping = mapping.simplified(structure);
        let stem = crate::inner_file_stem(&mapping.name);
        let source = render(&module, &stem, &mapping);
        declared.push(DeclaredMapping {
            module,
            native_type,
            name: mapping.name,
            stem,
            source,
        });
    }
    declared.sort_by(|left, right| (&left.module, &left.name).cmp(&(&right.module, &right.name)));
    Ok(declared)
}

/// The file declaring `mapping`.
fn render(module: &str, stem: &str, mapping: &MappingDecl) -> String {
    let (method, what) = match mapping.direction {
        MappingDirection::Import => ("import_mapping", "becomes objects"),
        MappingDirection::Export => ("export_mapping", "is made of objects"),
    };
    let mut source = format!(
        "//! Mapping `{module}.{name}`: how the JSON of `{structure}` {what}.\n\nuse mxrs::prelude::*;\n\n#[declaration(module = {module_literal})]\npub fn {stem}(module: &mut ModuleBuilder) {{\n    module.{method}(\n        {name_literal},\n        {structure_literal},\n        |mapping| {{\n",
        name = mapping.name,
        structure = mapping.json_structure,
        module_literal = crate::rust_string(module),
        name_literal = crate::rust_string(&mapping.name),
        structure_literal = crate::rust_string(&mapping.json_structure),
    );
    let indent = "            ";
    if !mapping.documentation.is_empty() {
        let _ = writeln!(
            source,
            "{indent}mapping.documentation({});",
            crate::rust_string(&mapping.documentation)
        );
    }
    if !mapping.sample_values {
        let _ = writeln!(source, "{indent}mapping.sample_values(false);");
    }
    if mapping.null_value_option != NullValueOption::LeaveOutElement {
        let _ = writeln!(
            source,
            "{indent}mapping.null_value_option(NullValueOption::{});",
            mapping.null_value_option.as_str()
        );
    }
    if mapping.excluded {
        let _ = writeln!(source, "{indent}mapping.excluded(true);");
    }
    if mapping.export_level == ExportLevel::Published {
        let _ = writeln!(
            source,
            "{indent}mapping.export_level(ExportLevel::Published);"
        );
    }
    for element in &mapping.elements {
        element_source(&mut source, "mapping", element, indent);
    }
    source.push_str("        },\n    );\n}\n");
    source
}

/// The call on `owner` that declares `element`, at `indent`.
fn element_source(source: &mut String, owner: &str, element: &MappingElement, indent: &str) {
    let key = crate::rust_string(&element.key);
    match &element.kind {
        MappingElementKind::Value(value) => {
            let mut call = format!("{owner}.value({key})");
            if let Some(attribute) = &value.attribute {
                let _ = write!(call, ".attribute({})", crate::rust_string(attribute));
            }
            if value.is_key {
                call.push_str(".key()");
            }
            if !value.converter.is_empty() {
                let _ = write!(call, ".converter({})", crate::rust_string(&value.converter));
            }
            if let Some(value_type) = &value.value_type {
                let _ = write!(call, ".value_type({})", value_type_source(value_type));
            }
            let _ = writeln!(source, "{indent}{call};");
        }
        MappingElementKind::Object(object) => {
            let variable = variable_name(&element.key, object, owner);
            let opening = match &object.entity {
                Some(entity) => format!(
                    "{owner}.object({key}, {}, |{variable}| {{",
                    crate::rust_string(entity)
                ),
                None => format!("{owner}.array({key}, |{variable}| {{"),
            };
            let _ = writeln!(source, "{indent}{opening}");
            let inner = format!("{indent}    ");
            match &object.association {
                MappingAssociation::Named => {}
                MappingAssociation::None => {
                    let _ = writeln!(source, "{inner}{variable}.unassociated();");
                }
                MappingAssociation::Other(association) => {
                    let _ = writeln!(
                        source,
                        "{inner}{variable}.via({});",
                        crate::rust_string(association)
                    );
                }
            }
            if let Some(handling) = object.handling {
                let _ = writeln!(
                    source,
                    "{inner}{variable}.handling(ObjectHandling::{});",
                    handling.as_str()
                );
            }
            if let Some(backup) = object.backup {
                let _ = writeln!(
                    source,
                    "{inner}{variable}.backup(ObjectHandling::{});",
                    backup.as_str()
                );
            }
            if object.allow_override {
                let _ = writeln!(source, "{inner}{variable}.allow_override(true);");
            }
            for child in &object.children {
                element_source(source, &variable, child, &inner);
            }
            let _ = writeln!(source, "{indent}}});");
        }
    }
}

/// The name of the closure parameter an object's builder is bound to: its
/// entity's, or its member's for an array of none — never its owner's.
fn variable_name(key: &str, object: &ObjectMapping, owner: &str) -> String {
    let base = match &object.entity {
        Some(entity) => entity.rsplit('.').next().unwrap_or(entity).to_string(),
        None => key.trim_matches(['(', ')']).to_string(),
    };
    let mut name = crate::inner_file_stem(&base);
    if name.is_empty() || name == "mapping" {
        name = "items".to_string();
    }
    if name == owner {
        name.push_str("_item");
    }
    name
}

fn value_type_source(value_type: &MappingValueType) -> String {
    match value_type {
        MappingValueType::Enumeration(enumeration) => format!(
            "MappingValueType::Enumeration({}.into())",
            crate::rust_string(enumeration)
        ),
        other => format!("MappingValueType::{other:?}"),
    }
}
