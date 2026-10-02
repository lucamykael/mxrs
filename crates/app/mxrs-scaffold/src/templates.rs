//! Rust source emitted by the artifact scaffolds, plus the naming rules that
//! turn a Mendix artifact name into a Rust module path.
//!
//! mxrb's equivalent (`lib/mxrb/scaffold/templates.rb`) emits Ruby that is
//! only parsed when `project.rb` runs, so a template may mention a DSL method
//! that does not exist yet. Rust has no such slack: a scaffold that emitted a
//! call to a builder method `mxrs-dsl` does not have would break `cargo check`
//! for the whole project. Every template here therefore calls only methods
//! that exist today; a command whose mxrb template depends on a declaration
//! surface mxrs lacks is not implemented at all rather than scaffolded into
//! something that will not compile.

/// The domain layer of a fresh project: the modules it declares.
pub(crate) fn domain_layer() -> String {
    "//! Business data and rules that do not depend on delivery or infrastructure.\n\n\
     pub mod modules;\n"
        .to_string()
}

pub(crate) fn services_layer() -> String {
    "//! What the application does: every module's microflows as services, one\n//! folder per Mendix module and one service per subject.\n".to_string()
}

/// The user-interface layer of a fresh project: the application layout,
/// the home page that uses it, and the navigation that opens it.
pub(crate) fn ui_layer() -> String {
    "//! The user interface: pages, layouts, nanoflows and navigation.\n\n\
     pub mod layouts;\n\
     pub mod navigation;\n\
     pub mod pages;\n"
        .to_string()
}

/// `src/domain/modules/mod.rs`: the Mendix modules the project declares, one
/// file each.
pub(crate) fn module_registry(first: &str) -> String {
    format!("//! The Mendix modules this project declares, one file each.\n\npub mod {first};\n")
}

/// The home page `mxrs new` starts a project with. `title` is already
/// escaped for a Rust string literal.
pub(crate) fn home_page(title: &str) -> String {
    format!(
        "//! Page `Main.Home`.\n\
         //!\n\
         //! Widget vocabulary: `mxrs::PageBuilder`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[page(module = \"Main\")]\n\
         pub fn home(page: &mut PageBuilder) {{\n    \
         page.layout(\"Main.ApplicationLayout\", \"Main\");\n    \
         page.text(\"Welcome to {title}\");\n\
         }}\n"
    )
}

/// The navigation `mxrs new` starts a project with: one Responsive profile
/// opening the home page.
pub(crate) fn navigation() -> String {
    "//! The project's navigation profiles.\n\n\
     use mxrs::prelude::*;\n\n\
     #[navigation]\n\
     pub fn navigation(navigation: &mut NavigationBuilder) {\n    \
     navigation.profile(\"Responsive\", |profile| {\n        \
     profile.home_page(\"Main.Home\");\n    \
     });\n\
     }\n"
    .to_string()
}

/// The navigation item a templated/chain page adds for itself, declared in
/// the page's own file. `#[navigation_item]` appends to the profile it
/// names — creating the profile if the application no longer declares it —
/// so the generated item cannot break a build by assuming one.
fn page_navigation_item(module_name: &str, name: &str, function: &str) -> String {
    let caption = humanize(name);
    format!(
        "\n#[navigation_item(profile = \"Responsive\", caption = {caption:?})]\n\
         pub fn {function}_navigation(item: &mut NavigationItemBuilder) {{\n    \
         item.page(\"{module_name}.{name}\");\n    \
         item.icon(\"file\");\n\
         }}\n"
    )
}

pub(crate) fn infrastructure_layer() -> String {
    "//! Outbound adapters and generated platform integration.\n".to_string()
}

/// A Mendix name as the Rust type that declares it, the way the importer
/// spells it: each `_`-separated segment capitalized. `explicit` is whether
/// the declaration has to state the Mendix name because the type does not
/// read back to it.
pub(crate) fn type_name(name: &str) -> (String, bool) {
    let type_name = pascal_case(name);
    let explicit = type_name != name;
    (type_name, explicit)
}

