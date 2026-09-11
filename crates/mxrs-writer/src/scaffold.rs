//! Fresh-project scaffolding: the `Settings$ProjectSettings`/
//! `Texts$SystemTextCollection`/`Projects$ProjectConversion` baseline (from
//! `mxrs_schema::project_template_units`, sanitized) plus a default,
//! DSL-empty `Security$ProjectSecurity` and `Navigation$NavigationDocument`.
//! Without these, a `mxrs-writer`-authored `.mpr` has no security/navigation/
//! settings units at all and Studio Pro can't open it — this was Phase 3's
//! flagged known gap. Ports the fresh-project slice of `Writer#apply`'s
//! `apply_default_project_units`/`ensure_project_documents`/
//! `sanitize_project_settings!`/`project_security_doc`/`user_role_doc`/
//! `modern_navigation_doc`, narrowed the same way as the rest of this crate:
//! no `previous`/merge handling (that's the incremental re-sync path, out of
//! scope here), and no DSL surface yet for customizing security/navigation
//! content — the DSL only asks for these units to *exist* with sane
//! defaults, matching what `mxrb generate` produces for a brand-new project.

use mxrs_bson::{Bson, Document, doc};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_mpr::MprFile;
use mxrs_schema::TemplateUnit;

use crate::error::Result;

/// Inserts the version's template units (Settings/SystemTexts/
/// ProjectConversion) plus a default Security and Navigation document as
/// direct children of the project root. Call once per fresh `.mpr`, after
/// the root unit exists and before/independent of module writes.
pub fn write_default_project_units(
    mpr: &mut MprFile,
    root_id: &str,
    version: &str,
    identity: ProjectIdentity,
) -> Result<()> {
    let units = mxrs_schema::project_template_units(version)?;
    for TemplateUnit {
        containment,
        mut doc,
    } in units
    {
        if doc.get_str("$Type").ok() == Some("Settings$ProjectSettings") {
            sanitize_project_settings(&mut doc, version);
        }
        let kind = match doc.get_str("$Type").unwrap_or_default() {
            "Settings$ProjectSettings" => ArtifactKind::ProjectSettings,
            "Projects$ProjectConversion" => ArtifactKind::ProjectConversion,
            "Texts$SystemTextCollection" => ArtifactKind::SystemTexts,
            other => unreachable!("unexpected project template unit {other}"),
        };
        let id = identity.artifact_id(kind, "project");
        doc.insert("$ID", id.clone());
        mpr.insert_unit(root_id, &containment, doc, Some(&id))?;
    }

    let security_id = identity.artifact_id(ArtifactKind::ProjectSecurity, "project");
    mpr.insert_unit(
        root_id,
        "ProjectDocuments",
        default_security_doc(),
        Some(&security_id),
    )?;

    let navigation_id = identity.artifact_id(ArtifactKind::Navigation, "project");
    mpr.insert_unit(
        root_id,
        "ProjectDocuments",
        default_navigation_doc(),
        Some(&navigation_id),
    )?;

    Ok(())
}

/// Ports `Writer#sanitize_project_settings!`: a project template is
/// structural seed data copied from a donor Studio Pro project, not this
/// project's own application baseline, so donor-specific lifecycle
/// callbacks and client-mode quirks must not leak into a freshly authored
/// project.
fn sanitize_project_settings(document: &mut Document, version: &str) {
    let Some(Bson::Array(settings)) = document.get("Settings").cloned() else {
        return;
    };
    let parsed = mxrs_bson::parse_array(Some(&settings));
    let major_version: i64 = version
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let items: Vec<Bson> = parsed
        .items
        .into_iter()
        .map(|item| {
            let Bson::Document(mut setting) = item else {
                return item;
            };
            match setting.get_str("$Type").ok() {
                Some("Forms$WebUIProjectSettingsPart") => {
                    setting.insert("EnableNewStringBehavior", true);
                    if major_version == 10 {
                        setting.insert("UseOptimizedClient", "Yes");
                    }
                    if setting.contains_key("ThemeModuleName") {
                        setting.insert("ThemeModuleName", "");
                    }
                }
                Some("Settings$ModelSettings") => {
                    for key in [
                        "AfterStartupMicroflow",
                        "BeforeShutdownMicroflow",
                        "HealthCheckMicroflow",
                    ] {
                        if setting.contains_key(key) {
                            setting.insert(key, "");
                        }
                    }
                }
                Some("Settings$LanguageSettings") => {
                    let default_code = setting
                        .get_str("DefaultLanguageCode")
                        .unwrap_or_default()
                        .to_string();
                    if let Some(Bson::Array(languages)) = setting.get("Languages").cloned() {
                        let parsed_languages = mxrs_bson::parse_array(Some(&languages));
                        let updated: Vec<Bson> = parsed_languages
                            .items
                            .into_iter()
                            .map(|lang| {
                                let Bson::Document(mut lang_doc) = lang else {
                                    return lang;
                                };
                                if lang_doc.get_str("Code").ok() != Some(default_code.as_str()) {
                                    lang_doc.insert("CheckCompleteness", false);
                                }
                                Bson::Document(lang_doc)
                            })
                            .collect();
                        setting.insert(
                            "Languages",
                            mxrs_bson::build_array(updated, parsed_languages.marker),
                        );
                    }
                }
                _ => {}
            }
            Bson::Document(setting)
        })
        .collect();

    document.insert("Settings", mxrs_bson::build_array(items, parsed.marker));
}

