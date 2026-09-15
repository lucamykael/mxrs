//! Structural project comparison — ports the snapshot-and-diff shape of
//! mxrb's `Mxrb::Compare` (`compare.rb`): the flat unit list,
//! per-module entities/associations/microflows/nanoflows/pages/menus, the
//! project's security configuration (`security_summary` — read directly off
//! the raw `Security$ProjectSecurity` unit, mxrs-model has no dedicated
//! Security reader, same as `compare.rb` itself doesn't go through a model
//! layer for this either), and a filesystem design-asset inventory
//! (`design_asset_summary` — SHA-256 per file under the same
//! `ASSET_DIRECTORIES` mxrb's `Model::DesignSystem` scans, next to the
//! `.mpr`; that's the only piece of `Model::DesignSystem` this crate needs
//! — the rest of it, token extraction/quality metrics, is out of scope for
//! a comparison snapshot).
//!
//! Diffing reuses `compare.rb`'s two key insights directly:
//! - Named collections (anything shaped `[{ "name": ..., ... }, ...]`) diff
//!   by name, not by position — an item present on both sides under the
//!   same name is compared field-by-field; an unmatched item is reported as
//!   wholesale `added`/`removed` (with a same-content-but-renamed pass in
//!   between, so a rename reads as one change instead of a spurious
//!   remove+add pair).
//! - A microflow's `objects`/`flows` are graph-shaped, not list-shaped —
//!   comparing them positionally (or by their real `$ID`s, which are new
//!   every regeneration) would make every equivalent flow look different.
//!   `assign_flow_ids` (ported below) walks the graph from its
//!   `StartEvent`(s) and assigns each object a position-in-traversal index;
//!   [`normalize_flow_value`] then drops volatile presentation-only fields
//!   (`$ID`, `X`, `Y`, ...) and rewrites any field whose value is a known
//!   object's `$ID` into `{"object": <index>}`, so two structurally
//!   equivalent flows compare equal regardless of their real ids or
//!   declaration order.
//!
//! Decision edges retain normalized case values and use them to order graph
//! traversal, so changing a branch condition is observable while reordering
//! equivalent branches does not change their identities.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use mxrs_bson::{Bson, Document};
use mxrs_model::{Association, Entity, Menu, MenuItem, Microflow, Module, Page, Project, Widget};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn snapshot(path: impl AsRef<Path>) -> mxrs_model::Result<Value> {
    let path = path.as_ref();
    let project = Project::open(path, true)?;
    let mut modules = project.modules()?;
    modules.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(json!({
        "project": {
            "mendix_version": project.mendix_version()?,
            "format_version": match project.mpr().format() {
                mxrs_mpr::StorageFormat::V1 => "v1",
                mxrs_mpr::StorageFormat::V2 => "v2",
            },
        },
        "security": security_summary(&project)?,
        "navigation": navigation_summary(&project)?,
        "design_assets": design_asset_summary(path)?,
        "units": unit_summary(&project)?,
        "modules": modules.iter().map(module_summary).collect::<mxrs_model::Result<Vec<_>>>()?,
    }))
}

fn unit_summary(project: &Project) -> mxrs_model::Result<Vec<Value>> {
    let mut summary: Vec<Value> = project
        .all_units()?
        .into_iter()
        .filter(|u| u.unit_id != u.container_id)
        .map(|u| {
            let doc = project.mpr().parse_contents(&u)?;
            let ty = doc.get_str("$Type").unwrap_or_default().to_string();
            let name = doc
                .get_str("Name")
                .or_else(|_| doc.get_str("name"))
                .unwrap_or_default()
                .to_string();
            Ok(json!({ "containment": u.containment_name, "container_root": false, "type": ty, "name": name }))
        })
        .collect::<mxrs_model::Result<Vec<_>>>()?;
    summary.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v["containment"].as_str().unwrap_or_default().to_string(),
                v["type"].as_str().unwrap_or_default().to_string(),
                v["name"].as_str().unwrap_or_default().to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    Ok(summary)
}

fn module_summary(module: &Module) -> mxrs_model::Result<Value> {
    let mut entities: Vec<&Entity> = module.entities().iter().collect();
    entities.sort_by(|a, b| a.name.cmp(&b.name));
    let mut associations: Vec<&Association> = module.associations();
    associations.sort_by(|a, b| a.name.cmp(&b.name));
    let mut pages: Vec<&Page> = module.pages.iter().collect();
    pages.sort_by(|a, b| a.name.cmp(&b.name));
    let mut menus: Vec<&Menu> = module.menus.iter().collect();
    menus.sort_by(|a, b| a.name.cmp(&b.name));
    // Studio Pro projects can contain duplicate flow names. Sorting only by
    // name makes their relative order depend on SQLite row order, producing
    // hundreds of false changes after a byte-preserving restore. The
    // normalized body is the stable semantic tie-breaker.
    let mut microflows = module
        .microflows
        .iter()
        .map(flow_summary)
        .collect::<Vec<_>>();
    microflows.sort_by_key(|flow| (name_of(flow), flow.to_string()));
    let mut nanoflows = module
        .nanoflows
        .iter()
        .map(flow_summary)
        .collect::<Vec<_>>();
    nanoflows.sort_by_key(|flow| (name_of(flow), flow.to_string()));
    let mut roles: Vec<_> = module.module_roles.iter().collect();
    roles.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(json!({
        "name": module.name,
        "entities": entities.iter().map(|e| entity_summary(e)).collect::<mxrs_model::Result<Vec<_>>>()?,
        "associations": associations.iter().map(|a| association_summary(a)).collect::<Vec<_>>(),
        "pages": pages.iter().map(|p| page_summary(p)).collect::<Vec<_>>(),
        "menus": menus.iter().map(|m| menu_summary(m)).collect::<Vec<_>>(),
        "module_roles": roles.iter().map(|role| json!({"id": role.id, "name": role.name, "description": role.description})).collect::<Vec<_>>(),
        "microflows": microflows,
        "nanoflows": nanoflows,
    }))
}

