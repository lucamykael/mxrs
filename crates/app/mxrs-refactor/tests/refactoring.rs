//! The claim that matters for a refactoring: after applying it, the model
//! reads back the way the preview said it would, and nothing that referred to
//! the old name still does. Unit tests over string substitution cannot
//! establish that — these build a real `.mpr`, refactor it, and reopen it.

use mxrs_dsl::ProjectBuilder;
use mxrs_model::Project;
use mxrs_refactor::{RefactorError, plan_move, plan_remove, plan_rename};

#[allow(dead_code, non_snake_case, non_camel_case_types)]
mod markers {
    pub struct Order;
    impl mxrs_ir::EntityMarker for Order {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "Order";
    }
    pub struct Order_Number;
    impl mxrs_ir::AttributeMarker for Order_Number {
        type Entity = Order;
        const NAME: &'static str = "Number";
    }
    impl mxrs_expr::TypedAttributeMarker for Order_Number {
        type Value = mxrs_expr::MxString;
    }
    pub struct ACT_SubmitOrder;
    impl mxrs_ir::MicroflowMarker for ACT_SubmitOrder {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "ACT_SubmitOrder";
    }
}

/// A project whose page references a microflow, so a rename has a real
/// cross-document reference to follow rather than only the declaration.
fn project(path: &std::path::Path) {
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.role("User", "Regular user");
        module.entity("Order", |entity| {
            entity.string("Number");
            entity.decimal("Total");
        });
        module.microflow("ACT_SubmitOrder", |_flow| {});
        module.microflow("ACT_Unreferenced", |_flow| {});
        module.layout("ApplicationLayout", |layout| {
            layout.placeholder("Main");
        });
        module.page("OrderOverview", |page| {
            page.layout("Sales.ApplicationLayout", "Main");
            page.button("Submit", |button| {
                button.call_microflow(mxrs_ir::MicroflowRef::<markers::ACT_SubmitOrder>::new());
            });
        });
    });
    mxrs_writer::write_project(path, &project.build()).unwrap();
}

fn open(path: &std::path::Path) -> Project {
    Project::open(path, false).unwrap()
}

fn microflow_names(project: &Project) -> Vec<String> {
    project.modules().unwrap()[0]
        .microflows
        .iter()
        .filter_map(|flow| flow.name.clone())
        .collect()
}

/// Every string anywhere in the file, for asserting that a name is really
/// gone rather than merely gone from the places we thought to check.
fn all_strings(project: &Project) -> Vec<String> {
    let mut found = Vec::new();
    fn walk(value: &mxrs_bson::Bson, out: &mut Vec<String>) {
        match value {
            mxrs_bson::Bson::Document(document) => {
                for (_, child) in document {
                    walk(child, out);
                }
            }
            mxrs_bson::Bson::Array(items) => {
                for child in items {
                    walk(child, out);
                }
            }
            mxrs_bson::Bson::String(text) => out.push(text.clone()),
            _ => {}
        }
    }
    for unit in project.all_units().unwrap() {
        let document = project.mpr().parse_contents(&unit).unwrap();
        walk(&mxrs_bson::Bson::Document(document), &mut found);
    }
    found
}

#[test]
fn renaming_a_microflow_rewrites_its_declaration_and_every_reference_to_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Sales.mpr");
    project(&path);

    let plan = {
        let project = open(&path);
        plan_rename(&project, "Sales.ACT_SubmitOrder", "ACT_PlaceOrder").unwrap()
    };
    assert_eq!(plan.target, "Sales.ACT_PlaceOrder");
    assert_eq!(plan.source.kind, "microflow");
    assert!(!plan.is_empty());
    // The preview must name both halves: the microflow's own declaration and
    // the page button that calls it.
    assert!(
        plan.changes
            .iter()
            .any(|change| change.before == "ACT_SubmitOrder" && change.after == "ACT_PlaceOrder"),
        "declaration not previewed: {:?}",
        plan.changes
    );
    assert!(
        plan.changes
            .iter()
            .any(|change| change.before == "Sales.ACT_SubmitOrder"
                && change.after == "Sales.ACT_PlaceOrder"),
        "reference not previewed: {:?}",
        plan.changes
    );
    assert!(plan.affected_units() >= 2, "{}", plan.affected_units());

    // Planning alone changes nothing on disk.
    assert!(microflow_names(&open(&path)).contains(&"ACT_SubmitOrder".to_string()));

    let mut project = open(&path);
    let plan = plan_rename(&project, "Sales.ACT_SubmitOrder", "ACT_PlaceOrder").unwrap();
    plan.apply(&mut project).unwrap();
    drop(project);

    let project = open(&path);
    let names = microflow_names(&project);
    assert!(names.contains(&"ACT_PlaceOrder".to_string()), "{names:?}");
    assert!(!names.contains(&"ACT_SubmitOrder".to_string()), "{names:?}");
    let strings = all_strings(&project);
    assert!(
        !strings.iter().any(|text| text.contains("ACT_SubmitOrder")),
        "old name survived somewhere in the file"
    );
    assert!(strings.iter().any(|text| text == "Sales.ACT_PlaceOrder"));
}

