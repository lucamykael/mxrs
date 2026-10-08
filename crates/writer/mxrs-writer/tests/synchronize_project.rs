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

fn document_by_type(path: &std::path::Path, native_type: &str) -> mxrs_bson::Document {
    let mpr = mxrs_mpr::MprFile::open(path, true).unwrap();
    mpr.all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            (document.get_str("$Type").ok() == Some(native_type)).then_some(document)
        })
        .unwrap()
}

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
    // The table an entity's data lives in is named by the same identity:
    // a runtime keeping data by it finds the table after every build.
    let storage = |path: &std::path::Path| {
        let project = Project::open(path, true).unwrap();
        let modules = project.modules().unwrap();
        let sales = modules
            .iter()
            .find(|module| module.name.as_deref() == Some("Sales"))
            .unwrap();
        let order = &sales.entities()[0];
        (
            order.data_storage_guid.clone().unwrap(),
            order.attributes[0].data_storage_guid.clone().unwrap(),
        )
    };
    assert_eq!(storage(&first), storage(&second));
    assert_ne!(storage(&first).0, storage(&first).1);
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
    // Every module has the settings Studio Pro gives one, as 11.12 stores
    // them.
    let mpr = project.mpr();
    let settings = mpr.units_by_containment("ModuleSettings").unwrap();
    assert_eq!(settings.len(), 2);
    let document = mpr.parse_contents(&settings[0]).unwrap();
    assert_eq!(
        document.get_str("$Type").unwrap(),
        "Projects$ModuleSettings"
    );
    assert_eq!(document.get_str("Version").unwrap(), "1.0.0");
    assert_eq!(document.get_str("ProtectedModuleType").unwrap(), "AddOn");
    assert!(document.contains_key("EnableDetailedTroubleshooting"));
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
fn imported_external_associations_survive_when_the_typed_target_is_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ExternalAssociation.mpr");
    let declaration = || {
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
        });
        project.build()
    };
    mxrs_writer::write_project(&path, &declaration()).unwrap();

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let module = mpr
        .units_by_containment("Modules")
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .and_then(|document| document.get_str("Name").ok().map(str::to_string))
                .as_deref()
                == Some("Sales")
        })
        .unwrap();
    let domain_unit = mpr
        .children_of(&module.unit_id)
        .unwrap()
        .into_iter()
        .find(|unit| unit.containment_name == "DomainModel")
        .unwrap();
    let mut domain = mpr.parse_contents(&domain_unit).unwrap();
    let entities_key = if domain.contains_key("entities") {
        "entities"
    } else {
        "Entities"
    };
    let entities = mxrs_bson::parse_array(domain.get_array(entities_key).ok().map(Vec::as_slice));
    let order_id =
        mxrs_bson::extract_id(entities.items[0].as_document().unwrap().get("$ID").unwrap())
            .unwrap();
    let cross_key = if domain.contains_key("crossAssociations") {
        "crossAssociations"
    } else {
        "CrossAssociations"
    };
    let mut cross = mxrs_bson::parse_array(domain.get_array(cross_key).ok().map(Vec::as_slice));
    cross.items.push(mxrs_bson::Bson::Document(mxrs_bson::doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "DomainModels$CrossAssociation",
        "Name": "Order_UserRole",
        "Documentation": "external system target",
        "ParentID": order_id,
        "Child": "System.UserRole",
        "Type": "Reference",
        "Owner": "Default",
        "StorageFormat": "Table",
        "ExportLevel": "Hidden",
    }));
    domain.insert(cross_key, mxrs_bson::build_array(cross.items, cross.marker));
    mpr.update_unit(&domain_unit.unit_id, domain).unwrap();
    drop(mpr);

    let expected = {
        let document = document_by_type(&path, "DomainModels$DomainModel");
        mxrs_bson::parse_array(document.get_array(cross_key).ok().map(Vec::as_slice))
            .items
            .into_iter()
            .find_map(|value| value.as_document().cloned())
            .unwrap()
    };
    mxrs_writer::synchronize_project(&path, &declaration()).unwrap();
    let actual = {
        let document = document_by_type(&path, "DomainModels$DomainModel");
        mxrs_bson::parse_array(document.get_array(cross_key).ok().map(Vec::as_slice))
            .items
            .into_iter()
            .find_map(|value| value.as_document().cloned())
            .unwrap()
    };
    assert_eq!(actual, expected);
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
fn a_structural_edit_keeps_the_notes_drawn_on_the_flow() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |m| {
        m.microflow("ACT_Note", |f| {
            f.return_value(mxrs_dsl::integer(1));
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    // A note drawn on the flow in Studio Pro.
    {
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let (id, mut flow) = mpr
            .all_units()
            .unwrap()
            .into_iter()
            .find_map(|unit| {
                let document = mpr.parse_contents(&unit).ok()?;
                (document.get_str("Name").ok() == Some("ACT_Note"))
                    .then(|| (unit.unit_id.clone(), document))
            })
            .unwrap();
        flow.get_document_mut("ObjectCollection")
            .unwrap()
            .get_array_mut("Objects")
            .unwrap()
            .push(mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": "0b6f3f53-3a8c-4a1e-9a0e-1d2c3b4a5f60",
                "$Type": "Microflows$Annotation",
                "Caption": "Keep me",
                "RelativeMiddlePoint": "100;20",
                "Size": "200;50",
            }));
        mpr.update_unit(&id, flow).unwrap();
    }

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |m| {
        m.microflow("ACT_Note", |f| {
            f.return_value(mxrs_dsl::integer(1));
        });
    });
    let mut updated = updated.build();
    updated.modules[0].microflows[0].activities.insert(
        0,
        mxrs_ir::Activity::Decision {
            condition: "true".into(),
            true_branch: vec![],
            false_branch: vec![],
        },
    );
    mxrs_writer::synchronize_project(&path, &updated).unwrap();

    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    let flow = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            (document.get_str("Name").ok() == Some("ACT_Note")).then_some(document)
        })
        .unwrap();
    let objects = mxrs_writer::flow_graph::documents(
        flow.get_document("ObjectCollection")
            .unwrap()
            .get("Objects")
            .unwrap(),
    )
    .unwrap();
    assert!(
        objects
            .iter()
            .any(|object| object.get_str("$Type").ok() == Some("Microflows$ExclusiveSplit")),
        "the edit landed"
    );
    assert!(objects.iter().any(|object| {
        object.get_str("$Type").ok() == Some("Microflows$Annotation")
            && object.get_str("Caption").ok() == Some("Keep me")
    }));
}