fn page_summary(page: &Page) -> Value {
    let mut allowed_roles = page.allowed_module_roles.clone();
    allowed_roles.sort();
    json!({
        "name": page.name,
        "title": page.title,
        "layout": page.layout_id,
        "allowed_roles": allowed_roles,
        "data_source": page.data_source.as_ref().map(|d| bson_to_json(&Bson::Document(d.clone()))),
        "widgets": page.widgets.iter().map(widget_summary).collect::<Vec<_>>(),
    })
}

fn widget_summary(widget: &Widget) -> Value {
    json!({
        "type": widget.widget_type,
        "name": widget.name,
        "options": bson_to_json(&Bson::Document(widget.options.clone())),
        "events": widget.events.iter().map(|e| bson_to_json(&Bson::Document(e.clone()))).collect::<Vec<_>>(),
        "children": widget.children.iter().map(widget_summary).collect::<Vec<_>>(),
    })
}

fn menu_summary(menu: &Menu) -> Value {
    json!({
        "name": menu.name,
        "items": menu.items.iter().map(menu_item_summary).collect::<Vec<_>>(),
    })
}

/// Mirrors `compare.rb#menu_item_summary`'s quirk verbatim: `"name"` is the
/// item's *caption*, not its `Name` field — `MenuItem` carries a `name`
/// field too (mxrs-model reads it), but the oracle deliberately uses the
/// caption for both so this stays a faithful port, not an "improvement".
fn menu_item_summary(item: &MenuItem) -> Value {
    json!({
        "name": item.caption,
        "caption": item.caption,
        "page": item.page,
        "items": item.items.iter().map(menu_item_summary).collect::<Vec<_>>(),
    })
}

/// General BSON → JSON conversion with no dropped keys and no id
/// substitution — unlike [`normalize_flow_value`], which is specifically
/// for microflow bodies (drops volatile presentation fields, rewrites
/// known-object pointers). Used for widget options/events/data sources and
/// the project's security document, none of which have that flow-specific
/// shape.
fn bson_to_json(value: &Bson) -> Value {
    match value {
        Bson::Document(d) => Value::Object(
            d.iter()
                .map(|(k, v)| (k.clone(), bson_to_json(v)))
                .collect(),
        ),
        Bson::Array(items) => Value::Array(items.iter().map(bson_to_json).collect()),
        Bson::String(s) => Value::String(s.clone()),
        Bson::Boolean(b) => Value::Bool(*b),
        Bson::Int32(i) => json!(i),
        Bson::Int64(i) => json!(i),
        Bson::Double(d) => Bson::Double(*d).into_relaxed_extjson(),
        Bson::Null => Value::Null,
        other => other.clone().into_relaxed_extjson(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Ports `compare.rb#security_summary`: reads the raw `Security$ProjectSecurity`
/// unit directly (there's no dedicated Security reader in `mxrs-model`,
/// matching `compare.rb` itself, which doesn't go through a model layer for
/// this either). `None` when the project has no such unit at all.
fn security_summary(project: &Project) -> mxrs_model::Result<Option<Value>> {
    let Some(doc) = project_document(project, "Security$ProjectSecurity")? else {
        return Ok(None);
    };

    let mut demo_users: Vec<Value> = string_array_items(&doc, "DemoUsers")
        .into_iter()
        .filter_map(|b| match b {
            Bson::Document(u) => Some(u),
            _ => None,
        })
        .map(|u| {
            let mut roles = string_list(&u, "UserRoles");
            roles.sort();
            json!({
                "name": u.get_str("UserName").ok(),
                "entity": u.get_str("Entity").ok(),
                "roles": roles,
                "password_sha256": sha256_hex(u.get_str("Password").unwrap_or_default().as_bytes()),
            })
        })
        .collect();
    demo_users.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_string());

    let mut user_roles: Vec<Value> = string_array_items(&doc, "UserRoles")
        .into_iter()
        .filter_map(|b| match b {
            Bson::Document(r) => Some(r),
            _ => None,
        })
        .map(|r| {
            let mut module_roles = string_list(&r, "ModuleRoles");
            module_roles.sort();
            json!({
                "name": r.get_str("Name").ok(),
                "admin": r.get_bool("ManageAllRoles").unwrap_or(false),
                "module_roles": module_roles,
            })
        })
        .collect();
    user_roles.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_string());

    Ok(Some(json!({
        "security_level": doc.get_str("SecurityLevel").ok(),
        "check_security": doc.get_bool("CheckSecurity").ok(),
        "admin_user_name": doc.get_str("AdminUserName").ok(),
        "admin_user_role": doc.get_str("AdminUserRole").ok(),
        "demo_users_enabled": doc.get_bool("EnableDemoUsers").ok(),
        "demo_users": demo_users,
        "guest_access_enabled": doc.get_bool("EnableGuestAccess").ok(),
        "guest_user_role": doc.get_str("GuestUserRole").ok(),
        "sign_in_microflow": doc.get_str("SignInMicroflow").ok(),
        "password_policy": doc.get("PasswordPolicySettings").map(|v| normalize_flow_value(v, &HashMap::new())).unwrap_or(Value::Null),
        "user_roles": user_roles,
    })))
}

fn project_document(project: &Project, type_name: &str) -> mxrs_model::Result<Option<Document>> {
    for unit in project.all_units()? {
        let document = project.mpr().parse_contents(&unit)?;
        if document.get_str("$Type").ok() == Some(type_name) {
            return Ok(Some(document));
        }
    }
    Ok(None)
}

fn navigation_summary(project: &Project) -> mxrs_model::Result<Value> {
    let document = project_document(project, "Navigation$NavigationDocument")?.unwrap_or_default();
    navigation_document_summary(&document)
}

fn navigation_document_summary(document: &Document) -> mxrs_model::Result<Value> {
    let mut profiles = document_items(document.get("Profiles"), "Navigation.Profiles")?;
    if profiles.is_empty() {
        for (key, name) in [
            ("DesktopProfile", "Desktop"),
            ("TabletProfile", "Tablet"),
            ("PhoneProfile", "Phone"),
            ("OfflinePhoneProfile", "OfflinePhone"),
            ("HybridPhoneProfile6", "HybridPhone"),
            ("HybridTabletProfile6", "HybridTablet"),
        ] {
            if let Ok(profile) = document.get_document(key) {
                let mut profile = profile.clone();
                if !profile.contains_key("Name") {
                    profile.insert("Name", name);
                }
                profiles.push(profile);
            }
        }
    }
    let profiles = profiles
        .iter()
        .enumerate()
        .map(|(index, profile)| {
            navigation_profile_summary(profile, &format!("Navigation.Profiles[{index}]"))
        })
        .collect::<mxrs_model::Result<Vec<_>>>()?;
    Ok(json!({ "profiles": profiles }))
}

fn navigation_profile_summary(profile: &Document, path: &str) -> mxrs_model::Result<Value> {
    let (home_key, role_homes) = navigation_field(profile, "HomeItems", "RoleBasedHomePages");
    let role_homes = document_items(role_homes, &format!("{path}.{home_key}"))?;
    let (menu_key, menu) = navigation_field(profile, "Menu", "MenuItemCollection");
    let menu = match menu {
        None | Some(Bson::Null) => None,
        Some(Bson::Document(menu)) => Some(menu),
        Some(_) => {
            return Err(invalid_navigation(
                &format!("{path}.{menu_key}"),
                "document",
            ));
        }
    };
    let items_path = format!("{path}.{menu_key}.Items");
    let items = document_items(menu.and_then(|menu| menu.get("Items")), &items_path)?
        .iter()
        .enumerate()
        .map(|(index, item)| navigation_item_summary(item, &format!("{items_path}[{index}]")))
        .collect::<mxrs_model::Result<Vec<_>>>()?;
    let kind = profile.get_str("Kind").unwrap_or_default();
    Ok(json!({
        "name": profile.get_str("Name").unwrap_or_default(),
        "kind": kind,
        "home_page": profile.get_document("HomePage").ok().and_then(|home| reference_value(home.get("Page"))),
        "home_microflow": profile.get_document("HomePage").ok().and_then(|home| reference_value(home.get("Microflow"))),
        "sign_in_page": profile.get_document("LoginPageSettings").ok().and_then(|login| reference_value(login.get("Form"))),
        "role_homes": role_homes.iter().map(|home| {
            let mut result = serde_json::Map::new();
            for (field, key) in [("role", "UserRole"), ("page", "Page"), ("microflow", "Microflow")] {
                if let Some(value) = reference_value(home.get(key)) { result.insert(field.to_string(), json!(value)); }
            }
            Value::Object(result)
        }).collect::<Vec<_>>(),
        "offline": kind.to_ascii_lowercase().contains("offline"),
        "app_icon": profile.get("AppIcon").map(bson_to_json),
        "app_title": navigation_text(profile.get("AppTitle"), &format!("{path}.AppTitle"))?,
        "items": items,
    }))
}

fn navigation_field<'a>(
    document: &'a Document,
    key: &'a str,
    fallback: &'a str,
) -> (&'a str, Option<&'a Bson>) {
    match document.get(key) {
        Some(value) if !matches!(value, Bson::Null) => (key, Some(value)),
        _ => (fallback, document.get(fallback)),
    }
}

