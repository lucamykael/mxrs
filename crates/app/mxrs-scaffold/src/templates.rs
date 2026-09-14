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

pub(crate) fn application_layer() -> String {
    "pub mod modules;\n\npub fn build() -> mxrs::ProjectDecl {\n    let mut project = crate::domain::build();\n    modules::apply(&mut project);\n    project\n}\n"
        .to_string()
}

pub(crate) fn empty_presentation_layer() -> String {
    "pub mod modules;\n\npub fn apply(project: &mut mxrs::ProjectDecl) {\n    modules::apply(project);\n}\n"
        .to_string()
}

pub(crate) fn infrastructure_layer() -> String {
    "//! Outbound adapters and generated platform integration.\n".to_string()
}

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
         impl crate::application::repositories::{stem}::Port for Adapter {{}}\n"
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

/// Declarative counterpart of mxrb's Ruby functional-test template. The file
/// is accepted directly by `mxrs test <mpr> <suite> --plan`.
pub(crate) fn functional_test(module_name: &str, name: &str) -> String {
    let display = humanize(name);
    format!(
        "{{\n  \"tests\": [\n    {{\n      \"name\": {display:?},\n      \"call\": {target:?},\n      \"expect\": {{}}\n    }}\n  ]\n}}\n",
        target = format!("{module_name}.{name}")
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

/// Which flow a templated page's Refresh button calls, mirroring mxrb's
/// `page_refresh_action`: a `page:microflow` chain calls the microflow
/// directly, and any chain containing a nanoflow goes through the nanoflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshAction {
    Microflow,
    Nanoflow,
}

/// Marker impls the scaffolded page and flows reference.
///
/// mxrb's templates name flows and entities with strings, so its generated
/// slice needs nothing like this. The mxrs DSL takes marker *types*
/// (`MicroflowRef<M>`, `text_box::<A>()`), so a generated vertical slice has
/// to generate the markers too — hand-implemented markers are a supported
/// alternative to `mxrs-typegen` output (see `mxrs_ir::markers`). They are
/// declared `pub` in a `markers` module so sibling files in the slice can
/// refer to them by path.
fn entity_markers(module_name: &str, entity: &str, attributes: &[(&str, &str)]) -> String {
    let mut out = format!(
        "#[allow(non_camel_case_types, non_snake_case, dead_code)]\n\
         pub mod markers {{\n    \
         //! Marker types for the generated slice. `mxrs-typegen` emits the\n    \
         //! same shapes from a manifest, under the same `Entity_Attribute`\n    \
         //! naming; these are written out because a scaffolded artifact has\n    \
         //! no manifest entry yet.\n\n    \
         pub struct {entity};\n    \
         impl ::mxrs::EntityMarker for {entity} {{\n        \
         const MODULE: &'static str = {module_name:?};\n        \
         const NAME: &'static str = {entity:?};\n    \
         }}\n"
    );
    for (attribute, value) in attributes {
        out.push_str(&format!(
            "\n    pub struct {entity}_{attribute};\n    \
             impl ::mxrs::AttributeMarker for {entity}_{attribute} {{\n        \
             type Entity = {entity};\n        \
             const NAME: &'static str = {attribute:?};\n    \
             }}\n    \
             impl ::mxrs::TypedAttributeMarker for {entity}_{attribute} {{\n        \
             type Value = ::mxrs::{value};\n    \
             }}\n"
        ));
    }
    out.push_str("}\n");
    out
}

fn flow_marker(module_name: &str, name: &str, trait_name: &str) -> String {
    format!(
        "#[allow(non_camel_case_types, non_snake_case, dead_code)]\n\
         pub mod markers {{\n    \
         //! See the entity slice's `markers` module for why these are\n    \
         //! hand-written rather than generated from a manifest.\n\n    \
         pub struct {name};\n    \
         impl ::mxrs::{trait_name} for {name} {{\n        \
         const MODULE: &'static str = {module_name:?};\n        \
         const NAME: &'static str = {name:?};\n    \
         }}\n\
         }}\n"
    )
}

/// The three attributes mxrb's `page_chain_entity` template declares. Kept in
/// one place because the entity template, the loader that populates them and
/// the form template that binds them must agree.
const CHAIN_ATTRIBUTES: &[(&str, &str)] = &[
    ("Reference", "MxString"),
    ("Total", "MxDecimal"),
    ("Active", "MxBool"),
];

pub(crate) fn page_chain_entity(module_name: &str, name: &str) -> String {
    let markers = entity_markers(module_name, name, CHAIN_ATTRIBUTES);
    format!(
        "//! Backing entity for the executable vertical slice generated by\n\
         //! `mxrs page new --chain`.\n\n\
         {markers}\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.entity({name:?}, |entity| {{\n        \
         entity.string(\"Reference\").length = Some(80);\n        \
         entity.decimal(\"Total\").default_value = Some(\"0\".into());\n        \
         entity.boolean(\"Active\").default_value = Some(\"true\".into());\n    \
         }});\n\
         }}\n"
    )
}

/// `ACT_Load<Feature>`: creates the context object the data-backed page's
/// data view renders. Mirrors mxrb's `page_chain_loader`, including creating
/// the record uncommitted with the same seed values.
pub(crate) fn page_chain_loader(module_name: &str, feature: &str) -> String {
    let entity_path = format!(
        "crate::domain::modules::{}::entities::{}::markers",
        snake_case(module_name),
        snake_case(feature)
    );
    let name = format!("ACT_Load{feature}");
    let markers = flow_marker(module_name, &name, "MicroflowMarker");
    format!(
        "//! Loader for the `{module_name}.{feature}` page slice.\n\
         //!\n\
         //! Returns an uncommitted record for the page's data view to bind to.\n\n\
         use {entity_path}::{{{feature}, {feature}_Active, {feature}_Reference, {feature}_Total}};\n\n\
         {markers}\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.microflow({name:?}, |flow| {{\n        \
         let record = flow.create_object(\n            \
         \"record\",\n            \
         ::mxrs::Ref::<{feature}>::new(),\n            \
         vec![\n                \
         ::mxrs::attribute::<{feature}_Reference>(::mxrs::string(\"NEW\")),\n                \
         ::mxrs::attribute::<{feature}_Total>(::mxrs::decimal(0.0)),\n                \
         ::mxrs::attribute::<{feature}_Active>(::mxrs::boolean(true)),\n            \
         ],\n            \
         false,\n        \
         );\n        \
         flow.return_value(record);\n    \
         }});\n\
         }}\n"
    )
}

/// `ACT_Refresh<Feature>`: the server-side half of a chain that ends in a
/// microflow. mxrb's template logs a message here; `mxrs_ir::Activity` has no
/// log activity, so the body is left empty rather than faked with an
/// unrelated activity.
pub(crate) fn page_chain_action(module_name: &str, feature: &str) -> String {
    let name = format!("ACT_Refresh{feature}");
    let markers = flow_marker(module_name, &name, "MicroflowMarker");
    format!(
        "//! Refresh action for the `{module_name}.{feature}` page slice.\n\
         //!\n\
         //! Body intentionally empty: mxrb's template logs a message here and\n\
         //! `mxrs_ir::Activity` has no log activity to mirror it with.\n\n\
         {markers}\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.microflow({name:?}, |_flow| {{}});\n\
         }}\n"
    )
}

/// `NAN_Refresh<Feature>`: the client-side half. With a
/// `page:nanoflow:microflow` chain it calls the generated microflow, which is
/// the only part of mxrb's two nanoflow templates that has an mxrs equivalent
/// (its `show_message` does not).
pub(crate) fn page_chain_nanoflow(
    module_name: &str,
    feature: &str,
    calls_microflow: bool,
) -> String {
    let name = format!("NAN_Refresh{feature}");
    let markers = flow_marker(module_name, &name, "NanoflowMarker");
    let (note, body) = if calls_microflow {
        let action_path = format!(
            "crate::application::modules::{}::use_cases::act_refresh_{}::markers::ACT_Refresh{feature}",
            snake_case(module_name),
            snake_case(feature)
        );
        (
            "//! Calls the generated microflow, completing the client-to-server\n\
             //! half of a `page:nanoflow:microflow` chain.\n"
                .to_string(),
            format!(
                "|flow| {{\n        \
                 flow.call_microflow(\n            \
                 ::mxrs::MicroflowRef::<{action_path}>::new(),\n            \
                 None,\n            \
                 false,\n            \
                 vec![],\n        \
                 );\n    \
                 }}"
            ),
        )
    } else {
        (
            "//! Body intentionally empty: mxrb's template shows a client\n\
             //! message here and `mxrs_ir::Activity` has no equivalent.\n"
                .to_string(),
            "|_flow| {}".to_string(),
        )
    };
    format!(
        "//! Client refresh action for the `{module_name}.{feature}` page slice.\n\
         //!\n\
         {note}\n\
         {markers}\n\
         pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.nanoflow({name:?}, {body});\n\
         }}\n"
    )
}

/// Renders one of the catalogued page patterns. Mirrors mxrb's
/// `page_from_template` dispatch, with two deviations forced by the typed
/// DSL and named here rather than left to be discovered:
///
/// 1. mxrb's `number_input` has no mxrs widget; a decimal attribute binds to
///    a `text_box`, which is what Mendix itself renders for one.
/// 2. Widgets bind through attribute *markers*, so `form-vertical` references
///    the generated entity slice's `markers` module by path.
pub(crate) fn page_from_template(
    module_name: &str,
    name: &str,
    layout_parameter: &str,
    template: &str,
    refresh: Option<RefreshAction>,
    roles: &[String],
) -> String {
    let title = humanize(name);
    let allowed = roles
        .iter()
        .map(|role| format!("        page.allow_role({role:?});\n"))
        .collect::<String>();
    let header = format!(
        "pub fn declare(module: &mut ::mxrs::ModuleBuilder) {{\n    \
         module.page({name:?}, |page| {{\n        \
         page.title({title:?});\n        \
         page.layout(\"{module_name}.ApplicationLayout\", {layout_parameter:?});\n\
         {allowed}"
    );
    let body = match template {
        "starter" => format!(
            "        page.container(|header| {{\n            \
             header.name(\"pageHeader\");\n            \
             header.class(\"mxrs-page-header\");\n            \
             header.text({title:?});\n            \
             header.text(\"Page generated from the starter template\");\n        \
             }});\n\
             {}",
            refresh_button(module_name, name, refresh, "page", 2)
        ),
        "blank" => format!(
            "        page.container(|content| {{\n            \
             content.name(\"content\");\n            \
             content.class(\"mxrs-page-content\");\n\
             {}        \
             }});\n",
            refresh_button(module_name, name, refresh, "content", 3)
        ),
        "dashboard" => format!(
            "        page.container(|header| {{\n            \
             header.name(\"pageHeader\");\n            \
             header.class(\"mxrs-page-header\");\n            \
             header.text({title:?});\n            \
             header.text(\"Dashboard overview\");\n        \
             }});\n        \
             page.container(|dashboard| {{\n            \
             dashboard.name(\"dashboard\");\n            \
             dashboard.class(\"mxrs-dashboard-grid\");\n\
             {}        \
             }});\n\
             {}",
            dashboard_cards(),
            refresh_button(module_name, name, refresh, "page", 2)
        ),
        _ => form_vertical_body(module_name, name, &title, refresh),
    };
    let imports = if template == DATA_BACKED_TEMPLATE {
        format!(
            "use crate::domain::modules::{}::entities::{}::markers::{{\n    \
             {name}_Active, {name}_Reference, {name}_Total,\n\
             }};\n\n",
            snake_case(module_name),
            snake_case(name)
        )
    } else {
        String::new()
    };
    format!(
        "//! Page `{module_name}.{name}` from the `{template}` template.\n\
         //!\n\
         //! Widget vocabulary: `mxrs::PageBuilder`.\n\n\
         {imports}{header}{body}    }});\n}}\n"
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
            "                dashboard.container(|card| {{\n                    \
             card.name(\"{slot}Metric\");\n                    \
             card.class(\"mxrs-card\");\n                    \
             card.text({label:?});\n                    \
             card.text({value:?});\n                    \
             card.text({help:?});\n                \
             }});\n"
        )
    })
    .collect()
}

