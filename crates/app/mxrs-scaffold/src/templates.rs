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

/// Entry function every scaffolded artifact file exposes, and that the family
/// aggregator's `DECLARATIONS` table points at.
pub(crate) const DECLARE: &str = "declare";
pub(crate) const DECLARATIONS_LIST: &str = "DECLARATIONS";
pub(crate) const FAMILIES_LIST: &str = "FAMILIES";
pub(crate) const MODULES_LIST: &str = "MODULES";

pub(crate) fn entity(module_name: &str, name: &str) -> String {
    format!(
        "//! Domain entity `{module_name}.{name}`.\n\
         //!\n\
         //! Attribute and association vocabulary: `mxrs::EntityBuilder`.\n\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.entity({name:?}, |_entity| {{}});\n\
         }}\n"
    )
}

pub(crate) fn enumeration(module_name: &str, name: &str) -> String {
    format!(
        "//! Enumeration `{module_name}.{name}`.\n\
         //!\n\
         //! Value vocabulary: `mxrs::EnumerationBuilder`.\n\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.enumeration({name:?}, |_enumeration| {{}});\n\
         }}\n"
    )
}

/// mxrb's `constant` template creates a string constant with an empty value,
/// and says so in its help ("Create a string constant"). Same default here:
/// the type and value are one edit away, but an empty string is the only
/// starting value that cannot be mistaken for a real one.
pub(crate) fn constant(module_name: &str, name: &str) -> String {
    format!(
        "//! Constant `{module_name}.{name}`.\n\
         //!\n\
         //! Type and value vocabulary: `mxrs::ConstantBuilder`.\n\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.constant({name:?}, |constant| {{\n        \
         constant.value(\"\");\n    \
         }});\n\
         }}\n"
    )
}

/// mxrb's `scheduled-event` template creates "scheduled event and handler" —
/// the event plus the microflow it runs, both under the same name. Mirrored
/// exactly, including the daily cadence: a scheduled event naming a microflow
/// that does not exist would fail the writer's reference checks on the very
/// first build, so scaffolding only half of the pair is not an option.
pub(crate) fn scheduled_event(module_name: &str, name: &str) -> String {
    format!(
        "//! Scheduled event `{module_name}.{name}` and the microflow it runs.\n\
         //!\n\
         //! Cadence vocabulary: `mxrs::ScheduledEventBuilder`. Day schedules\n\
         //! run once per day; `every` applies to minutes and hours.\n\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.microflow({name:?}, |_flow| {{}});\n    \
         module.scheduled_event({name:?}, {name:?}, ::mxrs::ScheduleUnit::Days, |_event| {{}});\n\
         }}\n"
    )
}