/// `order_date` → `OrderDate`: the casing the declaration macros apply to a
/// Rust identifier to get its Mendix name.
pub(crate) fn pascal_case(value: &str) -> String {
    value
        .split('_')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `module = "Sales"`, plus `name = "..."` when the Rust item does not
/// spell the Mendix name.
fn declaration_arguments(module_name: &str, name: &str, explicit: bool) -> String {
    if explicit {
        format!("module = {module_name:?}, name = {name:?}")
    } else {
        format!("module = {module_name:?}")
    }
}

pub(crate) fn entity(module_name: &str, name: &str) -> String {
    let (type_name, explicit) = type_name(name);
    let arguments = declaration_arguments(module_name, name, explicit);
    format!(
        "//! Entity `{module_name}.{name}`.\n\
         //!\n\
         //! Fields are attributes (`MxString`, `MxDecimal`, `MxBool`, ...) and\n\
         //! associations (`Reference<T>`, `ReferenceSet<T>`); `#[mxrs(...)]`\n\
         //! states what a type cannot: `length`, `required`, `default`, `index(...)`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[entity({arguments})]\n\
         pub struct {type_name} {{}}\n"
    )
}

pub(crate) fn enumeration(module_name: &str, name: &str) -> String {
    let (type_name, explicit) = type_name(name);
    let arguments = declaration_arguments(module_name, name, explicit);
    format!(
        "//! Enumeration `{module_name}.{name}`.\n\
         //!\n\
         //! Variants are the values; `#[mxrs(caption = \"...\")]` captions one.\n\n\
         use mxrs::prelude::*;\n\n\
         #[enumeration({arguments})]\n\
         pub enum {type_name} {{}}\n"
    )
}

/// The function declaring a document named `name`, and whether the
/// declaration has to state that name.
fn document_function(name: &str) -> (String, bool) {
    let mut function = snake_case(name);
    if crate::is_rust_keyword(&function) {
        function.push('_');
    }
    let explicit = pascal_case(&function) != name;
    (function, explicit)
}

/// mxrb's `constant` template creates a string constant with an empty value,
/// and says so in its help ("Create a string constant"). Same default here:
/// the type and value are one edit away, but an empty string is the only
/// starting value that cannot be mistaken for a real one.
pub(crate) fn constant(module_name: &str, name: &str) -> String {
    let (function, explicit) = document_function(name);
    let arguments = declaration_arguments(module_name, name, explicit);
    format!(
        "//! Constant `{module_name}.{name}`.\n\
         //!\n\
         //! Type and value vocabulary: `mxrs::ConstantBuilder`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[constant({arguments})]\n\
         pub fn {function}(constant: &mut ConstantBuilder) {{\n    \
         constant.value(\"\");\n\
         }}\n"
    )
}

/// mxrb's `scheduled-event` template creates "scheduled event and handler" —
/// the event plus the microflow it runs, both under the same name. Mirrored
/// exactly, including the daily cadence: a scheduled event naming a microflow
/// that does not exist would fail the writer's reference checks on the very
/// first build, so scaffolding only half of the pair is not an option.
pub(crate) fn scheduled_event(module_name: &str, name: &str) -> String {
    let flow = flow_declaration("microflow", module_name, name);
    format!(
        "//! Scheduled event `{module_name}.{name}` and the microflow it runs.\n\
         //!\n\
         //! Cadence vocabulary: `mxrs::ScheduledEventBuilder`. Day schedules\n\
         //! run once per day; `every` applies to minutes and hours.\n\n\
         use mxrs::prelude::*;\n\n\
         #[{attribute}]\n\
         pub fn {function}(_flow: &mut FlowBuilder) {{}}\n\n\
         #[declaration(module = {module_name:?})]\n\
         pub fn {function}_event(module: &mut ModuleBuilder) {{\n    \
         module.scheduled_event({name:?}, {name:?}, ScheduleUnit::Days, |_event| {{}});\n\
         }}\n",
        attribute = flow.attribute,
        function = flow.function,
    )
}

pub(crate) fn use_case(module_name: &str, name: &str) -> String {
    flow(
        module_name,
        name,
        "microflow",
        "Application service",
        String::new(),
    )
}

pub(crate) fn validation(module_name: &str, name: &str) -> String {
    flow(
        module_name,
        name,
        "microflow",
        "Application validation",
        String::new(),
    )
}

pub(crate) fn integration(module_name: &str, name: &str) -> String {
    flow(
        module_name,
        name,
        "microflow",
        "Infrastructure integration adapter",
        String::new(),
    )
}

pub(crate) fn evaluation(_name: &str) -> String {
    "{\n  \"checks\": [\n    { \"type\": \"no_call_cycles\" },\n    { \"type\": \"no_missing_internal_references\" }\n  ]\n}\n".to_string()
}

pub(crate) fn github_workflow() -> String {
    "name: MXRS\n\non:\n  push:\n  pull_request:\n\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - uses: dtolnay/rust-toolchain@stable\n        with:\n          components: rustfmt, clippy\n      - run: cargo fmt --all -- --check\n      - run: cargo clippy --workspace --all-targets -- -D warnings\n      - run: cargo test --workspace --all-targets\n".to_string()
}

pub(crate) fn nanoflow(module_name: &str, name: &str) -> String {
    flow(
        module_name,
        name,
        "nanoflow",
        "Client nanoflow",
        String::new(),
    )
}

/// Mirrors mxrb's `published_rest` template, including its scope statement:
/// mxrb scaffolds a microflow and says publishing the REST document itself is
/// still a native Studio Pro operation. That is equally true here — mxrs has
/// no `Rest$PublishedRestService` declaration surface — so the scaffold
/// creates the editable half and names the half it cannot create. The
/// handler is a microflow, so it is an application service like any other;
/// `src/controllers/` holds what an import generates from a published
/// service, which is the half this cannot write.
pub(crate) fn published_rest(module_name: &str, name: &str) -> String {
    flow(
        module_name,
        name,
        "microflow",
        "Published REST handler",
        "//! Publishing the REST service document itself stays a native Studio Pro\n\
         //! operation: mxrs has no published-REST declaration surface, so this\n\
         //! scaffold creates only the handler microflow the service calls.\n"
            .to_string(),
    )
}

pub(crate) fn consumed_rest(module_name: &str, name: &str) -> String {
    flow(
        module_name,
        name,
        "microflow",
        "Consumed REST adapter",
        "//! The consumed-REST call itself stays a native Studio Pro operation:\n\
         //! `mxrs_ir::Activity` has no REST-call variant, so this scaffold creates\n\
         //! only the adapter microflow that will wrap it.\n"
            .to_string(),
    )
}

pub(crate) fn java_action(module_name: &str, name: &str) -> String {
    flow(
        module_name,
        name,
        "microflow",
        "Java Action adapter",
        "//! The Java Action document stays a native Studio Pro operation:\n\
         //! `mxrs_ir::Activity` has no Java-action-call variant, so this scaffold\n\
         //! creates only the adapter microflow that will call it.\n"
            .to_string(),
    )
}

pub(crate) fn repository_port(module_name: &str, name: &str) -> String {
    format!(
        "//! Application repository port `{module_name}.{name}`.\n\
         //!\n\
         //! Add domain-specific operations here; infrastructure adapters depend\n\
         //! on this contract, never the reverse.\n\n\
         pub trait Port {{}}\n"
    )
}

pub(crate) fn repository_adapter(module_name: &str, name: &str, stem: &str) -> String {
    format!(
        "//! Infrastructure adapter for `{module_name}.{name}`.\n\n\
         #[derive(Debug, Default, Clone, Copy)]\n\
         pub struct Adapter;\n\n\
         impl crate::ports::repositories::{stem}::Port for Adapter {{}}\n"
    )
}

/// How a scaffolded flow is declared: the attribute and the function it
/// sits on. `ACT_CreateOrder` is `#[microflow(ACT, module = "Sales")]` on
/// `create_order` — the same naming the importer applies, so a scaffolded
/// flow and an imported one are the same file.
pub(crate) struct FlowDeclaration {
    pub(crate) attribute: String,
    pub(crate) function: String,
}

/// `ACT_CreateOrder` → (`ACT`, `CreateOrder`): the capitals before the first
/// underscore, when the name follows that convention.
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

pub(crate) fn flow_declaration(kind: &str, module_name: &str, name: &str) -> FlowDeclaration {
    let (prefix, rest) = split_prefix(name);
    let mut function = snake_case(rest);
    // A flow body calls the prelude's expression functions, and the flow is
    // named by a type carrying its Mendix name: the function is neither.
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
    let derived = match prefix {
        Some(prefix) => format!("{prefix}_{}", pascal_case(&function)),
        None => pascal_case(&function),
    };
    let mut arguments = Vec::new();
    if let Some(prefix) = prefix {
        arguments.push(prefix.to_string());
    }
    arguments.push(format!("module = {module_name:?}"));
    if derived != name {
        arguments.push(format!("name = {name:?}"));
    }
    FlowDeclaration {
        attribute: format!("{kind}({})", arguments.join(", ")),
        function,
    }
}

/// The file a scaffolded microflow service lives in.
pub(crate) fn service_stem(name: &str) -> String {
    let function = flow_declaration("microflow", "", name).function;
    format!("{}_service", function.trim_end_matches('_'))
}

/// The file a scaffolded nanoflow lives in.
pub(crate) fn nanoflow_stem(name: &str) -> String {
    let function = flow_declaration("nanoflow", "", name).function;
    let stem = function.trim_end_matches('_');
    // The same rule the importer applies: a file Rust cannot name, or one
    // the folder keeps for itself, says what it holds instead.
    if stem.is_empty() || crate::is_rust_keyword(stem) || matches!(stem, "mod" | "imported") {
        format!("{stem}_nanoflow")
    } else {
        stem.to_string()
    }
}

fn flow(module_name: &str, name: &str, kind: &str, description: &str, note: String) -> String {
    let FlowDeclaration {
        attribute,
        function,
    } = flow_declaration(kind, module_name, name);
    format!(
        "//! {description} `{module_name}.{name}`.\n\
         {note}//!\n\
         //! Activity vocabulary: `mxrs::FlowBuilder`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[{attribute}]\n\
         pub fn {function}(_flow: &mut FlowBuilder) {{}}\n"
    )
}

/// `layout_parameter` names the placeholder on `<module>.ApplicationLayout`
/// that this page's widgets attach to; `layouts` scaffolds the same name.
pub(crate) fn page(
    module_name: &str,
    name: &str,
    layout_parameter: &str,
    roles: &[String],
) -> String {
    let title = humanize(name);
    let (function, explicit) = document_function(name);
    let arguments = declaration_arguments(module_name, name, explicit);
    let allowed = roles
        .iter()
        .map(|role| format!("    page.allow_role({role:?});\n"))
        .collect::<String>();
    format!(
        "//! Page `{module_name}.{name}`.\n\
         //!\n\
         //! Widget vocabulary: `mxrs::PageBuilder`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[page({arguments})]\n\
         pub fn {function}(page: &mut PageBuilder) {{\n    \
         page.title({title:?});\n    \
         page.layout(\"{module_name}.ApplicationLayout\", {layout_parameter:?});\n\
         {allowed}    \
         page.text({title:?});\n\
         }}\n"
    )
}

pub(crate) fn layouts(module_name: &str, layout_parameter: &str) -> String {
    format!(
        "//! Page layout for the `{module_name}` Mendix module.\n\
         //!\n\
         //! Scaffolded pages reference `{module_name}.ApplicationLayout`, so the\n\
         //! page scaffold creates this alongside the first page rather than\n\
         //! emitting a declaration that resolves to nothing.\n\n\
         use mxrs::prelude::*;\n\n\
         #[layout(module = {module_name:?})]\n\
         pub fn application_layout(layout: &mut LayoutBuilder) {{\n    \
         layout.placeholder({layout_parameter:?});\n\
         }}\n"
    )
}

pub(crate) fn module_roles(module_name: &str) -> String {
    format!(
        "//! Module roles for `{module_name}`.\n\
         //!\n\
         //! The enum is authoritative for this module: the writer replaces the\n\
         //! module's persisted role set with these rather than merging into it.\n\
         //! A variant's comment is the role's description.\n\n\
         use mxrs::prelude::*;\n\n\
         #[module_roles(module = {module_name:?})]\n\
         pub enum Role {{\n    \
         /// Application user\n    \
         User,\n    \
         /// Module administrator\n    \
         Administrator,\n\
         }}\n"
    )
}

/// Project security, declared once: `security init` writes it for the first
/// module and leaves it alone afterwards, so initializing a second module
/// never replaces the first module's bindings.
///
/// `clear_roles` comes first because the builder already carries an
/// `Administrator` role: declaring one without clearing would hand the
/// writer two roles of the same name, which it rejects. Clearing means
/// `System.Administrator` has to be restated, and it is — dropping it would
/// silently unbind the platform's own administrator module role.
pub(crate) fn project_security(module_name: &str) -> String {
    format!(
        "//! The project's security. Each module's own roles are declared in\n\
         //! `crate::domain::module_security`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[security]\n\
         pub fn security(security: &mut SecurityBuilder) {{\n    \
         security.level(SecurityLevel::CheckEverything);\n    \
         security.clear_roles();\n    \
         security.role(\"User\", |role| {{\n        \
         role.description(\"Application user\");\n        \
         role.module_role(\"{module_name}.User\");\n    \
         }});\n    \
         security.role(\"Administrator\", |role| {{\n        \
         role.administrator(true);\n        \
         role.description(\"Application administrator\");\n        \
         role.module_role(\"System.Administrator\");\n        \
         role.module_role(\"{module_name}.Administrator\");\n    \
         }});\n\
         }}\n"
    )
}

/// Compiled fallback theme, byte-identical to the one mxrb's `design init`
/// materializes (rendered from its `theme_compiled` template).
pub(crate) const THEME_COMPILED: &str = include_str!("../assets/theme/theme.compiled.css");
pub(crate) const THEME_SETTINGS: &str = include_str!("../assets/theme/settings.json");

pub(crate) fn theme_custom_variables() -> String {
    "// Project-specific Sass variables belong here. When Atlas Core is\n\
     // installed, mxrs marketplace adds its legacy variable definitions.\n"
        .to_string()
}

/// MXRB's `theme_main`: the custom-variables import followed by the whole
/// compiled fallback, so a project renders before any Sass pipeline runs.
pub(crate) fn theme_main() -> String {
    format!("@import \"custom-variables\";\n\n{THEME_COMPILED}")
}

pub(crate) fn theme_exclusion_variables() -> String {
    "// Atlas Core imports this file. Add Sass variables here to exclude\n\
     // optional Atlas components from the compiled theme.\n"
        .to_string()
}

/// One demo-user declaration. The password never appears here: the writer
/// resolves the named environment variable at write time, and an existing
/// stored password is preserved when the variable is absent.
pub(crate) fn demo_user(name: &str, entity: &str, roles: &[String], password_env: &str) -> String {
    let mut function = snake_case(name);
    if crate::is_rust_keyword(&function) {
        function.push('_');
    }
    let attribute = if function == name {
        "#[demo_user]".to_string()
    } else {
        format!("#[demo_user(name = {name:?})]")
    };
    let mut lines = Vec::new();
    if entity != "System.User" {
        lines.push(format!("    user.entity({entity:?});\n"));
    }
    for role in roles {
        lines.push(format!("    user.role({role:?});\n"));
    }
    lines.push(format!("    user.password_from_env({password_env:?});\n"));
    format!(
        "//! Demo user `{name}`.\n\n\
         use mxrs::prelude::*;\n\n\
         {attribute}\n\
         pub fn {function}(user: &mut DemoUserBuilder) {{\n{}}}\n",
        lines.concat()
    )
}

/// The declaration `mxrs module new` writes into the module registry: a
/// Mendix module exists as a named thing before it holds anything.
pub(crate) fn module_declaration(module_name: &str) -> String {
    let mut function = snake_case(module_name);
    if crate::is_rust_keyword(&function) {
        function.push('_');
    }
    format!(
        "//! The {module_name} Mendix module.\n\n\
         use mxrs::prelude::*;\n\n\
         #[declaration(module = {module_name:?})]\n\
         pub fn {function}(_module: &mut ModuleBuilder) {{}}\n"
    )
}

/// A concept index: the header and the first module folder it holds. Every
/// declaration registers itself, so `pub mod` is all an index ever holds.
pub(crate) fn registering_concept_index(concept: &str, first: &str) -> String {
    format!("//! Every module's `{concept}`, one folder per Mendix module.\n\npub mod {first};\n")
}

/// One module's folder inside a concept, holding its first file.
pub(crate) fn registering_folder_index(module_name: &str, folder: &str, first: &str) -> String {
    format!("//! The {module_name} module's `{folder}`.\n\npub mod {first};\n")
}

/// The index of a concept that holds one file per module rather than a
/// folder: the module registry, module security.
pub(crate) fn registry_index(concept: &str) -> String {
    format!("//! Each Mendix module's `{concept}`, one file per module.\n")
}

/// An index created before it holds anything.
pub(crate) fn empty_concept_index(concept: &str) -> String {
    format!("//! Every module's `{concept}`, one folder per Mendix module.\n")
}

/// A module folder index created before it holds anything.
pub(crate) fn empty_folder_index(module_name: &str, folder: &str) -> String {
    format!("//! The {module_name} module's `{folder}`.\n")
}

/// mxrb's `Templates.snake_case`, character for character — the generated file
/// name has to stay predictable for anyone moving between the two tools.
pub(crate) fn snake_case(value: &str) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len() + 4);
    for (index, character) in characters.iter().enumerate() {
        let previous = index.checked_sub(1).and_then(|index| characters.get(index));
        let next = characters.get(index + 1);
        let boundary = match (previous, character, next) {
            (Some(previous), current, _)
                if previous.is_ascii_lowercase() || previous.is_ascii_digit() =>
            {
                current.is_ascii_uppercase()
            }
            (Some(previous), current, Some(next)) => {
                previous.is_ascii_uppercase()
                    && current.is_ascii_uppercase()
                    && next.is_ascii_lowercase()
            }
            _ => false,
        };
        if boundary {
            output.push('_');
        }
        output.push(if *character == '-' {
            '_'
        } else {
            character.to_ascii_lowercase()
        });
    }
    output
}