#[test]
fn a_rename_leaves_documentation_and_unrelated_prose_alone() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Prose.mpr");
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.entity("Order", |entity| {
            entity.documentation("An Order is created by Sales.ACT_SubmitOrder.");
            entity.string("Number");
        });
        module.microflow("ACT_SubmitOrder", |_flow| {});
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();

    let mut project = open(&path);
    let plan = plan_rename(&project, "Sales.ACT_SubmitOrder", "ACT_PlaceOrder").unwrap();
    plan.apply(&mut project).unwrap();
    drop(project);

    let project = open(&path);
    // The documentation still says what a human wrote; a rename is not a
    // licence to edit prose.
    assert_eq!(
        project.modules().unwrap()[0].entities()[0].documentation,
        "An Order is created by Sales.ACT_SubmitOrder."
    );
    assert!(microflow_names(&project).contains(&"ACT_PlaceOrder".to_string()));
}

#[test]
fn renaming_refuses_unknown_names_collisions_and_relocation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Refusals.mpr");
    project(&path);
    let project = open(&path);

    assert!(matches!(
        plan_rename(&project, "Sales.NoSuchFlow", "Whatever"),
        Err(RefactorError::Semantic(_))
    ));
    // Renaming onto a name that already exists would merge two artifacts.
    assert!(matches!(
        plan_rename(&project, "Sales.ACT_SubmitOrder", "ACT_Unreferenced"),
        Err(RefactorError::NameCollision(name)) if name == "Sales.ACT_Unreferenced"
    ));
    assert!(matches!(
        plan_rename(&project, "Sales.ACT_SubmitOrder", "Other.ACT_SubmitOrder"),
        Err(RefactorError::CrossContainerRename { .. })
    ));
}

#[test]
fn removal_is_blocked_while_anything_still_references_the_artifact() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Blocked.mpr");
    project(&path);

    let mut project = open(&path);
    let plan = plan_remove(&project, "Sales.ACT_SubmitOrder").unwrap();
    assert!(!plan.is_safe());
    assert!(!plan.incoming.is_empty());
    assert!(
        plan.incoming
            .iter()
            .any(|reference| reference.from.contains("OrderOverview")),
        "{:?}",
        plan.incoming
    );

    let name = plan.artifact.qualified_name.clone();
    let error = plan.apply(&mut project).unwrap_err();
    assert!(matches!(
        error,
        RefactorError::RemovalBlocked { name: blocked, .. } if blocked == name
    ));
    // The blocked apply must not have deleted anything.
    drop(project);
    assert!(microflow_names(&open(&path)).contains(&"ACT_SubmitOrder".to_string()));
}

#[test]
fn an_unreferenced_artifact_can_be_removed_and_stays_removed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Removable.mpr");
    project(&path);

    let mut project = open(&path);
    let plan = plan_remove(&project, "Sales.ACT_Unreferenced").unwrap();
    assert!(plan.is_safe());
    assert!(plan.children.is_empty());
    plan.apply(&mut project).unwrap();
    drop(project);

    let project = open(&path);
    let names = microflow_names(&project);
    assert!(
        !names.contains(&"ACT_Unreferenced".to_string()),
        "{names:?}"
    );
    // Its neighbour is untouched.
    assert!(names.contains(&"ACT_SubmitOrder".to_string()), "{names:?}");
}

#[test]
fn removing_something_that_is_not_its_own_unit_is_refused_rather_than_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("NotAUnit.mpr");
    project(&path);
    let project = open(&path);

    // An attribute lives inside its entity's domain model, not in a unit of
    // its own; deleting "its unit" would either do nothing or delete the
    // entity. The index spells a member reference with a slash.
    assert!(matches!(
        plan_remove(&project, "Sales.Order/Total"),
        Err(RefactorError::NotAUnit { ref name, kind })
            if name == "Sales.Order/Total" && kind == "attribute"
    ));
    // Entities and modules are refused for the same reason, and named as
    // such rather than silently deleting the wrong unit.
    for (name, kind) in [("Sales.Order", "entity"), ("Sales", "module")] {
        assert!(
            matches!(
                plan_remove(&project, name),
                Err(RefactorError::NotAUnit { kind: actual, .. }) if actual == kind
            ),
            "{name} was not refused"
        );
    }
}

#[test]
fn moving_reports_the_containers_and_refuses_to_change_a_qualified_name() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Move.mpr");
    project(&path);
    let project = open(&path);

    // Moving into the module it already belongs to is a no-op, not an error.
    let plan = plan_move(&project, "Sales.ACT_SubmitOrder", "Sales").unwrap();
    assert!(plan.is_empty());
    assert_eq!(plan.before_container, plan.after_container);
    assert_eq!(plan.containment, "Documents");

    // A destination in another module would change the artifact's qualified
    // name, which is `rename`'s job.
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.microflow("ACT_SubmitOrder", |_flow| {});
    });
    builder.module("Billing", |module| {
        module.microflow("ACT_Invoice", |_flow| {});
    });
    let cross_path = directory.path().join("Cross.mpr");
    mxrs_writer::write_project(&cross_path, &builder.build()).unwrap();
    let cross = open(&cross_path);
    assert!(matches!(
        plan_move(&cross, "Sales.ACT_SubmitOrder", "Billing"),
        Err(RefactorError::CrossContainerRename { .. })
    ));
    assert!(matches!(
        plan_move(&cross, "Sales.ACT_SubmitOrder", "NoSuchModule"),
        Err(RefactorError::UnknownContainer(name)) if name == "NoSuchModule"
    ));
    // A page is a unit and a legitimate move subject; the microflow above
    // already proved the no-op path, so this only checks the kind gate lets
    // it through rather than refusing it as "not a unit".
    assert!(plan_move(&project, "Sales.OrderOverview", "Sales").is_ok());
}
