//! Exercises `synchronize_project` — the "write changes back into an
//! *existing* project" half of the round trip `mxrs-exporter` opens the
//! other half of (read a project out as editable Rust source). Builds a
//! project via `write_project`, then calls `synchronize_project` with a
//! second, edited `ProjectDecl` against the same file, and reads back via
//! `mxrs-model` to confirm the update landed and existing identity survived
//! where it should (mirrors `mxrs-writer::domain`'s own resync tests, one
//! level up — a full project rather than one domain model in isolation).

use mxrs_dsl::ProjectBuilder;
use mxrs_model::Project;

#[test]
fn synchronize_project_adds_a_new_module_to_an_existing_project() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
        });
    });
    updated.module("CRM", |m| {
        m.entity("Account", |e| {
            e.string("Name");
        });
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let project = Project::open(&path, true).unwrap();
    let mut names: Vec<String> = project
        .modules()
        .unwrap()
        .into_iter()
        .filter_map(|m| m.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["CRM".to_string(), "Sales".to_string()]);
}

#[test]
fn synchronize_project_adds_an_attribute_and_preserves_the_entitys_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let original_id = {
        let project = Project::open(&path, true).unwrap();
        let sales = project
            .modules()
            .unwrap()
            .into_iter()
            .find(|m| m.name.as_deref() == Some("Sales"))
            .unwrap();
        sales.entities()[0].id.clone()
    };

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
            e.decimal("Total");
        });
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let project = Project::open(&path, true).unwrap();
    let sales = project
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let order = &sales.entities()[0];
    assert_eq!(
        order.id, original_id,
        "the entity's $ID must survive the resync"
    );
    let attr_names: Vec<&str> = order
        .attributes
        .iter()
        .filter_map(|a| a.name.as_deref())
        .collect();
    assert!(attr_names.contains(&"Number"));
    assert!(attr_names.contains(&"Total"));
}

#[test]
fn synchronize_project_upserts_a_microflow_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |m| {
        m.microflow("ACT_GetConstant", |f| {
            f.return_value("1");
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |m| {
        m.microflow("ACT_GetConstant", |f| {
            f.return_value("2");
        });
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let project = Project::open(&path, true).unwrap();
    let sales = project
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert_eq!(sales.microflows.len(), 1, "upsert, not a duplicate insert");
    assert_eq!(sales.microflows[0].name.as_deref(), Some("ACT_GetConstant"));
}