/// mxrb's `Templates.humanize`, used for the caption a page scaffold renders.
pub(crate) fn humanize(value: &str) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len() + 4);
    for (index, character) in characters.iter().enumerate() {
        if *character == '_' {
            if !output.ends_with(' ') {
                output.push(' ');
            }
            continue;
        }
        if character.is_ascii_uppercase()
            && index
                .checked_sub(1)
                .and_then(|index| characters.get(index))
                .is_some_and(|previous| previous.is_ascii_lowercase() || previous.is_ascii_digit())
        {
            output.push(' ');
        }
        output.push(*character);
    }
    output.trim().to_string()
}

/// Declarative counterpart of mxrb's Ruby functional-test template. The file
/// is accepted directly by `mxrs test <mpr> <suite> --plan`.
pub(crate) fn functional_test(module_name: &str, name: &str) -> String {
    let display = humanize(name);
    format!(
        "{{\n  \"tests\": [\n    {{\n      \"name\": {display:?},\n      \"call\": {target:?},\n      \"expect\": {{}}\n    }}\n  ]\n}}\n",
        target = format!("{module_name}.{name}")
    )
}

pub(crate) fn presentation_layout(module_name: &str) -> String {
    format!(
        "//! Application layout for `{module_name}`.\n\nuse mxrs::prelude::*;\n\n#[layout(module = {module_name:?})]\npub fn application_layout(layout: &mut LayoutBuilder) {{\n    layout.class(\"mxrb-application-shell\");\n    layout.application_shell(\"ApplicationLayout\", Some(\"Responsive\"));\n}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_convert_the_same_way_mxrbs_templates_convert_them() {
        for (input, expected) in [
            ("Order", "order"),
            ("OrderLine", "order_line"),
            ("ACT_LoadOrder", "act_load_order"),
            ("HTTPResponse", "http_response"),
            ("Order2Line", "order2_line"),
            ("published-rest", "published_rest"),
        ] {
            assert_eq!(snake_case(input), expected, "{input}");
        }
        for (input, expected) in [
            ("OrderOverview", "Order Overview"),
            ("order_overview", "order overview"),
            ("ACT_LoadOrder", "ACT Load Order"),
            ("__Order__", "Order"),
        ] {
            assert_eq!(humanize(input), expected, "{input}");
        }
    }

    /// Every scaffolded artifact is the same file an import would have
    /// written: the prelude and one annotated item that registers itself.
    #[test]
    fn every_template_is_one_self_registering_declaration() {
        for (source, declaration) in [
            (
                entity("Sales", "Order"),
                "#[entity(module = \"Sales\")]\npub struct Order {}\n",
            ),
            (
                entity("Sales", "order_line"),
                "#[entity(module = \"Sales\", name = \"order_line\")]\npub struct OrderLine {}\n",
            ),
            (
                enumeration("Sales", "Status"),
                "#[enumeration(module = \"Sales\")]\npub enum Status {}\n",
            ),
            (
                use_case("Sales", "ACT_Create"),
                "#[microflow(ACT, module = \"Sales\")]\npub fn create(_flow: &mut FlowBuilder) {}\n",
            ),
            (
                validation("Sales", "VAL_Order_Total"),
                "#[microflow(VAL, module = \"Sales\", name = \"VAL_Order_Total\")]\npub fn order_total(_flow: &mut FlowBuilder) {}\n",
            ),
            (
                nanoflow("Sales", "NAN_Refresh"),
                "#[nanoflow(NAN, module = \"Sales\")]\npub fn refresh(_flow: &mut FlowBuilder) {}\n",
            ),
            (
                published_rest("Sales", "Handle"),
                "#[microflow(module = \"Sales\")]\npub fn handle(_flow: &mut FlowBuilder) {}\n",
            ),
            (
                consumed_rest("Sales", "String"),
                "#[microflow(module = \"Sales\")]\npub fn string_(_flow: &mut FlowBuilder) {}\n",
            ),
            (
                java_action("Sales", "invoke"),
                "#[microflow(module = \"Sales\", name = \"invoke\")]\npub fn invoke_flow(_flow: &mut FlowBuilder) {}\n",
            ),
            (
                constant("Sales", "MaximumOrders"),
                "#[constant(module = \"Sales\")]\npub fn maximum_orders(constant: &mut ConstantBuilder) {\n    constant.value(\"\");\n}\n",
            ),
            (
                layouts("Sales", "Main"),
                "#[layout(module = \"Sales\")]\npub fn application_layout(layout: &mut LayoutBuilder) {\n    layout.placeholder(\"Main\");\n}\n",
            ),
        ] {
            assert!(source.starts_with("//! "), "{source}");
            assert!(source.contains("\nuse mxrs::prelude::*;\n\n"), "{source}");
            assert!(source.ends_with(declaration), "{source}");
            for legacy in ["declaration()", "ModuleBuilder", "into_decl", "markers"] {
                assert!(!source.contains(legacy), "{legacy}\n{source}");
            }
        }
        let page = page("Sales", "Overview", "Main", &["Sales.User".to_string()]);
        assert!(
            page.ends_with(
                "#[page(module = \"Sales\")]\npub fn overview(page: &mut PageBuilder) {\n    page.title(\"Overview\");\n    page.layout(\"Sales.ApplicationLayout\", \"Main\");\n    page.allow_role(\"Sales.User\");\n    page.text(\"Overview\");\n}\n"
            ),
            "{page}"
        );
        // The flow a service file declares decides the file's name.
        assert_eq!(service_stem("ACT_CreateOrder"), "create_order_service");
        assert_eq!(service_stem("String"), "string_service");
        assert_eq!(nanoflow_stem("NAN_Refresh"), "refresh");
        assert_eq!(nanoflow_stem("Imported"), "imported_nanoflow");
        // Every index is its header and the modules it holds — nothing
        // composes, so there is no `apply` anywhere.
        for index in [
            registering_concept_index("entities", "sales"),
            registering_folder_index("Sales", "entities", "order"),
            empty_concept_index("entities"),
            empty_folder_index("Sales", "entities"),
            registry_index("modules"),
            module_registry("main"),
            domain_layer(),
            services_layer(),
            ui_layer(),
        ] {
            assert!(index.starts_with("//!"), "{index}");
            assert!(!index.contains("fn "), "{index}");
        }
    }

    /// What used to be composed by hand is declared like everything else.
    #[test]
    fn project_level_declarations_are_annotated_items() {
        assert!(module_declaration("Sales").ends_with(
            "#[declaration(module = \"Sales\")]\npub fn sales(_module: &mut ModuleBuilder) {}\n"
        ));
        assert!(
            module_roles("Sales").ends_with(
                "#[module_roles(module = \"Sales\")]\npub enum Role {\n    /// Application user\n    User,\n    /// Module administrator\n    Administrator,\n}\n"
            )
        );
        let security = project_security("Sales");
        assert!(
            security.contains("#[security]\npub fn security(security: &mut SecurityBuilder) {\n"),
            "{security}"
        );
        assert!(security.contains("role.module_role(\"Sales.Administrator\");"));
        assert_eq!(
            demo_user(
                "demo_admin",
                "System.User",
                &["Administrator".to_string()],
                "MXRS_DEMO_USER_DEMO_ADMIN_PASSWORD"
            ),
            "//! Demo user `demo_admin`.\n\nuse mxrs::prelude::*;\n\n#[demo_user]\npub fn demo_admin(user: &mut DemoUserBuilder) {\n    user.role(\"Administrator\");\n    user.password_from_env(\"MXRS_DEMO_USER_DEMO_ADMIN_PASSWORD\");\n}\n"
        );
        let named = demo_user("DemoAdmin", "Sales.Account", &[], "X");
        assert!(named.contains("#[demo_user(name = \"DemoAdmin\")]\npub fn demo_admin("));
        assert!(named.contains("    user.entity(\"Sales.Account\");\n"));
        assert!(navigation().contains("#[navigation]\npub fn navigation("));
        assert!(home_page("Shop").contains("#[page(module = \"Main\")]\npub fn home("));
    }

    /// A page slice names the model through the files that declare it.
    #[test]
    fn a_page_slice_imports_what_it_names() {
        // The loader is a method of the slice's entity's service.
        let loader = page_chain_loader("Sales", "OrderOverview");
        assert_eq!(loader.name, "ACT_LoadOrderOverview");
        assert_eq!(
            loader.imports,
            ["use crate::domain::entities::sales::order_overview::OrderOverview;"]
        );
        assert!(
            loader
                .body
                .contains(&"        OrderOverview::total().set(0.0),".to_string()),
            "{:?}",
            loader.body
        );
        let nanoflow = page_chain_nanoflow(
            "Sales",
            "OrderOverview",
            Some("crate::services::sales::order_overview_service"),
        );
        assert!(
            nanoflow.contains(
                "use crate::services::sales::order_overview_service::ACT_RefreshOrderOverview;"
            ),
            "{nanoflow}"
        );
        let page = page_from_template(
            "Sales",
            "OrderOverview",
            "Main",
            "form-vertical",
            Some(RefreshAction::Nanoflow),
            None,
            &[],
        );
        for expected in [
            "use crate::services::sales::order_overview_service::ACT_LoadOrderOverview;\n",
            "use crate::domain::entities::sales::order_overview::OrderOverview;\n",
            "use crate::ui::nanoflows::sales::refresh_order_overview::NAN_RefreshOrderOverview;\n",
            "#[page(module = \"Sales\")]\npub fn order_overview(page: &mut PageBuilder) {\n",
            "    page.data_view_from_microflow(MicroflowRef::<ACT_LoadOrderOverview>::new(), |view| {\n",
            "        view.text_box(OrderOverview::reference());\n",
            "            b.call_nanoflow(NanoflowRef::<NAN_RefreshOrderOverview>::new());\n",
            // The page adds itself to the navigation, from its own file.
            "\n#[navigation_item(profile = \"Responsive\", caption = \"Order Overview\")]\npub fn order_overview_navigation(item: &mut NavigationItemBuilder) {\n    item.page(\"Sales.OrderOverview\");\n    item.icon(\"file\");\n}\n",
        ] {
            assert!(page.contains(expected), "{expected}\n{page}");
        }
    }
}