/// Ports `Writer#project_security_doc({})`: `SecurityLevel: CheckNothing`,
/// a single default `Administrator` role, no demo users, no guest access.
fn default_security_doc() -> Document {
    doc! {
        "$Type": "Security$ProjectSecurity",
        "SecurityLevel": "CheckNothing",
        "CheckSecurity": true,
        "AdminUserName": "MxAdmin",
        "AdminPassword": "1",
        "AdminUserRole": "Administrator",
        "EnableDemoUsers": false,
        "EnableGuestAccess": false,
        "GuestUserRole": "",
        "StrictMode": false,
        "StrictPageUrlCheck": true,
        "UserRoles": mxrs_bson::build_array(vec![Bson::Document(default_admin_role_doc())], 2),
        "DemoUsers": mxrs_bson::build_array(vec![], 2),
        "FileDocumentAccess": access_container_doc("Security$FileDocumentAccessRuleContainer"),
        "ImageAccess": access_container_doc("Security$ImageAccessRuleContainer"),
        "PasswordPolicySettings": doc! {
            "$Type": "Security$PasswordPolicySettings",
            "MinimumLength": 6,
            "RequireDigit": true,
            "RequireMixedCase": true,
            "RequireSymbol": false,
        },
    }
}

/// Ports `Writer#user_role_doc({name: "Administrator", admin: true, module_roles: []})`.
fn default_admin_role_doc() -> Document {
    doc! {
        "$Type": "Security$UserRole",
        "Name": "Administrator",
        "Description": "",
        "CheckSecurity": true,
        "GUID": uuid::Uuid::new_v4().to_string(),
        "ManageableRoles": mxrs_bson::build_array(vec![], 1),
        "ManageAllRoles": true,
        "ManageUsersWithoutRoles": false,
        "ModuleRoles": mxrs_bson::build_array(vec![Bson::String("System.Administrator".into())], 1),
    }
}

fn access_container_doc(bson_type: &str) -> Document {
    doc! {
        "$Type": bson_type,
        "AccessRules": mxrs_bson::build_array(vec![], 3),
    }
}

/// Ports `Writer#modern_navigation_doc({}, profiles: [])`: no profiles
/// declared yet — the DSL has no navigation surface in this pass, so this
/// is deliberately the empty baseline `Writer#ensure_project_documents`
/// falls back to for the same case.
fn default_navigation_doc() -> Document {
    doc! {
        "$Type": "Navigation$NavigationDocument",
        "Profiles": mxrs_bson::build_array(vec![], 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clears_lifecycle_callbacks_on_model_settings() {
        let mut settings = mxrs_schema::project_template_units("11.12.1")
            .unwrap()
            .into_iter()
            .find(|u| u.doc.get_str("$Type").ok() == Some("Settings$ProjectSettings"))
            .unwrap()
            .doc;
        sanitize_project_settings(&mut settings, "11.12.1");

        let Some(Bson::Array(items)) = settings.get("Settings") else {
            panic!("no Settings array")
        };
        let parsed = mxrs_bson::parse_array(Some(items));
        let model_settings = parsed
            .items
            .iter()
            .find_map(|i| match i {
                Bson::Document(d) if d.get_str("$Type").ok() == Some("Settings$ModelSettings") => {
                    Some(d)
                }
                _ => None,
            })
            .expect("Settings$ModelSettings present");
        for key in [
            "AfterStartupMicroflow",
            "BeforeShutdownMicroflow",
            "HealthCheckMicroflow",
        ] {
            if model_settings.contains_key(key) {
                assert_eq!(model_settings.get_str(key).unwrap(), "");
            }
        }
    }

    #[test]
    fn default_security_doc_has_a_single_administrator_role() {
        let doc = default_security_doc();
        assert_eq!(doc.get_str("$Type").unwrap(), "Security$ProjectSecurity");
        assert_eq!(doc.get_str("AdminUserRole").unwrap(), "Administrator");
        let Some(Bson::Array(roles)) = doc.get("UserRoles") else {
            panic!("no UserRoles array")
        };
        let parsed = mxrs_bson::parse_array(Some(roles));
        assert_eq!(parsed.items.len(), 1);
    }

    #[test]
    fn default_navigation_doc_has_no_profiles() {
        let doc = default_navigation_doc();
        assert_eq!(
            doc.get_str("$Type").unwrap(),
            "Navigation$NavigationDocument"
        );
        let Some(Bson::Array(profiles)) = doc.get("Profiles") else {
            panic!("no Profiles array")
        };
        let parsed = mxrs_bson::parse_array(Some(profiles));
        assert_eq!(parsed.items.len(), 0);
    }
}
