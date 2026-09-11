//! `build_domain_model` resolves `mxrs_ir::{EntityDecl, AssociationDecl}`
//! (name-based) into a persistable `mxrs_model::DomainModel` (id-based) —
//! mirrors the entity-id assignment + association resolution done inline in
//! `Writer#write_domain_model`, narrowed to fresh-project creation (no
//! reconciliation against an existing domain model).
//!
//! `synchronize_domain_model` (composing `synchronize_domain_entities` +
//! `synchronize_domain_associations`) covers that same reconciliation for an
//! *already-existing* domain model — mirrors mxrb's own
//! `write_domain_model` (which mxrb itself reuses for both fresh creation
//! and incremental resync, unlike the narrower, entity-structure-only
//! `synchronize_ruby_entity_structures!`). See each function's doc comment
//! for exactly what's preserved vs. re-derived, and what's still not ported
//! (indexes/access-rules/lifecycle/generalization-target reconciliation —
//! `EntityDecl` has no DSL surface for those yet, so existing entities keep
//! whatever they already had for those fields, verbatim).
//!
//! Cross-module associations mirror mxrb's own `cross_association_doc`:
//! unlike same-module associations, the target is **not** resolved to an
//! id — it's persisted as the literal `"Module.Entity"` qualified-name
//! string in a `Child` field, and the association is routed into
//! `DomainModel::cross_associations` rather than `associations`
//! (`Association::to_bson` picks the `DomainModels$CrossAssociation` BSON
//! shape automatically whenever `to_entity_id` contains a `.`). The caller
//! still validates the target against `known_entities` — every
//! `"Module.Entity"` qualified name declared anywhere in the project — so a
//! typo'd or renamed cross-module target fails at write time rather than
//! producing a `.mpr` Studio Pro can't open.
//!
//! Routing between the two is **not** "does the target string contain a
//! dot" — `mxrs_dsl::EntityBuilder::association` always emits a
//! fully-qualified `"Module.Entity"` target now (via `Ref<M>::qualified_name()`,
//! see `mxrs_ir::markers`), so a same-module association is dotted too.
//! [`resolve_association`] instead splits the target into `(module, name)`
//! and compares the module against the association's *own* declaring
//! module — see its doc comment.

use std::collections::{HashMap, HashSet};