/// Which flow a templated page's Refresh button calls, mirroring mxrb's
/// `page_refresh_action`: a `page:microflow` chain calls the microflow
/// directly, and any chain containing a nanoflow goes through the nanoflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshAction {
    Microflow,
    Nanoflow,
}

/// The three attributes mxrb's `page_chain_entity` template declares. Kept in
/// one place because the entity template, the loader that populates them and
/// the form template that binds them must agree.
const CHAIN_FIELDS: [&str; 3] = ["reference", "total", "active"];

/// Where the slice's backing entity is declared, and the struct declaring it.
fn chain_entity(module_name: &str, feature: &str) -> (String, String) {
    (
        format!(
            "crate::domain::entities::{}::{}",
            snake_case(module_name),
            snake_case(feature)
        ),
        type_name(feature).0,
    )
}

/// `ACT_Load<Feature>`, `ACT_Refresh<Feature>`: the server-side flows of a
/// page slice, named by the type their own declaration generates.
/// The loader is a method of the slice's own entity's service; the refresh
/// action is wherever its scaffold placed it (`refresh_service`).
fn chain_microflow(
    module_name: &str,
    action: &str,
    feature: &str,
    refresh_service: Option<&str>,
) -> (String, String) {
    let name = format!("ACT_{action}{feature}");
    let path = match refresh_service {
        Some(path) if action == "Refresh" => path.to_string(),
        _ => format!(
            "crate::services::{}::{}_service",
            snake_case(module_name),
            snake_case(feature)
        ),
    };
    (path, name)
}

