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
//! for exactly what's preserved vs. re-derived. Generalization, system
//! members, indexes, lifecycle callbacks, and access rules have typed
//! declaration surfaces. Collections are three-state: absence preserves an
//! imported value, while a declaration — including an explicitly empty one —
//! is authoritative. Future-schema fields remain in lossless nested sidecars.
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
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::declaration::{
    AccessMemberKind, AccessRuleDecl, AssociationOwner, AssociationStorage, AssociationType,
    AttributeDecl, AttributeType, EntityDecl, EntityImageDecl, EntityIndexDecl,
    EntityInheritanceDecl, EntitySourceDecl, IndexMemberDecl, LifecycleDecl, MemberRights,
    SystemMember,
};
use mxrs_model::association::{
    Association, AssociationType as ModelAssociationType, Owner, StorageFormat,
};
use mxrs_model::entity::{
    AccessMember as ModelAccessMember, AccessMemberKind as ModelAccessMemberKind,
    AccessRule as ModelAccessRule, Entity, EntityIndex, Generalization, IndexMemberKind,
    IndexedAttribute, IndexedSystemMember, LifecycleCallback, Location, SystemMembers,
    access_rule_bson,
};
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
    identity: ProjectIdentity,
) -> Result<(DomainModel, HashMap<String, String>)> {
    validate_entity_extras(module_name, decls, Some(known_entities))?;
    let mut entity_ids = HashMap::new();
    let mut entities = Vec::with_capacity(decls.len());
    for decl in decls {
        let qualified_name = format!("{module_name}.{}", decl.name);
        let id = identity.artifact_id(ArtifactKind::Entity, &qualified_name);
        entity_ids.insert(decl.name.clone(), id.clone());
        entities.push(fresh_entity(module_name, decl, id, identity)?);
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
                identity,
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
    let identity = project_identity(mpr)?;
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
    let mut declared_names = HashSet::new();
    for entity in entities {
        for association in &entity.associations {
            if !declared_names.insert(association.name.clone()) {
                return Err(WriterError::DuplicateAssociation {
                    module_name: module_name.to_string(),
                    name: association.name.clone(),
                });
            }
        }
    }

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
    let preserve_unmodeled_external = |item: &Bson| -> bool {
        let Bson::Document(document) = item else {
            return false;
        };
        let association = Association::from_bson(document);
        let Some(name) = association.name.as_deref() else {
            return false;
        };
        let Some(target) = association.to_entity_id.as_deref() else {
            return false;
        };
        !declared_names.contains(name) && target.contains('.') && !known_entities.contains(target)
    };
    let mut new_local: Vec<Bson> = local_raw
        .items
        .into_iter()
        .filter(|item| !is_owned(item) || preserve_unmodeled_external(item))
        .collect();
    let mut new_cross: Vec<Bson> = cross_raw
        .items
        .into_iter()
        .filter(|item| !is_owned(item) || preserve_unmodeled_external(item))
        .collect();

    for entity in entities {
        let from_id = entity_ids
            .get(&entity.name)
            .expect("validated present above")
            .clone();
        for assoc in &entity.associations {
            let prior = previous_by_name.get(&assoc.name);
            let built = resolve_association(
                assoc,
                module_name,
                &from_id,
                &entity_ids,
                known_entities,
                prior,
                identity,
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
    identity: ProjectIdentity,
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
        id: prior.and_then(|p| p.id.clone()).or_else(|| {
            Some(identity.artifact_id(
                ArtifactKind::Association,
                &format!("{module_name}.{}", assoc.name),
            ))
        }),
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

fn model_attribute(
    decl: &AttributeDecl,
    id: Option<String>,
    data_storage_guid: Option<String>,
) -> Attribute {
    Attribute {
        id,
        name: Some(decl.name.clone()),
        documentation: decl.documentation.clone(),
        attribute_type: model_attribute_type(decl.attribute_type),
        default_value: decl.default_value.clone(),
        data_storage_guid,
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
/// nil` branch, including the complete typed entity structure and behavior
/// declarations.
fn fresh_entity(
    module_name: &str,
    decl: &EntityDecl,
    id: String,
    identity: ProjectIdentity,
) -> Result<Entity> {
    let entity_name = format!("{module_name}.{}", decl.name);
    let validation_rules =
        reconcile_validation_rules(module_name, &decl.name, &decl.attributes, vec![], identity);
    let mut attributes: Vec<Attribute> = decl
        .attributes
        .iter()
        .map(|attribute| {
            let qualified_name = format!("{entity_name}.{}", attribute.name);
            model_attribute(
                attribute,
                Some(identity.artifact_id(ArtifactKind::Attribute, &qualified_name)),
                Some(identity.artifact_id(ArtifactKind::DataStorage, &qualified_name)),
            )
        })
        .collect();
    if matches!(decl.source.as_ref(), Some(EntitySourceDecl::OqlView { .. })) {
        for attribute in &mut attributes {
            let name = attribute.name.as_deref().unwrap_or("Unnamed");
            attribute.raw_value_doc = Some(mxrs_bson::doc! {
                "$ID": identity.artifact_id(
                    ArtifactKind::OqlViewValue,
                    &format!("{entity_name}.{name}"),
                ),
                "$Type": "DomainModels$OqlViewValue",
                "Reference": name,
            });
        }
    }
    let attribute_ids = attributes
        .iter()
        .filter_map(|attribute| Some((attribute.name.clone()?, attribute.id.clone()?)))
        .collect();
    Ok(Entity {
        id: Some(id),
        name: Some(decl.name.clone()),
        qualified_name: Some(format!("{module_name}.{}", decl.name)),
        documentation: decl.documentation.clone(),
        persistable: decl.persistable,
        location: Location { x: 0, y: 0 },
        // The table's identity, as stable as the entity's own: a runtime
        // that keeps data by it finds the same table after every build.
        data_storage_guid: Some(identity.artifact_id(ArtifactKind::DataStorage, &entity_name)),
        image: Some(match decl.image.as_ref() {
            Some(EntityImageDecl::Reference(reference)) => reference.clone(),
            Some(EntityImageDecl::None) | None => String::new(),
        }),
        export_level: "Hidden".into(),
        generalization: Some(reconcile_generalization(
            module_name,
            &decl.name,
            decl.persistable,
            decl.inheritance.as_ref(),
            None,
            identity,
        )),
        access_rules: reconcile_access_rules(
            module_name,
            &decl.name,
            decl.access_rules.as_deref(),
            &[],
            identity,
        )?
        .unwrap_or_default(),
        indexes: reconcile_indexes(
            module_name,
            &decl.name,
            decl.indexes.as_deref().unwrap_or(&[]),
            &[],
            &attribute_ids,
            identity,
        )?,
        system_members: declared_system_members(decl.inheritance.as_ref()),
        lifecycle: reconcile_lifecycle(
            module_name,
            &decl.name,
            decl.lifecycle.as_deref().unwrap_or(&[]),
            &[],
            identity,
        )?,
        validation_rules,
        source: match decl.source.as_ref() {
            Some(source @ EntitySourceDecl::OqlView { .. }) => Some(entity_source_document(
                module_name,
                &decl.name,
                source,
                None,
                identity,
            )),
            Some(EntitySourceDecl::Stored) | None => None,
        },
        oql_query: None,
        native_type: Some(
            match decl.source.as_ref() {
                Some(EntitySourceDecl::OqlView { .. }) => "DomainModels$ViewEntity",
                Some(EntitySourceDecl::Stored) | None => "DomainModels$EntityImpl",
            }
            .to_string(),
        ),
        attributes,
    })
}

fn entity_source_document(
    module_name: &str,
    entity_name: &str,
    source: &EntitySourceDecl,
    previous: Option<&Document>,
    identity: ProjectIdentity,
) -> Document {
    match source {
        EntitySourceDecl::Stored => Document::new(),
        EntitySourceDecl::OqlView { source_document } => {
            let qualified_source = if source_document.contains('.') {
                source_document.clone()
            } else {
                format!("{module_name}.{source_document}")
            };
            let mut document = previous.cloned().unwrap_or_default();
            document.insert(
                "$ID",
                previous
                    .and_then(|document| document.get("$ID"))
                    .and_then(mxrs_bson::extract_id)
                    .unwrap_or_else(|| {
                        identity.artifact_id(
                            ArtifactKind::EntitySource,
                            &format!("{module_name}.{entity_name}"),
                        )
                    }),
            );
            document.insert("$Type", "DomainModels$OqlViewEntitySource");
            let source_key = native_key(&document, "sourceDocument", "SourceDocument");
            document.insert(source_key, qualified_source);
            document
        }
    }
}

/// Reads whichever of `"name"`/`"Name"` a raw entity/attribute doc carries.
fn doc_name(doc: &Document) -> Option<String> {
    match doc.get("name").or_else(|| doc.get("Name")) {
        Some(Bson::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn validation_rule_key(document: &Document) -> Option<(String, &'static str)> {
    let attribute = document
        .get_str("Attribute")
        .ok()?
        .rsplit('.')
        .next()?
        .to_string();
    let rule_type = document
        .get_document("RuleInfo")
        .ok()?
        .get_str("$Type")
        .ok()?;
    let kind = if rule_type.ends_with("RequiredRuleInfo") {
        "required"
    } else if rule_type.ends_with("UniqueRuleInfo") {
        "unique"
    } else {
        return None;
    };
    Some((attribute, kind))
}

fn new_validation_rule(
    module_name: &str,
    entity_name: &str,
    attribute_name: &str,
    kind: &'static str,
    identity: ProjectIdentity,
) -> Document {
    let qualified_attribute = format!("{module_name}.{entity_name}.{attribute_name}");
    let identity_name = format!("{qualified_attribute}.{kind}");
    let description = if kind == "required" {
        "is required"
    } else {
        "must be unique"
    };
    mxrs_bson::doc! {
        "$ID": identity.artifact_id(ArtifactKind::ValidationRule, &identity_name),
        "$Type": "DomainModels$ValidationRule",
        "Attribute": qualified_attribute,
        "Message": {
            "$ID": identity.artifact_id(ArtifactKind::ValidationRule, &format!("{identity_name}.message")),
            "$Type": "Texts$Text",
            "Items": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                "$ID": identity.artifact_id(ArtifactKind::ValidationRule, &format!("{identity_name}.translation.en_US")),
                "$Type": "Texts$Translation",
                "LanguageCode": "en_US",
                "Text": format!("{attribute_name} {description}"),
            })], 3),
        },
        "RuleInfo": {
            "$ID": identity.artifact_id(ArtifactKind::ValidationRule, &format!("{identity_name}.info")),
            "$Type": if kind == "required" {
                "DomainModels$RequiredRuleInfo"
            } else {
                "DomainModels$UniqueRuleInfo"
            },
        },
    }
}

fn reconcile_validation_rules(
    module_name: &str,
    entity_name: &str,
    attributes: &[AttributeDecl],
    previous: Vec<Document>,
    identity: ProjectIdentity,
) -> Vec<Document> {
    let mut controlled = HashMap::new();
    let mut output = Vec::new();
    for rule in previous {
        if let Some(key) = validation_rule_key(&rule) {
            controlled.insert(key, rule);
        } else {
            output.push(rule);
        }
    }
    for attribute in attributes {
        for (kind, enabled) in [
            ("required", attribute.required),
            ("unique", attribute.unique),
        ] {
            if enabled {
                output.push(
                    controlled
                        .remove(&(attribute.name.clone(), kind))
                        .unwrap_or_else(|| {
                            new_validation_rule(
                                module_name,
                                entity_name,
                                &attribute.name,
                                kind,
                                identity,
                            )
                        }),
                );
            }
        }
    }
    output
}

fn declared_system_members(inheritance: Option<&EntityInheritanceDecl>) -> SystemMembers {
    match inheritance {
        Some(EntityInheritanceDecl::Root(members)) => SystemMembers {
            owner: members.owner,
            created_date: members.created_date,
            changed_date: members.changed_date,
            changed_by: members.changed_by,
        },
        _ => SystemMembers::default(),
    }
}

fn validate_entity_extras(
    module_name: &str,
    entities: &[EntityDecl],
    known_entities: Option<&HashSet<String>>,
) -> Result<()> {
    for entity in entities {
        if let Some(EntityInheritanceDecl::Generalizes(target)) = &entity.inheritance
            && (target.is_empty()
                || known_entities
                    .is_some_and(|known| !target.starts_with("System.") && !known.contains(target)))
        {
            return Err(WriterError::UnknownGeneralizationTarget {
                module_name: module_name.to_string(),
                name: entity.name.clone(),
                target: target.clone(),
            });
        }

        let attribute_names: HashSet<&str> = entity
            .attributes
            .iter()
            .map(|attribute| attribute.name.as_str())
            .collect();
        let mut signatures = HashSet::new();
        for index in entity.indexes.as_deref().unwrap_or(&[]) {
            if index.members.is_empty() {
                return Err(WriterError::EmptyEntityIndex {
                    module_name: module_name.to_string(),
                    name: entity.name.clone(),
                });
            }
            let members: Vec<String> = index
                .members
                .iter()
                .map(|member| match member {
                    IndexMemberDecl::Attribute { name, .. } => format!("attribute:{name}"),
                    IndexMemberDecl::System { member, .. } => {
                        format!("system:{}", member.native_name())
                    }
                })
                .collect();
            for member in &index.members {
                if let IndexMemberDecl::Attribute { name, .. } = member
                    && !attribute_names.contains(name.as_str())
                {
                    return Err(WriterError::UnknownIndexedAttribute {
                        module_name: module_name.to_string(),
                        name: entity.name.clone(),
                        attribute: name.clone(),
                    });
                }
            }
            if !signatures.insert(members.clone()) {
                return Err(WriterError::DuplicateEntityIndex {
                    module_name: module_name.to_string(),
                    name: entity.name.clone(),
                    members,
                });
            }
        }

        let mut events = HashSet::new();
        for callback in entity.lifecycle.as_deref().unwrap_or(&[]) {
            if callback.handler.is_empty() {
                return Err(WriterError::EmptyLifecycleHandler {
                    module_name: module_name.to_string(),
                    name: entity.name.clone(),
                    event: callback.event.rust_name().to_string(),
                });
            }
            if !events.insert(callback.event) {
                return Err(WriterError::DuplicateLifecycleEvent {
                    module_name: module_name.to_string(),
                    name: entity.name.clone(),
                    event: callback.event.rust_name().to_string(),
                });
            }
        }
    }
    Ok(())
}

fn reconcile_generalization(
    module_name: &str,
    entity_name: &str,
    persistable: bool,
    declared: Option<&EntityInheritanceDecl>,
    previous: Option<&Generalization>,
    identity: ProjectIdentity,
) -> Generalization {
    let qualified = format!("{module_name}.{entity_name}");
    let id = previous
        .and_then(|generalization| generalization.id.clone())
        .unwrap_or_else(|| identity.artifact_id(ArtifactKind::EntityGeneralization, &qualified));
    let raw = previous
        .map(|generalization| generalization.raw.clone())
        .unwrap_or_default();
    match declared {
        Some(EntityInheritanceDecl::Generalizes(target)) => Generalization {
            id: Some(id),
            native_type: "DomainModels$Generalization".to_string(),
            target: Some(target.clone()),
            persistable: None,
            system_members: SystemMembers::default(),
            raw,
        },
        Some(EntityInheritanceDecl::Root(_)) | None => Generalization {
            id: Some(id),
            native_type: "DomainModels$NoGeneralization".to_string(),
            target: None,
            persistable: Some(persistable),
            system_members: declared_system_members(declared),
            raw,
        },
    }
}

fn index_decl_signature(index: &EntityIndexDecl) -> String {
    index
        .members
        .iter()
        .map(|member| match member {
            IndexMemberDecl::Attribute { name, .. } => format!("attribute:{name}"),
            IndexMemberDecl::System { member, .. } => {
                format!("system:{}", member.native_name())
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn model_index_signature(index: &EntityIndex) -> Option<String> {
    if index.raw.get_str("$Type").ok() != Some("DomainModels$EntityIndex") {
        return None;
    }
    index
        .members
        .iter()
        .map(|member| match &member.kind {
            IndexMemberKind::Attribute(name) => Some(format!(
                "attribute:{}",
                name.rsplit('.').next().unwrap_or(name)
            )),
            IndexMemberKind::System(system) => Some(format!("system:{}", system.native_name())),
            IndexMemberKind::Unresolved(_) => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|members| members.join("+"))
}

fn model_system_member(member: SystemMember) -> IndexedSystemMember {
    match member {
        SystemMember::CreatedDate => IndexedSystemMember::CreatedDate,
        SystemMember::ChangedDate => IndexedSystemMember::ChangedDate,
        SystemMember::Owner => IndexedSystemMember::Owner,
        SystemMember::ChangedBy => IndexedSystemMember::ChangedBy,
    }
}

fn reconcile_indexes(
    module_name: &str,
    entity_name: &str,
    declared: &[EntityIndexDecl],
    previous: &[EntityIndex],
    attribute_ids: &HashMap<String, String>,
    identity: ProjectIdentity,
) -> Result<Vec<EntityIndex>> {
    let qualified = format!("{module_name}.{entity_name}");
    let previous_by_signature: HashMap<String, &EntityIndex> = previous
        .iter()
        .filter_map(|index| Some((model_index_signature(index)?, index)))
        .collect();
    let mut output = Vec::with_capacity(declared.len());
    for index in declared {
        let signature = index_decl_signature(index);
        let prior = previous_by_signature.get(&signature).copied();
        let index_identity = format!("{qualified}#{signature}");
        let prior_members: HashMap<String, &IndexedAttribute> = prior
            .into_iter()
            .flat_map(|index| &index.members)
            .filter_map(|member| {
                let key = match &member.kind {
                    IndexMemberKind::Attribute(name) => {
                        format!("attribute:{}", name.rsplit('.').next().unwrap_or(name))
                    }
                    IndexMemberKind::System(system) => format!("system:{}", system.native_name()),
                    IndexMemberKind::Unresolved(_) => return None,
                };
                Some((key, member))
            })
            .collect();
        let members = index
            .members
            .iter()
            .map(|member| {
                let (key, kind, pointer, ascending) = match member {
                    IndexMemberDecl::Attribute { name, ascending } => (
                        format!("attribute:{name}"),
                        IndexMemberKind::Attribute(format!("{qualified}.{name}")),
                        Some(attribute_ids.get(name).cloned().ok_or_else(|| {
                            WriterError::UnknownIndexedAttribute {
                                module_name: module_name.to_string(),
                                name: entity_name.to_string(),
                                attribute: name.clone(),
                            }
                        })?),
                        *ascending,
                    ),
                    IndexMemberDecl::System { member, ascending } => (
                        format!("system:{}", member.native_name()),
                        IndexMemberKind::System(model_system_member(*member)),
                        Some("00000000-0000-0000-0000-000000000000".to_string()),
                        *ascending,
                    ),
                };
                let prior = prior_members.get(&key).copied();
                Ok(IndexedAttribute {
                    id: Some(
                        prior
                            .and_then(|member| member.id.clone())
                            .unwrap_or_else(|| {
                                identity.artifact_id(
                                    ArtifactKind::EntityIndexMember,
                                    &format!("{index_identity}.{key}"),
                                )
                            }),
                    ),
                    kind,
                    attribute_pointer: pointer,
                    ascending,
                    raw: prior.map(|member| member.raw.clone()).unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        output.push(EntityIndex {
            id: Some(prior.and_then(|index| index.id.clone()).unwrap_or_else(|| {
                identity.artifact_id(ArtifactKind::EntityIndex, &index_identity)
            })),
            guid: Some(
                prior
                    .and_then(|index| index.guid.clone())
                    .unwrap_or_else(|| {
                        identity.artifact_id(ArtifactKind::DataStorage, &index_identity)
                    }),
            ),
            include_offline: index.include_offline,
            members,
            raw: prior.map(|index| index.raw.clone()).unwrap_or_default(),
        });
    }
    // Future-schema indexes stay byte-preserved even when typed indexes are
    // authoritative. They are explicit `Unresolved` values in the model API,
    // never silently mistaken for a supported declaration.
    output.extend(
        previous
            .iter()
            .filter(|index| model_index_signature(index).is_none())
            .cloned(),
    );
    Ok(output)
}

fn reconcile_lifecycle(
    module_name: &str,
    entity_name: &str,
    declared: &[LifecycleDecl],
    previous: &[LifecycleCallback],
    identity: ProjectIdentity,
) -> Result<Vec<LifecycleCallback>> {
    let qualified = format!("{module_name}.{entity_name}");
    let previous_by_event: HashMap<&str, &LifecycleCallback> = previous
        .iter()
        .filter(|callback| lifecycle_callback_is_supported(callback))
        .map(|callback| (callback.event.as_str(), callback))
        .collect();
    let mut output = Vec::with_capacity(declared.len());
    for declaration in declared {
        let event = declaration.event.rust_name();
        let previous = previous_by_event.get(event).copied();
        let (moment, native_event) = declaration.event.native_parts();
        let mut raw = previous
            .map(|callback| callback.raw.clone())
            .unwrap_or_default();
        raw.insert("$Type", "DomainModels$EventHandler");
        raw.insert("Moment", moment);
        raw.insert("Event", native_event);
        raw.insert("Microflow", declaration.handler.clone());
        raw.insert("PassEventObject", declaration.pass_event_object);
        raw.insert("RaiseErrorOnFalse", declaration.raise_error_on_false);
        output.push(LifecycleCallback {
            id: Some(
                previous
                    .and_then(|callback| callback.id.clone())
                    .unwrap_or_else(|| {
                        identity.artifact_id(
                            ArtifactKind::LifecycleCallback,
                            &format!("{qualified}.{event}"),
                        )
                    }),
            ),
            event: event.to_string(),
            handler: declaration.handler.clone(),
            pass_event_object: declaration.pass_event_object,
            raise_error_on_false: declaration.raise_error_on_false,
            raw,
        });
    }
    // Preserve future lifecycle kinds losslessly, while known callbacks are
    // controlled by their one typed event declaration.
    output.extend(
        previous
            .iter()
            .filter(|callback| !lifecycle_callback_is_supported(callback))
            .cloned(),
    );
    Ok(output)
}

fn lifecycle_callback_is_supported(callback: &LifecycleCallback) -> bool {
    callback.raw.get_str("$Type").ok() == Some("DomainModels$EventHandler")
        && matches!(
            callback.event.as_str(),
            "before_commit" | "after_commit" | "before_delete" | "after_delete"
        )
}

/// Rebuilds one attribute doc for a declared `AttributeDecl`, preserving the
/// prior attribute's `$ID`/`dataStorageGuid` when its name matches — every
/// other field (type, default, length, ...) is fully re-derived from the
/// storage-independent declaration every time. A declaration always carries
/// a complete value, not a partial override.
fn reconcile_attribute_doc(
    decl: &AttributeDecl,
    previous: Option<&Document>,
    qualified_name: &str,
    identity: ProjectIdentity,
) -> Document {
    let Some(prev) = previous else {
        return model_attribute(
            decl,
            Some(identity.artifact_id(ArtifactKind::Attribute, qualified_name)),
            Some(identity.artifact_id(ArtifactKind::DataStorage, qualified_name)),
        )
        .to_bson();
    };
    // Mirrors mxrb `Writer#attribute_doc`: merge onto the prior raw doc,
    // keep every field at its native key casing, and rebuild the nested
    // type document only when the storage type actually changed — so a
    // resynchronized attribute stays byte-identical unless the declaration
    // really moved something.
    let mut output = prev.clone();
    let name_key = native_key(prev, "name", "Name");
    output.insert(name_key, decl.name.clone());
    let documentation_key = native_key(prev, "documentation", "Documentation");
    output.insert(documentation_key, decl.documentation.clone());

    let storage_type = model_attribute_type(decl.attribute_type).storage_type();
    let type_key = ["type", "Type", "newType", "NewType"]
        .into_iter()
        .find(|key| prev.contains_key(key))
        .unwrap_or("NewType");
    let previous_type = prev.get_document(type_key).ok();
    let mut type_doc = match previous_type {
        Some(previous_type) if previous_type.get_str("$Type").ok() == Some(storage_type) => {
            previous_type.clone()
        }
        _ => mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": storage_type,
        },
    };
    if matches!(decl.attribute_type, AttributeType::Enumeration) {
        type_doc.remove("enumeration");
        type_doc.remove("Enumeration");
        if let Some(enumeration) = &decl.enumeration {
            type_doc.insert("Enumeration", enumeration.clone());
        }
    }
    if matches!(decl.attribute_type, AttributeType::String)
        && (decl.length.is_some()
            || previous_type
                .is_none_or(|doc| !doc.contains_key("length") && !doc.contains_key("Length")))
    {
        let length_key = previous_type.map_or("Length", |doc| native_key(doc, "length", "Length"));
        type_doc.insert(
            length_key,
            decl.length
                .unwrap_or(mxrs_model::attribute::DEFAULT_STRING_LENGTH),
        );
    }
    if matches!(decl.attribute_type, AttributeType::DateTime)
        && let Some(localize_date) = decl.localize_date
    {
        // A document that leaves the key out already means "localized": the
        // platform's default. Declaring that default must not rewrite the
        // implicit form into an explicit one, or a model that only restates
        // what it imported would no longer rebuild to the same bytes.
        let implicit = previous_type.is_some_and(|doc| {
            doc.get_str("$Type").ok() == Some(storage_type)
                && !doc.contains_key("localizeDate")
                && !doc.contains_key("LocalizeDate")
        });
        if !(implicit && localize_date) {
            let localize_key = previous_type.map_or("LocalizeDate", |doc| {
                native_key(doc, "localizeDate", "LocalizeDate")
            });
            type_doc.insert(localize_key, localize_date);
        }
    }
    output.insert(type_key, type_doc);

    let value_key = native_key(prev, "value", "Value");
    let previous_value = prev.get_document(value_key).ok();
    match previous_value {
        Some(previous_value)
            if decl.default_value.is_none()
                && previous_value.get_str("$Type").ok() != Some("DomainModels$OqlViewValue") => {}
        Some(previous_value)
            if previous_value.get_str("$Type").ok() == Some("DomainModels$StoredValue") =>
        {
            let mut value = previous_value.clone();
            let default_key = native_key(previous_value, "defaultValue", "DefaultValue");
            value.insert(default_key, decl.default_value.clone().unwrap_or_default());
            output.insert(value_key, value);
        }
        _ => {
            output.insert(
                value_key,
                mxrs_bson::doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "DomainModels$StoredValue",
                    "DefaultValue": decl.default_value.clone().unwrap_or_default(),
                },
            );
        }
    }
    output
}

fn reconcile_oql_attribute_value(
    attribute: &mut Document,
    previous: Option<&Document>,
    attribute_name: &str,
    qualified_name: &str,
    identity: ProjectIdentity,
) {
    let value_key = native_key(attribute, "value", "Value");
    let previous_value = previous.and_then(|document| {
        let key = native_key(document, "value", "Value");
        document.get_document(key).ok()
    });
    let mut value = previous_value
        .filter(|value| value.get_str("$Type").ok() == Some("DomainModels$OqlViewValue"))
        .cloned()
        .unwrap_or_default();
    value.insert(
        "$ID",
        previous_value
            .and_then(|value| value.get("$ID"))
            .and_then(mxrs_bson::extract_id)
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::OqlViewValue, qualified_name)),
    );
    value.insert("$Type", "DomainModels$OqlViewValue");
    let reference_key = native_key(&value, "reference", "Reference");
    value.insert(reference_key, attribute_name);
    value.remove("defaultValue");
    value.remove("DefaultValue");
    attribute.insert(value_key, value);
}

/// Rebuilds one entity doc for a declared `EntityDecl` against its prior
/// on-disk counterpart (`None` for a brand-new entity). For an existing
/// entity, everything **not** explicitly re-declared — including `location`,
/// inheritance, access rules, indexes, callbacks, `source`/`oqlQuery`, and
/// unknown/foreign fields —
/// survives untouched, because this merges onto a clone of the prior raw
/// document. Any `Some` declaration is authoritative and reconciles nested
/// IDs by semantic identity; `None` preserves the corresponding native
/// structure byte-for-byte. `name`, documentation, attributes, and
/// required/unique validation rules are always authoritative; unrelated
/// validation-rule kinds remain untouched.
fn build_entity_doc(
    module_name: &str,
    decl: &EntityDecl,
    previous: Option<&Document>,
    id: String,
    index: usize,
    identity: ProjectIdentity,
) -> Result<Document> {
    let Some(prev) = previous else {
        return Ok(fresh_entity(module_name, decl, id, identity)?.to_bson());
    };

    let previous_entity = Entity::from_bson(prev);
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
            let qualified_name = format!("{module_name}.{}.{}", decl.name, a.name);
            let mut document = reconcile_attribute_doc(a, prior, &qualified_name, identity);
            if matches!(decl.source.as_ref(), Some(EntitySourceDecl::OqlView { .. })) {
                reconcile_oql_attribute_value(
                    &mut document,
                    prior,
                    &a.name,
                    &qualified_name,
                    identity,
                );
            }
            Bson::Document(document)
        })
        .collect();
    out.insert(
        attrs_key,
        Bson::Array(mxrs_bson::build_array(new_attrs, prev_attrs_raw.marker)),
    );
    let updated_entity = Entity::from_bson(&out);
    let attribute_ids: HashMap<String, String> = updated_entity
        .attributes
        .iter()
        .filter_map(|attribute| Some((attribute.name.clone()?, attribute.id.clone()?)))
        .collect();

    if let Some(inheritance) = decl.inheritance.as_ref() {
        let generalization = reconcile_generalization(
            module_name,
            &decl.name,
            decl.persistable,
            Some(inheritance),
            previous_entity.generalization.as_ref(),
            identity,
        );
        // Studio Pro stores the hierarchy under `MaybeGeneralization`;
        // writing it under any other spelling would leave the stored
        // document in place beside the new one, identities and all.
        let key = [
            "generalization",
            "Generalization",
            "maybeGeneralization",
            "MaybeGeneralization",
        ]
        .into_iter()
        .find(|key| prev.contains_key(*key))
        .unwrap_or("generalization");
        out.insert(key, generalization.to_bson());
    }

    if let Some(image) = decl.image.as_ref() {
        let key = native_key(prev, "image", "Image");
        out.insert(
            key,
            match image {
                EntityImageDecl::None => String::new(),
                EntityImageDecl::Reference(reference) => reference.clone(),
            },
        );
    }
    if let Some(source) = decl.source.as_ref() {
        out.insert(
            "$Type",
            match source {
                EntitySourceDecl::Stored => "DomainModels$EntityImpl",
                EntitySourceDecl::OqlView { .. } => "DomainModels$ViewEntity",
            },
        );
        let key = native_key(prev, "source", "Source");
        match source {
            EntitySourceDecl::Stored => {
                out.insert(key, Bson::Null);
            }
            EntitySourceDecl::OqlView { .. } => {
                out.insert(
                    key,
                    entity_source_document(
                        module_name,
                        &decl.name,
                        source,
                        previous_entity.source.as_ref(),
                        identity,
                    ),
                );
            }
        }
    }

    if let Some(indexes) = decl.indexes.as_deref() {
        let key = native_key(prev, "indexes", "Indexes");
        let marker = mxrs_bson::parse_array(array_field(prev, key)).marker;
        let reconciled = reconcile_indexes(
            module_name,
            &decl.name,
            indexes,
            &previous_entity.indexes,
            &attribute_ids,
            identity,
        )?;
        out.insert(
            key,
            Bson::Array(mxrs_bson::build_array(
                reconciled
                    .iter()
                    .map(|index| Bson::Document(index.to_bson()))
                    .collect(),
                marker,
            )),
        );
    }

    if let Some(callbacks) = decl.lifecycle.as_deref() {
        let key = native_key(prev, "eventHandlers", "EventHandlers");
        let marker = mxrs_bson::parse_array(array_field(prev, key)).marker;
        let reconciled = reconcile_lifecycle(
            module_name,
            &decl.name,
            callbacks,
            &previous_entity.lifecycle,
            identity,
        )?;
        out.insert(
            key,
            Bson::Array(mxrs_bson::build_array(
                reconciled
                    .iter()
                    .map(|callback| Bson::Document(callback.to_bson()))
                    .collect(),
                marker,
            )),
        );
    }
    let validation_key = native_key(prev, "validationRules", "ValidationRules");
    let previous_validation = mxrs_bson::parse_array(array_field(prev, validation_key));
    let previous_documents = previous_validation
        .items
        .into_iter()
        .filter_map(|item| match item {
            Bson::Document(document) => Some(document),
            _ => None,
        })
        .collect();
    let validation_rules = reconcile_validation_rules(
        module_name,
        &decl.name,
        &decl.attributes,
        previous_documents,
        identity,
    );
    out.insert(
        validation_key,
        Bson::Array(mxrs_bson::build_array(
            validation_rules.into_iter().map(Bson::Document).collect(),
            previous_validation.marker,
        )),
    );
    // Only written when the declaration is authoritative: an entity that
    // declares no rules keeps the imported array verbatim.
    let rules_key = native_key(prev, "accessRules", "AccessRules");
    let previous_rules = mxrs_bson::parse_array(array_field(prev, rules_key));
    let previous_documents: Vec<Document> = previous_rules
        .items
        .iter()
        .filter_map(Bson::as_document)
        .cloned()
        .collect();
    if let Some(rules) = reconcile_access_rules(
        module_name,
        &decl.name,
        decl.access_rules.as_deref(),
        &previous_documents,
        identity,
    )? {
        out.insert(
            rules_key,
            Bson::Array(mxrs_bson::build_array(
                rules.iter().map(access_rule_bson).collect(),
                previous_rules.marker,
            )),
        );
    }

    let _ = index; // reserved: mxrb positions brand-new entities by index; fresh_entity already defaults to (0, 0)
    Ok(out)
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
    validate_entity_extras(module_name, entities, None)?;
    let identity = project_identity(mpr)?;
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
        let qualified_name = format!("{module_name}.{}", decl.name);
        let id = previous
            .and_then(|p| p.get("$ID"))
            .and_then(mxrs_bson::extract_id)
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::Entity, &qualified_name));
        entity_ids.insert(decl.name.clone(), id.clone());
        new_items.push(Bson::Document(build_entity_doc(
            module_name,
            decl,
            previous,
            id,
            index,
            identity,
        )?));
    }

    doc.insert(
        entities_key,
        Bson::Array(mxrs_bson::build_array(new_items, existing_raw.marker)),
    );
    mpr.update_unit(&dm_id, doc)?;
    Ok(entity_ids)
}

fn project_identity(mpr: &MprFile) -> Result<ProjectIdentity> {
    let root_id = mpr
        .root_unit()?
        .ok_or(WriterError::MissingRootUnit)?
        .unit_id;
    Ok(ProjectIdentity::from_project_root(&root_id)?)
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
    validate_entity_extras(module_name, entities, Some(known_entities))?;
    synchronize_domain_entities(mpr, module_id, module_name, entities)?;
    synchronize_domain_associations(mpr, module_id, module_name, entities, known_entities)?;
    Ok(())
}

/// Lowers declared access rules into `mxrs_model::AccessRule` values, which
/// `Entity::to_bson` then serializes into native `DomainModels$AccessRule`
/// documents. Mirrors `Writer#access_rule_doc`.
///
/// Returns `None` when the entity declares no rules at all
/// (`EntityDecl::access_rules == None`): that means "leave whatever the
/// imported model had", so the caller keeps the previous array untouched
/// rather than writing an empty one. An explicit `Some(vec![])` does clear
/// them.
///
/// Identities derive from the role set and the member reference rather than
/// from array position, so reordering rules in source does not renumber them;
/// a previous rule whose role set matches keeps its `$ID`, the same
/// preservation the attribute and validation-rule paths already do.
fn reconcile_access_rules(
    module_name: &str,
    entity_name: &str,
    declared: Option<&[AccessRuleDecl]>,
    previous: &[Document],
    identity: ProjectIdentity,
) -> Result<Option<Vec<ModelAccessRule>>> {
    let Some(declared) = declared else {
        return Ok(None);
    };
    // mxrb raises "access_rule requires at least one module role"; a rule with
    // an empty role set matches nobody, so it is a declaration mistake rather
    // than a no-op worth persisting.
    if declared.iter().any(|rule| rule.roles.is_empty()) {
        return Err(WriterError::AccessRuleWithoutRoles {
            module_name: module_name.to_string(),
            name: entity_name.to_string(),
        });
    }
    let qualified_entity = format!("{module_name}.{entity_name}");
    let previous_by_roles: HashMap<String, &Document> = previous
        .iter()
        .map(|document| (rule_role_key(document), document))
        .collect();
    let mut seen: HashMap<String, usize> = HashMap::new();
    Ok(Some(
        declared
            .iter()
            .map(|rule| {
                let roles: Vec<String> = rule
                    .roles
                    .iter()
                    .map(|role| qualify_role(module_name, role))
                    .collect();
                let role_key = roles.join("+");
                // Two rules may legally target the same roles; the occurrence
                // index disambiguates only that case, so the common
                // one-rule-per-role-set shape keeps a position-independent key.
                let occurrence = *seen
                    .entry(role_key.clone())
                    .and_modify(|count| *count += 1)
                    .or_insert(0);
                let rule_key = if occurrence == 0 {
                    format!("{qualified_entity}#{role_key}")
                } else {
                    format!("{qualified_entity}#{role_key}#{occurrence}")
                };
                let prior = previous_by_roles.get(&role_key).copied();
                let previous_members = previous_member_ids(prior);
                let previous_member_documents = previous_member_documents(prior);
                ModelAccessRule {
                    id: Some(
                        prior
                            .and_then(|document| document.get("$ID"))
                            .and_then(mxrs_bson::extract_id)
                            .unwrap_or_else(|| {
                                identity.artifact_id(ArtifactKind::AccessRule, &rule_key)
                            }),
                    ),
                    roles,
                    create: rule.allow_create,
                    delete: rule.allow_delete,
                    documentation: rule.documentation.clone(),
                    default_rights: rights_name(rule.default_rights).to_string(),
                    members: rule
                        .members
                        .iter()
                        .map(|member| ModelAccessMember {
                            id: Some(
                                previous_members
                                    .get(&member.reference)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        identity.artifact_id(
                                            ArtifactKind::AccessMember,
                                            &format!("{rule_key}.{}", member.reference),
                                        )
                                    }),
                            ),
                            name: member
                                .reference
                                .rsplit('.')
                                .next()
                                .unwrap_or(&member.reference)
                                .to_string(),
                            reference: member.reference.clone(),
                            rights: rights_name(member.rights).to_string(),
                            kind: match member.kind {
                                AccessMemberKind::Attribute => ModelAccessMemberKind::Attribute,
                                AccessMemberKind::Association => ModelAccessMemberKind::Association,
                            },
                            raw: previous_member_documents
                                .get(&member.reference)
                                .cloned()
                                .unwrap_or_default(),
                        })
                        .collect(),
                    xpath: rule.xpath_constraint.clone(),
                    xpath_caption: rule.xpath_caption.clone().or_else(|| {
                        prior
                            .and_then(|document| document.get_str("XPathConstraintCaption").ok())
                            .filter(|caption| !caption.is_empty())
                            .map(str::to_string)
                    }),
                    raw: prior.cloned().unwrap_or_default(),
                }
            })
            .collect(),
    ))
}

fn previous_member_ids(previous: Option<&Document>) -> HashMap<String, String> {
    let Some(previous) = previous else {
        return HashMap::new();
    };
    mxrs_bson::parse_array(previous.get_array("MemberAccesses").ok().map(Vec::as_slice))
        .items
        .iter()
        .filter_map(Bson::as_document)
        .filter_map(|member| {
            let association = member.get_str("Association").unwrap_or("");
            let reference = if association.is_empty() {
                member.get_str("Attribute").unwrap_or("")
            } else {
                association
            };
            if reference.is_empty() {
                return None;
            }
            Some((
                reference.to_string(),
                mxrs_bson::extract_id(member.get("$ID")?)?,
            ))
        })
        .collect()
}

fn previous_member_documents(previous: Option<&Document>) -> HashMap<String, Document> {
    let Some(previous) = previous else {
        return HashMap::new();
    };
    mxrs_bson::parse_array(previous.get_array("MemberAccesses").ok().map(Vec::as_slice))
        .items
        .iter()
        .filter_map(Bson::as_document)
        .filter_map(|member| {
            let association = member.get_str("Association").unwrap_or("");
            let reference = if association.is_empty() {
                member.get_str("Attribute").unwrap_or("")
            } else {
                association
            };
            (!reference.is_empty()).then(|| (reference.to_string(), member.clone()))
        })
        .collect()
}

fn rule_role_key(document: &Document) -> String {
    mxrs_bson::parse_array(
        document
            .get_array("AllowedModuleRoles")
            .or_else(|_| document.get_array("ModuleRoles"))
            .ok()
            .map(Vec::as_slice),
    )
    .items
    .iter()
    .filter_map(Bson::as_str)
    .collect::<Vec<_>>()
    .join("+")
}

/// `AllowedModuleRoles` holds qualified role names; an unqualified
/// declaration resolves against the module that owns the entity.
fn qualify_role(module_name: &str, role: &str) -> String {
    if role.contains('.') {
        role.to_string()
    } else {
        format!("{module_name}.{role}")
    }
}

fn rights_name(rights: MemberRights) -> &'static str {
    match rights {
        MemberRights::None => "None",
        MemberRights::ReadOnly => "ReadOnly",
        MemberRights::ReadWrite => "ReadWrite",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_ir::{
        EntityIndexDecl, EntityInheritanceDecl, IndexMemberDecl, LifecycleDecl, LifecycleEvent,
        SystemMembersDecl,
    };

    #[test]
    fn absent_imported_attribute_default_is_not_materialized_as_an_empty_string() {
        let declaration = AttributeDecl::new("TabIndex", AttributeType::Integer);
        let mut previous = model_attribute(&declaration, None, None).to_bson();
        previous
            .get_document_mut("value")
            .unwrap()
            .remove("defaultValue");

        let output = reconcile_attribute_doc(
            &declaration,
            Some(&previous),
            "Sales.Order.TabIndex",
            ProjectIdentity::for_project("Defaults"),
        );

        assert!(
            !output
                .get_document("value")
                .unwrap()
                .contains_key("defaultValue")
        );
    }

    /// A document that leaves `localizeDate` out already means "localized".
    /// Restating that default keeps the implicit form; anything else — a
    /// stated key, the non-default value, a changed type — is written.
    #[test]
    fn a_restated_localized_date_default_keeps_the_imported_form() {
        let identity = ProjectIdentity::for_project("Defaults");
        let mut declaration = AttributeDecl::new("SubmittedAt", AttributeType::DateTime);
        declaration.localize_date = Some(true);
        let reconcile = |declaration: &AttributeDecl, previous: &Document| {
            reconcile_attribute_doc(
                declaration,
                Some(previous),
                "Sales.Order.SubmittedAt",
                identity,
            )
        };
        let stated = model_attribute(&declaration, None, None).to_bson();
        let mut implicit = stated.clone();
        implicit
            .get_document_mut("type")
            .unwrap()
            .remove("localizeDate");

        assert_eq!(reconcile(&declaration, &implicit), implicit);
        assert_eq!(reconcile(&declaration, &stated), stated);

        declaration.localize_date = Some(false);
        let localized = |document: &Document| {
            document
                .get_document("type")
                .unwrap()
                .get_bool("localizeDate")
                .ok()
        };
        assert_eq!(localized(&reconcile(&declaration, &implicit)), Some(false));

        // A type that changed is a new type document, written in full.
        let text = model_attribute(
            &AttributeDecl::new("SubmittedAt", AttributeType::String),
            None,
            None,
        )
        .to_bson();
        declaration.localize_date = Some(true);
        assert_eq!(localized(&reconcile(&declaration, &text)), Some(true));
    }

    #[test]
    fn semantic_entity_extras_keep_nested_identity_and_unknown_fields() {
        let identity = ProjectIdentity::for_project("EntityExtras");
        let attribute_id = uuid::Uuid::new_v4().to_string();
        let index_id = uuid::Uuid::new_v4().to_string();
        let member_id = uuid::Uuid::new_v4().to_string();
        let callback_id = uuid::Uuid::new_v4().to_string();
        let previous = Entity::from_bson(&mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$QualifiedName": "Sales.Order",
            "name": "Order",
            "attributes": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                "$ID": attribute_id.clone(), "name": "Number",
                "type": { "$Type": "DomainModels$StringAttributeType" },
            })], 3),
            "indexes": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                "$ID": index_id.clone(), "$Type": "DomainModels$EntityIndex",
                "GUID": uuid::Uuid::new_v4().to_string(),
                "IncludeInOffline": false, "FutureIndexField": "keep",
                "Attributes": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                    "$ID": member_id.clone(), "$Type": "DomainModels$IndexedAttribute",
                    "Type": "Normal", "AttributePointer": attribute_id.clone(),
                    "AssociationPointer": "00000000-0000-0000-0000-000000000000",
                    "Ascending": true, "FutureMemberField": "keep",
                })], 3),
            })], 3),
            "eventHandlers": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                "$ID": callback_id.clone(), "$Type": "DomainModels$EventHandler",
                "Moment": "Before", "Event": "Commit", "Microflow": "Sales.Old",
                "PassEventObject": true, "RaiseErrorOnFalse": true,
                "FutureCallbackField": "keep",
            })], 3),
        });
        let indexes = reconcile_indexes(
            "Sales",
            "Order",
            &[EntityIndexDecl {
                members: vec![IndexMemberDecl::Attribute {
                    name: "Number".to_string(),
                    ascending: false,
                }],
                include_offline: true,
            }],
            &previous.indexes,
            &HashMap::from([("Number".to_string(), attribute_id)]),
            identity,
        )
        .unwrap();
        assert_eq!(indexes[0].id.as_deref(), Some(index_id.as_str()));
        assert_eq!(
            indexes[0].members[0].id.as_deref(),
            Some(member_id.as_str())
        );
        let index = indexes[0].to_bson();
        assert_eq!(index.get_str("FutureIndexField").unwrap(), "keep");
        assert!(index.get_bool("IncludeInOffline").unwrap());
        let member = mxrs_bson::parse_array(index.get_array("Attributes").ok().map(Vec::as_slice))
            .items
            .remove(0);
        let member = member.as_document().unwrap();
        assert_eq!(member.get_str("FutureMemberField").unwrap(), "keep");
        assert!(!member.get_bool("Ascending").unwrap());

        let callbacks = reconcile_lifecycle(
            "Sales",
            "Order",
            &[LifecycleDecl {
                event: LifecycleEvent::BeforeCommit,
                handler: "Sales.New".to_string(),
                pass_event_object: false,
                raise_error_on_false: true,
            }],
            &previous.lifecycle,
            identity,
        )
        .unwrap();
        assert_eq!(callbacks[0].id.as_deref(), Some(callback_id.as_str()));
        let callback = callbacks[0].to_bson();
        assert_eq!(callback.get_str("FutureCallbackField").unwrap(), "keep");
        assert_eq!(callback.get_str("Microflow").unwrap(), "Sales.New");
        assert!(!callback.get_bool("PassEventObject").unwrap());
    }

    #[test]
    fn entity_extra_validation_fails_closed() {
        let mut entity = EntityDecl::new("Order");
        entity
            .attributes
            .push(AttributeDecl::new("Number", AttributeType::String));
        entity.indexes = Some(vec![EntityIndexDecl {
            members: vec![IndexMemberDecl::Attribute {
                name: "Missing".to_string(),
                ascending: true,
            }],
            include_offline: false,
        }]);
        let error = validate_entity_extras("Sales", &[entity], None).unwrap_err();
        assert!(
            matches!(error, WriterError::UnknownIndexedAttribute { attribute, .. } if attribute == "Missing")
        );

        let mut child = EntityDecl::new("Child");
        child.inheritance = Some(EntityInheritanceDecl::Root(SystemMembersDecl::default()));
        child.lifecycle = Some(vec![
            LifecycleDecl {
                event: LifecycleEvent::AfterCommit,
                handler: "Sales.One".to_string(),
                pass_event_object: true,
                raise_error_on_false: false,
            },
            LifecycleDecl {
                event: LifecycleEvent::AfterCommit,
                handler: "Sales.Two".to_string(),
                pass_event_object: true,
                raise_error_on_false: false,
            },
        ]);
        assert!(matches!(
            validate_entity_extras("Sales", &[child], None).unwrap_err(),
            WriterError::DuplicateLifecycleEvent { .. }
        ));
    }
}
