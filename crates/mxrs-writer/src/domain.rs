//! Resolves `mxrs_ir::{EntityDecl, AssociationDecl}` (name-based) into a
//! persistable `mxrs_model::DomainModel` (id-based) — mirrors the entity-id
//! assignment + association resolution done inline in `Writer#write_domain_model`,
//! narrowed to fresh-project creation (no reconciliation against an existing
//! domain model — that's `synchronize_ruby_*!`'s incremental-rewrite job,
//! out of scope for this pass) and to same-module associations only
//! (cross-module targets use a qualified-name string rather than a resolved
//! id in mxrb's own `cross_association_doc`, which this pass doesn't port).

use std::collections::HashMap;

use mxrs_ir::declaration::EntityDecl;
use mxrs_model::association::Association;
use mxrs_model::entity::{Entity, Location, SystemMembers};
use mxrs_model::DomainModel;

use crate::error::{Result, WriterError};

/// Returns the built domain model alongside a `name -> id` map for this
/// module's entities, so callers (e.g. a future cross-module association
/// pass) can look up ids assigned here.
pub fn build_domain_model(module_name: &str, decls: &[EntityDecl]) -> Result<(DomainModel, HashMap<String, String>)> {
    let mut entity_ids = HashMap::new();
    let mut entities = Vec::with_capacity(decls.len());
    for decl in decls {
        let id = uuid::Uuid::new_v4().to_string();
        entity_ids.insert(decl.name.clone(), id.clone());
        entities.push(Entity {
            id: Some(id),
            name: Some(decl.name.clone()),
            qualified_name: Some(format!("{module_name}.{}", decl.name)),
            documentation: decl.documentation.clone(),
            persistable: decl.persistable,
            location: Location { x: 0, y: 0 },
            data_storage_guid: None,
            export_level: "Hidden".into(),
            generalization: None,
            access_rules: vec![],
            indexes: vec![],
            system_members: SystemMembers::default(),
            lifecycle: vec![],
            validation_rules: vec![],
            source: None,
            oql_query: None,
            native_type: None,
            attributes: decl.attributes.clone(),
        });
    }

    let mut associations = Vec::new();
    for decl in decls {
        let from_id = entity_ids.get(&decl.name).expect("just inserted above").clone();
        for assoc in &decl.associations {
            if assoc.target.contains('.') {
                return Err(WriterError::UnsupportedCrossModuleAssociation(assoc.name.clone()));
            }
            let to_id =
                entity_ids.get(&assoc.target).cloned().ok_or_else(|| WriterError::UnknownAssociationTarget(assoc.target.clone()))?;
            associations.push(Association {
                id: None,
                name: Some(assoc.name.clone()),
                documentation: assoc.documentation.clone(),
                from_entity_id: Some(from_id.clone()),
                to_entity_id: Some(to_id),
                association_type: assoc.association_type,
                owner: assoc.owner,
                storage_format: assoc.storage_format,
                delete_behavior: None,
                export_level: "Hidden".into(),
            });
        }
    }

    Ok((DomainModel { documentation: String::new(), entities, associations, cross_associations: vec![] }, entity_ids))
}