pub(crate) fn page_chain_entity(module_name: &str, name: &str) -> String {
    let (type_name, explicit) = type_name(name);
    let arguments = declaration_arguments(module_name, name, explicit);
    let [reference, total, active] = CHAIN_FIELDS;
    format!(
        "//! Backing entity for the executable vertical slice generated by\n\
         //! `mxrs page new --chain`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[entity({arguments})]\n\
         pub struct {type_name} {{\n    \
         #[mxrs(length = 80)]\n    \
         pub {reference}: MxString,\n    \
         pub {total}: MxDecimal,\n    \
         #[mxrs(default = true)]\n    \
         pub {active}: MxBool,\n\
         }}\n"
    )
}

/// `ACT_Load<Feature>`: creates the context object the data-backed page's
/// data view renders. Mirrors mxrb's `page_chain_loader`, including creating
/// the record uncommitted with the same seed values. A method of the
/// feature's service.
pub(crate) fn page_chain_loader(module_name: &str, feature: &str) -> crate::service::ServiceMethod {
    let (entity_path, entity) = chain_entity(module_name, feature);
    let [reference, total, active] = CHAIN_FIELDS;
    crate::service::ServiceMethod {
        name: format!("ACT_Load{feature}"),
        docs: vec![
            format!("Loader for the `{module_name}.{feature}` page slice."),
            String::new(),
            "Returns an uncommitted record for the page's data view to bind to.".to_string(),
        ],
        imports: vec![format!("use {entity_path}::{entity};")],
        body: vec![
            "let record = flow.create_object(".to_string(),
            "    \"record\",".to_string(),
            format!("    Ref::<{entity}>::new(),"),
            "    vec![".to_string(),
            format!("        {entity}::{reference}().set(\"NEW\"),"),
            format!("        {entity}::{total}().set(0.0),"),
            format!("        {entity}::{active}().set(true),"),
            "    ],".to_string(),
            "    false,".to_string(),
            ");".to_string(),
            String::new(),
            "flow.return_value(record);".to_string(),
        ],
    }
}

