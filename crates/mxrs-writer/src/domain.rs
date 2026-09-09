//! `build_domain_model` resolves `mxrs_ir::{EntityDecl, AssociationDecl}`
//! (name-based) into a persistable `mxrs_model::DomainModel` (id-based) —
//! mirrors the entity-id assignment + association resolution done inline in
//! `Writer#write_domain_model`, narrowed to fresh-project creation (no
//! reconciliation against an existing domain model).
//!
//! `synchronize_domain_associations` covers the association-reconciliation
//! half of that gap for an *already-existing* domain model — mirrors
//! `Writer#synchronize_ruby_domain_associations!`, see its own doc comment
//! for the scope this narrows (entity structures are not synced here).
//!
//! Cross-module associations (`target` contains a `.`) mirror mxrb's own
//! `cross_association_doc`: unlike same-module associations, the target is
//! **not** resolved to an id — it's persisted as the literal `"Module.Entity"`
//! qualified-name string in a `Child` field, and the association is routed
//! into `DomainModel::cross_associations` rather than `associations`
//! (`Association::to_bson` picks the `DomainModels$CrossAssociation` BSON
//! shape automatically whenever `to_entity_id` contains a `.`). The caller
//! still validates the target against `known_entities` — every
//! `"Module.Entity"` qualified name declared anywhere in the project — so a
//! typo'd or renamed cross-module target fails at write time rather than
//! producing a `.mpr` Studio Pro can't open.

use std::collections::{HashMap, HashSet};

use mxrs_bson::{Bson, Document};
use mxrs_ir::declaration::EntityDecl;
use mxrs_model::association::Association;
use mxrs_model::entity::{Entity, Location, SystemMembers};
use mxrs_model::DomainModel;
use mxrs_mpr::MprFile;

use crate::error::{Result, WriterError};