use mxrs_bson::{Bson, Document};
use mxrs_ir::declaration::{
    AssociationOwner, AssociationStorage, AssociationType, AttributeDecl, AttributeType, EntityDecl,
};
use mxrs_model::association::{
    Association, AssociationType as ModelAssociationType, Owner, StorageFormat,
};
use mxrs_model::entity::{Entity, Location, SystemMembers};
use mxrs_model::{Attribute, AttributeType as ModelAttributeType, DomainModel};
use mxrs_mpr::{MprFile, RawUnit};

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
        entities.push(fresh_entity(module_name, decl, id));
    }

    let mut associations = Vec::new();
    let mut cross_associations = Vec::new();
    for decl in decls {
        let from_id = entity_ids
            .get(&decl.name)
            .expect("just inserted above")
            .clone();
        for assoc in &decl.associations {
            let built = resolve_association(
                assoc,
                module_name,
                &from_id,
                &entity_ids,
                known_entities,
                None,
            )?;
            if built.is_cross_module() {
                cross_associations.push(built);
            } else {
                associations.push(built);
            }
        }
    }

    Ok((
        DomainModel {
            id: None,
            native_type: None,
            documentation: String::new(),
            entities,
            associations,
            cross_associations,
        },
        entity_ids,
    ))
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
    let dm_unit = find_domain_model_unit(mpr, module_id, module_name)?;
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

    let missing: Vec<String> = entities
        .iter()
        .map(|e| e.name.clone())
        .filter(|name| !entity_ids.contains_key(name))
        .collect();
    if !missing.is_empty() {
        return Err(WriterError::EntitiesMissingFromDomainModel {
            module_name: module_name.to_string(),
            missing,
        });
    }
    let owned_ids: HashSet<&str> = entities
        .iter()
        .filter_map(|e| entity_ids.get(&e.name).map(String::as_str))
        .collect();

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
            Bson::Document(d) => Association::from_bson(d)
                .from_entity_id
                .is_some_and(|id| owned_ids.contains(id.as_str())),
            _ => false,
        }
    };
    let mut new_local: Vec<Bson> = local_raw
        .items
        .into_iter()
        .filter(|b| !is_owned(b))
        .collect();
    let mut new_cross: Vec<Bson> = cross_raw
        .items
        .into_iter()
        .filter(|b| !is_owned(b))
        .collect();

    let mut declared_names: HashSet<String> = HashSet::new();
    for entity in entities {
        let from_id = entity_ids
            .get(&entity.name)
            .expect("validated present above")
            .clone();
        for assoc in &entity.associations {
            if !declared_names.insert(assoc.name.clone()) {
                return Err(WriterError::DuplicateAssociation {
                    module_name: module_name.to_string(),
                    name: assoc.name.clone(),
                });
            }

            let prior = previous_by_name.get(&assoc.name);
            let built = resolve_association(
                assoc,
                module_name,
                &from_id,
                &entity_ids,
                known_entities,
                prior,
            )?;

            if built.is_cross_module() {
                new_cross.push(Bson::Document(built.to_bson()));
            } else {
                new_local.push(Bson::Document(built.to_bson()));
            }
        }
    }

    doc.insert(
        associations_key,
        Bson::Array(mxrs_bson::build_array(new_local, local_raw.marker)),
    );
    doc.insert(
        cross_key,
        Bson::Array(mxrs_bson::build_array(new_cross, cross_raw.marker)),
    );
    mpr.update_unit(&dm_id, doc)?;
    Ok(())
}

/// Splits a declared association target into `(module, entity)` — a dotted
/// `"Module.Entity"` target names a cross-module entity, otherwise the
/// target is resolved against `default_module` (mirrors mxrb's
/// `Writer#association_target`).
///
/// Critically, a *dotted* target whose module happens to equal
/// `default_module` is still local — `mxrs_dsl::EntityBuilder::association`
/// now always emits a fully-qualified `"Module.Entity"` target (via
/// `Ref<M>::qualified_name()`, see `mxrs_ir::markers`), so same-module
/// associations routinely arrive dotted too. Routing on "contains a dot"
/// instead of "resolves to a different module" would misfile every
/// same-module association through the cross-module path, emitting the
/// wrong BSON shape (`DomainModels$CrossAssociation` with a qualified-name
/// `Child` field, instead of `DomainModels$Association` with a `ChildID`
/// pointer) for what Studio Pro expects to be a same-module association.
fn association_target<'a>(target: &'a str, default_module: &'a str) -> (&'a str, String) {
    match target.split_once('.') {
        Some((module, name)) => (module, name.to_string()),
        None => (default_module, target.to_string()),
    }
}