/// `ACT_Refresh<Feature>`: the server-side half of a chain that ends in a
/// microflow. mxrb's template logs a message here; `mxrs_ir::Activity` has no
/// log activity, so the body is left empty rather than faked with an
/// unrelated activity.
pub(crate) fn page_chain_action(module_name: &str, feature: &str) -> crate::service::ServiceMethod {
    crate::service::ServiceMethod {
        name: format!("ACT_Refresh{feature}"),
        docs: vec![
            format!("Refresh action for the `{module_name}.{feature}` page slice."),
            String::new(),
            "Body intentionally empty: mxrb's template logs a message here and".to_string(),
            "`mxrs_ir::Activity` has no log activity to mirror it with.".to_string(),
        ],
        imports: Vec::new(),
        body: Vec::new(),
    }
}

/// `NAN_Refresh<Feature>`: the client-side half. With a
/// `page:nanoflow:microflow` chain it calls the generated microflow, which is
/// the only part of mxrb's two nanoflow templates that has an mxrs equivalent
/// (its `show_message` does not).
pub(crate) fn page_chain_nanoflow(
    module_name: &str,
    feature: &str,
    refresh_service: Option<&str>,
) -> String {
    let calls_microflow = refresh_service.is_some();
    let FlowDeclaration {
        attribute,
        function,
    } = flow_declaration("nanoflow", module_name, &format!("NAN_Refresh{feature}"));
    if !calls_microflow {
        return format!(
            "//! Client refresh action for the `{module_name}.{feature}` page slice.\n\
             //!\n\
             //! Body intentionally empty: mxrb's template shows a client\n\
             //! message here and `mxrs_ir::Activity` has no equivalent.\n\n\
             use mxrs::prelude::*;\n\n\
             #[{attribute}]\n\
             pub fn {function}(_flow: &mut FlowBuilder) {{}}\n"
        );
    }
    let (action_path, action) = chain_microflow(module_name, "Refresh", feature, refresh_service);
    format!(
        "//! Client refresh action for the `{module_name}.{feature}` page slice.\n\
         //!\n\
         //! Calls the generated microflow, completing the client-to-server\n\
         //! half of a `page:nanoflow:microflow` chain.\n\n\
         use mxrs::prelude::*;\n\n\
         use {action_path}::{action};\n\n\
         #[{attribute}]\n\
         pub fn {function}(flow: &mut FlowBuilder) {{\n    \
         flow.call_microflow(MicroflowRef::<{action}>::new(), None, false, vec![]);\n\
         }}\n"
    )
}

