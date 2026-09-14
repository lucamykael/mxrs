use super::*;
use mxrs_bson::doc;

fn fixtures() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left/Project.mpr");
    let right = directory.path().join("right/Project.mpr");
    for path in [&left, &right] {
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.microflow("Decide", |flow| {
                flow.decision(
                    mxrs_dsl::boolean(true),
                    |yes| {
                        yes.return_value(mxrs_dsl::integer(1));
                    },
                    |no| {
                        no.return_value(mxrs_dsl::integer(2));
                    },
                );
            });
            module.nanoflow("Client", |flow| {
                flow.return_value(mxrs_dsl::integer(1));
            });
        });
        mxrs_writer::write_project(path, &builder.build()).unwrap();
    }
    assert!(compare(&left, &right).unwrap().is_identical());
    (directory, left, right)
}

fn edit_document(path: &Path, type_name: &str, change: impl FnOnce(&mut Document)) {
    let mut mpr = mxrs_mpr::MprFile::open(path, false).unwrap();
    let (unit, mut document) = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let document = mpr.parse_contents(&unit).unwrap();
            (document.get_str("$Type").ok() == Some(type_name)).then_some((unit, document))
        })
        .unwrap();
    change(&mut document);
    mpr.transaction(|mpr| {
        mpr.update_unit(&unit.unit_id, document)?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn case_only_edits_in_a_persisted_decision_are_observable() {
    let (_directory, left, right) = fixtures();
    edit_document(&right, "Microflows$Microflow", |flow| {
        let cases = flow
            .get_array_mut("Flows")
            .unwrap()
            .iter_mut()
            .filter_map(Bson::as_document_mut)
            .find_map(|edge| {
                edge.get_array_mut("CaseValues").ok().and_then(|cases| {
                    cases
                        .iter_mut()
                        .filter_map(Bson::as_document_mut)
                        .find(|case| case.get_str("Value").ok() == Some("true"))
                })
            })
            .unwrap();
        cases.insert("Value", "false");
    });
    let changes = compare(&left, &right).unwrap().changes;
    assert!(
        changes
            .iter()
            .any(|change| change.path.iter().any(|part| part == "cases")),
        "{changes:#?}"
    );
}

#[test]
fn reordered_decision_objects_and_edges_keep_the_same_behavioral_identity() {
    let (_directory, left, right) = fixtures();
    edit_document(&right, "Microflows$Microflow", |flow| {
        flow.get_document_mut("ObjectCollection")
            .unwrap()
            .get_array_mut("Objects")
            .unwrap()[1..]
            .reverse();
        flow.get_array_mut("Flows").unwrap()[1..].reverse();
    });
    let result = compare(&left, &right).unwrap();
    assert!(result.is_identical(), "{:#?}", result.changes);
}

#[test]
fn microflow_and_nanoflow_permission_edits_are_detected_but_role_order_is_not_a_change() {
    for type_name in ["Microflows$Microflow", "Microflows$Nanoflow"] {
        let (_directory, left, right) = fixtures();
        edit_document(&left, type_name, |flow| {
            flow.insert(
                "AllowedModuleRoles",
                mxrs_bson::build_array(
                    vec![
                        Bson::String("Sales.User".into()),
                        Bson::String("Sales.Admin".into()),
                    ],
                    1,
                ),
            );
        });
        edit_document(&right, type_name, |flow| {
            flow.insert(
                "AllowedModuleRoles",
                mxrs_bson::build_array(
                    vec![
                        Bson::String("Sales.Admin".into()),
                        Bson::String("Sales.User".into()),
                    ],
                    1,
                ),
            );
        });
        assert!(compare(&left, &right).unwrap().is_identical());
        edit_document(&right, type_name, |flow| {
            flow.insert(
                "AllowedModuleRoles",
                mxrs_bson::build_array(vec![Bson::String("Sales.Admin".into())], 1),
            );
        });
        let changes = compare(&left, &right).unwrap().changes;
        assert!(
            changes
                .iter()
                .any(|change| change.path.iter().any(|part| part == "allowed_roles")),
            "{changes:#?}"
        );
    }
}

fn profile() -> Document {
    doc! {
        "Name": "Responsive", "Kind": "ResponsiveOffline",
        "HomePage": { "Page": "Sales.Home", "Microflow": "" },
        "LoginPageSettings": { "Form": "Sales.Login" },
        "HomeItems": mxrs_bson::build_array(vec![Bson::Document(doc! { "UserRole": "Admin", "Microflow": "Sales.Start" })], 2),
        "AppIcon": { "Code": 42_i64 },
        "AppTitle": { "Translations": mxrs_bson::build_array(vec![Bson::Document(doc! { "LanguageCode": "en_US", "Text": "App" })], 2) },
        "Menu": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
            "Caption": { "Translations": mxrs_bson::build_array(vec![Bson::Document(doc! { "LanguageCode": "en_US", "Text": "Orders" })], 2) },
            "Action": { "FormSettings": { "Form": "Sales.Orders" } },
            "Icon": { "Code": 57361_i64 },
            "Items": mxrs_bson::build_array(vec![Bson::Document(doc! { "Action": { "MicroflowSettings": { "Microflow": "Sales.Refresh" } } })], 2),
        })], 2) },
    }
}

