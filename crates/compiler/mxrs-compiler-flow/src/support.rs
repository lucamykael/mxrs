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

#[derive(Debug, Clone, PartialEq, Eq)]
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
                            qualified_name(module_name, &entity)?,
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
                    qualified_name(module_name, document)?,
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

    /// Adds the audited Runtime-owned System associations omitted from MPR
    /// files. Callers provide the actual project version: unavailable seeds
    /// are errors, not guessed metadata or aliases for unknown associations.
    /// Explicit source declarations remain authoritative.
    pub fn with_system_model(mut self, version: &str) -> Result<Self, CompilerError> {
        let documents = mxrs_schema::system_model_documents(version)?
            .into_iter()
            .map(|document| ("System".to_string(), document))
            .collect::<Vec<_>>();
        for (name, association) in Self::from_documents(&documents).associations {
            self.associations.entry(name).or_insert(association);
        }
        Ok(self)
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
            let Some(name) = qualified_name(module_name, &association) else {
                continue;
            };
            let parent_id = get_id_any(&association, &["ParentPointer", "ParentID", "parentId"]);
            let child_id = get_id_any(&association, &["ChildPointer", "ChildID", "childId"]);
            let reference_set =
                get_str_any(&association, &["Type", "type"]).as_deref() == Some("ReferenceSet");
            self.associations.insert(
                name,
                AssociationInfo {
                    parent: parent_id.and_then(|id| entity_qualified_name_by_id.get(&id).cloned()),
                    child: child_id.and_then(|id| entity_qualified_name_by_id.get(&id).cloned()),
                    reference_set,
                },
            );
        }
        for association in array_docs(document, &["CrossAssociations", "crossAssociations"]) {
            let Some(name) = qualified_name(module_name, &association) else {
                continue;
            };
            let parent_id = get_id_any(&association, &["ParentPointer", "ParentID", "parentId"]);
            let child_name = get_str_any(&association, &["Child", "child"]);
            let reference_set =
                get_str_any(&association, &["Type", "type"]).as_deref() == Some("ReferenceSet");
            self.associations.insert(
                name,
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
                    index.index_domain_associations(
                        module_name,
                        &document,
                        &entity_qualified_name_by_id,
                    );
                }
                _ => {}
            }
        }

        match project.mendix_version()? {
            Some(version) => index.with_system_model(&version),
            None => Ok(index),
        }
    }
}

