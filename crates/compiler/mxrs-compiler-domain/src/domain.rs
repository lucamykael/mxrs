//! Compiles an `mxrs-model` `DomainModel` (editor shape) into Mendix
//! Runtime shape — ports `lib/mxrb/compiler/domain_document_compiler.rb`
//! (156 lines). First crate of Phase 5's *compiler* pipeline: an
//! already-persisted model (read via `mxrs-model`) → document compilers →
//! runtime shape → materializers → deployment tree. Not to be confused
//! with the *writer* path `mxrs-writer` already covers — see
//! `decisions/mxrs-rust-rewrite-plan.md`'s "two independent pipelines"
//! note in this project's ai-memory.
//!
//! Reads from `mxrs-model`'s already-typed `Entity`/`Attribute`/
//! `Association` structs rather than raw BSON, per the crate table's
//! `mxrs-compiler-domain: mxrs-model, mxrs-schema` dependency (the plan's
//! `mxrs-ir` note about `mxrs-model` already having bidirectional BSON
//! structs applies here too: no reason to re-parse what Phase 2 already
//! decoded). Two consequences worth calling out:
//!
//! - `mxrs-model::Entity`'s `access_rules`/`indexes`/`validation_rules`
//!   collapse some editor nuance the Runtime compiler needs back: an
//!   entity's own `MaybeGeneralization` sub-document is kept **raw**
//!   (`Entity.generalization`) specifically because `inherited_flags`
//!   (below) needs to distinguish "this entity's own doc doesn't set
//!   `Persistable`" from "this entity's own doc sets `Persistable: false`"
//!   — `mxrs-model`'s own summarized `Entity.persistable`/`SystemMembers`
//!   fields already collapsed that distinction (defaults `false` either
//!   way), so this crate recomputes the walk directly off the raw doc
//!   instead of reusing those fields.
//! - `mxrs-model` retains the raw fields whose Runtime representation is
//!   opaque (`Image`, association `Source`/`GUID`, and lifecycle handler
//!   documents), while this pass builds a project-wide index for OQL
//!   view-source resolution.

use std::collections::HashMap;

use mxrs_bson::{Binary, BinarySubtype, Bson, Document, doc};
use mxrs_model::{Attribute, AttributeType, DomainModel, Entity, Module, Project};

use crate::CompilerError;
use crate::security::SecurityCompiler;
use mxrs_compiler_support::{
    array_docs, get_any, get_bool_any, get_id_any, get_str_any, new_id, plain_value,
};

pub struct DomainCompiler<'a> {
    entities_by_qualified_name: HashMap<&'a str, &'a Entity>,
    oql_by_qualified_name: HashMap<String, String>,
    security: SecurityCompiler,
}

impl<'a> DomainCompiler<'a> {
    /// `modules` must be every module of `project` (not just the ones
    /// you're about to compile) — generalization inheritance can cross
    /// module boundaries, so resolving it needs the whole project's entity
    /// set indexed up front, mirroring
    /// `DomainDocumentCompiler#initialize` building `@entities` from
    /// `source.units_of('DomainModels$DomainModel')` (every such unit in
    /// the project, not just the one being compiled).
    pub fn new(project: &Project, modules: &'a [Module]) -> Result<Self, CompilerError> {
        let mut entities_by_qualified_name = HashMap::new();
        for module in modules {
            for entity in module.entities() {
                if let Some(qualified_name) = entity.qualified_name.as_deref() {
                    entities_by_qualified_name.insert(qualified_name, entity);
                }
            }
        }
        Ok(DomainCompiler {
            entities_by_qualified_name,
            oql_by_qualified_name: index_oql_sources(project, modules)?,
            security: SecurityCompiler::new(project)?,
        })
    }

    /// `None` when `module` has no domain model at all (mirrors
    /// `compile` only ever being called on real `DomainModels$DomainModel`
    /// units — a module without one simply has no such unit to compile).
    pub fn compile_module(&self, module: &Module) -> Result<Option<Document>, CompilerError> {
        let Some(domain_model) = &module.domain_model else {
            return Ok(None);
        };
        let module_name = module.name.as_deref().unwrap_or_default();
        Ok(Some(self.compile_domain_model(domain_model, module_name)?))
    }