#[test]
fn navigation_summary_matches_the_ruby_profile_contract_including_int64_glyphs() {
    let document = doc! { "Profiles": mxrs_bson::build_array(vec![Bson::Document(profile())], 2) };
    assert_eq!(
        navigation_document_summary(&document).unwrap(),
        json!({ "profiles": [{
        "name": "Responsive", "kind": "ResponsiveOffline", "offline": true,
        "home_page": "Sales.Home", "home_microflow": null, "sign_in_page": "Sales.Login",
        "role_homes": [{ "role": "Admin", "microflow": "Sales.Start" }],
        "app_icon": { "Code": 42 }, "app_title": { "en_US": "App" },
        "items": [{ "caption": { "en_US": "Orders" }, "page": "Sales.Orders", "icon": 57361,
            "items": [{ "caption": {}, "microflow": "Sales.Refresh", "items": [] }] }],
    }] })
    );
}

#[test]
fn navigation_changes_are_detected_after_real_storage_round_trips() {
    let (_directory, left, right) = fixtures();
    for path in [&left, &right] {
        edit_document(path, "Navigation$NavigationDocument", |navigation| {
            navigation.insert(
                "Profiles",
                mxrs_bson::build_array(vec![Bson::Document(profile())], 2),
            );
        });
    }
    assert!(compare(&left, &right).unwrap().is_identical());
    edit_document(&right, "Navigation$NavigationDocument", |navigation| {
        let profile = navigation.get_array_mut("Profiles").unwrap()[1]
            .as_document_mut()
            .unwrap();
        profile
            .get_document_mut("HomePage")
            .unwrap()
            .insert("Page", "Sales.Other");
        profile
            .get_document_mut("Menu")
            .unwrap()
            .get_array_mut("Items")
            .unwrap()[1]
            .as_document_mut()
            .unwrap()
            .get_document_mut("Icon")
            .unwrap()
            .insert("Code", 57362_i64);
    });
    let changes = compare(&left, &right).unwrap().changes;
    for key in ["home_page", "icon"] {
        assert!(
            changes
                .iter()
                .any(
                    |change| change.path.first().map(String::as_str) == Some("navigation")
                        && change.path.iter().any(|part| part == key)
                ),
            "missing {key}: {changes:#?}"
        );
    }
}

