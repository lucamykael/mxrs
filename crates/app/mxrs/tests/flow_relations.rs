//! A flow's declaration says what the flow is beside what it does: who may
//! run it, what it calls, which entities it works with and what uses it —
//! each named by the Rust item that declares it, so the compiler checks the
//! names and an editor follows them.

use mxrs::prelude::*;

#[module_roles(module = "Sales")]
pub enum Role {
    Administrator,
    Clerk,
}

#[entity(module = "Sales")]
pub struct Order {
    pub number: MxString,
}

/// Nobody may run it directly, and nothing but the flow below calls it.
#[microflow(SUB, module = "Sales", roles(), uses(Order), used_by(ACT_ShipOrder))]
pub fn validate_order(flow: &mut FlowBuilder) {
    flow.parameter_of("Order", DataType::object::<Order>(), |_| {});
}

#[microflow(
    ACT,
    module = "Sales",
    roles(Role::Administrator, Role::Clerk),
    calls(SUB_ValidateOrder),
    uses(Order),
    used_by(ACT_Ship)
)]
pub fn ship_order(flow: &mut FlowBuilder) {
    let order = flow.parameter_of("Order", DataType::object::<Order>(), |_| {});
    flow.call(MicroflowRef::<SUB_ValidateOrder>::new(), |call| {
        call.argument("Order", mx("$Order"));
    });
    flow.commit_with(&order, |_| {});
}

/// What no Rust item declares is named as the model names it.
#[nanoflow(
    module = "Sales",
    name = "ACT_Ship",
    roles("Sales.Clerk"),
    calls(ACT_ShipOrder)
)]
pub fn ship(flow: &mut FlowBuilder) {
    flow.call(MicroflowRef::<ACT_ShipOrder>::new(), |call| {
        call.argument("Order", mx("empty"));
    });
}

/// States nothing about itself: nothing is checked, and the model keeps
/// whatever roles it has.
#[microflow(module = "Sales")]
pub fn unrelated(_flow: &mut FlowBuilder) {}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

fn sales(project: &mxrs::ProjectDecl) -> &mxrs::ModuleDecl {
    project
        .modules
        .iter()
        .find(|module| module.name == "Sales")
        .expect("the Sales module is declared")
}

fn flow<'a>(project: &'a mxrs::ProjectDecl, name: &str) -> &'a mxrs::MicroflowDecl {
    let module = sales(project);
    module
        .microflows
        .iter()
        .chain(&module.nanoflows)
        .find(|flow| flow.name == name)
        .unwrap_or_else(|| panic!("{name} is declared"))
}

fn declared_ship_order(project: &mut mxrs::ProjectDecl) -> &mut mxrs::MicroflowDecl {
    project
        .modules
        .iter_mut()
        .find(|module| module.name == "Sales")
        .unwrap()
        .microflows
        .iter_mut()
        .find(|flow| flow.name == "ACT_ShipOrder")
        .unwrap()
}

fn names(items: &[&str]) -> Option<Vec<String>> {
    Some(items.iter().map(|item| item.to_string()).collect())
}

#[test]
fn a_declaration_names_what_the_flow_is_related_to() {
    let project = Application::build();
    let ship = flow(&project, "ACT_ShipOrder");
    assert_eq!(
        ship.allowed_roles,
        names(&["Sales.Administrator", "Sales.Clerk"])
    );
    assert_eq!(ship.relations.calls, names(&["Sales.SUB_ValidateOrder"]));
    assert_eq!(ship.relations.uses, names(&["Sales.Order"]));
    assert_eq!(ship.relations.used_by, names(&["Sales.ACT_Ship"]));

    // Stating no role is a statement; stating nothing is not.
    let validate = flow(&project, "SUB_ValidateOrder");
    assert_eq!(validate.allowed_roles, Some(Vec::new()));
    assert_eq!(validate.relations.calls, None);
    let unrelated = flow(&project, "Unrelated");
    assert_eq!(unrelated.allowed_roles, None);
    assert!(!unrelated.relations.is_stated());

    // A nanoflow is named the same way, and a role by its qualified name.
    let nanoflow = flow(&project, "ACT_Ship");
    assert_eq!(nanoflow.allowed_roles, names(&["Sales.Clerk"]));
    assert_eq!(nanoflow.relations.calls, names(&["Sales.ACT_ShipOrder"]));
}

#[test]
fn the_roles_are_written_and_the_relations_are_held_against_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Sales.mpr");
    let mut project = Application::build();
    mxrs::write_project(&path, &project).unwrap();

    let model = mxrs_model::Project::open(&path, true).unwrap();
    let relations = mxrs_model::relations::flow_relations(&model).unwrap();
    let ship = &relations["Sales.ACT_ShipOrder"];
    let set = |items: &[&str]| -> std::collections::BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    };
    assert_eq!(ship.roles, set(&["Sales.Administrator", "Sales.Clerk"]));
    assert_eq!(ship.calls, set(&["Sales.SUB_ValidateOrder"]));
    assert_eq!(ship.uses, set(&["Sales.Order"]));
    assert_eq!(ship.used_by, set(&["Sales.ACT_Ship"]));
    assert!(relations["Sales.SUB_ValidateOrder"].roles.is_empty());
    drop(model);

    // What the declarations say is what the model holds: nothing to report.
    assert_eq!(
        mxrs::flow_relation_warnings(&path, &project).unwrap(),
        Vec::<String>::new()
    );

    // A declaration that falls behind its flow is told so, relation by
    // relation, without the build failing for it.
    let ship = declared_ship_order(&mut project);
    ship.relations.calls = Some(vec!["Sales.SUB_Gone".into()]);
    ship.relations.used_by = Some(Vec::new());
    assert_eq!(
        mxrs::flow_relation_warnings(&path, &project).unwrap(),
        [
            "Sales.ACT_ShipOrder: calls(...) leaves out Sales.SUB_ValidateOrder, which it calls",
            "Sales.ACT_ShipOrder: calls(...) lists Sales.SUB_Gone, which it does not call",
            "Sales.ACT_ShipOrder: used_by(...) leaves out Sales.ACT_Ship, which uses it",
        ]
    );
}

#[test]
fn roles_a_declaration_does_not_state_stay_the_models() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Sales.mpr");
    let mut project = Application::build();
    mxrs::write_project(&path, &project).unwrap();
    let roles = |name: &str| {
        let model = mxrs_model::Project::open(&path, true).unwrap();
        mxrs_model::relations::flow_relations(&model).unwrap()[name]
            .roles
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        roles("Sales.ACT_ShipOrder"),
        ["Sales.Administrator", "Sales.Clerk"]
    );

    // Not saying leaves them; saying otherwise replaces them; saying none
    // removes them.
    declared_ship_order(&mut project).allowed_roles = None;
    mxrs::synchronize_project(&path, &project).unwrap();
    assert_eq!(
        roles("Sales.ACT_ShipOrder"),
        ["Sales.Administrator", "Sales.Clerk"]
    );
    declared_ship_order(&mut project).allowed_roles = Some(vec!["Sales.Clerk".into()]);
    mxrs::synchronize_project(&path, &project).unwrap();
    assert_eq!(roles("Sales.ACT_ShipOrder"), ["Sales.Clerk"]);
    declared_ship_order(&mut project).allowed_roles = Some(Vec::new());
    mxrs::synchronize_project(&path, &project).unwrap();
    assert!(roles("Sales.ACT_ShipOrder").is_empty());
}