#[test]
fn a_flow_allowed_to_a_role_its_module_does_not_have_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");
    let project = |role: &'static str| {
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |m| {
            m.role("User", "");
            m.microflow("ACT_Ship", move |f| {
                f.allowed_roles([role]);
                f.return_value(mxrs_dsl::integer(1));
            });
        });
        project.build()
    };
    let error = mxrs_writer::write_project(&path, &project("Sales.Missing"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Sales.Missing"), "{error}");
    mxrs_writer::write_project(&path, &project("Sales.User")).unwrap();
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
fn typed_security_and_module_roles_are_authoritative_and_keep_identities() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Secure.mpr");

    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |module| {
        module.role("User", "Initial");
    });
    initial.security(|security| {
        security
            .level(mxrs_dsl::SecurityLevel::CheckEverything)
            .check_security(false)
            .password_policy(|policy| policy.minimum_length = 12)
            .clear_roles()
            .role("Administrator", |role| {
                role.administrator(true).module_role("Sales.User");
            });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let before_security = document_by_type(&path, "Security$ProjectSecurity");
    let before_module = document_by_type(&path, "Security$ModuleSecurity");
    let before_security_id = mxrs_bson::extract_id(before_security.get("$ID").unwrap()).unwrap();
    let before_module_id = mxrs_bson::extract_id(before_module.get("$ID").unwrap()).unwrap();
    assert_eq!(
        before_security.get_str("SecurityLevel").unwrap(),
        "CheckEverything"
    );

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |module| {
        module.role("User", "Updated");
    });
    updated.security(|security| {
        security
            .check_security(false)
            .password_policy(|policy| policy.minimum_length = 12)
            .clear_roles()
            .role("Administrator", |role| {
                role.administrator(true).module_role("Sales.User");
            });
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let after_security = document_by_type(&path, "Security$ProjectSecurity");
    let after_module = document_by_type(&path, "Security$ModuleSecurity");
    assert_eq!(
        mxrs_bson::extract_id(after_security.get("$ID").unwrap()).as_deref(),
        Some(before_security_id.as_str())
    );
    assert_eq!(
        mxrs_bson::extract_id(after_module.get("$ID").unwrap()).as_deref(),
        Some(before_module_id.as_str())
    );
    assert!(!after_security.get_bool("CheckSecurity").unwrap());
    assert_eq!(
        after_security
            .get_document("PasswordPolicySettings")
            .unwrap()
            .get_i64("MinimumLength")
            .unwrap(),
        12
    );
    let Some(mxrs_bson::Bson::Array(roles)) = after_module.get("ModuleRoles") else {
        panic!("module roles array missing")
    };
    let role = mxrs_bson::parse_array(Some(roles)).items.remove(0);
    let mxrs_bson::Bson::Document(role) = role else {
        panic!("module role document missing")
    };
    assert_eq!(role.get_str("Description").unwrap(), "Updated");
}

#[test]
fn typed_security_rejects_an_unknown_module_role() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Secure.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |_module| {});
    project.security(|security| {
        security.clear_roles().role("Administrator", |role| {
            role.administrator(true).module_role("Sales.Missing");
        });
    });
    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(
        matches!(error, mxrs_writer::WriterError::UnknownModuleRole(name) if name == "Sales.Missing")
    );
}

#[test]
fn typed_navigation_round_trips_nested_items_and_preserves_document_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Navigation.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |module| {
        module.page("Home", |_page| {});
        module.microflow("ACT_Refresh", |_flow| {});
    });
    initial.security(|_security| {});
    initial.navigation(|navigation| {
        navigation.profile("Responsive", |profile| {
            profile
                .title("en_US", "Orders")
                .home_page("Sales.Home")
                .home_for_page("Administrator", "Sales.Home")
                .item("Orders", |item| {
                    item.icon_code(57369)
                        .page("Sales.Home")
                        .item("Refresh", |child| {
                            child.microflow("Sales.ACT_Refresh");
                        });
                });
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();
    let before = document_by_type(&path, "Navigation$NavigationDocument");
    let before_id = mxrs_bson::extract_id(before.get("$ID").unwrap()).unwrap();

    let navigation = Project::open(&path, true).unwrap().navigation().unwrap();
    assert_eq!(navigation.profiles.len(), 1);
    let profile = &navigation.profiles[0];
    assert_eq!(profile.home_page.as_deref(), Some("Sales.Home"));
    assert_eq!(
        profile.app_title.get("en_US").map(String::as_str),
        Some("Orders")
    );
    assert_eq!(profile.menu_items.len(), 1);
    assert_eq!(
        profile.menu_items[0].icon,
        Some(mxrs_model::navigation::NavigationIcon::Code(57369))
    );
    assert_eq!(profile.menu_items[0].items.len(), 1);

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.security(|_security| {});
    updated.navigation(|navigation| {
        navigation.profile("Responsive", |profile| {
            profile.title("en_US", "Orders 2").home_page("Sales.Home");
        });
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();
    let after = document_by_type(&path, "Navigation$NavigationDocument");
    assert_eq!(
        mxrs_bson::extract_id(after.get("$ID").unwrap()).as_deref(),
        Some(before_id.as_str())
    );
    let navigation = Project::open(&path, true).unwrap().navigation().unwrap();
    assert_eq!(
        navigation.profiles[0]
            .app_title
            .get("en_US")
            .map(String::as_str),
        Some("Orders 2")
    );
}

#[test]
fn typed_navigation_rejects_an_unknown_page_reference() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Navigation.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.navigation(|navigation| {
        navigation.profile("Responsive", |profile| {
            profile.home_page("Sales.Missing");
        });
    });
    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::UnknownNavigationTarget { kind, reference, .. }
            if kind == "page" && reference == "Sales.Missing"
    ));
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
fn synchronize_project_updates_documents_inside_folders_without_duplicating_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("NestedDocuments.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |module| {
        module.enumeration("Status", |enumeration| {
            enumeration.value("Open");
        });
        module.constant("Limit", |constant| {
            constant.value("10");
        });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let module = mpr
        .units_by_containment("Modules")
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .and_then(|document| document.get_str("Name").ok().map(str::to_string))
                .as_deref()
                == Some("Sales")
        })
        .unwrap();
    let folder_id = mpr
        .insert_unit(
            &module.unit_id,
            "Folders",
            mxrs_bson::doc! { "$Type": "Projects$Folder", "Name": "Configuration" },
            None,
        )
        .unwrap();
    let nested_ids = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .filter_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            matches!(
                document.get_str("$Type").ok(),
                Some("Enumerations$Enumeration" | "Constants$Constant")
            )
            .then_some(unit.unit_id)
        })
        .collect::<Vec<_>>();
    for id in &nested_ids {
        mpr.relocate_unit(id, &folder_id, "Documents").unwrap();
    }
    drop(mpr);

    // The declaration states the folder the documents are in.
    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |module| {
        module.folder("Configuration", |module| {
            module.enumeration("Status", |enumeration| {
                enumeration.value("Open");
                enumeration.value("Closed");
            });
            module.constant("Limit", |constant| {
                constant.value("20");
            });
        });
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    let documents = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .filter_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            matches!(
                document.get_str("$Type").ok(),
                Some("Enumerations$Enumeration" | "Constants$Constant")
            )
            .then_some((unit, document))
        })
        .collect::<Vec<_>>();
    assert_eq!(documents.len(), 2);
    assert!(
        documents
            .iter()
            .all(|(unit, _)| unit.container_id == folder_id)
    );
    assert!(
        nested_ids
            .iter()
            .all(|id| documents.iter().any(|(unit, _)| &unit.unit_id == id))
    );
    assert_eq!(
        documents
            .iter()
            .find(|(_, document)| document.get_str("Name").ok() == Some("Limit"))
            .unwrap()
            .1
            .get_str("DefaultValue")
            .unwrap(),
        "20"
    );
}