fn reference_value(value: Option<&Bson>) -> Option<String> {
    value
        .and_then(mxrs_bson::extract_id)
        .filter(|reference| !reference.is_empty())
}

fn invalid_navigation(path: &str, expected: &'static str) -> mxrs_model::ModelError {
    mxrs_model::ModelError::InvalidStructure {
        path: path.to_string(),
        expected,
    }
}

/// Collection shape is checked without requiring model identity fields: these
/// documents can also be legal property projections without `$ID` or `$Type`.
fn document_items(value: Option<&Bson>, path: &str) -> mxrs_model::Result<Vec<Document>> {
    match value {
        None | Some(Bson::Null) => Ok(Vec::new()),
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items))
            .items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                if let Bson::Document(document) = item {
                    Ok(document)
                } else {
                    Err(invalid_navigation(&format!("{path}[{index}]"), "document"))
                }
            })
            .collect(),
        Some(_) => Err(invalid_navigation(path, "array of documents")),
    }
}

fn navigation_text(value: Option<&Bson>, path: &str) -> mxrs_model::Result<Value> {
    let Some(document) = value.and_then(Bson::as_document) else {
        return Ok(json!({}));
    };
    let (key, translations) = navigation_field(document, "Translations", "Items");
    let mut values = serde_json::Map::new();
    for translation in document_items(translations, &format!("{path}.{key}"))? {
        values.insert(
            translation
                .get_str("LanguageCode")
                .unwrap_or_default()
                .to_string(),
            json!(translation.get_str("Text").unwrap_or_default()),
        );
    }
    values.retain(|_, value| value != "");
    Ok(Value::Object(values))
}

fn navigation_item_summary(item: &Document, path: &str) -> mxrs_model::Result<Value> {
    let action = item.get_document("Action").ok();
    let mut result = serde_json::Map::new();
    result.insert(
        "caption".to_string(),
        navigation_text(item.get("Caption"), &format!("{path}.Caption"))?,
    );
    for (name, settings, key) in [
        ("page", "FormSettings", "Form"),
        ("microflow", "MicroflowSettings", "Microflow"),
    ] {
        if let Some(reference) = action
            .and_then(|action| action.get_document(settings).ok())
            .and_then(|settings| reference_value(settings.get(key)))
        {
            result.insert(name.to_string(), json!(reference));
        }
    }
    if let Some(code) = item
        .get_document("Icon")
        .ok()
        .and_then(|icon| icon.get("Code"))
        .filter(|value| !matches!(value, Bson::Null))
    {
        result.insert("icon".to_string(), bson_to_json(code));
    }
    result.insert(
        "items".to_string(),
        Value::Array(
            document_items(item.get("Items"), &format!("{path}.Items"))?
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    navigation_item_summary(item, &format!("{path}.Items[{index}]"))
                })
                .collect::<mxrs_model::Result<Vec<_>>>()?,
        ),
    );
    Ok(Value::Object(result))
}

