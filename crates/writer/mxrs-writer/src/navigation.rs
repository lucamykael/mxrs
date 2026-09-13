//! Loss-preserving persistence for Cargo-native navigation profiles.

use std::collections::{HashMap, HashSet};

use mxrs_bson::{Bson, Document, build_array, doc, parse_array};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::{NavigationDecl, NavigationItemDecl, NavigationProfileDecl, RoleHomeDecl};
use mxrs_mpr::MprFile;

use crate::error::{Result, WriterError};

pub(crate) fn synchronize_navigation(
    mpr: &mut MprFile,
    root_id: &str,
    declaration: &NavigationDecl,
    identity: ProjectIdentity,
) -> Result<()> {
    validate_navigation(mpr, declaration)?;
    let existing = mpr.children_of(root_id)?.into_iter().find_map(|unit| {
        let document = mpr.parse_contents(&unit).ok()?;
        (document.get_str("$Type").ok() == Some("Navigation$NavigationDocument"))
            .then_some((unit, document))
    });
    let previous_profiles = existing
        .as_ref()
        .map(|(_, document)| documents_by_name(document, "Profiles"))
        .unwrap_or_default();
    let profiles = declaration
        .profiles
        .iter()
        .map(|profile| {
            profile_document(profile, previous_profiles.get(&profile.name), identity)
                .map(Bson::Document)
        })
        .collect::<Result<Vec<_>>>()?;

    let (unit_id, mut document) = match existing {
        Some((unit, document)) => (unit.unit_id, document),
        None => {
            let id = identity.artifact_id(ArtifactKind::Navigation, "project");
            (
                id.clone(),
                doc! { "$ID": id, "$Type": "Navigation$NavigationDocument" },
            )
        }
    };
    let marker = array_marker(document.get("Profiles"), 2);
    document.insert("$ID", unit_id.clone());
    document.insert("$Type", "Navigation$NavigationDocument");
    document.insert("Profiles", build_array(profiles, marker));
    if mpr.unit(&unit_id)?.is_some() {
        mpr.update_unit(&unit_id, document)?;
    } else {
        mpr.insert_unit(root_id, "ProjectDocuments", document, Some(&unit_id))?;
    }
    Ok(())
}

fn profile_document(
    declaration: &NavigationProfileDecl,
    previous: Option<&Document>,
    identity: ProjectIdentity,
) -> Result<Document> {
    let mut document = previous.cloned().unwrap_or_default();
    stable_nested_id(
        &mut document,
        identity,
        &format!("profile:{}", declaration.name),
    );
    document.insert("$Type", "Navigation$NavigationProfile");
    document.insert("Name", declaration.name.clone());
    document.insert("Kind", declaration.kind.clone());
    document.insert(
        "HomePage",
        home_document(
            declaration.home_page.as_deref(),
            declaration.home_microflow.as_deref(),
            previous.and_then(|value| value.get_document("HomePage").ok()),
            identity,
            &format!("profile:{}:home", declaration.name),
        ),
    );
    document.insert(
        "HomeItems",
        indexed_documents(
            &declaration.role_homes,
            previous.and_then(|value| value.get("HomeItems")),
            2,
            |home, prior, index| {
                role_home_document(
                    home,
                    prior,
                    identity,
                    &format!("profile:{}:role-home:{index}", declaration.name),
                )
            },
        )?,
    );
    document.insert(
        "Menu",
        menu_document(
            &declaration.items,
            previous.and_then(|value| value.get_document("Menu").ok()),
            identity,
            &format!("profile:{}:menu", declaration.name),
        )?,
    );
    document.insert(
        "AppTitle",
        translated_text(
            &declaration.app_title,
            previous.and_then(|value| value.get_document("AppTitle").ok()),
            identity,
            &format!("profile:{}:title", declaration.name),
        ),
    );
    if let Some(page) = &declaration.sign_in_page {
        let mut settings = previous
            .and_then(|value| value.get_document("LoginPageSettings").ok())
            .cloned()
            .unwrap_or_default();
        stable_nested_id(
            &mut settings,
            identity,
            &format!("profile:{}:sign-in", declaration.name),
        );
        settings.insert("$Type", "Forms$FormSettings");
        settings.insert("Form", page.clone());
        settings.insert("ParameterMappings", build_array(vec![], 2));
        settings.insert("TitleOverride", Bson::Null);
        document.insert("LoginPageSettings", settings);
    } else {
        document.insert("LoginPageSettings", Bson::Null);
    }
    document.insert("OfflineEntityConfigs", build_array(vec![], 3));
    document.insert("ProgressiveWebAppSettings", Bson::Null);
    document.insert("NotFoundHomepage", Bson::Null);
    document.insert("ThrowPartialSyncError", true);
    Ok(document)
}

