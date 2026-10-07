//! The JavaScript actions of the modules a project made, as the Rust that
//! declares each one: `src/ports/<module>/javascript_actions/<action>.rs`.
//!
//! An action is declared only when its declaration states the stored
//! document again (`JavaScriptActionDecl::from_document`); one it cannot
//! say — a toolbox icon, a type parameter — stays in the imported model.

use std::fmt::Write as _;

use mxrs_ir::{CodeActionType, ExportLevel, JavaScriptActionDecl, JavaScriptPlatform};

/// One action the importer declares: its module, its declaration, and the
/// file that states it.
pub(crate) struct DeclaredAction {
    pub(crate) module: String,
    pub(crate) declaration: JavaScriptActionDecl,
    pub(crate) stem: String,
    pub(crate) source: String,
}

/// The JavaScript actions of `modules` their declaration restates, of the
/// modules `authored` says the project made.
pub(crate) fn declare(
    project: &mxrs_model::Project,
    authored: impl Fn(&str) -> bool,
) -> crate::Result<Vec<DeclaredAction>> {
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
        if document.get_str("$Type").ok() != Some("JavaScriptActions$JavaScriptAction") {
            continue;
        }
        let Some(module) = module_of(&unit.unit_id, &containers, &modules) else {
            continue;
        };
        if !authored(&module) {
            continue;
        }
        // A parameter's description and category are what the builder does
        // not say: an action with them stays in the imported model.
        let Some(declaration) = mxrs_writer::stated_document(&document)
            .ok()
            .and_then(|stated| JavaScriptActionDecl::from_document(&stated))
            .filter(|declaration| {
                declaration.parameters.iter().all(|parameter| {
                    parameter.description.is_empty() && parameter.category.is_empty()
                })
            })
        else {
            continue;
        };
        let stem = crate::inner_file_stem(&declaration.name);
        let source = render(&module, &stem, &declaration);
        declared.push(DeclaredAction {
            module,
            declaration,
            stem,
            source,
        });
    }
    declared.sort_by(|left, right| {
        (&left.module, &left.declaration.name).cmp(&(&right.module, &right.declaration.name))
    });
    Ok(declared)
}

fn module_of(
    unit: &str,
    containers: &std::collections::HashMap<&str, &str>,
    modules: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let mut current = unit;
    let mut visited = std::collections::HashSet::new();
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

/// The file declaring `action`, stating only what differs from a new one.
fn render(module: &str, stem: &str, action: &JavaScriptActionDecl) -> String {
    let source_path = format!(
        "javascriptsource/{}/actions/{}.js",
        module.to_lowercase(),
        action.name
    );
    let mut calls = Vec::new();
    for parameter in &action.parameters {
        let name = crate::rust_string(&parameter.name);
        calls.push(match (rust_type(&parameter.ty), parameter.required) {
            (Some(ty), true) => format!(".takes::<{ty}>({name})"),
            (Some(ty), false) => format!(".takes_optional::<{ty}>({name})"),
            (None, true) => format!(".takes_type({name}, {})", type_expression(&parameter.ty)),
            (None, false) => format!(
                ".takes_optional_type({name}, {})",
                type_expression(&parameter.ty)
            ),
        });
    }
    match &action.return_type {
        CodeActionType::Void => {}
        ty => calls.push(match rust_type(ty) {
            Some(ty) => format!(".returns::<{ty}>()"),
            None => format!(".returns_type({})", type_expression(ty)),
        }),
    }
    if action.return_name != "ReturnValueName" {
        calls.push(format!(
            ".return_name({})",
            crate::rust_string(&action.return_name)
        ));
    }
    if action.platform != JavaScriptPlatform::All {
        calls.push(format!(
            ".platform(JavaScriptPlatform::{})",
            action.platform.as_str()
        ));
    }
    if !action.documentation.is_empty() {
        calls.push(format!(
            ".documentation({})",
            crate::rust_string(&action.documentation)
        ));
    }
    if action.excluded {
        calls.push(".excluded(true)".to_string());
    }
    if action.export_level == ExportLevel::Published {
        calls.push(".export_level(ExportLevel::Published)".to_string());
    }
    let mut source = format!(
        "//! JavaScript action `{module}.{}`: what a nanoflow's call takes and\n//! returns. Its JavaScript is `{source_path}`.\n\nuse mxrs::prelude::*;\n\n#[declaration(module = {})]\npub fn {stem}(module: &mut ModuleBuilder) {{\n",
        action.name,
        crate::rust_string(module),
    );
    let name = crate::rust_string(&action.name);
    if calls.is_empty() {
        let _ = writeln!(
            source,
            "    module.javascript_action({name}, |_action| {{}});"
        );
    } else {
        let _ = writeln!(source, "    module.javascript_action({name}, |action| {{");
        let _ = writeln!(source, "        action");
        for (index, call) in calls.iter().enumerate() {
            let end = if index + 1 == calls.len() { ";" } else { "" };
            let _ = writeln!(source, "            {call}{end}");
        }
        let _ = writeln!(source, "    }});");
    }
    source.push_str("}\n");
    source
}

/// The Rust type a value of `ty` is, when a type names it.
fn rust_type(ty: &CodeActionType) -> Option<&'static str> {
    Some(match ty {
        CodeActionType::Boolean => "MxBool",
        CodeActionType::DateTime => "MxDateTime",
        CodeActionType::Decimal => "MxDecimal",
        CodeActionType::Integer => "MxInteger",
        CodeActionType::String => "MxString",
        _ => return None,
    })
}

/// `ty` as the `CodeActionType` expression that states it.
fn type_expression(ty: &CodeActionType) -> String {
    match ty {
        CodeActionType::Object(entity) => {
            format!(
                "CodeActionType::Object({}.into())",
                crate::rust_string(entity)
            )
        }
        CodeActionType::List(entity) => {
            format!(
                "CodeActionType::List({}.into())",
                crate::rust_string(entity)
            )
        }
        CodeActionType::Enumeration(enumeration) => format!(
            "CodeActionType::Enumeration({}.into())",
            crate::rust_string(enumeration)
        ),
        other => format!("CodeActionType::{other:?}"),
    }
}