/// Resolves one declared association against already-assigned entity ids,
/// shared by [`build_domain_model`] (fresh creation, `prior: None`) and
/// [`synchronize_domain_associations`] (`prior: Some(existing)` preserves
/// `$ID`/`DeleteBehavior` on a name match) — the only difference between
/// the two call sites is where `prior` comes from.
fn resolve_association(
    assoc: &mxrs_ir::declaration::AssociationDecl,
    module_name: &str,
    from_id: &str,
    entity_ids: &HashMap<String, String>,
    known_entities: &HashSet<String>,
    prior: Option<&Association>,
) -> Result<Association> {
    let (target_module, target_name) = association_target(&assoc.target, module_name);
    let to_entity_id = if target_module == module_name {
        entity_ids
            .get(&target_name)
            .cloned()
            .ok_or_else(|| WriterError::UnknownAssociationTarget(assoc.target.clone()))?
    } else {
        let qualified = format!("{target_module}.{target_name}");
        if !known_entities.contains(&qualified) {
            return Err(WriterError::UnknownCrossModuleAssociationTarget(qualified));
        }
        qualified
    };

    Ok(Association {
        id: prior.and_then(|p| p.id.clone()),
        name: Some(assoc.name.clone()),
        documentation: assoc.documentation.clone(),
        from_entity_id: Some(from_id.to_string()),
        to_entity_id: Some(to_entity_id),
        association_type: model_association_type(assoc.association_type),
        owner: model_association_owner(assoc.owner),
        storage_format: model_association_storage(assoc.storage),
        source: prior.and_then(|p| p.source.clone()),
        guid: prior.and_then(|p| p.guid.clone()),
        delete_behavior: prior.and_then(|p| p.delete_behavior.clone()),
        export_level: prior
            .map(|p| p.export_level.clone())
            .unwrap_or_else(|| "Hidden".into()),
    })
}

fn model_association_type(value: AssociationType) -> ModelAssociationType {
    match value {
        AssociationType::Reference => ModelAssociationType::Reference,
        AssociationType::ReferenceSet => ModelAssociationType::ReferenceSet,
    }
}

fn model_association_owner(value: AssociationOwner) -> Owner {
    match value {
        AssociationOwner::Default => Owner::Default,
        AssociationOwner::Both => Owner::Both,
    }
}

fn model_association_storage(value: AssociationStorage) -> StorageFormat {
    match value {
        AssociationStorage::Column => StorageFormat::Column,
        AssociationStorage::Table => StorageFormat::Table,
    }
}

fn model_attribute_type(value: AttributeType) -> ModelAttributeType {
    match value {
        AttributeType::String => ModelAttributeType::String,
        AttributeType::Integer => ModelAttributeType::Integer,
        AttributeType::Long => ModelAttributeType::Long,
        AttributeType::Float => ModelAttributeType::Float,
        AttributeType::Decimal => ModelAttributeType::Decimal,
        AttributeType::Boolean => ModelAttributeType::Boolean,
        AttributeType::DateTime => ModelAttributeType::DateTime,
        AttributeType::AutoNumber => ModelAttributeType::AutoNumber,
        AttributeType::HashString => ModelAttributeType::HashString,
        AttributeType::Binary => ModelAttributeType::Binary,
        AttributeType::Enumeration => ModelAttributeType::Enum,
    }
}