fn home_document(
    page: Option<&str>,
    microflow: Option<&str>,
    previous: Option<&Document>,
    identity: ProjectIdentity,
    key: &str,
) -> Document {
    let mut document = previous.cloned().unwrap_or_default();
    stable_nested_id(&mut document, identity, key);
    document.insert("$Type", "Navigation$HomePage");
    document.insert("Page", page.unwrap_or_default());
    document.insert("Microflow", microflow.unwrap_or_default());
    document
}

fn role_home_document(
    home: &RoleHomeDecl,
    previous: Option<&Document>,
    identity: ProjectIdentity,
    key: &str,
) -> Result<Document> {
    let mut document = previous.cloned().unwrap_or_default();
    stable_nested_id(&mut document, identity, key);
    document.insert("$Type", "Navigation$RoleBasedHomePage");
    document.insert("UserRole", home.user_role.clone());
    document.insert("Page", home.page.clone().unwrap_or_default());
    document.insert("Microflow", home.microflow.clone().unwrap_or_default());
    Ok(document)
}

fn menu_document(
    items: &[NavigationItemDecl],
    previous: Option<&Document>,
    identity: ProjectIdentity,
    key: &str,
) -> Result<Document> {
    let mut document = previous.cloned().unwrap_or_default();
    stable_nested_id(&mut document, identity, key);
    document.insert("$Type", "Menus$MenuItemCollection");
    let previous_items = previous.and_then(|value| value.get("Items"));
    document.insert(
        "Items",
        indexed_documents(items, previous_items, 2, |item, prior, index| {
            menu_item_document(item, prior, identity, &format!("{key}:item:{index}"))
        })?,
    );
    Ok(document)
}

fn menu_item_document(
    item: &NavigationItemDecl,
    previous: Option<&Document>,
    identity: ProjectIdentity,
    key: &str,
) -> Result<Document> {
    let mut document = previous.cloned().unwrap_or_default();
    stable_nested_id(&mut document, identity, key);
    document.insert("$Type", "Menus$MenuItem");
    document.insert(
        "Caption",
        translated_text(
            &item.caption,
            previous.and_then(|value| value.get_document("Caption").ok()),
            identity,
            &format!("{key}:caption"),
        ),
    );
    document.insert(
        "Action",
        action_document(
            item,
            previous.and_then(|value| value.get_document("Action").ok()),
            identity,
            &format!("{key}:action"),
        ),
    );
    match &item.icon {
        Some(code) => {
            let mut icon = previous
                .and_then(|value| value.get_document("Icon").ok())
                .cloned()
                .unwrap_or_default();
            stable_nested_id(&mut icon, identity, &format!("{key}:icon"));
            icon.insert("$Type", "Forms$GlyphIcon");
            icon.insert("Code", code.clone());
            document.insert("Icon", icon);
        }
        None => {
            document.insert("Icon", Bson::Null);
        }
    }
    document.insert(
        "Items",
        indexed_documents(
            &item.items,
            previous.and_then(|value| value.get("Items")),
            2,
            |child, prior, index| {
                menu_item_document(child, prior, identity, &format!("{key}:item:{index}"))
            },
        )?,
    );
    Ok(document)
}

fn action_document(
    item: &NavigationItemDecl,
    previous: Option<&Document>,
    identity: ProjectIdentity,
    key: &str,
) -> Document {
    let mut action = previous.cloned().unwrap_or_default();
    stable_nested_id(&mut action, identity, key);
    if let Some(page) = &item.page {
        action.insert("$Type", "Forms$FormAction");
        action.insert("DisabledDuringExecution", false);
        let mut settings = previous
            .and_then(|value| value.get_document("FormSettings").ok())
            .cloned()
            .unwrap_or_default();
        stable_nested_id(&mut settings, identity, &format!("{key}:settings"));
        settings.insert("$Type", "Forms$FormSettings");
        settings.insert("Form", page.clone());
        settings.insert("ParameterMappings", build_array(vec![], 2));
        settings.insert("TitleOverride", Bson::Null);
        action.insert("FormSettings", settings);
        action.insert("NumberOfPagesToClose2", "");
        action.insert("PagesForSpecializations", build_array(vec![], 2));
    } else if let Some(microflow) = &item.microflow {
        action.insert("$Type", "Forms$MicroflowAction");
        let mut settings = previous
            .and_then(|value| value.get_document("MicroflowSettings").ok())
            .cloned()
            .unwrap_or_default();
        stable_nested_id(&mut settings, identity, &format!("{key}:settings"));
        settings.insert("$Type", "Forms$MicroflowSettings");
        settings.insert("Microflow", microflow.clone());
        action.insert("MicroflowSettings", settings);
    } else {
        action.insert("$Type", "Forms$NoAction");
    }
    action
}