pub(crate) fn use_case(module_name: &str, name: &str) -> String {
    flow(
        module_name,
        name,
        "microflow",
        "Application use case",
        String::new(),
    )
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
/// creates the editable half and names the half it cannot create.
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

fn flow(
    module_name: &str,
    name: &str,
    builder_method: &str,
    description: &str,
    note: String,
) -> String {
    format!(
        "//! {description} `{module_name}.{name}`.\n\
         {note}//!\n\
         //! Activity vocabulary: `mxrs::FlowBuilder`.\n\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.{builder_method}({name:?}, |_flow| {{}});\n\
         }}\n"
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
    let allowed = roles
        .iter()
        .map(|role| format!("        page.allow_role({role:?});\n"))
        .collect::<String>();
    format!(
        "//! Page `{module_name}.{name}`.\n\
         //!\n\
         //! Widget vocabulary: `mxrs::PageBuilder`.\n\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.page({name:?}, |page| {{\n        \
         page.title({title:?});\n        \
         page.layout(\"{module_name}.ApplicationLayout\", {layout_parameter:?});\n\
         {allowed}        \
         page.text({title:?});\n    \
         }});\n\
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
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.layout(\"ApplicationLayout\", |layout| {{\n        \
         layout.placeholder({layout_parameter:?});\n    \
         }});\n\
         }}\n"
    )
}

pub(crate) fn module_roles(module_name: &str) -> String {
    format!(
        "//! Module roles for `{module_name}`.\n\
         //!\n\
         //! Declaring roles here makes them authoritative for this module: the\n\
         //! writer replaces the module's persisted role set rather than merging\n\
         //! into it (see `mxrs_ir::ModuleDecl::roles`).\n\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.role(\"User\", \"Application user\");\n    \
         module.role(\"Administrator\", \"Module administrator\");\n\
         }}\n"
    )
}

/// Project security is declared as its own `apply` peer rather than inside a
/// module, matching the layout `mxrs import` already generates. It returns
/// early when security is already declared so re-running `security init` for a
/// second module never silently replaces the first module's bindings.
///
/// `clear_roles` comes first because `ProjectSecurityDecl::default` already
/// carries an `Administrator` role: declaring one without clearing would hand
/// the writer two roles of the same name, which it rejects. Clearing means
/// `System.Administrator` has to be restated, and it is — dropping it would
/// silently unbind the platform's own administrator module role.
pub(crate) fn project_security(module_name: &str) -> String {
    format!(
        "//! Project-level security.\n\
         //!\n\
         //! `None` means \"preserve whatever the imported project already has\"\n\
         //! (see `mxrs_ir::ProjectDecl::security`), so this only declares\n\
         //! security when nothing else has.\n\n\
         pub fn apply(project: &mut ::mxrs::ProjectDecl) {{\n    \
         if project.security.is_some() {{\n        \
         return;\n    \
         }}\n    \
         let mut builder = ::mxrs::ProjectBuilder::new(project.mendix_version.clone());\n    \
         builder.security(|security| {{\n        \
         security.level(::mxrs::SecurityLevel::CheckEverything);\n        \
         security.clear_roles();\n        \
         security.role(\"User\", |role| {{\n            \
         role.description(\"Application user\");\n            \
         role.module_role(\"{module_name}.User\");\n        \
         }});\n        \
         security.role(\"Administrator\", |role| {{\n            \
         role.administrator(true);\n            \
         role.description(\"Application administrator\");\n            \
         role.module_role(\"System.Administrator\");\n            \
         role.module_role(\"{module_name}.Administrator\");\n        \
         }});\n    \
         }});\n    \
         project.security = builder.build().security;\n\
         }}\n"
    )
}

pub(crate) fn family_aggregator(module_name: &str, family: &str) -> String {
    format!(
        "//! `{family}` declarations for the `{module_name}` Mendix module.\n\n\
         pub fn apply(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         for declare in {DECLARATIONS_LIST} {{\n        \
         declare(module);\n    \
         }}\n\
         }}\n\n\
         const {DECLARATIONS_LIST}: &[fn(&mut ::mxrs::ModuleBuilder)] = &[];\n"
    )
}

/// Merges rather than pushing a module, so scaffolding into a project
/// imported from an existing `.mpr` extends that module instead of declaring
/// a second one with the same name.
pub(crate) fn module_aggregator(module_name: &str) -> String {
    format!(
        "//! Cargo-native declarations for the `{module_name}` Mendix module.\n\n\
         pub fn apply(project: &mut ::mxrs::ProjectDecl) {{\n    \
         let mut builder = ::mxrs::ProjectBuilder::new(project.mendix_version.clone());\n    \
         builder.module({module_name:?}, |module| {{\n        \
         for family in {FAMILIES_LIST} {{\n            \
         family(module);\n        \
         }}\n    \
         }});\n    \
         for declared in builder.build().modules {{\n        \
         project.merge_module(declared);\n    \
         }}\n\
         }}\n\n\
         const {FAMILIES_LIST}: &[fn(&mut ::mxrs::ModuleBuilder)] = &[];\n"
    )
}

pub(crate) fn modules_aggregator() -> String {
    format!(
        "//! Scaffolded Mendix modules, applied on top of whatever `build()`\n\
         //! has already composed.\n\n\
         pub fn apply(project: &mut ::mxrs::ProjectDecl) {{\n    \
         for declare in {MODULES_LIST} {{\n        \
         declare(project);\n    \
         }}\n\
         }}\n\n\
         const {MODULES_LIST}: &[fn(&mut ::mxrs::ProjectDecl)] = &[];\n"
    )
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

    #[test]
    fn every_template_declares_the_function_its_aggregator_calls() {
        let sources = [
            entity("Sales", "Order"),
            enumeration("Sales", "Status"),
            use_case("Sales", "ACT_Create"),
            nanoflow("Sales", "NAN_Refresh"),
            published_rest("Sales", "Handle"),
            consumed_rest("Sales", "Fetch"),
            java_action("Sales", "Invoke"),
            page("Sales", "Overview", "Main", &["Sales.User".to_string()]),
            layouts("Sales", "Main"),
            module_roles("Sales"),
        ];
        for source in &sources {
            assert!(
                source.contains("pub fn declare(module: &mut ::mxrs::ModuleBuilder) {"),
                "{source}"
            );
            assert!(source.ends_with("}\n"), "{source}");
        }
        assert!(sources[7].contains("page.allow_role(\"Sales.User\");"));
        assert!(sources[7].contains("page.layout(\"Sales.ApplicationLayout\", \"Main\");"));
        assert!(project_security("Sales").contains("pub fn apply(project"));
        assert!(
            family_aggregator("Sales", "entities")
                .contains("const DECLARATIONS: &[fn(&mut ::mxrs::ModuleBuilder)] = &[];")
        );
        assert!(module_aggregator("Sales").contains("project.merge_module(declared);"));
        assert!(
            modules_aggregator().contains("const MODULES: &[fn(&mut ::mxrs::ProjectDecl)] = &[];")
        );
    }
}