/// A document is in the folder its declaration states: the folders the
/// model lacks are made under stable identities, a document moves between
/// them, and one that states none is at its module's root.
#[test]
fn a_declared_folder_places_its_documents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Folders.mpr");
    let declare = |limit: &str, status: Option<&str>| {
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.folder(limit, |module| {
                module.constant("Limit", |constant| {
                    constant.value("10");
                });
            });
            match status {
                Some(folder) => {
                    module.folder(folder, |module| {
                        module.enumeration("Status", |enumeration| {
                            enumeration.value("Open");
                        });
                    });
                }
                None => {
                    module.enumeration("Status", |enumeration| {
                        enumeration.value("Open");
                    });
                }
            }
        });
        project.build()
    };
    let placed = |path: &std::path::Path| {
        let mpr = mxrs_mpr::MprFile::open(path, true).unwrap();
        let units = mpr.all_units().unwrap();
        let named = |id: &str| {
            units
                .iter()
                .find(|unit| unit.unit_id == id)
                .and_then(|unit| mpr.parse_contents(unit).ok())
                .map(|document| {
                    (
                        document.get_str("$Type").unwrap_or_default().to_string(),
                        document.get_str("Name").unwrap_or_default().to_string(),
                    )
                })
                .unwrap_or_default()
        };
        let mut found = std::collections::BTreeMap::new();
        for unit in &units {
            let (ty, name) = named(&unit.unit_id);
            if !matches!(
                ty.as_str(),
                "Constants$Constant" | "Enumerations$Enumeration"
            ) {
                continue;
            }
            // The folder path, from the document up to its module.
            let mut path = Vec::new();
            let mut container = unit.container_id.clone();
            loop {
                let (ty, folder) = named(&container);
                if ty != "Projects$Folder" {
                    break;
                }
                path.push(folder);
                container = units
                    .iter()
                    .find(|unit| unit.unit_id == container)
                    .unwrap()
                    .container_id
                    .clone();
            }
            path.reverse();
            found.insert(name, path.join("/"));
        }
        let folders = units
            .iter()
            .filter(|unit| named(&unit.unit_id).0 == "Projects$Folder")
            .count();
        (found, folders)
    };

    mxrs_writer::write_project(&path, &declare("Config/Limits", Some("Config"))).unwrap();
    let (found, folders) = placed(&path);
    assert_eq!(found["Limit"], "Config/Limits");
    assert_eq!(found["Status"], "Config");
    assert_eq!(folders, 2);

    mxrs_writer::synchronize_project(&path, &declare("Other", None)).unwrap();
    let (found, folders) = placed(&path);
    assert_eq!(found["Limit"], "Other");
    assert_eq!(found["Status"], "");
    // The folders no declaration names any more are kept.
    assert_eq!(folders, 3);

    // Stating it again finds the folder it made, by its identity.
    mxrs_writer::synchronize_project(&path, &declare("Config/Limits", Some("Config"))).unwrap();
    let (found, folders) = placed(&path);
    assert_eq!(found["Limit"], "Config/Limits");
    assert_eq!(folders, 3);
}

