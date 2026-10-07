//! The data sets of the modules a project made, as the Rust that declares
//! each one: `src/domain/documents/<module>/<data_set>.rs`, its OQL query,
//! parameters and roles.
//!
//! A data set is declared only when its declaration states the stored
//! document again; one with a parameter of another type, a constraint, or
//! access Studio Pro would not derive stays in the imported model.

use std::fmt::Write as _;

use mxrs_ir::{DataSetDecl, DataSetParameterType, ExportLevel};

/// One data set the importer declares: its module, its name, and the file
/// that states it.
pub(crate) struct DeclaredDataSet {
    pub(crate) module: String,
    pub(crate) name: String,
    pub(crate) stem: String,
    pub(crate) source: String,
}

/// The data sets their declaration restates, of the modules `authored`
/// says the project made.
pub(crate) fn declare(
    project: &mxrs_model::Project,
    authored: impl Fn(&str) -> bool,
) -> crate::Result<Vec<DeclaredDataSet>> {
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
        if document.get_str("$Type").ok() != Some("DataSets$DataSet") {
            continue;
        }
        let Some(module) = crate::code_actions::module_of(&unit.unit_id, &containers, &modules)
        else {
            continue;
        };
        if !authored(&module) {
            continue;
        }
        let Some(data_set) = mxrs_writer::stated_document(&document)
            .ok()
            .and_then(|stated| DataSetDecl::from_document(&stated))
        else {
            continue;
        };
        let stem = crate::inner_file_stem(&data_set.name);
        let source = render(&module, &stem, &data_set);
        declared.push(DeclaredDataSet {
            module,
            name: data_set.name,
            stem,
            source,
        });
    }
    declared.sort_by(|left, right| (&left.module, &left.name).cmp(&(&right.module, &right.name)));
    Ok(declared)
}

/// The file declaring `data_set`.
fn render(module: &str, stem: &str, data_set: &DataSetDecl) -> String {
    let mut calls: Vec<String> = Vec::new();
    for parameter in &data_set.parameters {
        let name = crate::rust_string(&parameter.name);
        calls.push(match &parameter.ty {
            DataSetParameterType::Object(entity) => {
                format!(".object_parameter({name}, {})", crate::rust_string(entity))
            }
            DataSetParameterType::Enumeration(enumeration) => format!(
                ".enumeration_parameter({name}, {})",
                crate::rust_string(enumeration)
            ),
        });
    }
    if !data_set.roles.is_empty() {
        let roles: Vec<String> = data_set
            .roles
            .iter()
            .map(|role| crate::rust_string(role))
            .collect();
        calls.push(format!(".roles([{}])", roles.join(", ")));
    }
    if !data_set.documentation.is_empty() {
        calls.push(format!(
            ".documentation({})",
            crate::rust_string(&data_set.documentation)
        ));
    }
    if data_set.excluded {
        calls.push(".excluded(true)".to_string());
    }
    if data_set.export_level == ExportLevel::Published {
        calls.push(".export_level(ExportLevel::Published)".to_string());
    }
    let mut source = format!(
        "//! Data set `{module}.{name}`: the OQL query a report runs, the parameters\n//! it takes and the module roles that may run it.\n\nuse mxrs::prelude::*;\n\nconst QUERY: &str = {query};\n\n#[declaration(module = {module_literal})]\npub fn {stem}(module: &mut ModuleBuilder) {{\n",
        name = data_set.name,
        query = crate::json_structures::snippet_literal(&data_set.query),
        module_literal = crate::rust_string(module),
    );
    let name = crate::rust_string(&data_set.name);
    if calls.is_empty() {
        let _ = writeln!(source, "    module.data_set({name}, QUERY, |_set| {{}});");
    } else {
        let _ = writeln!(source, "    module.data_set({name}, QUERY, |set| {{");
        let _ = writeln!(source, "        set{};", calls.join(""));
        let _ = writeln!(source, "    }});");
    }
    source.push_str("}\n");
    source
}
