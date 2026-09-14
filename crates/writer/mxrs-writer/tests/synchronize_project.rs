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

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Sales", |module| {
        module.enumeration("Status", |enumeration| {
            enumeration.value("Open");
            enumeration.value("Closed");
        });
        module.constant("Limit", |constant| {
            constant.value("20");
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