#[test]
fn imported_document_extensions_and_absent_defaults_survive_an_unchanged_sync_byte_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ExtendedDocuments.mpr");
    let declaration = || {
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.enumeration("Status", |enumeration| {
                enumeration.value("Open").captions =
                    vec![("en_US".to_string(), "Open".to_string())];
            });
            module.constant("Limit", |constant| {
                constant
                    .value_type(mxrs_ir::ConstantType::Integer)
                    .value("10");
            });
        });
        project.build()
    };
    mxrs_writer::write_project(&path, &declaration()).unwrap();

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let mut enumeration = None;
    let mut constant = None;
    for unit in mpr.all_units().unwrap() {
        let document = mpr.parse_contents(&unit).unwrap();
        match document.get_str("$Type").ok() {
            Some("Enumerations$Enumeration") => enumeration = Some((unit.unit_id, document)),
            Some("Constants$Constant") => constant = Some((unit.unit_id, document)),
            _ => {}
        }
    }
    let (enumeration_id, mut enumeration) = enumeration.unwrap();
    enumeration.insert("VendorEnumeration", "keep");
    let mut values =
        mxrs_bson::parse_array(enumeration.get_array("Values").ok().map(Vec::as_slice));
    let value = values.items[0].as_document_mut().unwrap();
    value.remove("ExportLevel");
    value.insert("VendorValue", "keep");
    let caption = value.get_document_mut("Caption").unwrap();
    caption.insert("VendorCaption", "keep");
    let mut translations =
        mxrs_bson::parse_array(caption.get_array("Items").ok().map(Vec::as_slice));
    translations.items[0]
        .as_document_mut()
        .unwrap()
        .insert("VendorTranslation", "keep");
    caption.insert(
        "Items",
        mxrs_bson::build_array(translations.items, translations.marker),
    );
    enumeration.insert(
        "Values",
        mxrs_bson::build_array(values.items, values.marker),
    );
    mpr.update_unit(&enumeration_id, enumeration).unwrap();

    let (constant_id, mut constant) = constant.unwrap();
    constant.insert("VendorConstant", "keep");
    constant
        .get_document_mut("Type")
        .unwrap()
        .insert("VendorType", "keep");
    mpr.update_unit(&constant_id, constant).unwrap();

    let before = [&enumeration_id, &constant_id]
        .into_iter()
        .map(|id| {
            let unit = mpr.unit(id).unwrap().unwrap();
            (id.clone(), mpr.content_bytes(&unit).unwrap().unwrap())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    drop(mpr);

    mxrs_writer::synchronize_project(&path, &declaration()).unwrap();
    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    for (id, expected) in before {
        let unit = mpr.unit(&id).unwrap().unwrap();
        assert_eq!(mpr.content_bytes(&unit).unwrap().unwrap(), expected);
    }
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

/// Plants a stored `Security$DemoUserImpl` (plus one opaque entry) directly
/// in the ProjectSecurity unit, the way an imported Studio Pro project would
/// carry them, so the merge semantics can be observed without a password in
/// the environment.
fn plant_stored_demo_user(path: &std::path::Path, name: &str, password: &str) {
    let mut mpr = mxrs_mpr::MprFile::open(path, false).unwrap();
    let root = mpr.root_unit().unwrap().unwrap().unit_id;
    let unit = mpr
        .children_of(&root)
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .and_then(|document| document.get_str("$Type").ok().map(str::to_string))
                .as_deref()
                == Some("Security$ProjectSecurity")
        })
        .unwrap();
    let mut document = mpr.parse_contents(&unit).unwrap();
    let stored = mxrs_bson::doc! {
        "$ID": "11111111-2222-3333-4444-555555555555",
        "$Type": "Security$DemoUserImpl",
        "UserName": name,
        "Password": password,
        "Entity": "System.User",
        "UserRoles": mxrs_bson::build_array(vec![mxrs_bson::Bson::String("User".into())], 1),
        "ImportedExtra": "kept",
    };
    let opaque = mxrs_bson::doc! { "$Type": "Security$FutureUser", "UserName": "Opaque" };
    document.insert(
        "DemoUsers",
        mxrs_bson::build_array(
            vec![
                mxrs_bson::Bson::Document(stored),
                mxrs_bson::Bson::Document(opaque),
            ],
            2,
        ),
    );
    mpr.transaction(|mpr| mpr.update_unit(&unit.unit_id, document))
        .unwrap();
}