/// Editor names belong to the containing module, including after a rename.
/// Native Runtime documents instead carry qualified/unqualified names.
fn qualified_name(module_name: &str, document: &Document) -> Option<String> {
    get_str_any(document, &["Name", "name"])
        .map(|name| format!("{module_name}.{name}"))
        .or_else(|| get_str_any(document, &["QualifiedName"]))
        .or_else(|| {
            get_str_any(document, &["UnqualifiedName"]).map(|name| format!("{module_name}.{name}"))
        })
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
    fn native_system_associations_match_the_ruby_runtime_oracle_in_both_directions() {
        let index = ProjectFlowIndex::from_documents(&[])
            .with_system_model("11.12.1")
            .unwrap();
        let schema = mxrs_schema::RuntimeModelSchema::for_11(&[]).unwrap();
        let compiler = crate::FlowNodeCompiler::new(&schema, &index.associations, None);
        // Golden metadata from mxrb's SystemModelSeed 11.12.1 and
        // MicroflowNodeCompiler#runtime_association, not guessed endpoints.
        for (name, parent, child, reference_set) in [
            (
                "System.HttpHeaders",
                "System.HttpHeader",
                "System.HttpMessage",
                false,
            ),
            ("System.UserRoles", "System.User", "System.UserRole", true),
            (
                "System.User_Language",
                "System.User",
                "System.Language",
                false,
            ),
        ] {
            assert_eq!(
                index.associations[name],
                AssociationInfo {
                    parent: Some(parent.to_string()),
                    child: Some(child.to_string()),
                    reference_set,
                }
            );
            for (start, target, list) in [(parent, child, reference_set), (child, parent, true)] {
                let variables = HashMap::from([("object".to_string(), start.to_string())]);
                let source = Bson::Document(doc! {
                    "$ID": "11111111-1111-4111-8111-111111111111",
                    "$Type": "Microflows$AssociationRetrieveSource",
                    "AssociationId": name,
                    "StartVariableName": "object",
                });
                let Bson::Document(compiled) = compiler.compile(&source, &variables).unwrap()
                else {
                    panic!("expected compiled association retrieve");
                };
                let expected = if list {
                    format!("[{target}]")
                } else {
                    target.to_string()
                };
                assert_eq!(compiled.get_str("Type").unwrap(), expected);
                assert_eq!(compiled.get_str("AssociationId").unwrap(), name);
            }
        }
        let unknown = Bson::Document(doc! {
            "$Type": "Microflows$AssociationRetrieveSource",
            "AssociationId": "Sales.DeletedAssociation",
            "StartVariableName": "object",
        });
        assert!(matches!(
            compiler.compile(&unknown, &HashMap::new()),
            Err(CompilerError::UnknownAssociation { association_id })
                if association_id == "Sales.DeletedAssociation"
        ));
    }

    #[test]
    fn system_enrichment_is_version_checked_idempotent_and_preserves_explicit_metadata() {
        let mut index = ProjectFlowIndex::from_documents(&[]);
        let explicit = AssociationInfo {
            parent: None,
            child: None,
            reference_set: true,
        };
        index
            .associations
            .insert("System.HttpHeaders".to_string(), explicit.clone());
        let enriched = index.with_system_model("11.6.0").unwrap();
        assert_eq!(enriched.associations["System.HttpHeaders"], explicit);
        let once = enriched.associations.clone();
        assert_eq!(
            enriched.with_system_model("11.6.0").unwrap().associations,
            once
        );
        assert!(matches!(
            ProjectFlowIndex::from_documents(&[]).with_system_model("10.24.0"),
            Err(CompilerError::SystemModel(mxrs_schema::SystemModelError::UnsupportedVersion(version)))
                if version == "10.24.0"
        ));
    }

    #[test]
    fn association_names_use_the_owner_module_and_resolve_ids_across_domain_documents() {
        let documents = vec![
            (
                "Sales".to_string(),
                doc! {
                    "$Type": "DomainModels$DomainModel",
                    "entities": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "$ID": "order", "name": "Order", "QualifiedName": "Old.Order",
                    }), Bson::Document(doc! { "$ID": "unnamed" })], 3),
                    "associations": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "name": "Order_Customer", "QualifiedName": "Old.Order_Customer",
                        "parentId": "order", "childId": "customer", "type": "ReferenceSet",
                    }), Bson::Document(doc! {})], 3),
                    "crossAssociations": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "UnqualifiedName": "Order_User", "ParentID": "order", "child": "System.User",
                        "Type": "Reference",
                    }), Bson::Document(doc! {})], 3),
                },
            ),
            (
                "CRM".to_string(),
                doc! {
                    "$Type": "DomainModels$DomainModel",
                    "Entities": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "$ID": "customer", "QualifiedName": "CRM.Customer", "UnqualifiedName": "Customer",
                    })], 3),
                    "Associations": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "Name": "Order_Customer", "ParentPointer": "customer", "ChildPointer": "missing",
                    })], 3),
                },
            ),
        ];
        let index = ProjectFlowIndex::from_documents(&documents);
        assert_eq!(index.associations.len(), 3);
        assert_eq!(
            index.associations["Sales.Order_Customer"],
            AssociationInfo {
                parent: Some("Sales.Order".to_string()),
                child: Some("CRM.Customer".to_string()),
                reference_set: true,
            }
        );
        assert_eq!(
            index.associations["Sales.Order_User"],
            AssociationInfo {
                parent: Some("Sales.Order".to_string()),
                child: Some("System.User".to_string()),
                reference_set: false,
            }
        );
        assert_eq!(
            index.associations["CRM.Order_Customer"],
            AssociationInfo {
                parent: Some("CRM.Customer".to_string()),
                child: None,
                reference_set: false,
            }
        );
        assert!(!index.associations.contains_key("Old.Order_Customer"));
        assert!(!index.associations.contains_key("Order_Customer"));
    }

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