fn translated_text(
    translations: &std::collections::BTreeMap<String, String>,
    previous: Option<&Document>,
    identity: ProjectIdentity,
    key: &str,
) -> Document {
    let mut document = previous.cloned().unwrap_or_default();
    stable_nested_id(&mut document, identity, key);
    document.insert("$Type", "Texts$Text");
    let prior = previous
        .map(|value| documents_by_language(value, "Items"))
        .unwrap_or_default();
    let items = translations
        .iter()
        .map(|(locale, text)| {
            let mut translation = prior.get(locale).cloned().unwrap_or_default();
            stable_nested_id(&mut translation, identity, &format!("{key}:{locale}"));
            translation.insert("$Type", "Texts$Translation");
            translation.insert("LanguageCode", locale.clone());
            translation.insert("Text", text.clone());
            Bson::Document(translation)
        })
        .collect();
    let marker = previous.map_or(2, |value| array_marker(value.get("Items"), 2));
    document.insert("Items", build_array(items, marker));
    document
}

fn indexed_documents<T>(
    values: &[T],
    previous: Option<&Bson>,
    default_marker: i32,
    mut render: impl FnMut(&T, Option<&Document>, usize) -> Result<Document>,
) -> Result<Bson> {
    let (marker, prior) = array_documents(previous, default_marker);
    let documents = values
        .iter()
        .enumerate()
        .map(|(index, value)| render(value, prior.get(index), index).map(Bson::Document))
        .collect::<Result<Vec<_>>>()?;
    Ok(Bson::Array(build_array(documents, marker)))
}

fn array_documents(value: Option<&Bson>, default_marker: i32) -> (i32, Vec<Document>) {
    let Some(Bson::Array(values)) = value else {
        return (default_marker, vec![]);
    };
    let parsed = parse_array(Some(values));
    let documents = parsed
        .items
        .into_iter()
        .filter_map(|value| match value {
            Bson::Document(document) => Some(document),
            _ => None,
        })
        .collect();
    (parsed.marker, documents)
}

fn array_marker(value: Option<&Bson>, default_marker: i32) -> i32 {
    array_documents(value, default_marker).0
}

fn documents_by_name(document: &Document, field: &str) -> HashMap<String, Document> {
    array_documents(document.get(field), 2)
        .1
        .into_iter()
        .filter_map(|document| {
            let name = document.get_str("Name").ok()?.to_string();
            Some((name, document))
        })
        .collect()
}

fn documents_by_language(document: &Document, field: &str) -> HashMap<String, Document> {
    array_documents(document.get(field), 2)
        .1
        .into_iter()
        .filter_map(|document| {
            let language = document.get_str("LanguageCode").ok()?.to_string();
            Some((language, document))
        })
        .collect()
}

fn stable_nested_id(document: &mut Document, identity: ProjectIdentity, key: &str) {
    if !document.contains_key("$ID") {
        document.insert("$ID", identity.artifact_id(ArtifactKind::Navigation, key));
    }
}

