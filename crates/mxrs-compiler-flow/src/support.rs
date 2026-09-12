//! [`ProjectFlowIndex`]: a single `project.all_units()` pass
//! that classifies every unit this crate's compilers need but
//! `mxrs-model::Module` doesn't load (`DatabaseConnector$DatabaseConnection`,
//! `Constants$Constant`, `JavaScriptActions$JavaScriptAction`,
//! `Microflows$Nanoflow` by qualified name, `DomainModels$DomainModel`
//! associations and aggregate attribute types by qualified name) plus the module-role → user-role map
//! (`project_role_map`, identical purpose to
//! `mxrs-compiler-domain::security::project_role_map`, duplicated for the
//! same reason as the BSON readers above). Mirrors
//! `mxrs-compiler-domain::domain::index_oql_sources`'s precedent: a
//! document type only this pipeline stage needs is read directly off raw
//! units instead of widening `mxrs-model::Module`'s loader for a
//! single-purpose consumer.

use std::collections::HashMap;

use mxrs_bson::Document;
pub(crate) use mxrs_compiler_support::{
    array_docs, database_derived_id, derived_id, get_any, get_doc_any, get_id_any, get_str_any,
    project_role_map, stable_dedup, string_list,
};
use mxrs_model::Project;

use crate::CompilerError;

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
    /// `"Module.Entity.Attribute"` -> Runtime scalar type used by aggregate
    /// actions whose output type is omitted from editor-shape BSON.
    pub attribute_types: HashMap<String, String>,
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
    /// Builds the flow index from documents already decoded and associated
    /// with their owning module. This avoids a second full MPR decode when a
    /// higher-level compiler shares a project-wide document catalog.
    pub fn from_documents(documents: &[(String, Document)]) -> Self {
        let entity_qualified_name_by_id: HashMap<String, String> = documents
            .iter()
            .filter(|(_, document)| {
                get_str_any(document, &["$Type"]).as_deref() == Some("DomainModels$DomainModel")
            })
            .flat_map(|(module_name, document)| {
                array_docs(document, &["Entities", "entities"])
                    .into_iter()
                    .filter_map(move |entity| {
                        Some((
                            get_id_any(&entity, &["$ID"])?,
                            format!("{module_name}.{}", get_str_any(&entity, &["Name", "name"])?),
                        ))
                    })
            })
            .collect();
        let enumeration_qualified_name_by_id = documents
            .iter()
            .filter(|(_, document)| {
                get_str_any(document, &["$Type"]).as_deref() == Some("Enumerations$Enumeration")
            })
            .filter_map(|(module_name, document)| {
                Some((
                    get_id_any(document, &["$ID"])?,
                    format!("{module_name}.{}", get_str_any(document, &["Name"])?),
                ))
            })
            .collect::<HashMap<_, _>>();
        let mut index = ProjectFlowIndex {
            database_connections: HashMap::new(),
            constants: HashMap::new(),
            javascript_actions: HashMap::new(),
            nanoflows: HashMap::new(),
            associations: HashMap::new(),
            attribute_types: HashMap::new(),
            role_map: HashMap::new(),
        };
        for (module_name, document) in documents {
            let Some(type_name) = get_str_any(document, &["$Type"]) else {
                continue;
            };
            let name = get_str_any(document, &["Name"]).unwrap_or_default();
            match type_name.as_str() {
                "DatabaseConnector$DatabaseConnection" => {
                    index
                        .database_connections
                        .insert(format!("{module_name}.{name}"), document.clone());
                }
                "Constants$Constant" => {
                    index
                        .constants
                        .insert(format!("{module_name}.{name}"), document.clone());
                }
                "JavaScriptActions$JavaScriptAction" => {
                    index
                        .javascript_actions
                        .insert(format!("{module_name}.{name}"), document.clone());
                }
                "Microflows$Nanoflow" => {
                    index
                        .nanoflows
                        .insert(format!("{module_name}.{name}"), document.clone());
                }
                "DomainModels$DomainModel" => {
                    index.index_domain_attributes(
                        module_name,
                        document,
                        &enumeration_qualified_name_by_id,
                    );
                    index.index_domain_associations(
                        module_name,
                        document,
                        &entity_qualified_name_by_id,
                    );
                }
                _ => {}
            }
        }
        index
    }

    fn index_domain_attributes(
        &mut self,
        module_name: &str,
        document: &Document,
        enumeration_qualified_name_by_id: &HashMap<String, String>,
    ) {
        for entity in array_docs(document, &["Entities", "entities"]) {
            let Some(entity_name) = get_str_any(&entity, &["Name", "name"]) else {
                continue;
            };
            for attribute in array_docs(&entity, &["Attributes", "attributes"]) {
                let Some(attribute_name) = get_str_any(&attribute, &["Name", "name"]) else {
                    continue;
                };
                let Some(attribute_type) =
                    get_doc_any(&attribute, &["NewType", "newType", "Type", "type"])
                else {
                    continue;
                };
                let Some(runtime_type) =
                    aggregate_attribute_type(&attribute_type, enumeration_qualified_name_by_id)
                else {
                    continue;
                };
                self.attribute_types.insert(
                    format!("{module_name}.{entity_name}.{attribute_name}"),
                    runtime_type,
                );
            }
        }
    }

    fn index_domain_associations(
        &mut self,
        module_name: &str,
        document: &Document,
        entity_qualified_name_by_id: &HashMap<String, String>,
    ) {
        for association in array_docs(document, &["Associations", "associations"]) {
            let Some(name) = get_str_any(&association, &["Name", "name"]) else {
                continue;
            };
            let parent_id = get_id_any(&association, &["ParentPointer", "ParentID"]);
            let child_id = get_id_any(&association, &["ChildPointer", "ChildID"]);
            let reference_set =
                get_str_any(&association, &["Type", "type"]).as_deref() == Some("ReferenceSet");
            self.associations.insert(
                format!("{module_name}.{name}"),
                AssociationInfo {
                    parent: parent_id.and_then(|id| entity_qualified_name_by_id.get(&id).cloned()),
                    child: child_id.and_then(|id| entity_qualified_name_by_id.get(&id).cloned()),
                    reference_set,
                },
            );
        }
        for association in array_docs(document, &["CrossAssociations", "crossAssociations"]) {
            let Some(name) = get_str_any(&association, &["Name", "name"]) else {
                continue;
            };
            let parent_id = get_id_any(&association, &["ParentPointer", "ParentID"]);
            let child_name = get_str_any(&association, &["Child", "child"]);
            let reference_set =
                get_str_any(&association, &["Type", "type"]).as_deref() == Some("ReferenceSet");
            self.associations.insert(
                format!("{module_name}.{name}"),
                AssociationInfo {
                    parent: parent_id.and_then(|id| entity_qualified_name_by_id.get(&id).cloned()),
                    child: child_name,
                    reference_set,
                },
            );
        }
    }

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
        let enumeration_qualified_name_by_id = modules
            .iter()
            .filter_map(|module| module.name.as_deref().map(|name| (name, module)))
            .flat_map(|(module_name, module)| {
                module.artifact_units.iter().filter_map(move |document| {
                    (get_str_any(document, &["$Type"]).as_deref()
                        == Some("Enumerations$Enumeration"))
                    .then(|| {
                        Some((
                            get_id_any(document, &["$ID"])?,
                            format!("{module_name}.{}", get_str_any(document, &["Name"])?),
                        ))
                    })
                    .flatten()
                })
            })
            .collect::<HashMap<_, _>>();

        let mut index = ProjectFlowIndex {
            database_connections: HashMap::new(),
            constants: HashMap::new(),
            javascript_actions: HashMap::new(),
            nanoflows: HashMap::new(),
            associations: HashMap::new(),
            attribute_types: HashMap::new(),
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
                    index.index_domain_attributes(
                        module_name,
                        &document,
                        &enumeration_qualified_name_by_id,
                    );
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
                    // Cross-module associations live in a separate array with a
                    // different child-reference shape: `Child` already holds the
                    // dotted `"Module.Entity"` qualified name directly (see
                    // `mxrs_model::Association::is_cross_module`/`to_bson`), not an
                    // id needing `entity_qualified_name_by_id` resolution like
                    // `ChildPointer`/`ChildID` above.
                    for association in
                        array_docs(&document, &["CrossAssociations", "crossAssociations"])
                    {
                        let Some(name) = get_str_any(&association, &["Name", "name"]) else {
                            continue;
                        };
                        let parent_id = get_id_any(&association, &["ParentPointer", "ParentID"]);
                        let child_name = get_str_any(&association, &["Child", "child"]);
                        let reference_set = get_str_any(&association, &["Type", "type"]).as_deref()
                            == Some("ReferenceSet");
                        index.associations.insert(
                            format!("{module_name}.{name}"),
                            AssociationInfo {
                                parent: parent_id
                                    .and_then(|id| entity_qualified_name_by_id.get(&id).cloned()),
                                child: child_name,
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

fn aggregate_attribute_type(
    attribute_type: &Document,
    enumeration_qualified_name_by_id: &HashMap<String, String>,
) -> Option<String> {
    Some(match get_str_any(attribute_type, &["$Type"])?.as_str() {
        "DomainModels$StringAttributeType" | "DomainModels$HashedStringAttributeType" => {
            "String".to_string()
        }
        "DomainModels$IntegerAttributeType"
        | "DomainModels$LongAttributeType"
        | "DomainModels$AutoNumberAttributeType" => "Integer".to_string(),
        "DomainModels$FloatAttributeType" | "DomainModels$DecimalAttributeType" => {
            "Decimal".to_string()
        }
        "DomainModels$BooleanAttributeType" => "Boolean".to_string(),
        "DomainModels$DateTimeAttributeType" => "DateTime".to_string(),
        "DomainModels$BinaryAttributeType" => "Binary".to_string(),
        "DomainModels$EnumerationAttributeType" => {
            let enumeration = get_str_any(attribute_type, &["Enumeration", "enumeration"])
                .filter(|name| name.contains('.'))
                .or_else(|| {
                    get_id_any(attribute_type, &["Enumeration", "enumeration"])
                        .and_then(|id| enumeration_qualified_name_by_id.get(&id).cloned())
                })?;
            format!("#{enumeration}")
        }
        _ => return None,
    })
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

#[cfg(test)]
mod tests {
    use mxrs_bson::{Bson, doc};

    use super::*;

    #[test]
    fn indexes_runtime_scalar_types_for_aggregate_attributes() {
        let enumeration_id = "11111111-1111-4111-8111-111111111111";
        let documents = vec![
            (
                "Sales".to_string(),
                doc! {
                    "$ID": enumeration_id,
                    "$Type": "Enumerations$Enumeration",
                    "Name": "Status",
                },
            ),
            (
                "Sales".to_string(),
                doc! {
                    "$Type": "DomainModels$DomainModel",
                    "Entities": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "$ID": "entity-id",
                        "Name": "Order",
                        "Attributes": mxrs_bson::build_array(vec![
                            Bson::Document(doc! {
                                "Name": "Amount",
                                "NewType": { "$Type": "DomainModels$DecimalAttributeType" },
                            }),
                            Bson::Document(doc! {
                                "Name": "Sequence",
                                "NewType": { "$Type": "DomainModels$LongAttributeType" },
                            }),
                            Bson::Document(doc! {
                                "Name": "Status",
                                "NewType": {
                                    "$Type": "DomainModels$EnumerationAttributeType",
                                    "Enumeration": enumeration_id,
                                },
                            }),
                        ], 3),
                    })], 3),
                },
            ),
        ];

        let index = ProjectFlowIndex::from_documents(&documents);
        assert_eq!(
            index.attribute_types.get("Sales.Order.Amount"),
            Some(&"Decimal".to_string())
        );
        assert_eq!(
            index.attribute_types.get("Sales.Order.Sequence"),
            Some(&"Integer".to_string())
        );
        assert_eq!(
            index.attribute_types.get("Sales.Order.Status"),
            Some(&"#Sales.Status".to_string())
        );
    }
}