fn secured_project(demo: impl FnOnce(&mut mxrs_dsl::DemoUserBuilder)) -> mxrs_ir::ProjectDecl {
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.role("User", "App user");
    });
    project.security(|security| {
        security
            .clear_roles()
            .role("Administrator", |role| {
                role.administrator(true).module_role("Sales.User");
            })
            .role("User", |role| {
                role.module_role("Sales.User");
            })
            .demo_user("Manager", demo);
    });
    project.build()
}

#[test]
fn a_declared_demo_user_preserves_the_stored_password_identity_and_opaque_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |module| {
        module.role("User", "App user");
    });
    initial.security(|security| {
        security
            .clear_roles()
            .role("Administrator", |role| {
                role.administrator(true).module_role("Sales.User");
            })
            .role("User", |role| {
                role.module_role("Sales.User");
            });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();
    plant_stored_demo_user(&path, "Manager", "kept-secret");

    let declaration = secured_project(|user| {
        user.role("User")
            .password_from_env("MXRS_TEST_DEMO_USER_UNSET_VARIABLE");
    });
    mxrs_writer::synchronize_project(&path, &declaration).unwrap();

    let security = document_by_type(&path, "Security$ProjectSecurity");
    let Some(mxrs_bson::Bson::Array(raw)) = security.get("DemoUsers") else {
        panic!("DemoUsers array missing")
    };
    let users = mxrs_bson::parse_array(Some(raw)).items;
    assert_eq!(users.len(), 2, "declared + opaque");
    let mxrs_bson::Bson::Document(manager) = &users[0] else {
        panic!("first entry is not a document")
    };
    assert_eq!(manager.get_str("UserName").unwrap(), "Manager");
    assert_eq!(
        manager.get_str("Password").unwrap(),
        "kept-secret",
        "an absent environment variable preserves the stored password"
    );
    assert_eq!(
        mxrs_bson::extract_id(manager.get("$ID").unwrap()).as_deref(),
        Some("11111111-2222-3333-4444-555555555555"),
        "matching by name keeps the stored identity"
    );
    assert_eq!(
        manager.get_str("ImportedExtra").unwrap(),
        "kept",
        "unmanaged fields survive the merge"
    );
    let mxrs_bson::Bson::Document(opaque) = &users[1] else {
        panic!("opaque entry missing")
    };
    assert_eq!(opaque.get_str("$Type").unwrap(), "Security$FutureUser");
}

#[test]
fn a_new_demo_user_without_a_resolvable_password_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    let declaration = secured_project(|user| {
        user.role("User")
            .password_from_env("MXRS_TEST_DEMO_USER_ALSO_UNSET");
    });
    let error = mxrs_writer::write_project(&path, &declaration).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::MissingDemoUserPassword { ref name, ref variable }
            if name == "Manager" && variable == "MXRS_TEST_DEMO_USER_ALSO_UNSET"
    ));
}