fn form_vertical_body(
    module_name: &str,
    name: &str,
    title: &str,
    refresh: Option<RefreshAction>,
) -> String {
    let loader = format!(
        "crate::application::modules::{}::use_cases::act_load_{}::markers::ACT_Load{name}",
        snake_case(module_name),
        snake_case(name)
    );
    format!(
        "        page.data_view_from_microflow(\n            \
         ::mxrs::MicroflowRef::<{loader}>::new(),\n            \
         |view| {{\n                \
         view.container(|header| {{\n                    \
         header.name(\"pageHeader\");\n                    \
         header.class(\"mxrs-page-header\");\n                    \
         header.text({title:?});\n                    \
         header.text(\"Executable page scaffold\");\n                \
         }});\n                \
         view.text_box::<{name}_Reference>();\n                \
         view.text_box::<{name}_Total>();\n                \
         view.check_box::<{name}_Active>();\n                \
         view.button(\"Save\", |b| {{\n                    \
         b.name(\"save\");\n                    \
         b.save_changes();\n                \
         }});\n                \
         view.button(\"Cancel\", |b| {{\n                    \
         b.name(\"cancel\");\n                    \
         b.cancel_changes();\n                \
         }});\n\
         {}            \
         }},\n        \
         );\n",
        refresh_button(module_name, name, refresh, "view", 4)
    )
}

/// mxrb emits this button only when a chain was requested; without one the
/// template has nothing to call.
fn refresh_button(
    module_name: &str,
    name: &str,
    refresh: Option<RefreshAction>,
    receiver: &str,
    depth: usize,
) -> String {
    let Some(refresh) = refresh else {
        return String::new();
    };
    let indent = "    ".repeat(depth);
    let call = match refresh {
        RefreshAction::Microflow => format!(
            "b.call_microflow(::mxrs::MicroflowRef::<\
             crate::application::modules::{}::use_cases::act_refresh_{}::markers::ACT_Refresh{name}\
             >::new());",
            snake_case(module_name),
            snake_case(name)
        ),
        RefreshAction::Nanoflow => format!(
            "b.call_nanoflow(::mxrs::NanoflowRef::<\
             crate::presentation::modules::{}::nanoflows::nan_refresh_{}::markers::NAN_Refresh{name}\
             >::new());",
            snake_case(module_name),
            snake_case(name)
        ),
    };
    format!(
        "{indent}{receiver}.button(\"Refresh\", |b| {{\n{indent}    \
         b.name(\"refresh\");\n{indent}    \
         {call}\n{indent}\
         }});\n"
    )
}