#[test]
fn legacy_navigation_and_empty_translations_have_the_same_fallback_rules_as_ruby() {
    let mut legacy = profile();
    legacy.remove("Name");
    legacy.insert("HomeItems", Bson::Null);
    legacy.insert(
        "RoleBasedHomePages",
        mxrs_bson::build_array(
            vec![Bson::Document(
                doc! { "UserRole": "User", "Page": "Sales.Home" },
            )],
            2,
        ),
    );
    let menu = legacy.remove("Menu").unwrap();
    legacy.insert("MenuItemCollection", menu);
    legacy.insert("AppTitle", doc! { "Translations": Bson::Null, "Items": mxrs_bson::build_array(vec![Bson::Document(doc! { "LanguageCode": "en_US", "Text": "Old" }), Bson::Document(doc! { "LanguageCode": "en_US", "Text": "" })], 2) });
    let summary = navigation_document_summary(&doc! { "DesktopProfile": legacy }).unwrap();
    assert_eq!(summary["profiles"][0]["name"], "Desktop");
    assert_eq!(
        summary["profiles"][0]["role_homes"],
        json!([{ "role": "User", "page": "Sales.Home" }])
    );
    assert_eq!(summary["profiles"][0]["app_title"], json!({}));
    assert_eq!(summary["profiles"][0]["items"][0]["icon"], 57361);
    assert_eq!(
        navigation_document_summary(&Document::new()).unwrap(),
        json!({ "profiles": [] })
    );
}

fn replace_navigation_value(document: &mut Document, path: &[&str], value: Bson) {
    let (key, rest) = path.split_first().unwrap();
    if rest.is_empty() {
        document.insert(*key, value);
        return;
    }
    let document = match document.get_mut(*key).unwrap() {
        Bson::Document(document) => document,
        Bson::Array(items) => items.iter_mut().find_map(Bson::as_document_mut).unwrap(),
        value => panic!("invalid test fixture at {key}: {value:?}"),
    };
    replace_navigation_value(document, rest, value);
}

fn navigation_collection_paths() -> Vec<(Vec<&'static str>, &'static str)> {
    vec![
        (vec!["Profiles"], "Navigation.Profiles"),
        (
            vec!["Profiles", "HomeItems"],
            "Navigation.Profiles[0].HomeItems",
        ),
        (
            vec!["Profiles", "Menu", "Items"],
            "Navigation.Profiles[0].Menu.Items",
        ),
        (
            vec!["Profiles", "Menu", "Items", "Items"],
            "Navigation.Profiles[0].Menu.Items[0].Items",
        ),
        (
            vec!["Profiles", "AppTitle", "Translations"],
            "Navigation.Profiles[0].AppTitle.Translations",
        ),
        (
            vec!["Profiles", "Menu", "Items", "Caption", "Translations"],
            "Navigation.Profiles[0].Menu.Items[0].Caption.Translations",
        ),
    ]
}

#[test]
fn malformed_navigation_collections_and_items_are_typed_errors_not_filtered_values() {
    for (path, expected_path) in navigation_collection_paths() {
        for invalid in [
            Bson::String("malformed".into()),
            Bson::Document(Document::new()),
            Bson::Array(vec![Bson::Int32(2), Bson::Null]),
            Bson::Array(vec![Bson::String("malformed".into())]),
            Bson::Array(vec![
                Bson::Int64(2),
                Bson::Document(Document::new()),
                Bson::Boolean(false),
            ]),
        ] {
            let mut document = doc! { "Profiles": [2, profile()] };
            let (error_path, expected) = match &invalid {
                Bson::Array(items) if items.len() == 3 => {
                    (format!("{expected_path}[1]"), "document")
                }
                Bson::Array(_) => (format!("{expected_path}[0]"), "document"),
                _ => (expected_path.to_string(), "array of documents"),
            };
            replace_navigation_value(&mut document, &path, invalid);
            let error = navigation_document_summary(&document).unwrap_err();
            assert!(
                matches!(&error, mxrs_model::ModelError::InvalidStructure { path, expected: actual } if path == &error_path && actual == &expected),
                "{error:?}"
            );
            assert_eq!(
                error.to_string(),
                format!("invalid model structure at {error_path}: expected {expected}")
            );
        }
    }
}