    fn compile_domain_model(
        &self,
        domain_model: &DomainModel,
        module_name: &str,
    ) -> Result<Document, CompilerError> {
        let entities: Result<Vec<Document>, CompilerError> = domain_model
            .entities
            .iter()
            .map(|entity| self.compile_entity(entity, module_name))
            .collect();
        let associations: Vec<Document> = domain_model
            .associations
            .iter()
            .map(|association| self.security.association(association, module_name))
            .collect();
        let cross_associations: Vec<Document> = domain_model
            .cross_associations
            .iter()
            .map(|association| self.security.association(association, module_name))
            .collect();
        Ok(doc! {
            "$ID": domain_model.id.clone().unwrap_or_else(new_id),
            "$Type": domain_model.native_type.clone().unwrap_or_else(|| "DomainModels$DomainModel".to_string()),
            "Entities": entities?,
            "Associations": associations,
            "CrossAssociations": cross_associations,
        })
    }

    fn compile_entity(
        &self,
        entity: &Entity,
        module_name: &str,
    ) -> Result<Document, CompilerError> {
        let name = entity.name.clone().unwrap_or_default();
        let mut result = self.entity_content(entity);
        result.insert("Source", self.compile_entity_source(entity)?);
        result.insert("GUID", entity.data_storage_guid.clone().unwrap_or_default());
        result.insert("Image", entity.image.clone().unwrap_or_default());
        result.insert(
            "QualifiedName",
            entity
                .qualified_name
                .clone()
                .unwrap_or_else(|| format!("{module_name}.{name}")),
        );
        result.insert("UnqualifiedName", name);
        Ok(result)
    }

    fn entity_content(&self, entity: &Entity) -> Document {
        doc! {
            "$ID": entity.id.clone().unwrap_or_else(new_id),
            "$Type": entity.native_type.clone().unwrap_or_else(|| "DomainModels$Entity".to_string()),
            "MaybeGeneralization": self.compile_generalization(entity),
            "Attributes": entity.attributes.iter().map(compile_attribute).collect::<Vec<_>>(),
            "ValidationRules": entity.validation_rules.iter().map(compile_validation).collect::<Vec<_>>(),
            "Events": entity.lifecycle.iter().map(|callback| match plain_value(Bson::Document(callback.raw.clone())) {
                Bson::Document(document) => document,
                _ => unreachable!("plain_value preserves documents"),
            }).collect::<Vec<_>>(),
            "Indexes": entity.indexes.iter().map(compile_index).collect::<Vec<_>>(),
            "AccessRules": entity.access_rules.iter().map(|rule| self.security.access_rule(rule)).collect::<Vec<_>>(),
        }
    }

    fn compile_entity_source(&self, entity: &Entity) -> Result<Bson, CompilerError> {
        let Some(source) = entity.source.as_ref() else {
            return Ok(Bson::Null);
        };
        let source_type = get_str_any(source, &["$Type"]).unwrap_or_default();
        if source_type != "DomainModels$OqlViewEntitySource" {
            return Ok(plain_value(Bson::Document(source.clone())));
        }

        let qualified_name =
            get_str_any(source, &["SourceDocument", "sourceDocument"]).unwrap_or_default();
        let entity_name = entity
            .qualified_name
            .clone()
            .or_else(|| entity.name.clone())
            .unwrap_or_default();
        let oql = self
            .oql_by_qualified_name
            .get(&qualified_name)
            .ok_or_else(|| CompilerError::MissingOqlViewSource {
                entity: entity_name,
                source_name: qualified_name.clone(),
            })?;
        let mut compiled = Document::new();
        for (output_key, input_keys) in [
            ("$ID", &["$ID"][..]),
            ("$Type", &["$Type"][..]),
            ("SourceDocument", &["SourceDocument", "sourceDocument"][..]),
        ] {
            if let Some(value) = get_any(source, input_keys) {
                compiled.insert(output_key, value.clone());
            }
        }
        compiled.insert("OqlRuntime", oql.clone());
        compiled.insert("SourceDocumentName", qualified_name);
        compiled.insert("SourceType", "OQL");
        Ok(Bson::Document(compiled))
    }