#[test]
fn a_demo_user_with_an_undeclared_role_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    let declaration = secured_project(|user| {
        user.role("Ghost");
    });
    let error = mxrs_writer::write_project(&path, &declaration).unwrap_err();
    assert!(matches!(error, mxrs_writer::WriterError::UnknownUserRole(role) if role == "Ghost"));
}

#[test]
fn an_empty_demo_user_declaration_list_leaves_stored_demo_users_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |module| {
        module.role("User", "App user");
    });
    initial.security(|security| {
        security
            .clear_roles()
            .role("Administrator", |role| {
                role.administrator(true).module_role("Sales.User");
            })
            .role("User", |role| {
                role.module_role("Sales.User");
            });
    });
    let declaration = initial.build();
    mxrs_writer::write_project(&path, &declaration).unwrap();
    plant_stored_demo_user(&path, "Manager", "kept-secret");
    let before = document_by_type(&path, "Security$ProjectSecurity");

    mxrs_writer::synchronize_project(&path, &declaration).unwrap();

    let after = document_by_type(&path, "Security$ProjectSecurity");
    assert_eq!(
        before.get("DemoUsers"),
        after.get("DemoUsers"),
        "no declaration, no rewrite"
    );
}

#[test]
fn a_stored_demo_user_no_declaration_covers_fails_closed_instead_of_vanishing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |module| {
        module.role("User", "App user");
    });
    initial.security(|security| {
        security
            .clear_roles()
            .role("Administrator", |role| {
                role.administrator(true).module_role("Sales.User");
            })
            .role("User", |role| {
                role.module_role("Sales.User");
            });
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();
    // An imported project's stored user the new declaration does not name.
    plant_stored_demo_user(&path, "Legacy", "imported-secret");

    let declaration = secured_project(|user| {
        user.role("User")
            .password_from_env("MXRS_TEST_DEMO_USER_STILL_UNSET");
    });
    let error = mxrs_writer::synchronize_project(&path, &declaration).unwrap_err();
    assert!(
        matches!(
            error,
            mxrs_writer::WriterError::UndeclaredStoredDemoUsers { ref names } if names == "Legacy"
        ),
        "{error}"
    );
    // Nothing was dropped: the stored user is still there.
    let security = document_by_type(&path, "Security$ProjectSecurity");
    let Some(mxrs_bson::Bson::Array(raw)) = security.get("DemoUsers") else {
        panic!("DemoUsers array missing")
    };
    let users = mxrs_bson::parse_array(Some(raw)).items;
    assert!(users.iter().any(|user| matches!(
        user,
        mxrs_bson::Bson::Document(doc) if doc.get_str("UserName").ok() == Some("Legacy")
    )));
}

/// A project that declares demo users and no security of its own: what an
/// imported project is until someone rewrites its security in Rust.
fn standalone_demo_user(
    name: &str,
    configure: impl FnOnce(&mut mxrs_dsl::DemoUserBuilder),
) -> mxrs_ir::ProjectDecl {
    let mut project = ProjectBuilder::new("11.12.1").build();
    let mut user = mxrs_dsl::DemoUserBuilder::new(name);
    configure(&mut user);
    project.demo_users.push(user.into_decl());
    project
}

fn stored_security(path: &std::path::Path) {
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Sales", |module| {
        module.role("User", "App user");
    });
    initial.security(|security| {
        security
            .clear_roles()
            .role("Administrator", |role| {
                role.administrator(true).module_role("Sales.User");
            })
            .role("User", |role| {
                role.module_role("Sales.User");
            });
    });
    mxrs_writer::write_project(path, &initial.build()).unwrap();
}