#[test]
fn malformed_navigation_fallbacks_are_checked_and_do_not_hide_invalid_primary_fields() {
    let fallback_profiles = [
        (
            doc! { "HomeItems": Bson::Null, "RoleBasedHomePages": [2, "malformed"] },
            "Navigation.Profiles[0].RoleBasedHomePages[0]",
        ),
        (
            doc! { "Menu": Bson::Null, "MenuItemCollection": { "Items": [2, "malformed"] } },
            "Navigation.Profiles[0].MenuItemCollection.Items[0]",
        ),
        (
            doc! { "AppTitle": { "Translations": Bson::Null, "Items": [2, "malformed"] } },
            "Navigation.Profiles[0].AppTitle.Items[0]",
        ),
        (
            doc! { "Menu": { "Items": [{ "Caption": { "Translations": Bson::Null, "Items": [2, "malformed"] } }] } },
            "Navigation.Profiles[0].Menu.Items[0].Caption.Items[0]",
        ),
        (
            doc! { "HomeItems": "malformed", "RoleBasedHomePages": [] },
            "Navigation.Profiles[0].HomeItems",
        ),
        (
            doc! { "Menu": "malformed", "MenuItemCollection": { "Items": [] } },
            "Navigation.Profiles[0].Menu",
        ),
        (
            doc! { "MenuItemCollection": "malformed" },
            "Navigation.Profiles[0].MenuItemCollection",
        ),
        (
            doc! { "AppTitle": { "Translations": "malformed", "Items": [] } },
            "Navigation.Profiles[0].AppTitle.Translations",
        ),
    ];
    for (profile, expected_path) in fallback_profiles {
        let error = navigation_document_summary(&doc! { "Profiles": [profile] }).unwrap_err();
        assert!(
            matches!(&error, mxrs_model::ModelError::InvalidStructure { path, .. } if path == expected_path),
            "{error:?}"
        );
    }
    let error = navigation_document_summary(
        &doc! { "Profiles": [2, "malformed"], "DesktopProfile": profile() },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        mxrs_model::ModelError::InvalidStructure { .. }
    ));
}

#[test]
fn navigation_null_collections_and_unmarked_property_projections_remain_valid() {
    for (path, _) in navigation_collection_paths() {
        let mut empty = doc! { "Profiles": [2, profile()] };
        let mut null = empty.clone();
        replace_navigation_value(&mut empty, &path, Bson::Array(Vec::new()));
        replace_navigation_value(&mut null, &path, Bson::Null);
        assert_eq!(
            navigation_document_summary(&empty).unwrap(),
            navigation_document_summary(&null).unwrap(),
            "{path:?}"
        );
    }
    fn remove_markers(value: &mut Bson) {
        match value {
            Bson::Document(document) => {
                for (_, value) in document.iter_mut() {
                    remove_markers(value);
                }
            }
            Bson::Array(items) => {
                if matches!(items.first(), Some(Bson::Int32(_) | Bson::Int64(_))) {
                    items.remove(0);
                }
                for value in items {
                    remove_markers(value);
                }
            }
            _ => {}
        }
    }
    let marked = doc! { "Profiles": [2_i64, profile()] };
    let mut unmarked = Bson::Document(marked.clone());
    remove_markers(&mut unmarked);
    assert_eq!(
        navigation_document_summary(&marked).unwrap(),
        navigation_document_summary(unmarked.as_document().unwrap()).unwrap()
    );
    assert!(navigation_document_summary(&doc! { "Profiles": [{ "Menu": Bson::Null }] }).is_ok());
}