    fn compile_generalization(&self, entity: &Entity) -> Document {
        let Some(generalization) = entity.generalization.as_ref() else {
            // Every entity should carry a `MaybeGeneralization` doc in
            // practice; fall back to an explicit `NoGeneralization` root
            // rather than panicking on a malformed/hand-built fixture.
            return doc! {
                "$ID": new_id(), "$Type": "DomainModels$NoGeneralization",
                "Key": Bson::Null, "Persistable": false,
                "HasCreatedDateAttr": false, "HasChangedDateAttr": false,
                "HasOwnerAttr": false, "HasChangedByAttr": false,
                "Generalization": "",
            };
        };
        let type_name = get_str_any(generalization, &["$Type"])
            .unwrap_or_else(|| "DomainModels$NoGeneralization".to_string());
        let flags = self.generalization_flags(generalization, &type_name, 0);
        let mut result = doc! {
            "$ID": get_id_any(generalization, &["$ID"]).unwrap_or_else(new_id),
            "$Type": type_name.clone(),
        };
        if type_name == "DomainModels$NoGeneralization" {
            result.insert(
                "Key",
                get_any(generalization, &["Key"])
                    .cloned()
                    .unwrap_or(Bson::Null),
            );
        }
        result.insert("Persistable", flags.persistable);
        result.insert("HasCreatedDateAttr", flags.has_created_date);
        result.insert("HasChangedDateAttr", flags.has_changed_date);
        result.insert("HasOwnerAttr", flags.has_owner);
        result.insert("HasChangedByAttr", flags.has_changed_by);
        result.insert(
            "Generalization",
            get_str_any(generalization, &["Generalization", "generalization"]).unwrap_or_default(),
        );
        result
    }

    /// Key-presence-sensitive: `Persistable`/`HasXAttr` absent on this
    /// entity's own doc falls back to the parent's *resolved* flags (which
    /// may themselves be inherited further up the chain); present (even as
    /// `false`) always wins. `depth` is a malformed-input guard (a real
    /// Mendix model can't have a generalization cycle; a hand-built test
    /// fixture could) — past it we fall back to `system_flags` rather than
    /// overflowing the stack.
    fn generalization_flags(
        &self,
        doc: &Document,
        type_name: &str,
        depth: u32,
    ) -> GeneralizationFlags {
        let inherited = if type_name == "DomainModels$Generalization" && depth < 64 {
            let target =
                get_str_any(doc, &["Generalization", "generalization"]).unwrap_or_default();
            Some(self.inherited_flags(&target, depth + 1))
        } else {
            None
        };
        let fallback = |keys: &[&str], field: fn(&GeneralizationFlags) -> bool| {
            get_bool_any(doc, keys).unwrap_or_else(|| inherited.as_ref().is_some_and(field))
        };
        GeneralizationFlags {
            persistable: fallback(&["Persistable", "persistable"], |f| f.persistable),
            has_created_date: fallback(&["HasCreatedDateAttr", "hasCreatedDate"], |f| {
                f.has_created_date
            }),
            has_changed_date: fallback(&["HasChangedDateAttr", "hasChangedDate"], |f| {
                f.has_changed_date
            }),
            has_owner: fallback(&["HasOwnerAttr", "hasOwner"], |f| f.has_owner),
            has_changed_by: fallback(&["HasChangedByAttr", "hasChangedBy"], |f| f.has_changed_by),
        }
    }

    fn inherited_flags(&self, qualified_name: &str, depth: u32) -> GeneralizationFlags {
        if depth >= 64 || qualified_name.starts_with("System.") {
            return GeneralizationFlags::system();
        }
        let Some(parent) = self.entities_by_qualified_name.get(qualified_name) else {
            return GeneralizationFlags::system();
        };
        let Some(generalization) = parent.generalization.as_ref() else {
            return GeneralizationFlags::system();
        };
        let type_name = get_str_any(generalization, &["$Type"])
            .unwrap_or_else(|| "DomainModels$NoGeneralization".to_string());
        self.generalization_flags(generalization, &type_name, depth)
    }
}