#[test]
fn a_demo_user_declared_without_security_joins_the_stored_security() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    stored_security(&path);
    plant_stored_demo_user(&path, "Manager", "kept-secret");
    let before = document_by_type(&path, "Security$ProjectSecurity");

    let declaration = standalone_demo_user("Manager", |user| {
        user.role("Administrator")
            .password_from_env("MXRS_TEST_STANDALONE_DEMO_USER_UNSET");
    });
    mxrs_writer::synchronize_project(&path, &declaration).unwrap();

    let after = document_by_type(&path, "Security$ProjectSecurity");
    let Some(mxrs_bson::Bson::Array(raw)) = after.get("DemoUsers") else {
        panic!("DemoUsers array missing")
    };
    let users = mxrs_bson::parse_array(Some(raw)).items;
    assert_eq!(users.len(), 2, "declared + opaque");
    let mxrs_bson::Bson::Document(manager) = &users[0] else {
        panic!("first entry is not a document")
    };
    let Some(mxrs_bson::Bson::Array(roles)) = manager.get("UserRoles") else {
        panic!("UserRoles array missing")
    };
    assert_eq!(
        mxrs_bson::parse_array(Some(roles)).items,
        [mxrs_bson::Bson::String("Administrator".into())],
        "the declaration reached the model"
    );
    assert_eq!(manager.get_str("Password").unwrap(), "kept-secret");

    // Everything the declaration does not speak for is what was stored.
    for key in before.keys().filter(|key| key.as_str() != "DemoUsers") {
        assert_eq!(before.get(key), after.get(key), "{key}");
    }
    assert_eq!(before.keys().count(), after.keys().count());
}