fn string_array_items(doc: &Document, key: &str) -> Vec<Bson> {
    match doc.get(key) {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        _ => Vec::new(),
    }
}

fn string_list(doc: &Document, key: &str) -> Vec<String> {
    string_array_items(doc, key)
        .into_iter()
        .filter_map(|b| match b {
            Bson::String(s) => Some(s),
            _ => None,
        })
        .collect()
}

/// The subset of `Model::DesignSystem::ASSET_DIRECTORIES` this crate needs
/// — directories mxrb scans for theme/widget/library assets, relative to
/// the project root (the `.mpr`'s parent directory).
const ASSET_DIRECTORIES: &[&str] = &[
    "theme",
    "theme-cache",
    "themesource",
    "resources",
    "widgets",
    "javasource",
    "javascriptsource",
    "userlib",
    "vendorlib",
];

/// Ports `compare.rb#design_asset_summary`: SHA-256 of every regular file
/// (symlinks excluded, matching `compare.rb`'s own `!File.symlink?`) under
/// each `ASSET_DIRECTORIES` entry, keyed by its path relative to the
/// project root.
fn design_asset_summary(mpr_path: &Path) -> mxrs_mpr::Result<Value> {
    let root = mpr_path.parent().unwrap_or(Path::new("."));
    let mut assets: Vec<(String, String)> = Vec::new();
    for dir in ASSET_DIRECTORIES {
        let mut files = Vec::new();
        collect_regular_files(&root.join(dir), &mut files)?;
        for file in files {
            let bytes = std::fs::read(&file).map_err(|error| asset_io_error(&file, error))?;
            let relative = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .components()
                .map(|component| {
                    component
                        .as_os_str()
                        .to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            asset_io_error(
                                &file,
                                std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "asset path is not valid UTF-8",
                                ),
                            )
                        })
                })
                .collect::<std::io::Result<Vec<_>>>()?
                .join("/");
            assets.push((relative, sha256_hex(&bytes)));
        }
    }
    assets.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Object(
        assets
            .into_iter()
            .map(|(path, hash)| (path, Value::String(hash)))
            .collect(),
    ))
}