fn index_oql_sources(
    project: &Project,
    modules: &[Module],
) -> Result<HashMap<String, String>, CompilerError> {
    let units = project.all_units()?;
    let parent_by_id: HashMap<String, String> = units
        .iter()
        .map(|unit| (unit.unit_id.clone(), unit.container_id.clone()))
        .collect();
    let module_name_by_id: HashMap<String, String> = modules
        .iter()
        .filter_map(|module| Some((module.id.clone(), module.name.clone()?)))
        .collect();
    let mut result = HashMap::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        if get_str_any(&document, &["$Type"]).as_deref()
            != Some("DomainModels$ViewEntitySourceDocument")
        {
            continue;
        }
        let Some(module_name) =
            owning_module_name(&unit.container_id, &parent_by_id, &module_name_by_id)
        else {
            continue;
        };
        let name = get_str_any(&document, &["Name", "name"]).unwrap_or_default();
        let oql = get_str_any(&document, &["Oql", "oql", "OQL"]).unwrap_or_default();
        result.insert(format!("{module_name}.{name}"), oql);
    }
    Ok(result)
}

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

struct GeneralizationFlags {
    persistable: bool,
    has_created_date: bool,
    has_changed_date: bool,
    has_owner: bool,
    has_changed_by: bool,
}

impl GeneralizationFlags {
    /// A `System.*` base (e.g. `System.Owned`) or an unresolvable parent —
    /// mxrb's `system_flags`: all four true, always persistable.
    fn system() -> Self {
        GeneralizationFlags {
            persistable: true,
            has_created_date: true,
            has_changed_date: true,
            has_owner: true,
            has_changed_by: true,
        }
    }
}

fn compile_attribute(attribute: &Attribute) -> Document {
    doc! {
        "$ID": attribute.id.clone().unwrap_or_else(new_id),
        "$Type": "DomainModels$Attribute",
        "Value": attribute.raw_value_doc.clone().map_or_else(default_value_doc, |document| plain_value(Bson::Document(document))),
        "Type": compile_attribute_type(attribute),
        "Name": attribute.name.clone().unwrap_or_default(),
        "GUID": attribute.data_storage_guid.clone().unwrap_or_default(),
    }
}

fn default_value_doc() -> Bson {
    Bson::Document(doc! {
        "$ID": new_id(), "$Type": "DomainModels$StoredValue", "DefaultValue": "",
    })
}

fn compile_attribute_type(attribute: &Attribute) -> Document {
    let mut result = attribute.raw_type_doc.clone().unwrap_or_else(|| {
        doc! { "$ID": new_id(), "$Type": attribute.attribute_type.storage_type() }
    });
    result = match plain_value(Bson::Document(result)) {
        Bson::Document(document) => document,
        _ => unreachable!("plain_value preserves documents"),
    };
    if attribute.attribute_type == AttributeType::String && get_any(&result, &["Length"]).is_none()
    {
        result.insert(
            "Length",
            attribute
                .length
                .unwrap_or(mxrs_model::attribute::DEFAULT_STRING_LENGTH),
        );
    }
    if attribute.attribute_type == AttributeType::DateTime {
        result.insert("LocalizeDate", attribute.localize_date.unwrap_or(true));
    }
    result
}

fn compile_validation(rule: &Document) -> Document {
    let message = get_any(rule, &["Message"]).and_then(|b| match b {
        Bson::Document(d) => Some(d.clone()),
        _ => None,
    });
    doc! {
        "$ID": get_id_any(rule, &["$ID"]).unwrap_or_else(new_id),
        "$Type": get_str_any(rule, &["$Type"]).unwrap_or_else(|| "DomainModels$ValidationRule".to_string()),
        "Message": message.map_or_else(
            || doc! { "$ID": new_id(), "$Type": "Texts$Text" },
            |m| doc! {
                "$ID": get_id_any(&m, &["$ID"]).unwrap_or_default(),
                "$Type": get_str_any(&m, &["$Type"]).unwrap_or_default(),
            },
        ),
        "RuleInfo": get_any(rule, &["RuleInfo"]).cloned().map(plain_value).unwrap_or(Bson::Null),
        "Attribute": get_str_any(rule, &["Attribute"]).unwrap_or_default(),
    }
}