#[test]
fn a_demo_user_declared_without_security_is_checked_against_the_stored_roles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    stored_security(&path);

    let declaration = standalone_demo_user("Manager", |user| {
        user.role("Ghost");
    });
    let error = mxrs_writer::synchronize_project(&path, &declaration).unwrap_err();
    assert!(
        matches!(error, mxrs_writer::WriterError::UnknownUserRole(ref role) if role == "Ghost"),
        "{error}"
    );
}

#[test]
fn a_demo_user_with_no_security_to_join_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    mxrs_writer::write_project(&path, &ProjectBuilder::new("11.12.1").build()).unwrap();
    // A model that lost its project security: nothing a demo user could be
    // added to, and nothing to check its roles against.
    {
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let root = mpr.root_unit().unwrap().unwrap().unit_id;
        let unit = mpr
            .children_of(&root)
            .unwrap()
            .into_iter()
            .find(|unit| {
                mpr.parse_contents(unit)
                    .ok()
                    .and_then(|document| document.get_str("$Type").ok().map(str::to_string))
                    .as_deref()
                    == Some("Security$ProjectSecurity")
            })
            .unwrap();
        mpr.transaction(|mpr| mpr.delete_unit(&unit.unit_id))
            .unwrap();
    }

    let declaration = standalone_demo_user("Manager", |user| {
        user.role("Administrator");
    });
    let error = mxrs_writer::synchronize_project(&path, &declaration).unwrap_err();
    assert!(
        matches!(
            error,
            mxrs_writer::WriterError::DemoUsersWithoutProjectSecurity { ref names }
                if names == "Manager"
        ),
        "{error}"
    );
}

#[test]
fn a_demo_user_in_a_new_project_joins_the_security_every_model_starts_with() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Demo.mpr");
    mxrs_writer::write_project(&path, &ProjectBuilder::new("11.12.1").build()).unwrap();
    plant_stored_demo_user(&path, "Manager", "kept-secret");

    let declaration = standalone_demo_user("Manager", |user| {
        user.role("Administrator");
    });
    mxrs_writer::synchronize_project(&path, &declaration).unwrap();

    let security = document_by_type(&path, "Security$ProjectSecurity");
    let Some(mxrs_bson::Bson::Array(raw)) = security.get("DemoUsers") else {
        panic!("DemoUsers array missing")
    };
    let users = mxrs_bson::parse_array(Some(raw)).items;
    assert!(users.iter().any(|user| matches!(
        user,
        mxrs_bson::Bson::Document(doc) if doc.get_str("UserName").ok() == Some("Manager")
    )));
}
