//! Resolves project roles and association security into Runtime fields —
//! ports `lib/mxrb/compiler/domain_security_compiler.rb` (82 lines).
//!
//! `mxrs-model` has no dedicated `Security$ProjectSecurity` reader (same
//! gap `mxrs-cli::compare`'s own `security_summary` already works around);
//! this crate reads the raw unit directly rather than adding a dependency
//! neither side needs more than this one lookup for.

use std::collections::HashMap;

use mxrs_bson::{doc, Bson, Document};
use mxrs_model::entity::{AccessMember, AccessMemberKind, AccessRule};
use mxrs_model::{Association, Project};

use crate::support::{array_docs, get_doc_any, get_str_any, new_id, stable_dedup, string_list};
use crate::CompilerError;

pub struct SecurityCompiler {
    /// Module role name -> the user role names that grant it (built from
    /// `Security$ProjectSecurity`'s `UserRoles[].ModuleRoles`) — an
    /// `AccessRule`'s own `AllowedModuleRoles` only names module roles, so
    /// Runtime's `AllowedUserRoles` needs this reverse lookup to resolve
    /// who actually gets the access.
    role_map: HashMap<String, Vec<String>>,
}

impl SecurityCompiler {
    pub fn new(project: &Project) -> Result<Self, CompilerError> {
        Ok(SecurityCompiler {
            role_map: project_role_map(project)?,
        })
    }

    /// A `SecurityCompiler` with an empty role map, for `domain.rs`'s own
    /// generalization/attribute unit tests, which need a `DomainCompiler`
    /// but don't exercise security at all — building a real throwaway
    /// `Project` just to satisfy [`SecurityCompiler::new`]'s signature
    /// would be pure noise in those tests.
    #[cfg(test)]
    pub(crate) fn new_without_security() -> Self {
        SecurityCompiler {
            role_map: HashMap::new(),
        }
    }

    pub fn access_rule(&self, rule: &AccessRule) -> Document {
        let id = rule.id.clone().unwrap_or_else(new_id);
        let allowed_user_roles =
            stable_dedup(rule.roles.iter().flat_map(|module_role| {
                self.role_map.get(module_role).cloned().unwrap_or_default()
            }));
        doc! {
            "$ID": id,
            "$Type": "DomainModels$AccessRule",
            "MemberAccesses": rule.members.iter().map(member_access).collect::<Vec<_>>(),
            "AllowedUserRoles": allowed_user_roles,
            "AllowCreate": rule.create,
            "AllowDelete": rule.delete,
            "XPathConstraint": rule.xpath.clone(),
        }
    }

    /// `module_name` qualifies `QualifiedName`, same as
    /// `DomainDocumentCompiler#compile_entity` passing its own module name
    /// down for entities — an association carries no module reference of
    /// its own, so the caller (iterating one module's `DomainModel`) has to
    /// supply it.
    pub fn association(&self, association: &Association, module_name: &str) -> Document {
        let name = association.name.clone().unwrap_or_default();
        let mut result = self.association_fields(association);
        result.insert("QualifiedName", format!("{module_name}.{name}"));
        result.insert("UnqualifiedName", name);
        result.insert("Navigability", "BothDirections");
        let target = association.to_entity_id.clone().unwrap_or_default();
        if association.is_cross_module() {
            result.insert("Child", target);
        } else {
            result.insert("ChildPointer", target);
        }
        result
    }