/// `known_entities` is the set of every `"Module.Entity"` qualified name
/// declared anywhere in the project — used only to validate cross-module
/// association targets (same-module targets are resolved against `decls`
/// directly and don't need it).
///
/// Returns the built domain model alongside a `name -> id` map for this
/// module's entities, so callers (e.g. a future cross-module association
/// pass) can look up ids assigned here.
pub fn build_domain_model(
    module_name: &str,
    decls: &[EntityDecl],
    known_entities: &HashSet<String>,
) -> Result<(DomainModel, HashMap<String, String>)> {
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
    let mut cross_associations = Vec::new();
    for decl in decls {
        let from_id = entity_ids.get(&decl.name).expect("just inserted above").clone();
        for assoc in &decl.associations {
            if assoc.target.contains('.') {
                let target = assoc.target.clone();
                if !known_entities.contains(&target) {
                    return Err(WriterError::UnknownCrossModuleAssociationTarget(target));
                }
                cross_associations.push(Association {
                    id: None,
                    name: Some(assoc.name.clone()),
                    documentation: assoc.documentation.clone(),
                    from_entity_id: Some(from_id.clone()),
                    to_entity_id: Some(target),
                    association_type: assoc.association_type,
                    owner: assoc.owner,
                    storage_format: assoc.storage_format,
                    delete_behavior: None,
                    export_level: "Hidden".into(),
                });
                continue;
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

    Ok((DomainModel { documentation: String::new(), entities, associations, cross_associations }, entity_ids))
}

/// Re-syncs the association graph of an *existing* domain model against a
/// re-declared `entities` list, preserving every association's `$ID` (and
/// hand-authored `DeleteBehavior`) when its name matches a prior association
/// — mirrors `Writer#synchronize_ruby_domain_associations!`.
///
/// Deliberately narrower than the Ruby method's full scope: entity
/// structures (add/rename/remove attributes or entities) are **not**
/// synced here — every entity named in `entities` must already exist in the
/// on-disk domain model (same precondition mxrb's own method has: it's
/// always called *after* `synchronize_ruby_entity_structures!`, never
/// standalone). The `entities`/`Entities` array is read only to resolve
/// names to ids and is never rewritten, so unrelated entity content
/// (location, attributes, access rules, ...) is untouched — same as mxrb,
/// which only ever assigns `doc[associations_key]`/`doc[cross_key]`.
///
/// `known_entities` is the same project-wide `"Module.Entity"` validation
/// set `build_domain_model` takes, for cross-module targets.
pub fn synchronize_domain_associations(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    entities: &[EntityDecl],
    known_entities: &HashSet<String>,
) -> Result<()> {
    let dm_unit = mpr
        .units_by_containment("DomainModel")?
        .into_iter()
        .find(|u| u.container_id == module_id)
        .ok_or_else(|| WriterError::MissingDomainModel(module_name.to_string()))?;
    let dm_id = dm_unit.unit_id.clone();
    let mut doc = mpr.parse_contents(&dm_unit)?;

    let entities_key = native_key(&doc, "entities", "Entities");
    let entity_ids: HashMap<String, String> = match doc.get(entities_key) {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items))
            .items
            .into_iter()
            .filter_map(|b| match b {
                Bson::Document(d) => {
                    let entity = Entity::from_bson(&d);
                    Some((entity.name?, entity.id?))
                }
                _ => None,
            })
            .collect(),
        _ => HashMap::new(),
    };

    let missing: Vec<String> =
        entities.iter().map(|e| e.name.clone()).filter(|name| !entity_ids.contains_key(name)).collect();
    if !missing.is_empty() {
        return Err(WriterError::EntitiesMissingFromDomainModel { module_name: module_name.to_string(), missing });
    }
    let owned_ids: HashSet<&str> =
        entities.iter().filter_map(|e| entity_ids.get(&e.name).map(String::as_str)).collect();

    let associations_key = native_key(&doc, "associations", "Associations");
    let cross_key = native_key(&doc, "crossAssociations", "CrossAssociations");
    let local_raw = mxrs_bson::parse_array(array_field(&doc, associations_key));
    let cross_raw = mxrs_bson::parse_array(array_field(&doc, cross_key));

    let mut previous_by_name: HashMap<String, Association> = HashMap::new();
    for item in local_raw.items.iter().chain(cross_raw.items.iter()) {
        if let Bson::Document(d) = item {
            let assoc = Association::from_bson(d);
            if let Some(name) = &assoc.name {
                previous_by_name.insert(name.clone(), assoc);
            }
        }
    }

    let is_owned = |item: &Bson| -> bool {
        match item {
            Bson::Document(d) => {
                Association::from_bson(d).from_entity_id.is_some_and(|id| owned_ids.contains(id.as_str()))
            }
            _ => false,
        }
    };
    let mut new_local: Vec<Bson> = local_raw.items.into_iter().filter(|b| !is_owned(b)).collect();
    let mut new_cross: Vec<Bson> = cross_raw.items.into_iter().filter(|b| !is_owned(b)).collect();

    let mut declared_names: HashSet<String> = HashSet::new();
    for entity in entities {
        let from_id = entity_ids.get(&entity.name).expect("validated present above").clone();
        for assoc in &entity.associations {
            if !declared_names.insert(assoc.name.clone()) {
                return Err(WriterError::DuplicateAssociation {
                    module_name: module_name.to_string(),
                    name: assoc.name.clone(),
                });
            }

            let (target_module, target_name) = association_target(&assoc.target, module_name);
            let prior = previous_by_name.get(&assoc.name);
            let built = if target_module == module_name {
                let to_id = entity_ids
                    .get(&target_name)
                    .cloned()
                    .ok_or_else(|| WriterError::UnknownAssociationTarget(assoc.target.clone()))?;
                Association {
                    id: prior.and_then(|p| p.id.clone()),
                    name: Some(assoc.name.clone()),
                    documentation: assoc.documentation.clone(),
                    from_entity_id: Some(from_id.clone()),
                    to_entity_id: Some(to_id),
                    association_type: assoc.association_type,
                    owner: assoc.owner,
                    storage_format: assoc.storage_format,
                    delete_behavior: prior.and_then(|p| p.delete_behavior.clone()),
                    export_level: prior.map(|p| p.export_level.clone()).unwrap_or_else(|| "Hidden".into()),
                }
            } else {
                let qualified = format!("{target_module}.{target_name}");
                if !known_entities.contains(&qualified) {
                    return Err(WriterError::UnknownCrossModuleAssociationTarget(qualified));
                }
                Association {
                    id: prior.and_then(|p| p.id.clone()),
                    name: Some(assoc.name.clone()),
                    documentation: assoc.documentation.clone(),
                    from_entity_id: Some(from_id.clone()),
                    to_entity_id: Some(qualified),
                    association_type: assoc.association_type,
                    owner: assoc.owner,
                    storage_format: assoc.storage_format,
                    delete_behavior: prior.and_then(|p| p.delete_behavior.clone()),
                    export_level: prior.map(|p| p.export_level.clone()).unwrap_or_else(|| "Hidden".into()),
                }
            };

            if built.is_cross_module() {
                new_cross.push(Bson::Document(built.to_bson()));
            } else {
                new_local.push(Bson::Document(built.to_bson()));
            }
        }
    }

    doc.insert(associations_key, Bson::Array(mxrs_bson::build_array(new_local, local_raw.marker)));
    doc.insert(cross_key, Bson::Array(mxrs_bson::build_array(new_cross, cross_raw.marker)));
    mpr.update_unit(&dm_id, doc)?;
    Ok(())
}

/// Splits a declared association target into `(module, entity)` — a dotted
/// `"Module.Entity"` target names a cross-module entity, otherwise the
/// target is resolved against `default_module` (mirrors mxrb's
/// `Writer#association_target`).
fn association_target<'a>(target: &'a str, default_module: &'a str) -> (&'a str, String) {
    match target.split_once('.') {
        Some((module, name)) => (module, name.to_string()),
        None => (default_module, target.to_string()),
    }
}

/// Picks whichever key spelling is already present on `doc` (older/foreign
/// documents may use PascalCase), defaulting to `lower` since that's the
/// spelling `mxrs-writer` itself always creates (mirrors mxrb's
/// `Writer#native_key`, whose own default is the opposite for the same
/// reason — its own writer creates PascalCase).
fn native_key<'a>(doc: &Document, lower: &'a str, upper: &'a str) -> &'a str {
    if doc.contains_key(lower) {
        lower
    } else if doc.contains_key(upper) {
        upper
    } else {
        lower
    }
}

fn array_field<'a>(doc: &'a Document, key: &str) -> Option<&'a [Bson]> {
    match doc.get(key) {
        Some(Bson::Array(items)) => Some(items),
        _ => None,
    }
}
