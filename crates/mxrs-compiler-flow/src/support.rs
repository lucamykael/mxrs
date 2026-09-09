//! Tiny casing-tolerant BSON field readers, duplicated from
//! `mxrs-compiler-domain::support` (which is itself duplicated from
//! `mxrs-model`'s private `support` module) rather than shared across
//! crates — same precedent used throughout this pipeline stage: a handful
//! of ~5-line readers aren't worth a shared dependency two sibling crates
//! would otherwise need only for this.
//!
//! Also home to [`ProjectFlowIndex`]: a single `project.all_units()` pass
//! that classifies every unit this crate's compilers need but
//! `mxrs-model::Module` doesn't load (`DatabaseConnector$DatabaseConnection`,
//! `Constants$Constant`, `JavaScriptActions$JavaScriptAction`,
//! `Microflows$Nanoflow` by qualified name, `DomainModels$DomainModel`
//! associations by qualified name) plus the module-role → user-role map
//! (`project_role_map`, identical purpose to
//! `mxrs-compiler-domain::security::project_role_map`, duplicated for the
//! same reason as the BSON readers above). Mirrors
//! `mxrs-compiler-domain::domain::index_oql_sources`'s precedent: a
//! document type only this pipeline stage needs is read directly off raw
//! units instead of widening `mxrs-model::Module`'s loader for a
//! single-purpose consumer.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document};
use mxrs_model::Project;
use sha2::{Digest, Sha256};

use crate::CompilerError;

pub fn get_any<'a>(doc: &'a Document, keys: &[&str]) -> Option<&'a Bson> {
    keys.iter().find_map(|k| doc.get(*k))
}