#[test]
fn malformed_navigation_in_real_mpr_storage_cannot_compare_identical_to_empty_navigation() {
    let (_directory, left, right) = fixtures();
    edit_document(&left, "Navigation$NavigationDocument", |navigation| {
        navigation.insert("Profiles", vec![Bson::Int32(2)]);
    });
    for (path, expected_path) in navigation_collection_paths() {
        edit_document(&right, "Navigation$NavigationDocument", |navigation| {
            navigation.insert("Profiles", vec![Bson::Int32(2), Bson::Document(profile())]);
            replace_navigation_value(
                navigation,
                &path,
                Bson::Array(vec![Bson::Int32(2), Bson::String("malformed".into())]),
            );
        });
        let error = compare(&left, &right).unwrap_err();
        assert!(
            matches!(&error, mxrs_model::ModelError::InvalidStructure { path, expected: "document" } if path == &format!("{expected_path}[0]")),
            "{error:?}"
        );
    }
}

#[test]
fn modern_and_legacy_case_fields_share_one_normalization_and_drop_only_no_case() {
    let case = doc! { "$ID": "case-id", "$Type": "Microflows$EnumerationCase", "Value": "true" };
    let modern = doc! { "CaseValues": mxrs_bson::build_array(vec![Bson::Document(case.clone()), Bson::Document(doc! { "$Type": "Microflows$NoCase" })], 2) };
    let legacy = doc! { "NewCaseValue": case.clone() };
    let expected = vec![json!({ "$Type": "Microflows$EnumerationCase", "Value": "true" })];
    assert_eq!(normalized_case_values(&modern, &HashMap::new()), expected);
    assert_eq!(normalized_case_values(&legacy, &HashMap::new()), expected);
    assert_eq!(
        normalized_case_values(
            &doc! { "CaseValues": Bson::Null, "NewCaseValue": case },
            &HashMap::new()
        ),
        expected
    );
    assert!(normalized_case_values(&Document::new(), &HashMap::new()).is_empty());
    assert!(
        normalized_case_values(&doc! { "NewCaseValue": Bson::Null }, &HashMap::new()).is_empty()
    );
}

#[test]
fn identical_models_with_different_physical_storage_formats_are_not_identical_packages() {
    let (_directory, left, right) = fixtures();
    let mpr = mxrs_mpr::MprFile::open(&right, false).unwrap();
    mpr.raw_query("ALTER TABLE Unit ADD COLUMN Contents BLOB")
        .unwrap();
    for unit in mpr.all_units().unwrap() {
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let data = hex(&mpr.content_bytes(&unit).unwrap().unwrap());
        let id = hex(&mxrs_bson::uuid_to_blob(&unit.unit_id).unwrap());
        mpr.raw_query(&format!(
            "UPDATE Unit SET Contents = x'{data}' WHERE UnitID = x'{id}'"
        ))
        .unwrap();
    }
    drop(mpr);
    std::fs::rename(
        right.parent().unwrap().join("mprcontents"),
        right.parent().unwrap().join("original-external-units"),
    )
    .unwrap();
    let result = compare(&left, &right).unwrap();
    assert_eq!(result.changes.len(), 1, "{:#?}", result.changes);
    assert_eq!(result.changes[0].path, ["project", "format_version"]);
    assert_eq!(result.changes[0].before, Some(json!("v2")));
    assert_eq!(result.changes[0].after, Some(json!("v1")));
}

#[test]
fn undecodable_security_or_navigation_units_are_errors_not_missing_equal_snapshots() {
    for type_name in ["Security$ProjectSecurity", "Navigation$NavigationDocument"] {
        let (_directory, left, right) = fixtures();
        let mpr = mxrs_mpr::MprFile::open(&right, false).unwrap();
        let unit = mpr
            .all_units()
            .unwrap()
            .into_iter()
            .find(|unit| mpr.parse_contents(unit).unwrap().get_str("$Type").ok() == Some(type_name))
            .unwrap();
        std::fs::write(mpr.content_path(&unit).unwrap(), b"invalid BSON").unwrap();
        drop(mpr);
        assert!(compare(&left, &right).is_err());
    }
}
