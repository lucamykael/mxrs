//! Exercises `synchronize_project` — the "write changes back into an
//! *existing* project" half of the round trip `mxrs-exporter` opens the
//! other half of (read a project out as editable Rust source). Builds a
//! project via `write_project`, then calls `synchronize_project` with a
//! second, edited `ProjectDecl` against the same file, and reads back via
//! `mxrs-model` to confirm the update landed and existing identity survived
//! where it should (mirrors `mxrs-writer::domain`'s own resync tests, one
//! level up — a full project rather than one domain model in isolation).

use mxrs_dsl::ProjectBuilder;
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_model::Project;

fn enumeration_document(path: &std::path::Path, name: &str) -> mxrs_bson::Document {
    let mpr = mxrs_mpr::MprFile::open(path, true).unwrap();
    mpr.all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            (document.get_str("$Type").ok() == Some("Enumerations$Enumeration")
                && document.get_str("Name").ok() == Some(name))
            .then_some(document)
        })
        .unwrap()
}

#[test]
fn fresh_projects_with_the_same_logical_name_reuse_all_unit_identities() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("one/Project.mpr");
    let second = dir.path().join("two/Project.mpr");

    let mut declaration = ProjectBuilder::new("11.12.1");
    declaration.module("Sales", |module| {
        module.entity("Order", |entity| {
            entity.string("Number");
        });
        module.microflow("ACT_Order", |flow| {
            flow.return_value(mxrs_dsl::string(""));
        });
    });
    let declaration = declaration.build();
    mxrs_writer::write_project(&first, &declaration).unwrap();
    mxrs_writer::write_project(&second, &declaration).unwrap();

    let unit_ids = |path: &std::path::Path| {
        let mpr = mxrs_mpr::MprFile::open(path, true).unwrap();
        let mut ids: Vec<String> = mpr
            .all_units()
            .unwrap()
            .into_iter()
            .map(|unit| unit.unit_id)
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(unit_ids(&first), unit_ids(&second));
}

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

    let root_id = mxrs_mpr::MprFile::open(&path, true)
        .unwrap()
        .root_unit()
        .unwrap()
        .unwrap()
        .unit_id;
    let identity = ProjectIdentity::from_project_root(&root_id).unwrap();
    let total = order
        .attributes
        .iter()
        .find(|attribute| attribute.name.as_deref() == Some("Total"))
        .unwrap();
    let expected_total_id = identity.artifact_id(ArtifactKind::Attribute, "Sales.Order.Total");
    assert_eq!(total.id.as_deref(), Some(expected_total_id.as_str()));
}