fn asset_io_error(path: &Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn collect_regular_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(asset_io_error(dir, error)),
    };
    for entry in entries {
        let path = entry.map_err(|error| asset_io_error(dir, error))?.path();
        // Native glob ignores hidden entries and never descends into symlinks.
        if path
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
        {
            continue;
        }
        let meta =
            std::fs::symlink_metadata(&path).map_err(|error| asset_io_error(&path, error))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_regular_files(&path, out)?;
        } else if meta.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn entity_summary(entity: &Entity) -> mxrs_model::Result<Value> {
    let mut attributes = entity.attributes.clone();
    attributes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(json!({
        "name": entity.name,
        "documentation": entity.documentation,
        "persistable": entity.persistable,
        "generalization": generalization_summary(entity),
        "access_rules": entity.access_rules.iter().map(|rule| json!({
            "id": rule.id, "roles": rule.roles, "create": rule.create, "delete": rule.delete,
            "documentation": rule.documentation, "default_rights": rule.default_rights,
            "xpath": rule.xpath, "xpath_caption": rule.xpath_caption,
            "members": rule.members.iter().map(|member| json!({
                "id": member.id, "name": member.name, "reference": member.reference, "rights": member.rights,
                "kind": match member.kind { mxrs_model::entity::AccessMemberKind::Attribute => "attribute", mxrs_model::entity::AccessMemberKind::Association => "association" },
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "attributes": attributes.iter().map(|a| -> mxrs_model::Result<Value> { Ok(json!({
            "name": a.name,
            "documentation": a.documentation,
            "type": format!("{:?}", a.attribute_type).to_lowercase(),
            "default": attribute_default(a)?,
            "native_type": a.raw_type_doc.as_ref().map(|doc| normalize_flow_value(&Bson::Document(doc.clone()), &HashMap::new())),
            "native_value": a.raw_value_doc.as_ref().map(|doc| normalize_flow_value(&Bson::Document(doc.clone()), &HashMap::new())),
        })) }).collect::<mxrs_model::Result<Vec<_>>>()?,
    }))
}

fn attribute_default(attribute: &mxrs_model::Attribute) -> mxrs_model::Result<String> {
    let value = attribute.raw_value_doc.as_ref().and_then(|doc| {
        doc.get("defaultValue")
            .filter(|value| !matches!(value, Bson::Null | Bson::Boolean(false)))
            .or_else(|| doc.get("DefaultValue"))
    });
    Ok(match value {
        None | Some(Bson::Null) => String::new(),
        Some(Bson::String(text)) => text.clone(),
        Some(Bson::Boolean(value)) => value.to_string(),
        Some(Bson::Int32(value)) => value.to_string(),
        Some(Bson::Int64(value)) => value.to_string(),
        Some(Bson::Double(value)) => native_float_string(*value),
        Some(_) => {
            return Err(mxrs_model::ModelError::InvalidStructure {
                path: format!(
                    "attribute {} default",
                    attribute.name.as_deref().unwrap_or("(unnamed)")
                ),
                expected: "string, numeric, boolean or null default value",
            });
        }
    })
}

// Ruby's Float#to_s switches to scientific notation at exponents < -4 or
// >= 15, keeps a decimal point and pads exponents to at least two digits.
fn native_float_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity"
        } else {
            "Infinity"
        }
        .into();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("scientific float has exponent");
    let exponent: i32 = exponent.parse().expect("scientific exponent is numeric");
    if !(-4..15).contains(&exponent) {
        let decimal = if mantissa.contains('.') { "" } else { ".0" };
        format!("{mantissa}{decimal}e{exponent:+03}")
    } else {
        format!("{value:?}")
    }
}

fn generalization_summary(entity: &Entity) -> Value {
    let mut result = json!({"Persistable": false, "HasCreatedDateAttr": false, "HasChangedDateAttr": false, "HasOwnerAttr": false, "HasChangedByAttr": false});
    if let Some(generalization) = &entity.generalization {
        let raw =
            normalize_flow_value(&Bson::Document(generalization.raw.clone()), &HashMap::new());
        result
            .as_object_mut()
            .unwrap()
            .extend(raw.as_object().unwrap().clone());
    }
    result
}

fn association_summary(association: &Association) -> Value {
    json!({
        "name": association.name,
        "type": format!("{:?}", association.association_type),
        "owner": format!("{:?}", association.owner),
        "storage_format": format!("{:?}", association.storage_format),
        "delete_behavior": association.delete_behavior.as_ref().map(|doc| normalize_flow_value(&Bson::Document(doc.clone()), &HashMap::new())),
        "documentation": association.documentation,
    })
}

fn flow_summary(flow: &Microflow) -> Value {
    let mut ids: HashMap<String, usize> = HashMap::new();
    assign_flow_ids(&flow.objects, &flow.flows, &mut ids);

    let mut objects: Vec<&Document> = flow.objects.iter().collect();
    objects.sort_by_key(|o| {
        flow_id(o)
            .and_then(|id| ids.get(&id).copied())
            .unwrap_or(ids.len())
    });

    let mut normalized_flows: Vec<Value> = flow
        .flows
        .iter()
        .map(|edge| {
            json!({
                "origin": edge.get("OriginPointer").map(|v| normalize_flow_value(v, &ids)).unwrap_or(Value::Null),
                "destination": edge.get("DestinationPointer").map(|v| normalize_flow_value(v, &ids)).unwrap_or(Value::Null),
                "error_handler": edge.get_bool("IsErrorHandler").unwrap_or(false),
                "cases": normalized_case_values(edge, &ids),
            })
        })
        .collect();
    normalized_flows.sort_by_key(|edge| {
        (
            edge["origin"].to_string(),
            edge["destination"].to_string(),
            edge["error_handler"].to_string(),
            edge["cases"].to_string(),
        )
    });

    let mut allowed_roles = flow.allowed_module_roles.clone();
    allowed_roles.sort();
    json!({
        "name": flow.name,
        "return_type": flow.return_type.as_deref().unwrap_or_default(),
        "allowed_roles": allowed_roles,
        "parameters": flow.parameters.iter().map(|p| bson_to_json(&Bson::Document(p.clone()))).collect::<Vec<_>>(),
        "body": {
            "objects": objects.iter().map(|o| normalize_flow_value(&Bson::Document((*o).clone()), &ids)).collect::<Vec<_>>(),
            "flows": normalized_flows,
        },
    })
}

fn normalized_case_values(edge: &Document, ids: &HashMap<String, usize>) -> Vec<Value> {
    let values = match edge
        .get("CaseValues")
        .filter(|value| !matches!(value, Bson::Null))
    {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        Some(value) => vec![value.clone()],
        None => edge
            .get("NewCaseValue")
            .filter(|value| !matches!(value, Bson::Null))
            .cloned()
            .into_iter()
            .collect(),
    };
    values
        .iter()
        .filter(|value| {
            !value
                .as_document()
                .and_then(|document| document.get_str("$Type").ok())
                .is_some_and(|name| name.ends_with("$NoCase"))
        })
        .map(|value| normalize_flow_value(value, ids))
        .collect()
}

fn flow_id(doc: &Document) -> Option<String> {
    doc.get("$ID").and_then(mxrs_bson::extract_id)
}

/// Ports `Mxrb::Compare::Comparator#assign_flow_ids`: assigns every object
/// (recursing into nested `ObjectCollection`s, e.g. inside a decision's
/// branches) a position-in-traversal index, walked breadth-first from the
/// flow's `StartEvent`(s) plus any object with no incoming edge, then any
/// still-unreached object in declaration order (disconnected components).
fn assign_flow_ids(objects: &[Document], flows: &[Document], ids: &mut HashMap<String, usize>) {
    let local_ids: HashSet<String> = objects.iter().filter_map(flow_id).collect();
    let local_flows: Vec<&Document> = flows
        .iter()
        .filter(|e| {
            let origin = e.get("OriginPointer").and_then(mxrs_bson::extract_id);
            let dest = e.get("DestinationPointer").and_then(mxrs_bson::extract_id);
            origin.is_some_and(|o| local_ids.contains(&o))
                && dest.is_some_and(|d| local_ids.contains(&d))
        })
        .collect();

    let mut outgoing: HashMap<String, Vec<&Document>> = HashMap::new();
    let mut has_incoming: HashSet<String> = HashSet::new();
    for e in &local_flows {
        if let Some(origin) = e.get("OriginPointer").and_then(mxrs_bson::extract_id) {
            outgoing.entry(origin).or_default().push(e);
        }
        if let Some(dest) = e.get("DestinationPointer").and_then(mxrs_bson::extract_id) {
            has_incoming.insert(dest);
        }
    }
    let mut root_ids: HashSet<String> = HashSet::new();
    let mut roots: Vec<&Document> = Vec::new();
    for o in objects {
        if o.get_str("$Type").ok() == Some("Microflows$StartEvent")
            && let Some(id) = flow_id(o)
            && root_ids.insert(id)
        {
            roots.push(o);
        }
    }
    for o in objects {
        if let Some(id) = flow_id(o)
            && !has_incoming.contains(&id)
            && !root_ids.contains(&id)
        {
            root_ids.insert(id);
            roots.push(o);
        }
    }

    let mut queue: VecDeque<&Document> = roots.into_iter().collect();
    while let Some(object) = queue.pop_front() {
        let Some(id) = flow_id(object) else { continue };
        if ids.contains_key(&id) {
            continue;
        }
        ids.insert(id.clone(), ids.len());
        recurse_into_nested(object, flows, ids);

        let mut edges: Vec<&&Document> = outgoing.get(&id).into_iter().flatten().collect();
        edges.sort_by_key(|e| {
            let is_error = e.get_bool("IsErrorHandler").unwrap_or(false);
            (is_error, json!(normalized_case_values(e, ids)).to_string())
        });
        for edge in edges {
            if let Some(dest_id) = edge
                .get("DestinationPointer")
                .and_then(mxrs_bson::extract_id)
                && let Some(target) = objects
                    .iter()
                    .find(|o| flow_id(o).as_deref() == Some(dest_id.as_str()))
            {
                queue.push_back(target);
            }
        }
    }

    for object in objects {
        let Some(id) = flow_id(object) else { continue };
        if ids.contains_key(&id) {
            continue;
        }
        ids.insert(id.clone(), ids.len());
        recurse_into_nested(object, flows, ids);
    }
}

fn recurse_into_nested(object: &Document, flows: &[Document], ids: &mut HashMap<String, usize>) {
    let Ok(oc) = object.get_document("ObjectCollection") else {
        return;
    };
    let Ok(nested_arr) = oc.get_array("Objects") else {
        return;
    };
    let nested: Vec<Document> = mxrs_bson::parse_array(Some(nested_arr))
        .items
        .into_iter()
        .filter_map(|b| match b {
            Bson::Document(d) => Some(d),
            _ => None,
        })
        .collect();
    assign_flow_ids(&nested, flows, ids);
}

const DROPPED_FLOW_KEYS: &[&str] = &[
    "$ID",
    "X",
    "Y",
    "RelativeMiddlePoint",
    "Size",
    "OriginBezierVector",
    "DestinationBezierVector",
    "OriginConnectionIndex",
    "DestinationConnectionIndex",
    "Line",
];

fn normalize_flow_value(value: &Bson, ids: &HashMap<String, usize>) -> Value {
    if let Some(id) = mxrs_bson::extract_id(value)
        && let Some(idx) = ids.get(&id)
    {
        return json!({ "object": idx });
    }

    match value {
        Bson::Document(d) => {
            let mut map = serde_json::Map::new();
            for (k, v) in d {
                if DROPPED_FLOW_KEYS.contains(&k.as_str()) {
                    continue;
                }
                if k.ends_with("Model")
                    && let Bson::Document(inner) = v
                    && inner.get_str("$Type").ok() == Some("Expressions$NoExpression")
                {
                    continue;
                }
                map.insert(k.clone(), normalize_flow_value(v, ids));
            }
            Value::Object(map)
        }
        Bson::Array(items) => {
            let mut ordered: Vec<_> = items.iter().collect();
            if ordered.iter().all(|value| {
                value
                    .as_document()
                    .and_then(flow_id)
                    .is_some_and(|id| ids.contains_key(&id))
            }) {
                ordered.sort_by_key(|value| {
                    value
                        .as_document()
                        .and_then(flow_id)
                        .and_then(|id| ids.get(&id).copied())
                });
            }
            Value::Array(
                ordered
                    .into_iter()
                    .map(|value| normalize_flow_value(value, ids))
                    .collect(),
            )
        }
        Bson::String(s) => Value::String(s.clone()),
        Bson::Boolean(b) => Value::Bool(*b),
        Bson::Int32(i) => json!(i),
        Bson::Int64(i) => json!(i),
        Bson::Double(d) => Bson::Double(*d).into_relaxed_extjson(),
        Bson::Null => Value::Null,
        other => other.clone().into_relaxed_extjson(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub operation: Operation,
    pub path: Vec<String>,
    pub before: Option<Value>,
    pub after: Option<Value>,
    index_positions: Vec<usize>,
}

impl Change {
    /// JSON paths retain numeric indices separately from literal numeric names.
    pub fn json_path(&self) -> Vec<Value> {
        self.path
            .iter()
            .enumerate()
            .map(|(position, part)| {
                if self.index_positions.contains(&position) {
                    json!(
                        part.parse::<usize>()
                            .expect("array index generated by diff")
                    )
                } else {
                    json!(part)
                }
            })
            .collect()
    }

    pub fn format(&self) -> String {
        let path: Vec<_> = self
            .path
            .iter()
            .enumerate()
            .map(|(position, part)| {
                if self.index_positions.contains(&position) {
                    format!("[{part}]")
                } else {
                    part.clone()
                }
            })
            .collect();
        format!(
            "{}: {} != {}",
            path.join("."),
            format_change_value(&self.before),
            format_change_value(&self.after)
        )
    }

    pub fn format_diff(&self) -> String {
        let operation = match self.operation {
            Operation::Added => "added",
            Operation::Removed => "removed",
            Operation::Changed => "changed",
        };
        format!(
            "{operation}\t{}\t{}\t=>\t{}",
            self.path.join("."),
            format_change_value(&self.before),
            format_change_value(&self.after)
        )
    }
}

fn format_change_value(value: &Option<Value>) -> String {
    match value {
        None => "nil".into(),
        Some(value) => {
            let mut canonical = value.clone();
            canonical.sort_all_objects();
            canonical.to_string()
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompareResult {
    pub changes: Vec<Change>,
}

impl CompareResult {
    pub fn is_identical(&self) -> bool {
        self.changes.is_empty()
    }
}

pub fn compare(
    left: impl AsRef<Path>,
    right: impl AsRef<Path>,
) -> mxrs_model::Result<CompareResult> {
    let left_snapshot = snapshot(left)?;
    let right_snapshot = snapshot(right)?;
    Ok(CompareResult {
        changes: diff(&left_snapshot, &right_snapshot),
    })
}

pub fn diff(left: &Value, right: &Value) -> Vec<Change> {
    let mut path = Vec::new();
    diff_values(left, right, &mut path, &mut Vec::new())
}

// Native Ruby compares numeric values, not their BSON/JSON integer-vs-double
// representation. Avoid lossy integer-to-float conversion above 2^53.
fn values_equal(left: &Value, right: &Value) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            let (float, integer) = if left.is_f64() && !right.is_f64() {
                (left, right)
            } else if right.is_f64() && !left.is_f64() {
                (right, left)
            } else {
                return false;
            };
            let float = float.as_f64().expect("floating JSON number");
            let integer = integer
                .as_i64()
                .map(i128::from)
                .or_else(|| integer.as_u64().map(i128::from));
            float.fract() == 0.0 && integer.is_some_and(|integer| float as i128 == integer)
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| values_equal(a, b))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| values_equal(value, other))
                })
        }
        _ => false,
    }
}

fn diff_values(
    left: &Value,
    right: &Value,
    path: &mut Vec<String>,
    indices: &mut Vec<usize>,
) -> Vec<Change> {
    if values_equal(left, right) {
        return vec![];
    }

    match (left, right) {
        (Value::Object(l), Value::Object(r)) => {
            let mut keys: Vec<&String> = l.keys().chain(r.keys()).collect();
            keys.sort();
            keys.dedup();
            keys.into_iter()
                .flat_map(|k| {
                    path.push(k.clone());
                    let result = diff_values(
                        l.get(k).unwrap_or(&Value::Null),
                        r.get(k).unwrap_or(&Value::Null),
                        path,
                        indices,
                    );
                    path.pop();
                    result
                })
                .collect()
        }
        (Value::Array(l), Value::Array(r)) if named_array(l) && named_array(r) => {
            diff_named_arrays(l, r, path, indices)
        }
        (Value::Array(l), Value::Array(r)) => {
            let max = l.len().max(r.len());
            (0..max)
                .flat_map(|i| {
                    indices.push(path.len());
                    path.push(i.to_string());
                    let result = diff_values(
                        l.get(i).unwrap_or(&Value::Null),
                        r.get(i).unwrap_or(&Value::Null),
                        path,
                        indices,
                    );
                    path.pop();
                    indices.pop();
                    result
                })
                .collect()
        }
        _ => vec![Change {
            operation: change_operation(left, right),
            path: path.clone(),
            index_positions: indices.clone(),
            before: (!left.is_null()).then(|| left.clone()),
            after: (!right.is_null()).then(|| right.clone()),
        }],
    }
}

fn change_operation(left: &Value, right: &Value) -> Operation {
    if left.is_null() {
        Operation::Added
    } else if right.is_null() {
        Operation::Removed
    } else {
        Operation::Changed
    }
}

fn named_array(items: &[Value]) -> bool {
    if !items
        .iter()
        .all(|v| matches!(v, Value::Object(m) if m.contains_key("name")))
    {
        return false;
    }
    let mut names: Vec<String> = items.iter().map(name_of).collect();
    let len = names.len();
    names.sort();
    names.dedup();
    names.len() == len
}

fn name_of(v: &Value) -> String {
    v.get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn same_except_name(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => {
            let mut am2 = am.clone();
            am2.remove("name");
            let mut bm2 = bm.clone();
            bm2.remove("name");
            values_equal(&Value::Object(am2), &Value::Object(bm2))
        }
        _ => a == b,
    }
}

/// Ports `Mxrb::Compare::Comparator#diff_named_arrays`: matches items by
/// `"name"` rather than position, with a same-content-except-name pass so a
/// rename reports as one change instead of a spurious remove+add pair.
fn diff_named_arrays(
    left: &[Value],
    right: &[Value],
    path: &mut Vec<String>,
    indices: &mut Vec<usize>,
) -> Vec<Change> {
    let mut left_by_name: Vec<(String, Value)> =
        left.iter().map(|v| (name_of(v), v.clone())).collect();
    let mut right_by_name: Vec<(String, Value)> =
        right.iter().map(|v| (name_of(v), v.clone())).collect();
    let mut changes = Vec::new();

    let mut common_names: Vec<String> = left_by_name
        .iter()
        .filter(|(n, _)| right_by_name.iter().any(|(rn, _)| rn == n))
        .map(|(n, _)| n.clone())
        .collect();
    common_names.sort();
    for name in &common_names {
        let l = left_by_name
            .iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
            .clone();
        let r = right_by_name
            .iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
            .clone();
        path.push(name.clone());
        changes.extend(diff_values(&l, &r, path, indices));
        path.pop();
    }
    left_by_name.retain(|(n, _)| !common_names.contains(n));
    right_by_name.retain(|(n, _)| !common_names.contains(n));
    left_by_name.sort_by(|a, b| a.0.cmp(&b.0));
    right_by_name.sort_by(|a, b| a.0.cmp(&b.0));

    let left_only_snapshot = left_by_name.clone();
    for (name, value) in &left_only_snapshot {
        if let Some(pos) = right_by_name
            .iter()
            .position(|(_, rv)| same_except_name(value, rv))
        {
            let (_, rvalue) = right_by_name.remove(pos);
            path.push(name.clone());
            changes.extend(diff_values(value, &rvalue, path, indices));
            path.pop();
            left_by_name.retain(|(n, _)| n != name);
        }
    }

    left_by_name.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, value) in left_by_name {
        path.push(name);
        changes.extend(diff_values(&value, &Value::Null, path, indices));
        path.pop();
    }
    right_by_name.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, value) in right_by_name {
        path.push(name);
        changes.extend(diff_values(&Value::Null, &value, path, indices));
        path.pop();
    }
    changes
}