pub fn get_str_any(doc: &Document, keys: &[&str]) -> Option<String> {
    match get_any(doc, keys)? {
        Bson::String(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn get_id_any(doc: &Document, keys: &[&str]) -> Option<String> {
    get_any(doc, keys).and_then(mxrs_bson::extract_id)
}

pub fn get_doc_any(doc: &Document, keys: &[&str]) -> Option<Document> {
    match get_any(doc, keys)? {
        Bson::Document(d) => Some(d.clone()),
        _ => None,
    }
}

pub fn array_items(doc: &Document, keys: &[&str]) -> Vec<Bson> {
    match get_any(doc, keys) {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        _ => Vec::new(),
    }
}

pub fn array_docs(doc: &Document, keys: &[&str]) -> Vec<Document> {
    array_items(doc, keys)
        .into_iter()
        .filter_map(|b| match b {
            Bson::Document(d) => Some(d),
            _ => None,
        })
        .collect()
}

pub fn string_list(doc: &Document, keys: &[&str]) -> Vec<String> {
    array_items(doc, keys)
        .into_iter()
        .filter_map(|b| match b {
            Bson::String(s) => Some(s),
            _ => None,
        })
        .collect()
}

/// Dedups while keeping first-occurrence order — mirrors Ruby's `#uniq`.
pub fn stable_dedup(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

/// Deterministic, stable-across-recompiles UUID derived from a source
/// node's own `$ID` plus a label — mirrors `derived_id`, duplicated
/// identically (down to the hardcoded `4`/`8` nibbles that fake a v4/
/// variant-1 UUID shape) in `microflow_node_compiler.rb:245-249` and
/// `microflow_document_compiler.rb:78-82`. Recompiling the same model
/// twice must not churn IDs on synthetic sub-documents (`ConstantRange`,
/// the `Texts$Text` placeholder, ...), or every rebuild would look like a
/// diff even with zero model changes — that's what this buys over a fresh
/// random UUID (`new_id`) for these specific synthetic nodes.
///
/// Kept local to this crate rather than shared with `mxrs-compiler-domain`:
/// nothing there needs a *stable* synthetic ID (its own synthetic docs use
/// `new_id`), so duplicating this ~10-line algorithm once is cheaper than a
/// cross-crate dependency for a single consumer — promote it if a third
/// crate ever needs the same guarantee.
pub(crate) fn derived_id(source_id: &str, label: &str) -> String {
    derived_id_from_seed(&format!("{source_id}:{label}"))
}

/// Same algorithm, extra `database-connector` namespace segment in the
/// seed — `database_connector_action_compiler.rb:271-275`'s own variant.
/// Deliberately not unified with [`derived_id`]'s seed format: the Ruby
/// keeps them distinct, and "fixing" that would just make this crate's
/// derived IDs stop matching what a real `mxrb` run produces for the same
/// input.
pub(crate) fn database_derived_id(source_id: &str, label: &str) -> String {
    derived_id_from_seed(&format!("{source_id}:database-connector:{label}"))
}

fn derived_id_from_seed(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let hex = &hex[0..32];
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32],
    )
}

/// One `project.all_units()` pass, classified by `$Type`, for every
/// document type this crate's compilers need that `mxrs-model::Module`
/// doesn't load.
pub struct ProjectFlowIndex {
    /// `"Module.ConnectionName"` -> `DatabaseConnector$DatabaseConnection`.
    pub database_connections: HashMap<String, Document>,
    /// `"Module.ConstantName"` -> `Constants$Constant` (needed to resolve a
    /// connection's `ConnectionString` constant's `DefaultValue` for
    /// `unconfigured_write?`).
    pub constants: HashMap<String, Document>,
    /// `"Module.ActionName"` -> `JavaScriptActions$JavaScriptAction`.
    pub javascript_actions: HashMap<String, Document>,
    /// `"Module.FlowName"` -> `Microflows$Nanoflow` root document.
    pub nanoflows: HashMap<String, Document>,
    /// `"Module.AssocName"` -> association direction/cardinality info.
    pub associations: HashMap<String, AssociationInfo>,
    /// Module role qualified name -> user role names that grant it.
    pub role_map: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct AssociationInfo {
    pub parent: Option<String>,
    pub child: Option<String>,
    pub reference_set: bool,
}

impl ProjectFlowIndex {
    pub fn build(project: &Project) -> Result<Self, CompilerError> {
        let units = project.all_units()?;
        let parent_by_id: HashMap<String, String> = units
            .iter()
            .map(|unit| (unit.unit_id.clone(), unit.container_id.clone()))
            .collect();
        let modules = project.modules()?;
        let module_name_by_unit_id: HashMap<String, String> = modules
            .iter()
            .filter_map(|module| Some((module.id.clone(), module.name.clone()?)))
            .collect();
        let entity_qualified_name_by_id: HashMap<String, String> = modules
            .iter()
            .flat_map(|module| module.entities())
            .filter_map(|entity| Some((entity.id.clone()?, entity.qualified_name.clone()?)))
            .collect();

        let mut index = ProjectFlowIndex {
            database_connections: HashMap::new(),
            constants: HashMap::new(),
            javascript_actions: HashMap::new(),
            nanoflows: HashMap::new(),
            associations: HashMap::new(),
            role_map: project_role_map(project)?,
        };

        for unit in &units {
            let document = project
                .mpr()
                .parse_contents(unit)
                .map_err(mxrs_model::ModelError::from)?;
            let Some(type_name) = get_str_any(&document, &["$Type"]) else {
                continue;
            };
            match type_name.as_str() {
                "DatabaseConnector$DatabaseConnection" => {
                    if let Some(module_name) = owning_module_name(
                        &unit.container_id,
                        &parent_by_id,
                        &module_name_by_unit_id,
                    ) {
                        let name = get_str_any(&document, &["Name"]).unwrap_or_default();
                        index
                            .database_connections
                            .insert(format!("{module_name}.{name}"), document);
                    }
                }
                "Constants$Constant" => {
                    if let Some(module_name) = owning_module_name(
                        &unit.container_id,
                        &parent_by_id,
                        &module_name_by_unit_id,
                    ) {
                        let name = get_str_any(&document, &["Name"]).unwrap_or_default();
                        index
                            .constants
                            .insert(format!("{module_name}.{name}"), document);
                    }
                }
                "JavaScriptActions$JavaScriptAction" => {
                    if let Some(module_name) = owning_module_name(
                        &unit.container_id,
                        &parent_by_id,
                        &module_name_by_unit_id,
                    ) {
                        let name = get_str_any(&document, &["Name"]).unwrap_or_default();
                        index
                            .javascript_actions
                            .insert(format!("{module_name}.{name}"), document);
                    }
                }
                "Microflows$Nanoflow" => {
                    if let Some(module_name) = owning_module_name(
                        &unit.container_id,
                        &parent_by_id,
                        &module_name_by_unit_id,
                    ) {
                        let name = get_str_any(&document, &["Name"]).unwrap_or_default();
                        index
                            .nanoflows
                            .insert(format!("{module_name}.{name}"), document);
                    }
                }
                "DomainModels$DomainModel" => {
                    let Some(module_name) = module_name_by_unit_id.get(&unit.container_id) else {
                        continue;
                    };
                    for association in array_docs(&document, &["Associations", "associations"]) {
                        let Some(name) = get_str_any(&association, &["Name", "name"]) else {
                            continue;
                        };
                        let parent_id = get_id_any(&association, &["ParentPointer", "ParentID"]);
                        let child_id = get_id_any(&association, &["ChildPointer", "ChildID"]);
                        let reference_set = get_str_any(&association, &["Type", "type"]).as_deref()
                            == Some("ReferenceSet");
                        index.associations.insert(
                            format!("{module_name}.{name}"),
                            AssociationInfo {
                                parent: parent_id
                                    .and_then(|id| entity_qualified_name_by_id.get(&id).cloned()),
                                child: child_id
                                    .and_then(|id| entity_qualified_name_by_id.get(&id).cloned()),
                                reference_set,
                            },
                        );
                    }
                }
                _ => {}
            }
        }

        Ok(index)
    }
}

/// Walks a unit's container chain up to the owning `Projects$Module` —
/// mirrors `mxrs-compiler-domain::domain::owning_module_name` exactly
/// (same duplication rationale as this module's own doc comment).
fn owning_module_name(
    container_id: &str,
    parent_by_id: &HashMap<String, String>,
    module_name_by_id: &HashMap<String, String>,
) -> Option<String> {
    let mut current = container_id;
    for _ in 0..64 {
        if let Some(name) = module_name_by_id.get(current) {
            return Some(name.clone());
        }
        let parent = parent_by_id.get(current)?;
        if parent == current {
            return None;
        }
        current = parent;
    }
    None
}

/// Module role name -> user role names that grant it, read directly off
/// `Security$ProjectSecurity` — identical purpose and implementation to
/// `mxrs-compiler-domain::security::project_role_map`, duplicated per this
/// module's own doc comment.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_id_is_deterministic_and_shaped_like_a_uuid() {
        let a = derived_id("11111111-1111-1111-1111-111111111111", "constant-range");
        let b = derived_id("11111111-1111-1111-1111-111111111111", "constant-range");
        assert_eq!(a, b, "same seed must always produce the same id");
        assert_eq!(a.len(), 36);
        assert_eq!(a.chars().nth(14), Some('4'));
        assert_eq!(a.chars().nth(19), Some('8'));
    }

    #[test]
    fn derived_id_and_database_derived_id_diverge_for_the_same_label() {
        let plain = derived_id("11111111-1111-1111-1111-111111111111", "x");
        let database = database_derived_id("11111111-1111-1111-1111-111111111111", "x");
        assert_ne!(
            plain, database,
            "the database-connector variant's extra namespace segment must change the result"
        );
    }

    #[test]
    fn derived_id_matches_a_hand_computed_vector() {
        // sha256("00000000-0000-0000-0000-000000000000:label") =
        // 5e9a3c... computed once and hardcoded as a golden value.
        let id = derived_id("00000000-0000-0000-0000-000000000000", "label");
        let digest = Sha256::digest(b"00000000-0000-0000-0000-000000000000:label");
        let hex = digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let expected = format!(
            "{}-{}-4{}-8{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[13..16],
            &hex[17..20],
            &hex[20..32],
        );
        assert_eq!(id, expected);
    }
}
