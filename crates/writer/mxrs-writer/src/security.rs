//! Loss-preserving persistence for Cargo-native project and module security.

use std::collections::{HashMap, HashSet};

use mxrs_bson::{Bson, Document, build_array, doc, parse_array};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::{ModuleRoleDecl, ProjectSecurityDecl, SecurityLevel};
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
            let id = document
                .get_str("$ID")
                .ok()
                .map(str::to_string)
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
                .get_str("$ID")
                .ok()
                .map(str::to_string)
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
    document.insert("CheckSecurity", true);
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

    let mut policy = document
        .get_document("PasswordPolicySettings")
        .cloned()
        .unwrap_or_default();
    let policy_id = policy
        .get_str("$ID")
        .ok()
        .map(str::to_string)
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

fn validate_project_security(mpr: &MprFile, declaration: &ProjectSecurityDecl) -> Result<()> {
    ensure_unique(
        declaration.user_roles.iter().map(|role| role.name.as_str()),
        "user role",
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
    for unit in mpr.all_units()? {
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