#[cfg(test)]
#[path = "compare_parity_tests.rs"]
mod parity_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(path: &std::path::Path, configure: impl FnOnce(&mut mxrs_dsl::ModuleBuilder)) {
        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", configure);
        mxrs_writer::write_project(path, &project.build()).unwrap();
    }

    #[test]
    fn an_identical_project_compares_clean() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        write_fixture(&left, |m| {
            m.entity("Order", |e| {
                e.string("Number");
            });
        });
        write_fixture(&right, |m| {
            m.entity("Order", |e| {
                e.string("Number");
            });
        });

        let result = compare(&left, &right).unwrap();
        assert!(
            result.is_identical(),
            "unexpected changes: {:?}",
            result.changes
        );
    }

    #[test]
    fn a_changed_attribute_default_is_reported_by_name_not_position() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        write_fixture(&left, |m| {
            m.entity("Order", |e| {
                e.string("Number").default_value = Some("A-0".into());
            });
        });
        write_fixture(&right, |m| {
            m.entity("Order", |e| {
                e.string("Number").default_value = Some("A-1".into());
            });
        });

        let result = compare(&left, &right).unwrap();
        assert!(!result.is_identical());
        let change = result
            .changes
            .iter()
            .find(|c| c.path.contains(&"default".to_string()))
            .unwrap();
        assert_eq!(change.operation, Operation::Changed);
        assert!(change.path.contains(&"Number".to_string()));
    }

    #[test]
    fn an_added_entity_is_reported_as_added() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        write_fixture(&left, |m| {
            m.entity("Order", |_e| {});
        });
        write_fixture(&right, |m| {
            m.entity("Order", |_e| {});
            m.entity("Customer", |_e| {});
        });

        let result = compare(&left, &right).unwrap();
        assert!(
            result.changes.iter().any(
                |c| c.operation == Operation::Added && c.path.contains(&"Customer".to_string())
            )
        );
    }

    #[test]
    fn equivalent_microflows_compare_equal_despite_fresh_ids() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        let mk_flow = |m: &mut mxrs_dsl::ModuleBuilder| {
            m.microflow("ACT_Do", |f| {
                f.return_value(mxrs_dsl::integer(1));
            });
        };
        let mut left_project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        left_project.module("Sales", |m| {
            m.entity("Order", |_e| {});
            mk_flow(m);
        });
        mxrs_writer::write_project(&left, &left_project.build()).unwrap();
        let mut right_project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        right_project.module("Sales", |m| {
            m.entity("Order", |_e| {});
            mk_flow(m);
        });
        mxrs_writer::write_project(&right, &right_project.build()).unwrap();

        let result = compare(&left, &right).unwrap();
        assert!(
            result.is_identical(),
            "unexpected changes: {:?}",
            result.changes
        );
    }

    #[test]
    fn duplicate_flow_names_are_compared_by_normalized_body_not_row_order() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("Left.mpr");
        let right = dir.path().join("Right.mpr");
        write_fixture(&left, |_module| {});
        write_fixture(&right, |_module| {});

        let add_duplicates = |path: &Path, documentation: [&str; 2]| {
            let mut mpr = mxrs_mpr::MprFile::open(path, false).unwrap();
            let module_id = mpr.units_by_containment("Modules").unwrap()[0]
                .unit_id
                .clone();
            for value in documentation {
                mpr.insert_unit(
                    &module_id,
                    "Documents",
                    mxrs_bson::doc! {
                        "$Type": "Microflows$Microflow",
                        "Name": "Duplicate",
                        "Documentation": value,
                    },
                    None,
                )
                .unwrap();
            }
        };
        add_duplicates(&left, ["first", "second"]);
        add_duplicates(&right, ["second", "first"]);

        let result = compare(&left, &right).unwrap();
        assert!(
            result.is_identical(),
            "duplicate names produced false changes: {:?}",
            result.changes
        );
    }

    #[test]
    fn a_fresh_project_has_the_default_administrator_security_summary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Fresh.mpr");
        write_fixture(&path, |m| {
            m.entity("Order", |_e| {});
        });

        let snap = snapshot(&path).unwrap();
        let security = &snap["security"];
        assert!(!security.is_null());
        assert_eq!(security["security_level"], "CheckNothing");
        assert_eq!(security["admin_user_role"], "Administrator");
        let roles = security["user_roles"].as_array().unwrap();
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0]["name"], "Administrator");
        assert_eq!(roles[0]["admin"], true);
    }

    #[test]
    fn a_changed_theme_asset_is_reported_as_a_difference() {
        let dir = tempfile::tempdir().unwrap();
        let left_dir = dir.path().join("left");
        let right_dir = dir.path().join("right");
        std::fs::create_dir_all(left_dir.join("theme")).unwrap();
        std::fs::create_dir_all(right_dir.join("theme")).unwrap();
        let left = left_dir.join("Project.mpr");
        let right = right_dir.join("Project.mpr");
        write_fixture(&left, |m| {
            m.entity("Order", |_e| {});
        });
        write_fixture(&right, |m| {
            m.entity("Order", |_e| {});
        });
        std::fs::write(left_dir.join("theme/main.css"), "body { color: red; }").unwrap();
        std::fs::write(right_dir.join("theme/main.css"), "body { color: blue; }").unwrap();

        let result = compare(&left, &right).unwrap();
        assert!(
            result
                .changes
                .iter()
                .any(|c| c.path.contains(&"design_assets".to_string()))
        );
    }

    #[test]
    fn identical_theme_assets_compare_clean_and_a_symlink_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join("proj");
        std::fs::create_dir_all(project_dir.join("theme")).unwrap();
        let path = project_dir.join("Project.mpr");
        write_fixture(&path, |m| {
            m.entity("Order", |_e| {});
        });
        std::fs::write(project_dir.join("theme/main.css"), "body {}").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                project_dir.join("theme/main.css"),
                project_dir.join("theme/link.css"),
            )
            .unwrap();
        }

        let snap = snapshot(&path).unwrap();
        let assets = snap["design_assets"].as_object().unwrap();
        assert!(assets.contains_key("theme/main.css"));
        assert!(!assets.contains_key("theme/link.css"));
    }
}
