//! Loss-preserving persistence for Cargo-native project and module security.

use std::collections::{HashMap, HashSet};

use mxrs_bson::{Bson, Document, build_array, doc, parse_array};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::{DemoUserDecl, ModuleRoleDecl, ProjectDecl, ProjectSecurityDecl, SecurityLevel};
use mxrs_mpr::MprFile;

use crate::error::{Result, WriterError};

pub(crate) fn synchronize_module_security(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    roles: &[ModuleRoleDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    let existing = mpr.children_of(module_id)?.into_iter().find_map(|unit| {
        let document = mpr.parse_contents(&unit).ok()?;
        (document.get_str("$Type").ok() == Some("Security$ModuleSecurity"))
            .then_some((unit, document))
    });
    let previous_roles = existing
        .as_ref()
        .map(|(_, document)| documents_by_name(document, "ModuleRoles"))
        .unwrap_or_default();

    ensure_unique(roles.iter().map(|role| role.name.as_str()), "module role")?;
    let role_documents = roles
        .iter()
        .map(|role| {
            let mut document = previous_roles.get(&role.name).cloned().unwrap_or_default();
            // `$ID` is stored as a binary UUID in real projects — `get_str`
            // would miss it and mint a fresh identity on every rebuild.
            let id = document
                .get("$ID")
                .and_then(mxrs_bson::extract_id)
                .unwrap_or_else(|| {
                    identity.artifact_id(
                        ArtifactKind::ModuleRole,
                        &format!("{module_name}.{}", role.name),
                    )
                });
            document.insert("$ID", id);
            document.insert("$Type", "Security$ModuleRole");
            document.insert("Name", role.name.clone());
            document.insert("Description", role.description.clone());
            Bson::Document(document)
        })
        .collect();

    let (unit_id, mut document) = match existing {
        Some((unit, document)) => (unit.unit_id, document),
        None => {
            let id = identity.artifact_id(ArtifactKind::ModuleSecurity, module_name);
            (
                id.clone(),
                doc! { "$ID": id, "$Type": "Security$ModuleSecurity" },
            )
        }
    };
    document.insert("$ID", unit_id.clone());
    document.insert("$Type", "Security$ModuleSecurity");
    document.insert("ModuleRoles", build_array(role_documents, 2));
    if mpr.unit(&unit_id)?.is_some() {
        mpr.update_unit(&unit_id, document)?;
    } else {
        mpr.insert_unit(module_id, "ModuleSecurity", document, Some(&unit_id))?;
    }
    Ok(())
}

/// Applies what `project` says about security.
///
/// A declared security is authoritative, and demo users declared apart from
/// it are part of it. Without one the stored security is preserved as it is
/// — except that demo users declared on their own still join it, because a
/// declaration the build accepted and then ignored would be a silent loss.
pub(crate) fn synchronize_declared_security(
    mpr: &mut MprFile,
    root_id: &str,
    project: &ProjectDecl,
    identity: ProjectIdentity,
) -> Result<()> {
    match &project.security {
        Some(declaration) if project.demo_users.is_empty() => {
            synchronize_project_security(mpr, root_id, declaration, identity)
        }
        Some(declaration) => {
            let mut declaration = declaration.clone();
            declaration
                .demo_users
                .extend(project.demo_users.iter().cloned());
            synchronize_project_security(mpr, root_id, &declaration, identity)
        }
        None if project.demo_users.is_empty() => Ok(()),
        None => join_stored_security(mpr, root_id, &project.demo_users, identity),
    }
}

/// Adds `demo_users` to the project security the model stores, changing
/// nothing else in it.
fn join_stored_security(
    mpr: &mut MprFile,
    root_id: &str,
    demo_users: &[DemoUserDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    ensure_unique(
        demo_users.iter().map(|user| user.name.as_str()),
        "demo user",
    )?;
    let stored = mpr.children_of(root_id)?.into_iter().find_map(|unit| {
        let document = mpr.parse_contents(&unit).ok()?;
        (document.get_str("$Type").ok() == Some("Security$ProjectSecurity"))
            .then_some((unit.unit_id, document))
    });
    let Some((unit_id, mut document)) = stored else {
        let mut names = demo_users
            .iter()
            .map(|user| user.name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        return Err(WriterError::DemoUsersWithoutProjectSecurity {
            names: names.join(", "),
        });
    };
    let stored_roles = documents_by_name(&document, "UserRoles");
    for user in demo_users {
        for role in &user.roles {
            if !stored_roles.contains_key(role) {
                return Err(WriterError::UnknownUserRole(role.clone()));
            }
        }
    }
    let lowered = lower_demo_users(&document, demo_users, &identity)?;
    document.insert("DemoUsers", lowered);
    mpr.update_unit(&unit_id, document)?;
    Ok(())
}

pub(crate) fn synchronize_project_security(
    mpr: &mut MprFile,
    root_id: &str,
    declaration: &ProjectSecurityDecl,
    identity: ProjectIdentity,
) -> Result<()> {
    validate_project_security(mpr, declaration)?;
    let existing = mpr.children_of(root_id)?.into_iter().find_map(|unit| {
        let document = mpr.parse_contents(&unit).ok()?;
        (document.get_str("$Type").ok() == Some("Security$ProjectSecurity"))
            .then_some((unit, document))
    });
    let previous_roles = existing
        .as_ref()
        .map(|(_, document)| documents_by_name(document, "UserRoles"))
        .unwrap_or_default();

    let roles = declaration
        .user_roles
        .iter()
        .map(|role| {
            let mut document = previous_roles.get(&role.name).cloned().unwrap_or_default();
            let id = document
                .get("$ID")
                .and_then(mxrs_bson::extract_id)
                .unwrap_or_else(|| identity.artifact_id(ArtifactKind::UserRole, &role.name));
            let guid = document.get("GUID").cloned().unwrap_or_else(|| {
                Bson::String(
                    identity.artifact_id(ArtifactKind::UserRole, &format!("{}.guid", role.name)),
                )
            });
            document.insert("$ID", id);
            document.insert("$Type", "Security$UserRole");
            document.insert("Name", role.name.clone());
            document.insert("Description", role.description.clone());
            document.insert("CheckSecurity", role.check_security);
            document.insert("GUID", guid);
            document.insert(
                "ManageableRoles",
                build_array(
                    role.manageable_roles
                        .iter()
                        .cloned()
                        .map(Bson::String)
                        .collect(),
                    1,
                ),
            );
            document.insert("ManageAllRoles", role.administrator);
            document.insert("ManageUsersWithoutRoles", role.manage_users_without_roles);
            document.insert(
                "ModuleRoles",
                build_array(
                    role.module_roles
                        .iter()
                        .cloned()
                        .map(Bson::String)
                        .collect(),
                    1,
                ),
            );
            Bson::Document(document)
        })
        .collect();

    let (unit_id, mut document) = match existing {
        Some((unit, document)) => (unit.unit_id, document),
        None => {
            let id = identity.artifact_id(ArtifactKind::ProjectSecurity, "project");
            (
                id.clone(),
                doc! { "$ID": id, "$Type": "Security$ProjectSecurity" },
            )
        }
    };
    document.insert("$ID", unit_id.clone());
    document.insert("$Type", "Security$ProjectSecurity");
    document.insert("SecurityLevel", security_level(declaration.level));
    document.insert("CheckSecurity", declaration.check_security);
    document.insert("AdminUserName", "MxAdmin");
    document.insert("AdminPassword", "1");
    document.insert("AdminUserRole", declaration.admin_user_role.clone());
    document.insert("EnableGuestAccess", declaration.guest_user_role.is_some());
    document.insert(
        "GuestUserRole",
        declaration.guest_user_role.clone().unwrap_or_default(),
    );
    match &declaration.sign_in_microflow {
        Some(value) => {
            document.insert("SignInMicroflow", value.clone());
        }
        None => {
            document.remove("SignInMicroflow");
        }
    }
    document.insert("UserRoles", build_array(roles, 2));

    // An empty declaration list leaves the stored `DemoUsers` bytes untouched
    // (lossless preservation); MXRB rewrites the array unconditionally, but a
    // byte-preserving round-trip must not reserialize content nobody declared.
    if !declaration.demo_users.is_empty() {
        let lowered = lower_demo_users(&document, &declaration.demo_users, &identity)?;
        document.insert("DemoUsers", lowered);
    }

    let mut policy = document
        .get_document("PasswordPolicySettings")
        .cloned()
        .unwrap_or_default();
    let policy_id = policy
        .get("$ID")
        .and_then(mxrs_bson::extract_id)
        .unwrap_or_else(|| identity.artifact_id(ArtifactKind::PasswordPolicy, "project"));
    policy.insert("$ID", policy_id);
    policy.insert("$Type", "Security$PasswordPolicySettings");
    policy.insert("MinimumLength", declaration.password_policy.minimum_length);
    policy.insert("RequireDigit", declaration.password_policy.require_digit);
    policy.insert(
        "RequireMixedCase",
        declaration.password_policy.require_mixed_case,
    );
    policy.insert("RequireSymbol", declaration.password_policy.require_symbol);
    document.insert("PasswordPolicySettings", policy);

    if mpr.unit(&unit_id)?.is_some() {
        mpr.update_unit(&unit_id, document)?;
    } else {
        mpr.insert_unit(root_id, "ProjectDocuments", document, Some(&unit_id))?;
    }
    Ok(())
}

/// Lowers declared demo users into the stored `DemoUsers` array with MXRB's
/// merge semantics: entries other than `Security$DemoUserImpl` are opaque and
/// preserved verbatim, an entry whose `UserName` matches a declaration keeps
/// its identity and any field this slice does not manage, and passwords are
/// resolved from the declared environment variable — never from source. When
/// the variable is absent an existing password is preserved; a brand-new user
/// without a resolvable password fails closed.
fn lower_demo_users(
    document: &Document,
    demo_users: &[DemoUserDecl],
    identity: &ProjectIdentity,
) -> Result<Bson> {
    let raw = match document.get("DemoUsers") {
        Some(Bson::Array(items)) => Some(items.as_slice()),
        _ => None,
    };
    let payload = parse_array(raw);
    let (supported, opaque): (Vec<Bson>, Vec<Bson>) = payload.items.into_iter().partition(|item| {
        matches!(
            item,
            Bson::Document(user) if user.get_str("$Type").ok() == Some("Security$DemoUserImpl")
        )
    });
    let mut by_name: HashMap<String, Vec<&Document>> = HashMap::new();
    for item in &supported {
        if let Bson::Document(user) = item {
            by_name
                .entry(user.get_str("UserName").unwrap_or_default().to_string())
                .or_default()
                .push(user);
        }
    }
    // mxrb's `MemberIdentity#reject_ambiguous_changes!` refuses prior
    // members no declaration accounts for; the equivalent here is refusing
    // stored `DemoUserImpl` entries the declared list would silently drop
    // (the exporter deliberately emits no demo-user declarations, so an
    // imported project would otherwise lose its users on the first build).
    let declared: HashSet<&str> = demo_users.iter().map(|user| user.name.as_str()).collect();
    let mut orphaned: Vec<String> = by_name
        .keys()
        .filter(|name| !declared.contains(name.as_str()))
        .cloned()
        .collect();
    if !orphaned.is_empty() {
        orphaned.sort();
        return Err(WriterError::UndeclaredStoredDemoUsers {
            names: orphaned.join(", "),
        });
    }
    let mut users = Vec::with_capacity(demo_users.len());
    for user in demo_users {
        let matches = by_name.get(user.name.as_str());
        let prior = match matches.map(Vec::as_slice) {
            Some([single]) => (*single).clone(),
            _ => Document::new(),
        };
        let resolved = user
            .password_env
            .as_deref()
            .and_then(|variable| std::env::var(variable).ok())
            .filter(|value| !value.is_empty());
        let password = match resolved {
            Some(value) => value,
            None => {
                let stored = prior.get_str("Password").unwrap_or_default().to_string();
                if stored.is_empty() {
                    return Err(WriterError::MissingDemoUserPassword {
                        name: user.name.clone(),
                        variable: user
                            .password_env
                            .clone()
                            .unwrap_or_else(|| "a password_from_env variable".to_string()),
                    });
                }
                stored
            }
        };
        let role_marker = match prior.get("UserRoles") {
            Some(Bson::Array(items)) => parse_array(Some(items.as_slice())).marker,
            _ => 1,
        };
        let mut lowered = prior.clone();
        // Stored `$ID`s may be UUID binaries, not strings — decode them the
        // way every other identity-preserving path here does.
        let id = prior
            .get("$ID")
            .and_then(mxrs_bson::extract_id)
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::DemoUser, &user.name));
        lowered.insert("$ID", id);
        lowered.insert("$Type", "Security$DemoUserImpl");
        lowered.insert("UserName", user.name.clone());
        lowered.insert("Password", password);
        lowered.insert("Entity", user.entity.clone());
        lowered.insert(
            "UserRoles",
            build_array(
                user.roles.iter().cloned().map(Bson::String).collect(),
                role_marker,
            ),
        );
        users.push(Bson::Document(lowered));
    }
    users.extend(opaque);
    Ok(Bson::Array(build_array(users, payload.marker)))
}

fn validate_project_security(mpr: &MprFile, declaration: &ProjectSecurityDecl) -> Result<()> {
    ensure_unique(
        declaration.user_roles.iter().map(|role| role.name.as_str()),
        "user role",
    )?;
    ensure_unique(
        declaration.demo_users.iter().map(|user| user.name.as_str()),
        "demo user",
    )?;
    let role_names = declaration
        .user_roles
        .iter()
        .map(|role| role.name.as_str())
        .collect::<HashSet<_>>();
    if !role_names.contains(declaration.admin_user_role.as_str()) {
        return Err(WriterError::UnknownUserRole(
            declaration.admin_user_role.clone(),
        ));
    }
    if let Some(guest) = &declaration.guest_user_role
        && !role_names.contains(guest.as_str())
    {
        return Err(WriterError::UnknownUserRole(guest.clone()));
    }
    for role in &declaration.user_roles {
        for manageable in &role.manageable_roles {
            if !role_names.contains(manageable.as_str()) {
                return Err(WriterError::UnknownUserRole(manageable.clone()));
            }
        }
    }

    for referenced in demo_user_roles(mpr)? {
        if !role_names.contains(referenced.as_str()) {
            return Err(WriterError::UnknownUserRole(referenced));
        }
    }
    for user in &declaration.demo_users {
        for role in &user.roles {
            if !role_names.contains(role.as_str()) {
                return Err(WriterError::UnknownUserRole(role.clone()));
            }
        }
    }

    let known_module_roles = module_roles(mpr)?;
    for role in &declaration.user_roles {
        for module_role in &role.module_roles {
            if !module_role.starts_with("System.") && !known_module_roles.contains(module_role) {
                return Err(WriterError::UnknownModuleRole(module_role.clone()));
            }
        }
    }
    Ok(())
}

fn demo_user_roles(mpr: &MprFile) -> Result<HashSet<String>> {
    let mut roles = HashSet::new();
    let root_id = mpr
        .root_unit()?
        .ok_or(WriterError::MissingRootUnit)?
        .unit_id;
    for unit in mpr.children_of(&root_id)? {
        let document = mpr.parse_contents(&unit)?;
        if document.get_str("$Type").ok() != Some("Security$ProjectSecurity") {
            continue;
        }
        let Some(Bson::Array(users)) = document.get("DemoUsers") else {
            continue;
        };
        for user in parse_array(Some(users)).items {
            let Bson::Document(user) = user else {
                continue;
            };
            let Some(Bson::Array(user_roles)) = user.get("UserRoles") else {
                continue;
            };
            for role in parse_array(Some(user_roles)).items {
                if let Bson::String(role) = role {
                    roles.insert(role);
                }
            }
        }
    }
    Ok(roles)
}

fn module_roles(mpr: &MprFile) -> Result<HashSet<String>> {
    let mut names = HashSet::new();
    for module in mpr.units_by_containment("Modules")? {
        let module_document = mpr.parse_contents(&module)?;
        let Ok(module_name) = module_document.get_str("Name") else {
            continue;
        };
        for unit in mpr.children_of(&module.unit_id)? {
            let document = mpr.parse_contents(&unit)?;
            if document.get_str("$Type").ok() != Some("Security$ModuleSecurity") {
                continue;
            }
            for role in documents_by_name(&document, "ModuleRoles").keys() {
                names.insert(format!("{module_name}.{role}"));
            }
        }
    }
    Ok(names)
}

fn documents_by_name(document: &Document, field: &str) -> HashMap<String, Document> {
    let Some(Bson::Array(values)) = document.get(field) else {
        return HashMap::new();
    };
    parse_array(Some(values))
        .items
        .into_iter()
        .filter_map(|value| match value {
            Bson::Document(document) => {
                let name = document.get_str("Name").ok()?.to_string();
                Some((name, document))
            }
            _ => None,
        })
        .collect()
}

fn ensure_unique<'a>(names: impl Iterator<Item = &'a str>, kind: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(WriterError::DuplicateSecurityName {
                kind: kind.to_string(),
                name: name.to_string(),
            });
        }
    }
    Ok(())
}

fn security_level(level: SecurityLevel) -> &'static str {
    match level {
        SecurityLevel::CheckNothing => "CheckNothing",
        SecurityLevel::CheckFormsAndMicroflows => "CheckFormsAndMicroflows",
        SecurityLevel::CheckEverything => "CheckEverything",
    }
}