/// Renders one of the catalogued page patterns. Mirrors mxrb's
/// `page_from_template` dispatch, with one deviation forced by the typed DSL
/// and named here rather than left to be discovered: mxrb's `number_input`
/// has no mxrs widget, so a decimal attribute binds to a `text_box`, which is
/// what Mendix itself renders for one.
///
/// Widgets bind through the accessors the slice's entity declares, and the
/// flows are named by the types their own declarations generate — the page
/// imports both from the files that own them.
pub(crate) fn page_from_template(
    module_name: &str,
    name: &str,
    layout_parameter: &str,
    template: &str,
    refresh: Option<RefreshAction>,
    refresh_service: Option<&str>,
    roles: &[String],
) -> String {
    let title = humanize(name);
    let (function, explicit) = document_function(name);
    let arguments = declaration_arguments(module_name, name, explicit);
    let allowed = roles
        .iter()
        .map(|role| format!("    page.allow_role({role:?});\n"))
        .collect::<String>();
    let header = format!(
        "#[page({arguments})]\n\
         pub fn {function}(page: &mut PageBuilder) {{\n    \
         page.title({title:?});\n    \
         page.layout(\"{module_name}.ApplicationLayout\", {layout_parameter:?});\n\
         {allowed}"
    );
    let body = match template {
        "starter" => format!(
            "    page.container(|header| {{\n        \
             header.name(\"pageHeader\");\n        \
             header.class(\"mxrs-page-header\");\n        \
             header.text({title:?});\n        \
             header.text(\"Page generated from the starter template\");\n    \
             }});\n\
             {}",
            refresh_button(refresh, name, "page", 1)
        ),
        "blank" => format!(
            "    page.container(|content| {{\n        \
             content.name(\"content\");\n        \
             content.class(\"mxrs-page-content\");\n\
             {}    \
             }});\n",
            refresh_button(refresh, name, "content", 2)
        ),
        "dashboard" => format!(
            "    page.container(|header| {{\n        \
             header.name(\"pageHeader\");\n        \
             header.class(\"mxrs-page-header\");\n        \
             header.text({title:?});\n        \
             header.text(\"Dashboard overview\");\n    \
             }});\n    \
             page.container(|dashboard| {{\n        \
             dashboard.name(\"dashboard\");\n        \
             dashboard.class(\"mxrs-dashboard-grid\");\n\
             {}    \
             }});\n\
             {}",
            dashboard_cards(),
            refresh_button(refresh, name, "page", 1)
        ),
        _ => form_vertical_body(name, &title, refresh),
    };
    // What the page names beyond its own widgets: the flow its Refresh
    // button calls, and for the data-backed template the entity it binds
    // and the loader that supplies it.
    let mut imports = Vec::new();
    match refresh {
        Some(RefreshAction::Microflow) => {
            let (path, action) = chain_microflow(module_name, "Refresh", name, refresh_service);
            imports.push(format!("use {path}::{action};"));
        }
        Some(RefreshAction::Nanoflow) => imports.push(format!(
            "use crate::ui::nanoflows::{}::{}::NAN_Refresh{name};",
            snake_case(module_name),
            nanoflow_stem(&format!("NAN_Refresh{name}"))
        )),
        None => {}
    }
    if template == DATA_BACKED_TEMPLATE {
        let (path, loader) = chain_microflow(module_name, "Load", name, None);
        imports.push(format!("use {path}::{loader};"));
        let (path, entity) = chain_entity(module_name, name);
        imports.push(format!("use {path}::{entity};"));
    }
    imports.sort();
    let imports = if imports.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", imports.join("\n"))
    };
    let navigation = page_navigation_item(module_name, name, &function);
    format!(
        "//! Page `{module_name}.{name}` from the `{template}` template.\n\
         //!\n\
         //! Widget vocabulary: `mxrs::PageBuilder`.\n\n\
         use mxrs::prelude::*;\n\n\
         {imports}{header}{body}}}\n{navigation}"
    )
}