fn compile_index(index: &Document) -> Document {
    doc! {
        "$ID": get_id_any(index, &["$ID"]).unwrap_or_else(new_id),
        "$Type": get_str_any(index, &["$Type"]).unwrap_or_else(|| "DomainModels$Index".to_string()),
        "Attributes": array_docs(index, &["Attributes"]).iter().map(compile_indexed_attribute).collect::<Vec<_>>(),
        "GUID": get_str_any(index, &["GUID"]).unwrap_or_default(),
        "IncludeInOffline": get_bool_any(index, &["IncludeInOffline"]).unwrap_or(false),
    }
}

fn compile_indexed_attribute(attribute: &Document) -> Document {
    let mut result = match plain_value(Bson::Document(attribute.clone())) {
        Bson::Document(document) => document,
        _ => unreachable!("plain_value preserves documents"),
    };
    if get_any(&result, &["AssociationPointer"]).is_none() {
        result.insert("AssociationPointer", zero_association_pointer());
    }
    result
}

fn zero_association_pointer() -> Bson {
    let bytes = mxrs_bson::uuid_to_blob("00000000-0000-0000-0000-000000000000")
        .expect("the all-zero UUID is always a valid 32-hex-digit UUID string");
    Bson::Binary(Binary {
        subtype: BinarySubtype::Generic,
        bytes: bytes.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;
    use mxrs_model::Entity;

    /// A `DomainCompiler` needs a real `Project` only for `SecurityCompiler`
    /// (reads `Security$ProjectSecurity`) — these tests don't exercise
    /// security, so a project with no such unit (empty role map) is enough.
    fn compiler_without_security<'a>(entities: &'a [(&'a str, &'a Entity)]) -> DomainCompiler<'a> {
        DomainCompiler {
            entities_by_qualified_name: entities.iter().copied().collect(),
            oql_by_qualified_name: HashMap::new(),
            security: SecurityCompiler::new_without_security(),
        }
    }

    fn entity(qualified_name: &str, extra: Document) -> Entity {
        let mut d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$QualifiedName": qualified_name,
            "name": qualified_name.rsplit('.').next().unwrap(),
        };
        d.extend(extra);
        Entity::from_bson(&d)
    }

    #[test]
    fn a_no_generalization_entity_uses_its_own_flags_with_no_inheritance() {
        let e = entity(
            "Sales.Order",
            doc! { "generalization": { "$Type": "DomainModels$NoGeneralization", "persistable": true } },
        );
        let compiler = compiler_without_security(&[]);
        let g = compiler.compile_generalization(&e);
        assert_eq!(g.get_str("$Type").unwrap(), "DomainModels$NoGeneralization");
        assert!(g.get_bool("Persistable").unwrap());
        assert!(!g.get_bool("HasOwnerAttr").unwrap());
    }

    #[test]
    fn a_generalization_entity_inherits_absent_flags_from_its_parent() {
        let parent = entity(
            "Sales.Base",
            doc! { "generalization": { "$Type": "DomainModels$NoGeneralization", "persistable": true, "HasOwnerAttr": true } },
        );
        let child_doc = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$QualifiedName": "Sales.Order",
            "name": "Order",
            // Persistable/HasOwnerAttr absent on purpose: must fall back to the parent.
            "generalization": { "$Type": "DomainModels$Generalization", "Generalization": "Sales.Base" },
        };
        let child = Entity::from_bson(&child_doc);
        let entities = [("Sales.Base", &parent)];
        let compiler = compiler_without_security(&entities);
        let g = compiler.compile_generalization(&child);
        assert!(
            g.get_bool("Persistable").unwrap(),
            "should inherit Persistable=true from the parent"
        );
        assert!(
            g.get_bool("HasOwnerAttr").unwrap(),
            "should inherit HasOwnerAttr=true from the parent"
        );
        assert!(
            !g.get_bool("HasCreatedDateAttr").unwrap(),
            "parent didn't set this either, so it stays false"
        );
    }

    #[test]
    fn an_explicit_false_wins_over_an_inherited_true() {
        let parent = entity(
            "Sales.Base",
            doc! { "generalization": { "$Type": "DomainModels$NoGeneralization", "HasOwnerAttr": true } },
        );
        let child_doc = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$QualifiedName": "Sales.Order",
            "name": "Order",
            "generalization": {
                "$Type": "DomainModels$Generalization", "Generalization": "Sales.Base",
                "HasOwnerAttr": false,
            },
        };
        let child = Entity::from_bson(&child_doc);
        let entities = [("Sales.Base", &parent)];
        let compiler = compiler_without_security(&entities);
        let g = compiler.compile_generalization(&child);
        assert!(
            !g.get_bool("HasOwnerAttr").unwrap(),
            "an explicit false must not be overridden by inheritance"
        );
    }

    #[test]
    fn extending_a_system_entity_inherits_all_true() {
        let child_doc = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$QualifiedName": "Sales.Order",
            "name": "Order",
            "generalization": { "$Type": "DomainModels$Generalization", "Generalization": "System.Owned" },
        };
        let child = Entity::from_bson(&child_doc);
        let compiler = compiler_without_security(&[]);
        let g = compiler.compile_generalization(&child);
        assert!(g.get_bool("Persistable").unwrap());
        assert!(g.get_bool("HasOwnerAttr").unwrap());
        assert!(g.get_bool("HasChangedByAttr").unwrap());
    }

    #[test]
    fn a_missing_oql_view_source_is_a_loud_error() {
        let e = entity(
            "Sales.Report",
            doc! { "source": { "$Type": "DomainModels$OqlViewEntitySource", "SourceDocument": "Sales.ReportSource" } },
        );
        let compiler = compiler_without_security(&[]);
        let err = compiler.compile_entity(&e, "Sales").unwrap_err();
        assert!(
            matches!(err, CompilerError::MissingOqlViewSource { entity, source_name }
                if entity == "Sales.Report" && source_name == "Sales.ReportSource")
        );
    }

    #[test]
    fn a_string_attribute_without_a_length_gets_the_default() {
        let attr = Attribute::from_bson(&doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "name": "Number",
            "type": { "$Type": "DomainModels$StringAttributeType" },
        });
        let compiled = compile_attribute_type(&attr);
        assert_eq!(
            compiled.get_i32("Length").unwrap(),
            mxrs_model::attribute::DEFAULT_STRING_LENGTH
        );
    }

    #[test]
    fn a_string_attribute_with_an_explicit_length_keeps_it() {
        let attr = Attribute::from_bson(&doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "name": "Number",
            "type": { "$Type": "DomainModels$StringAttributeType", "Length": 50 },
        });
        let compiled = compile_attribute_type(&attr);
        assert_eq!(compiled.get_i32("Length").unwrap(), 50);
    }

    #[test]
    fn a_datetime_attribute_defaults_localize_date_to_true() {
        let attr = Attribute::from_bson(&doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "name": "CreatedAt",
            "type": { "$Type": "DomainModels$DateTimeAttributeType" },
        });
        let compiled = compile_attribute_type(&attr);
        assert!(compiled.get_bool("LocalizeDate").unwrap());
    }

    #[test]
    fn an_index_attribute_missing_association_pointer_gets_the_zero_default() {
        let index = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "Attributes": mxrs_bson::build_array(vec![Bson::Document(doc! { "Attribute": "Sales.Order/Number" })], 1),
        };
        let compiled = compile_index(&index);
        let attrs = compiled.get_array("Attributes").unwrap();
        let Bson::Document(first) = &attrs[0] else {
            panic!("expected a document")
        };
        assert!(first.get("AssociationPointer").is_some());
    }
}