fn model_attribute(decl: &AttributeDecl) -> Attribute {
    Attribute {
        id: None,
        name: Some(decl.name.clone()),
        documentation: decl.documentation.clone(),
        attribute_type: model_attribute_type(decl.attribute_type),
        default_value: decl.default_value.clone(),
        data_storage_guid: None,
        export_level: "Hidden".into(),
        raw_type_doc: None,
        raw_value_doc: None,
        length: decl.length,
        localize_date: decl.localize_date,
        enumeration: decl.enumeration.clone(),
        required: decl.required,
        unique: decl.unique,
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

fn find_domain_model_unit(mpr: &MprFile, module_id: &str, module_name: &str) -> Result<RawUnit> {
    mpr.units_by_containment("DomainModel")?
        .into_iter()
        .find(|u| u.container_id == module_id)
        .ok_or_else(|| WriterError::MissingDomainModel(module_name.to_string()))
}

/// Builds a brand-new `Entity` for a declaration with no prior on-disk
/// counterpart — shared by `build_domain_model` (every entity is new) and
/// `synchronize_domain_entities` (only entities absent from the existing
/// domain model take this path). Mirrors `Writer#entity_doc`'s `previous:
/// nil` branch, narrowed to what `EntityDecl` exposes today (no
/// indexes/access-rules/lifecycle/generalization-target DSL surface yet).
fn fresh_entity(module_name: &str, decl: &EntityDecl, id: String) -> Entity {
    Entity {
        id: Some(id),
        name: Some(decl.name.clone()),
        qualified_name: Some(format!("{module_name}.{}", decl.name)),
        documentation: decl.documentation.clone(),
        persistable: decl.persistable,
        location: Location { x: 0, y: 0 },
        data_storage_guid: None,
        image: None,
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
        attributes: decl.attributes.iter().map(model_attribute).collect(),
    }
}

/// Reads whichever of `"name"`/`"Name"` a raw entity/attribute doc carries.
fn doc_name(doc: &Document) -> Option<String> {
    match doc.get("name").or_else(|| doc.get("Name")) {
        Some(Bson::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Rebuilds one attribute doc for a declared `AttributeDecl`, preserving the
/// prior attribute's `$ID`/`dataStorageGuid` when its name matches — every
/// other field (type, default, length, ...) is fully re-derived from the
/// storage-independent declaration every time. A declaration always carries
/// a complete value, not a partial override.
fn reconcile_attribute_doc(decl: &AttributeDecl, previous: Option<&Document>) -> Document {
    let decl = model_attribute(decl);
    match previous {
        Some(prev) => {
            let prior = Attribute::from_bson(prev);
            Attribute {
                id: prior.id,
                data_storage_guid: prior.data_storage_guid,
                ..decl
            }
            .to_bson()
        }
        None => decl.to_bson(),
    }
}

/// Rebuilds one entity doc for a declared `EntityDecl` against its prior
/// on-disk counterpart (`None` for a brand-new entity). For an existing
/// entity, everything **not** explicitly re-declared — `location`,
/// `generalization`, `accessRules`, `indexes`, `eventHandlers`,
/// `validationRules`, `source`/`oqlQuery`, and any unknown/foreign fields —
/// survives untouched, because this merges onto a clone of the prior raw
/// document rather than rebuilding it from `Entity::to_bson` (which has no
/// surface to round-trip those fields — see its doc comment). Only `name`,
/// `documentation`, and the reconciled `attributes` array are overwritten.
/// Mirrors `Writer#entity_doc`, narrowed the same way `fresh_entity` is.
fn build_entity_doc(
    module_name: &str,
    decl: &EntityDecl,
    previous: Option<&Document>,
    id: String,
    index: usize,
) -> Document {
    let Some(prev) = previous else {
        return fresh_entity(module_name, decl, id).to_bson();
    };

    let mut out = prev.clone();
    out.insert("$ID", id);
    let name_key = native_key(prev, "name", "Name");
    out.insert(name_key, decl.name.clone());
    let doc_key = native_key(prev, "documentation", "Documentation");
    out.insert(doc_key, decl.documentation.clone());

    let attrs_key = native_key(prev, "attributes", "Attributes");
    let prev_attrs_raw = mxrs_bson::parse_array(array_field(prev, attrs_key));
    let prev_attrs_by_name: HashMap<String, Document> = prev_attrs_raw
        .items
        .iter()
        .filter_map(|b| match b {
            Bson::Document(d) => doc_name(d).map(|name| (name, d.clone())),
            _ => None,
        })
        .collect();
    let new_attrs: Vec<Bson> = decl
        .attributes
        .iter()
        .map(|a| {
            let prior = prev_attrs_by_name.get(&a.name);
            Bson::Document(reconcile_attribute_doc(a, prior))
        })
        .collect();
    out.insert(
        attrs_key,
        Bson::Array(mxrs_bson::build_array(new_attrs, prev_attrs_raw.marker)),
    );
    let _ = index; // reserved: mxrb positions brand-new entities by index; fresh_entity already defaults to (0, 0)
    out
}

/// Re-syncs the entity graph of an *existing* domain model against a
/// re-declared `entities` list, mirroring the entity-add/rename/remove half
/// of `Writer#write_domain_model` (the same method mxrb's own writer uses
/// for both fresh creation and incremental resync — unlike
/// `synchronize_ruby_entity_structures!`, which only ever mutates entities
/// that already exist by name).
///
/// - An entity whose name matches an existing one is reconciled via
///   [`build_entity_doc`]: its `$ID` and everything not re-declared here
///   survive; its attributes are reconciled by name (added, removed, or
///   `$ID`-preserved on an unchanged name) via [`reconcile_attribute_doc`].
/// - An entity whose name has no existing match is created fresh via
///   [`fresh_entity`] (new `$ID`, `Location { 0, 0 }` — same simplification
///   `build_domain_model` already makes for brand-new projects).
/// - An existing entity **not** named in `entities` is dropped — `entities`
///   is the complete authoritative list for the module, exactly as in
///   `write_domain_model` (which only ever emits `mod.fetch(:entities).map`,
///   never merges in leftover prior entities). Associations sourced from a
///   dropped entity are not cascade-removed here either, matching mxrb's own
///   behavior (`write_domain_model` doesn't clean those up on entity
///   removal — cascading is the caller's responsibility, same as upstream).
///
/// Returns the resulting `name -> id` map, so a caller can pass it straight
/// into [`synchronize_domain_associations`] (or the combined
/// [`synchronize_domain_model`] entry point does that already).
pub fn synchronize_domain_entities(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    entities: &[EntityDecl],
) -> Result<HashMap<String, String>> {
    let dm_unit = find_domain_model_unit(mpr, module_id, module_name)?;
    let dm_id = dm_unit.unit_id.clone();
    let mut doc = mpr.parse_contents(&dm_unit)?;

    let entities_key = native_key(&doc, "entities", "Entities");
    let existing_raw = mxrs_bson::parse_array(array_field(&doc, entities_key));
    let mut existing_by_name: HashMap<String, Document> = HashMap::new();
    for item in &existing_raw.items {
        if let Bson::Document(d) = item
            && let Some(name) = doc_name(d)
        {
            existing_by_name.insert(name, d.clone());
        }
    }

    let mut declared_names: HashSet<String> = HashSet::new();
    let mut entity_ids = HashMap::new();
    let mut new_items = Vec::with_capacity(entities.len());
    for (index, decl) in entities.iter().enumerate() {
        if !declared_names.insert(decl.name.clone()) {
            return Err(WriterError::DuplicateEntity {
                module_name: module_name.to_string(),
                name: decl.name.clone(),
            });
        }

        let previous = existing_by_name.get(&decl.name);
        let id = match previous.and_then(|p| p.get("$ID")) {
            Some(v) => mxrs_bson::extract_id(v).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            None => uuid::Uuid::new_v4().to_string(),
        };
        entity_ids.insert(decl.name.clone(), id.clone());
        new_items.push(Bson::Document(build_entity_doc(
            module_name,
            decl,
            previous,
            id,
            index,
        )));
    }

    doc.insert(
        entities_key,
        Bson::Array(mxrs_bson::build_array(new_items, existing_raw.marker)),
    );
    mpr.update_unit(&dm_id, doc)?;
    Ok(entity_ids)
}

/// Combined incremental re-sync entry point for one module's domain model:
/// runs [`synchronize_domain_entities`] then [`synchronize_domain_associations`]
/// against the same `.mpr`, mirroring the two-phase order mxrb itself
/// requires (`synchronize_ruby_entity_structures!` before
/// `synchronize_ruby_domain_associations!`) — associations resolve targets
/// against whatever entity ids exist *after* the entity pass, so brand-new
/// entities declared in the same call are valid association targets.
pub fn synchronize_domain_model(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    entities: &[EntityDecl],
    known_entities: &HashSet<String>,
) -> Result<()> {
    synchronize_domain_entities(mpr, module_id, module_name, entities)?;
    synchronize_domain_associations(mpr, module_id, module_name, entities, known_entities)?;
    Ok(())
}