/// The one catalogued template whose page renders a data view. Duplicated as
/// a name here rather than as a bare string literal so the renderer and
/// `page_templates::ENTRIES` cannot disagree about which one it is.
const DATA_BACKED_TEMPLATE: &str = "form-vertical";

fn dashboard_cards() -> String {
    [
        (
            "primary",
            "PRIMARY",
            "0",
            "Connect this card to your domain data.",
        ),
        (
            "secondary",
            "SECONDARY",
            "0",
            "Replace this metric with a business signal.",
        ),
        (
            "activity",
            "ACTIVITY",
            "Ready",
            "Add charts, lists or actions here.",
        ),
    ]
    .iter()
    .map(|(slot, label, value, help)| {
        format!(
            "        dashboard.container(|card| {{\n            \
             card.name(\"{slot}Metric\");\n            \
             card.class(\"mxrs-card\");\n            \
             card.text({label:?});\n            \
             card.text({value:?});\n            \
             card.text({help:?});\n        \
             }});\n"
        )
    })
    .collect()
}

fn form_vertical_body(name: &str, title: &str, refresh: Option<RefreshAction>) -> String {
    let entity = type_name(name).0;
    let [reference, total, active] = CHAIN_FIELDS;
    format!(
        "    page.data_view_from_microflow(MicroflowRef::<ACT_Load{name}>::new(), |view| {{\n        \
         view.container(|header| {{\n            \
         header.name(\"pageHeader\");\n            \
         header.class(\"mxrs-page-header\");\n            \
         header.text({title:?});\n            \
         header.text(\"Executable page scaffold\");\n        \
         }});\n        \
         view.text_box({entity}::{reference}());\n        \
         view.text_box({entity}::{total}());\n        \
         view.check_box({entity}::{active}());\n        \
         view.button(\"Save\", |b| {{\n            \
         b.name(\"save\");\n            \
         b.save_changes();\n        \
         }});\n        \
         view.button(\"Cancel\", |b| {{\n            \
         b.name(\"cancel\");\n            \
         b.cancel_changes();\n        \
         }});\n\
         {}    \
         }});\n",
        refresh_button(refresh, name, "view", 2)
    )
}

/// mxrb emits this button only when a chain was requested; without one the
/// template has nothing to call.
fn refresh_button(
    refresh: Option<RefreshAction>,
    name: &str,
    receiver: &str,
    depth: usize,
) -> String {
    let Some(refresh) = refresh else {
        return String::new();
    };
    let indent = "    ".repeat(depth);
    let call = match refresh {
        RefreshAction::Microflow => {
            format!("b.call_microflow(MicroflowRef::<ACT_Refresh{name}>::new());")
        }
        RefreshAction::Nanoflow => {
            format!("b.call_nanoflow(NanoflowRef::<NAN_Refresh{name}>::new());")
        }
    };
    format!(
        "{indent}{receiver}.button(\"Refresh\", |b| {{\n{indent}    \
         b.name(\"refresh\");\n{indent}    \
         {call}\n{indent}\
         }});\n"
    )
}
