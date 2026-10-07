//! The code actions of the modules a project made, as the Rust that
//! declares each one: `src/ports/<module>/java_actions/<action>.rs` and
//! `src/ports/<module>/javascript_actions/<action>.rs`.
//!
//! An action is declared only when its declaration states the stored
//! document again (`JavaActionDecl::from_document`,
//! `JavaScriptActionDecl::from_document`); one it cannot say — a toolbox
//! icon, a type parameter — stays in the imported model.

use std::fmt::Write as _;

use mxrs_ir::{
    CodeActionParameter, CodeActionType, ExportLevel, JavaActionDecl, JavaScriptActionDecl,
    JavaScriptPlatform, NativeDocument,
};

/// Which code actions: those a microflow runs in Java, or a nanoflow in
/// JavaScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Java,
    JavaScript,
}

impl Kind {
    pub(crate) fn native_type(self) -> &'static str {
        match self {
            Kind::Java => "JavaActions$JavaAction",
            Kind::JavaScript => "JavaScriptActions$JavaScriptAction",
        }
    }
}

/// An action as its declaration states it.
enum Declaration {
    Java(JavaActionDecl),
    JavaScript(JavaScriptActionDecl),
}

impl Declaration {
    fn read(kind: Kind, document: &NativeDocument) -> Option<Self> {
        match kind {
            Kind::Java => JavaActionDecl::from_document(document).map(Self::Java),
            Kind::JavaScript => JavaScriptActionDecl::from_document(document).map(Self::JavaScript),
        }
    }

    fn parameters(&self) -> &[CodeActionParameter] {
        match self {
            Declaration::Java(action) => &action.parameters,
            Declaration::JavaScript(action) => &action.parameters,
        }
    }

    fn name(&self) -> &str {
        match self {
            Declaration::Java(action) => &action.name,
            Declaration::JavaScript(action) => &action.name,
        }
    }
}

/// One action the importer declares: its module, its name, and the file
/// that states it.
pub(crate) struct DeclaredAction {
    pub(crate) module: String,
    pub(crate) name: String,
    pub(crate) stem: String,
    pub(crate) source: String,
}

/// The code actions of `kind` their declaration restates, of the modules
/// `authored` says the project made.
pub(crate) fn declare(
    project: &mxrs_model::Project,
    kind: Kind,
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
        if document.get_str("$Type").ok() != Some(kind.native_type()) {
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
            .and_then(|stated| Declaration::read(kind, &stated))
            .filter(|declaration| {
                declaration.parameters().iter().all(|parameter| {
                    parameter.description.is_empty() && parameter.category.is_empty()
                })
            })
        else {
            continue;
        };
        let stem = crate::inner_file_stem(declaration.name());
        let source = render(&module, &stem, &declaration);
        declared.push(DeclaredAction {
            module,
            name: declaration.name().to_string(),
            stem,
            source,
        });
    }
    declared.sort_by(|left, right| (&left.module, &left.name).cmp(&(&right.module, &right.name)));
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

/// The file declaring `declaration`, stating only what differs from a new
/// one.
fn render(module: &str, stem: &str, declaration: &Declaration) -> String {
    let (parameters, return_type, return_name, documentation, excluded, export_level) =
        match declaration {
            Declaration::Java(action) => (
                &action.parameters,
                &action.return_type,
                &action.return_name,
                &action.documentation,
                action.excluded,
                action.export_level,
            ),
            Declaration::JavaScript(action) => (
                &action.parameters,
                &action.return_type,
                &action.return_name,
                &action.documentation,
                action.excluded,
                action.export_level,
            ),
        };
    let name = declaration.name();
    let lowercase = module.to_lowercase();
    let mut calls = Vec::new();
    for parameter in parameters {
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
    match return_type {
        CodeActionType::Void => {}
        ty => calls.push(match rust_type(ty) {
            Some(ty) => format!(".returns::<{ty}>()"),
            None => format!(".returns_type({})", type_expression(ty)),
        }),
    }
    if return_name != "ReturnValueName" {
        calls.push(format!(".return_name({})", crate::rust_string(return_name)));
    }
    if let Declaration::JavaScript(action) = declaration
        && action.platform != JavaScriptPlatform::All
    {
        calls.push(format!(
            ".platform(JavaScriptPlatform::{})",
            action.platform.as_str()
        ));
    }
    if !documentation.is_empty() {
        calls.push(format!(
            ".documentation({})",
            crate::rust_string(documentation)
        ));
    }
    if excluded {
        calls.push(".excluded(true)".to_string());
    }
    if export_level == ExportLevel::Published {
        calls.push(".export_level(ExportLevel::Published)".to_string());
    }
    let (header, method) = match declaration {
        Declaration::Java(_) => (
            format!(
                "//! Java action `{module}.{name}`: what a microflow's call takes and\n//! returns. Its Java is `java/{lowercase}/actions/{name}.java`; in mxrs it runs\n//! the Rust registered for it.\n"
            ),
            "java_action",
        ),
        Declaration::JavaScript(_) => (
            format!(
                "//! JavaScript action `{module}.{name}`: what a nanoflow's call takes and\n//! returns. Its JavaScript is `javascriptsource/{lowercase}/actions/{name}.js`.\n"
            ),
            "javascript_action",
        ),
    };
    let mut source = format!(
        "{header}\nuse mxrs::prelude::*;\n\n#[declaration(module = {})]\npub fn {stem}(module: &mut ModuleBuilder) {{\n",
        crate::rust_string(module),
    );
    let name = crate::rust_string(name);
    if calls.is_empty() {
        let _ = writeln!(source, "    module.{method}({name}, |_action| {{}});");
    } else {
        let _ = writeln!(source, "    module.{method}({name}, |action| {{");
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