#[test]
fn synchronize_project_upserts_a_microflow_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |m| {
        m.microflow("ACT_GetConstant", |f| {
            f.return_value(mxrs_dsl::integer(1));
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |m| {
        m.microflow("ACT_GetConstant", |f| {
            f.return_value(mxrs_dsl::integer(2));
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

#[test]
fn synchronize_project_upserts_a_nanoflow_by_name_and_preserves_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |m| {
        m.nanoflow("NF_GetConstant", |f| {
            f.return_value(mxrs_dsl::integer(1));
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();
    let original_id = Project::open(&path, true).unwrap().modules().unwrap()[0].nanoflows[0]
        .id
        .clone();

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |m| {
        m.nanoflow("NF_GetConstant", |f| {
            f.return_value(mxrs_dsl::integer(2));
        });
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let project = Project::open(&path, true).unwrap();
    let sales = &project.modules().unwrap()[0];
    assert_eq!(sales.nanoflows.len(), 1, "upsert, not a duplicate insert");
    assert_eq!(sales.nanoflows[0].id, original_id);
}

#[test]
fn synchronize_project_preserves_enumeration_and_value_identities() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |module| {
        module.enumeration("Status", |enumeration| {
            enumeration.documentation("Initial");
            enumeration.value("Open");
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();
    let initial = enumeration_document(&path, "Status");
    let enumeration_id = mxrs_bson::extract_id(initial.get("$ID").unwrap()).unwrap();
    let initial_values =
        mxrs_bson::parse_array(initial.get_array("Values").ok().map(Vec::as_slice));
    let open_id = mxrs_bson::extract_id(
        initial_values.items[0]
            .as_document()
            .unwrap()
            .get("$ID")
            .unwrap(),
    )
    .unwrap();

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |module| {
        module.enumeration("Status", |enumeration| {
            enumeration.documentation("Updated");
            enumeration.value("Open").captions =
                vec![("en_US".to_string(), "Open order".to_string())];
            enumeration.value("Closed");
        });
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let updated = enumeration_document(&path, "Status");
    assert_eq!(updated.get_str("Documentation").unwrap(), "Updated");
    assert_eq!(
        mxrs_bson::extract_id(updated.get("$ID").unwrap()).as_deref(),
        Some(enumeration_id.as_str())
    );
    let values = mxrs_bson::parse_array(updated.get_array("Values").ok().map(Vec::as_slice));
    assert_eq!(values.items.len(), 2);
    let open = values.items[0].as_document().unwrap();
    assert_eq!(
        mxrs_bson::extract_id(open.get("$ID").unwrap()).as_deref(),
        Some(open_id.as_str())
    );
}

#[test]
fn validation_rules_are_authoritative_and_keep_stable_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let declaration = |required: bool, unique: bool| {
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.entity("Order", |entity| {
                let number = entity.string("Number");
                number.required = required;
                number.unique = unique;
            });
        });
        project.build()
    };

    mxrs_writer::write_project(&path, &declaration(true, true)).unwrap();
    let first = Project::open(&path, true).unwrap();
    let first_modules = first.modules().unwrap();
    let order = &first_modules[0].entities()[0];
    assert!(order.attributes[0].required);
    assert!(order.attributes[0].unique);
    let first_rule_ids: Vec<String> = order
        .validation_rules
        .iter()
        .filter_map(|rule| rule.get_str("$ID").ok().map(str::to_string))
        .collect();
    drop(first);

    mxrs_writer::synchronize_project(&path, &declaration(true, true)).unwrap();
    let second = Project::open(&path, true).unwrap();
    let second_modules = second.modules().unwrap();
    let order = &second_modules[0].entities()[0];
    let second_rule_ids: Vec<String> = order
        .validation_rules
        .iter()
        .filter_map(|rule| rule.get_str("$ID").ok().map(str::to_string))
        .collect();
    assert_eq!(second_rule_ids, first_rule_ids);
    drop(second);

    mxrs_writer::synchronize_project(&path, &declaration(false, false)).unwrap();
    let third = Project::open(&path, true).unwrap();
    let third_modules = third.modules().unwrap();
    let order = &third_modules[0].entities()[0];
    assert!(!order.attributes[0].required);
    assert!(!order.attributes[0].unique);
    assert!(order.validation_rules.is_empty());
}

/// `synchronize_project` is the path `mxrs-project::rebuild_imported_project`
/// uses to overlay a Rust-authored `ProjectDecl` on top of a restored
/// imported snapshot — this is the same upsert-by-name identity-preserving
/// behavior `synchronize_project_upserts_a_microflow_by_name` proves for
/// microflows, now proven for pages too.
#[test]
fn synchronize_project_upserts_a_page_by_name_and_preserves_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |m| {
        m.page("Home", |p| {
            p.layout("Atlas_Core.ApplicationLayout", "Main");
            p.text("v1");
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let first = Project::open(&path, true).unwrap();
    let original_id = first
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap()
        .pages[0]
        .id
        .clone()
        .unwrap();
    drop(first);

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |m| {
        m.page("Home", |p| {
            p.layout("Atlas_Core.ApplicationLayout", "Main");
            p.text("v2");
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
    assert_eq!(sales.pages.len(), 1, "upsert, not a duplicate insert");
    assert_eq!(sales.pages[0].id.as_deref(), Some(original_id.as_str()));
    assert_eq!(
        sales.pages[0].widgets[0]
            .options
            .get_str("caption")
            .unwrap(),
        "v2"
    );
}
