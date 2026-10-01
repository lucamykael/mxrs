//! Declarations spelled with the very words the macros work with: a function
//! called `project`, a field called `desc`, a flow called `union`. None of
//! them is special to the person writing it, so none may be special here.

use mxrs::prelude::*;

#[entity(module = "Edge")]
#[mxrs(stores(owner, changed_date), index(desc, system, desc(r#type)))]
pub struct Ledger {
    pub desc: MxString,
    pub system: MxString,
    pub r#type: MxString,
    #[mxrs(default = 10000000000)]
    pub big: MxLong,
    #[mxrs(default = -5)]
    pub offset: MxInteger,
}

#[entity(module = "Edge")]
#[mxrs(generalizes = mxrs::system::User)]
pub struct Account {}

#[entity(module = "Edge")]
#[mxrs(generalizes = Ledger)]
pub struct SubLedger {}

#[entity(module = "Edge")]
#[mxrs(preserve(inheritance))]
pub struct Kept {}

#[dto(module = "Edge")]
pub struct Plain {}

#[view(module = "Edge", source = "Edge.Totals")]
pub struct Totals {
    pub name: MxString,
}

#[microflow(ACT, module = "Edge")]
pub fn project(_flow: &mut FlowBuilder) {}

#[microflow(SUB, module = "Edge")]
pub fn module(_flow: &mut FlowBuilder) {}

#[microflow(module = "Edge", name = "union")]
pub fn union_flow(_flow: &mut FlowBuilder) {}

#[microflow(module = "Edge", name = "gen")]
pub fn gen_flow(_flow: &mut FlowBuilder) {}

#[page(module = "Edge")]
pub fn item(_page: &mut PageBuilder) {}

#[declaration(module = "Edge")]
pub fn builder(module: &mut ModuleBuilder) {
    module.regular_expression("Digits", "^[0-9]+$", |_| {});
}

#[demo_user]
pub fn user(user: &mut DemoUserBuilder) {
    user.role("Administrator");
}

#[navigation_item(profile = "Responsive", caption = "Item")]
pub fn security(item: &mut NavigationItemBuilder) {
    item.page("Edge.Item");
}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

fn edge() -> mxrs::ModuleDecl {
    Application::build()
        .modules
        .into_iter()
        .find(|module| module.name == "Edge")
        .expect("the Edge module is declared")
}

fn entity(name: &str) -> mxrs::EntityDecl {
    edge()
        .entities
        .into_iter()
        .find(|entity| entity.name == name)
        .unwrap_or_else(|| panic!("entity {name} is declared"))
}

#[test]
fn a_function_named_like_a_macro_local_still_declares() {
    let edge = edge();
    let flows = edge
        .microflows
        .iter()
        .map(|flow| flow.name.as_str())
        .collect::<Vec<_>>();
    assert!(flows.contains(&"ACT_Project"), "{flows:?}");
    assert!(flows.contains(&"SUB_Module"), "{flows:?}");
    assert_eq!(edge.pages[0].name, "Item");
    assert_eq!(edge.regular_expressions[0].name, "Digits");

    let project = Application::build();
    // No `#[security]` here: the demo user waits for the stored security.
    assert!(project.security.is_none());
    assert_eq!(project.demo_users[0].name, "user");
    assert_eq!(project.demo_users[0].roles, ["Administrator"]);
    let navigation = project.navigation.as_ref().unwrap();
    assert_eq!(
        navigation.profiles[0].items[0].page.as_deref(),
        Some("Edge.Item")
    );
}

#[test]
fn a_flow_named_with_a_rust_keyword_is_named_by_a_type_that_is_not_one() {
    assert_eq!(<union_ as MicroflowMarker>::qualified_name(), "Edge.union");
    assert_eq!(<gen_ as MicroflowMarker>::qualified_name(), "Edge.gen");
}

#[test]
fn fields_named_like_index_words_and_raw_identifiers_are_fields() {
    let ledger = entity("Ledger");
    assert_eq!(
        ledger
            .attributes
            .iter()
            .map(|attribute| attribute.name.as_str())
            .collect::<Vec<_>>(),
        ["Desc", "System", "Type", "Big", "Offset"]
    );
    assert_eq!(Ledger::r#type().name(), "Type");
    assert_eq!(Ledger::desc().qualified_name(), "Edge.Ledger.Desc");
    let index = &ledger.indexes.as_ref().unwrap()[0];
    assert_eq!(
        index.members,
        [
            mxrs::IndexMemberDecl::Attribute {
                name: "Desc".to_string(),
                ascending: true,
            },
            mxrs::IndexMemberDecl::Attribute {
                name: "System".to_string(),
                ascending: true,
            },
            mxrs::IndexMemberDecl::Attribute {
                name: "Type".to_string(),
                ascending: false,
            },
        ]
    );
}

#[test]
fn a_whole_number_default_is_the_digits_as_written() {
    let ledger = entity("Ledger");
    let default = |name: &str| {
        ledger
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .and_then(|attribute| attribute.default_value.clone())
    };
    // Beyond `i32`, which evaluating the literal would have required.
    assert_eq!(default("Big").as_deref(), Some("10000000000"));
    assert_eq!(default("Offset").as_deref(), Some("-5"));
}

#[test]
fn an_entity_states_its_place_in_the_hierarchy() {
    use mxrs::EntityInheritanceDecl::{Generalizes, Root};

    let Some(Root(members)) = entity("Ledger").inheritance else {
        panic!("Ledger is a root");
    };
    assert!(members.owner && members.changed_date);
    assert!(!members.created_date && !members.changed_by);

    assert_eq!(
        entity("Account").inheritance,
        Some(Generalizes("System.User".to_string()))
    );
    assert_eq!(
        entity("SubLedger").inheritance,
        Some(Generalizes("Edge.Ledger".to_string()))
    );
    // Stating nothing is stating a plain root — for a DTO as well.
    assert_eq!(
        entity("Plain").inheritance,
        Some(Root(mxrs::SystemMembersDecl::default()))
    );
    // Only an explicit `preserve` — or a view, which has no hierarchy to
    // state — leaves the imported model's answer alone.
    assert_eq!(entity("Kept").inheritance, None);
    assert_eq!(entity("Totals").inheritance, None);
}