fn validate_navigation(mpr: &MprFile, declaration: &NavigationDecl) -> Result<()> {
    let (known_pages, known_microflows) = navigation_targets(mpr)?;
    let mut profiles = HashSet::new();
    for profile in &declaration.profiles {
        if !profiles.insert(profile.name.as_str()) {
            return Err(WriterError::DuplicateNavigationProfile(
                profile.name.clone(),
            ));
        }
        if profile.home_page.is_some() && profile.home_microflow.is_some() {
            return Err(WriterError::ConflictingNavigationTargets(
                profile.name.clone(),
            ));
        }
        validate_reference(
            profile.home_page.as_deref(),
            &known_pages,
            "page",
            &format!("profile {} home", profile.name),
        )?;
        validate_reference(
            profile.home_microflow.as_deref(),
            &known_microflows,
            "microflow",
            &format!("profile {} home", profile.name),
        )?;
        validate_reference(
            profile.sign_in_page.as_deref(),
            &known_pages,
            "page",
            &format!("profile {} sign-in", profile.name),
        )?;
        validate_items(
            &profile.items,
            &format!("profile {}", profile.name),
            &known_pages,
            &known_microflows,
        )?;
    }

    let known_roles = project_role_names(mpr)?;
    for profile in &declaration.profiles {
        for home in &profile.role_homes {
            if !known_roles.contains(home.user_role.as_str()) {
                return Err(WriterError::UnknownUserRole(home.user_role.clone()));
            }
            if home.page.is_some() == home.microflow.is_some() {
                return Err(WriterError::ConflictingNavigationTargets(format!(
                    "{} role {}",
                    profile.name, home.user_role
                )));
            }
            validate_reference(
                home.page.as_deref(),
                &known_pages,
                "page",
                &format!("profile {} role {}", profile.name, home.user_role),
            )?;
            validate_reference(
                home.microflow.as_deref(),
                &known_microflows,
                "microflow",
                &format!("profile {} role {}", profile.name, home.user_role),
            )?;
        }
    }
    Ok(())
}

fn validate_items(
    items: &[NavigationItemDecl],
    path: &str,
    known_pages: &HashSet<String>,
    known_microflows: &HashSet<String>,
) -> Result<()> {
    for (index, item) in items.iter().enumerate() {
        if item.page.is_some() && item.microflow.is_some() {
            return Err(WriterError::ConflictingNavigationTargets(format!(
                "{path} item {index}"
            )));
        }
        let item_path = format!("{path} item {index}");
        validate_reference(item.page.as_deref(), known_pages, "page", &item_path)?;
        validate_reference(
            item.microflow.as_deref(),
            known_microflows,
            "microflow",
            &item_path,
        )?;
        validate_items(&item.items, &item_path, known_pages, known_microflows)?;
    }
    Ok(())
}

fn validate_reference(
    reference: Option<&str>,
    known: &HashSet<String>,
    kind: &str,
    path: &str,
) -> Result<()> {
    if let Some(reference) = reference
        && !known.contains(reference)
    {
        return Err(WriterError::UnknownNavigationTarget {
            kind: kind.to_string(),
            reference: reference.to_string(),
            path: path.to_string(),
        });
    }
    Ok(())
}

fn navigation_targets(mpr: &MprFile) -> Result<(HashSet<String>, HashSet<String>)> {
    let units = mpr.all_units()?;
    let mut containers = HashMap::<String, String>::new();
    let mut module_names = HashMap::<String, String>::new();
    for unit in &units {
        containers.insert(unit.unit_id.clone(), unit.container_id.clone());
        let document = mpr.parse_contents(unit)?;
        if document.get_str("$Type").ok() == Some("Projects$Module")
            && let Ok(name) = document.get_str("Name")
        {
            module_names.insert(unit.unit_id.clone(), name.to_string());
        }
    }

    let mut pages = HashSet::new();
    let mut microflows = HashSet::new();
    for unit in &units {
        let document = mpr.parse_contents(unit)?;
        let target = match document.get_str("$Type").ok() {
            Some("Forms$Page") | Some("Pages$Page") => &mut pages,
            Some("Microflows$Microflow") => &mut microflows,
            _ => continue,
        };
        target.insert(unit.unit_id.clone());
        if let Some(id) = document.get("$ID").and_then(mxrs_bson::extract_id) {
            target.insert(id);
        }
        let Ok(name) = document.get_str("Name") else {
            continue;
        };
        target.insert(name.to_string());
        let mut parent = Some(unit.container_id.as_str());
        while let Some(id) = parent {
            if let Some(module) = module_names.get(id) {
                target.insert(format!("{module}.{name}"));
                break;
            }
            parent = containers.get(id).map(String::as_str);
        }
    }
    Ok((pages, microflows))
}

fn project_role_names(mpr: &MprFile) -> Result<HashSet<String>> {
    for unit in mpr.all_units()? {
        let document = mpr.parse_contents(&unit)?;
        if document.get_str("$Type").ok() == Some("Security$ProjectSecurity") {
            let mut roles = HashSet::new();
            for (name, role) in documents_by_name(&document, "UserRoles") {
                roles.insert(name);
                if let Some(id) = role.get("$ID").and_then(mxrs_bson::extract_id) {
                    roles.insert(id);
                }
            }
            return Ok(roles);
        }
    }
    Ok(HashSet::new())
}