    fn association_fields(&self, association: &Association) -> Document {
        // Reuses `Association::to_bson`'s already-correct `$Type`/`Type`/
        // `Owner`/`StorageFormat` string encoding instead of duplicating
        // `AssociationType`/`Owner`/`StorageFormat`'s enum-to-string match
        // arms here (those are crate-private in mxrs-model) — this crate
        // only needs the string values, which `to_bson`'s editor-shape
        // document already carries.
        let editor = association.to_bson();
        let id = association.id.clone().unwrap_or_else(new_id);
        doc! {
            "$ID": id,
            "$Type": editor.get_str("$Type").unwrap_or("DomainModels$Association").to_string(),
            "DeleteBehavior": delete_behavior(association.delete_behavior.as_ref()),
            // Neither field is retained by mxrs-model::Association (no
            // `Source`/`GUID` fields on the struct) — narrowed the same
            // way `mxrs-model::Entity`'s missing `Image` field is in
            // `domain.rs`'s `compile_entity`, not silently invented.
            "Source": Bson::Null,
            "GUID": Bson::Null,
            "Type": editor.get_str("Type").unwrap_or("Reference").to_string(),
            "Owner": editor.get_str("Owner").unwrap_or("Default").to_string(),
            "StorageFormat": editor.get_str("StorageFormat").unwrap_or("Column").to_string(),
            "ParentPointer": association.from_entity_id.clone().unwrap_or_default(),
        }
    }
}

fn member_access(member: &AccessMember) -> Document {
    let id = member.id.clone().unwrap_or_else(new_id);
    let (attribute, association) = match member.kind {
        AccessMemberKind::Attribute => (member.reference.clone(), String::new()),
        AccessMemberKind::Association => (String::new(), member.reference.clone()),
    };
    doc! {
        "$ID": id,
        "$Type": "DomainModels$MemberAccess",
        "Attribute": attribute,
        "Association": association,
        "AccessRights": member.rights.clone(),
    }
}

fn delete_behavior(source: Option<&Document>) -> Document {
    let default = default_delete_behavior();
    let source = source.unwrap_or(&default);
    doc! {
        "$ID": get_str_any(source, &["$ID"]).unwrap_or_default(),
        "$Type": get_str_any(source, &["$Type"]).unwrap_or_else(|| "DomainModels$DeleteBehavior".to_string()),
        "ParentErrorMessage": text_reference(get_doc_any(source, &["ParentErrorMessage", "parentErrorMessage"])),
        "ChildErrorMessage": text_reference(get_doc_any(source, &["ChildErrorMessage", "childErrorMessage"])),
        "ParentDeleteBehavior": runtime_delete_behavior(
            get_str_any(source, &["ParentDeleteBehavior", "parentDeleteBehavior"]).unwrap_or_else(|| "NoAction".to_string())
        ),
        "ChildDeleteBehavior": runtime_delete_behavior(
            get_str_any(source, &["ChildDeleteBehavior", "childDeleteBehavior"]).unwrap_or_else(|| "NoAction".to_string())
        ),
    }
}

fn default_delete_behavior() -> Document {
    doc! {
        "$ID": new_id(),
        "$Type": "DomainModels$DeleteBehavior",
        "ParentDeleteBehavior": "NoAction",
        "ChildDeleteBehavior": "NoAction",
    }
}

fn text_reference(source: Option<Document>) -> Bson {
    match source {
        Some(d) => Bson::Document(doc! {
            "$ID": get_str_any(&d, &["$ID"]).unwrap_or_default(),
            "$Type": get_str_any(&d, &["$Type"]).unwrap_or_default(),
        }),
        None => Bson::Null,
    }
}

/// `NoAction` isn't a real Runtime delete-behavior value — Studio Pro's
/// editor-only default gets translated to the Runtime default at compile
/// time, verbatim per `domain_security_compiler.rb#runtime_delete_behavior`.
fn runtime_delete_behavior(value: String) -> String {
    if value == "NoAction" {
        "DeleteMeAndReferences".to_string()
    } else {
        value
    }
}

fn project_role_map(project: &Project) -> Result<HashMap<String, Vec<String>>, CompilerError> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let units = project.all_units()?;
    let security_doc = units.iter().find_map(|unit| {
        let doc = project.mpr().parse_contents(unit).ok()?;
        (get_str_any(&doc, &["$Type"]).as_deref() == Some("Security$ProjectSecurity"))
            .then_some(doc)
    });
    let Some(doc) = security_doc else {
        return Ok(map);
    };
    for role in array_docs(&doc, &["UserRoles"]) {
        let name = get_str_any(&role, &["Name"]).unwrap_or_default();
        for module_role in string_list(&role, &["ModuleRoles"]) {
            map.entry(module_role).or_default().push(name.clone());
        }
    }
    Ok(map)
}
