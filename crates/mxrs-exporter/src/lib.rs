//! Reads an existing `.mpr` (via `mxrs-model`) and emits Rust
//! `project! {}` source text — the forward half of the "open a Mendix
//! project as editable Rust, change it, write it back" round trip. The
//! backward half already existed: `mxrs-writer::synchronize_project`
//! upserts entities/attributes/associations/microflows by name with `$ID`
//! preservation. Un-defers what the original phased plan called
//! `mxrs-exporter` (`decisions/mxrs-rust-rewrite-plan.md` in this
//! project's ai-memory) — that entry meant `Exporter`, model → **Ruby**
//! source, dropped for good (no Ruby anywhere in mxrs). This is a
//! different, new capability: model → **Rust** source, which the original
//! plan didn't anticipate needing.
//!
//! `export_project` is fail-closed: it refuses projects whose unsupported
//! domain features would be erased or reset by a subsequent synchronization.
//! `export_project_lossy` retains the original best-effort behavior for source
//! inspection, but its output must be reviewed before write-back.
//!
//! **First-slice scope, loud not silent about what's outside it**:
//!
//! - **Domain model only** — entities, attributes, associations. Not
//!   microflows: reconstructing structured `create`/`change`/`if`/`call`
//!   statements from a microflow's persisted activity *graph* (arbitrary
//!   branching, not just the linear-plus-one-decision shape a hand-written
//!   `project! {}` body produces) is a real decompiler, out of scope for
//!   this pass. Concretely safe consequence: an exported source file
//!   declares no microflows for any module, and
//!   `mxrs-writer::synchronize_project` never deletes anything absent from
//!   what it's given (see its own doc comment) — so writing an exported
//!   file straight back through `synchronize_project` leaves every
//!   existing microflow on disk untouched, not deleted.
//! - **Unsupported attribute types are rejected by the safe entry point and
//!   commented by the lossy entry point**: `project! {}`'s grammar covers seven attribute kinds
//!   (string/integer/long/decimal/boolean/datetime/autonumber);
//!   `Float`/`HashString`/`Binary`/`Enum` attributes get a `// TODO` line
//!   naming the attribute and its real type instead of vanishing from the
//!   output.
//! - **Association `Owner`/`StorageFormat`/`Documentation` aren't
//!   round-tripped** — not an exporter gap specifically, `project! {}`'s
//!   own grammar has no syntax for them yet (`EntityBuilder::association`
//!   itself only takes name/target/type; see `mxrs-macros`' grammar doc).
//!   Every exported association compiles back with `mxrs-dsl`'s defaults
//!   (`Owner::Default`, `StorageFormat::Column`) regardless of the real
//!   project's values.
//! - **Entity `Image`/`indexes`/`access rules`/`lifecycle callbacks`/
//!   `generalization target`** have no `EntityDecl` DSL surface at all yet
//!   (same gap `mxrs-writer`'s own doc comment already names) — not
//!   emitted.
//!
//! Marker types (`EntityMarker` impls `EntityBuilder::association`
//! requires for its target) are emitted inline in the same file, one
//! per entity, in a `markers` module mirroring the project's own
//! module/entity nesting — self-contained, no `build.rs`/manifest
//! required to compile the output (`mxrs-typegen`'s codegen path is a
//! separate, more typed way to get markers; this pass doesn't need it).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use mxrs_model::attribute::AttributeType;
use mxrs_model::entity::Entity;
#[cfg(test)]
use mxrs_model::Attribute;
use mxrs_model::{Association, Module, Project};

/// One model feature the generated `project! {}` source cannot faithfully
/// represent yet. Paths use Mendix qualified names so the user can resolve
/// every finding without having to inspect storage ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundTripGap {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),

    #[error("refusing a lossy Rust export; {0} unsupported model feature(s) would not round-trip (pass --allow-lossy only if this is intentional)")]
    Lossy(usize, Vec<RoundTripGap>),
}

impl ExportError {
    pub fn gaps(&self) -> &[RoundTripGap] {
        match self {
            ExportError::Lossy(_, gaps) => gaps,
            ExportError::Model(_) => &[],
        }
    }
}

pub type Result<T> = std::result::Result<T, ExportError>;

/// Exports only when the generated source can be synchronized back without
/// erasing a feature this first-slice grammar cannot express. The old,
/// explicitly lossy behavior remains available as [`export_project_lossy`]
/// for inspection and assisted migrations.
pub fn export_project(path: impl AsRef<Path>) -> Result<String> {
    let project = Project::open(path, true)?;
    let mendix_version = project.mendix_version()?.unwrap_or_default();
    let mut modules = project.modules()?;
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    let gaps = round_trip_gaps(&modules);
    if !gaps.is_empty() {
        return Err(ExportError::Lossy(gaps.len(), gaps));
    }
    Ok(render(&mendix_version, &modules))
}

/// Emits best-effort source even when unsupported features must be rendered
/// as `TODO` comments. Callers must not feed this output back to
/// `synchronize_project` without reviewing every comment.
pub fn export_project_lossy(path: impl AsRef<Path>) -> mxrs_model::Result<String> {
    let project = Project::open(path, true)?;
    let mendix_version = project.mendix_version()?.unwrap_or_default();
    let mut modules = project.modules()?;
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(render(&mendix_version, &modules))
}

fn round_trip_gaps(modules: &[Module]) -> Vec<RoundTripGap> {
    let entity_qualified_name_by_id = index_entities_by_id(modules);
    let mut gaps = Vec::new();
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        let Some(domain_model) = &module.domain_model else {
            continue;
        };
        for entity in &domain_model.entities {
            let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
            for attribute in &entity.attributes {
                let attribute_name = attribute.name.as_deref().unwrap_or("Unnamed");
                let path = format!("{module_name}.{entity_name}.{attribute_name}");
                if project_attr_keyword(attribute.attribute_type).is_none() {
                    gaps.push(RoundTripGap {
                        path: path.clone(),
                        reason: format!(
                            "attribute type {:?} has no project! grammar",
                            attribute.attribute_type
                        ),
                    });
                }
                if !attribute.documentation.is_empty()
                    || attribute.length.is_some_and(|length| length != 200)
                    || attribute.localize_date == Some(false)
                    || attribute.export_level != "Hidden"
                {
                    gaps.push(RoundTripGap {
                        path,
                        reason: "attribute metadata has no project! grammar".to_string(),
                    });
                }
            }
        }
        for association in domain_model.all_associations() {
            let name = association.name.as_deref().unwrap_or("Unnamed");
            let path = format!("{module_name}.{name}");
            let target_resolves = association.to_entity_id.as_deref().is_some_and(|target| {
                target.contains('.') || entity_qualified_name_by_id.contains_key(target)
            });
            if !target_resolves {
                gaps.push(RoundTripGap {
                    path: path.clone(),
                    reason: "association target cannot be resolved".to_string(),
                });
            }
            if association.owner != mxrs_model::association::Owner::Default
                || association.storage_format != mxrs_model::association::StorageFormat::Column
                || !association.documentation.is_empty()
                || association.export_level != "Hidden"
            {
                gaps.push(RoundTripGap {
                    path,
                    reason: "association metadata has no project! grammar".to_string(),
                });
            }
        }
    }
    gaps
}

fn render(mendix_version: &str, modules: &[Module]) -> String {
    let entity_qualified_name_by_id = index_entities_by_id(modules);

    let mut out = String::new();
    let _ = writeln!(
        out,
        "//! Generated by `mxrs-exporter` — edit freely, then write changes"
    );
    let _ = writeln!(
        out,
        "//! back with `mxrs_writer::synchronize_project(path, &build())`."
    );
    let _ = writeln!(
        out,
        "//! Domain model only (entities/attributes/associations) — see"
    );
    let _ = writeln!(
        out,
        "//! `mxrs-exporter`'s crate doc for what's not round-tripped yet"
    );
    let _ = writeln!(
        out,
        "//! (microflows, association Owner/StorageFormat, entity indexes/"
    );
    let _ = writeln!(
        out,
        "//! access rules/lifecycle, unsupported attribute types)."
    );
    let _ = writeln!(out, "//!");
    let _ = writeln!(
        out,
        "//! Depends on `mxrs-macros`, `mxrs-ir`, `mxrs-dsl`, and `mxrs-model`"
    );
    let _ = writeln!(
        out,
        "//! (the `project! {{}}` macro's own expansion needs the latter two"
    );
    let _ = writeln!(out, "//! in scope — see `mxrs-macros`' crate doc).");
    out.push('\n');

    out.push_str(&render_markers(modules));
    out.push('\n');

    let _ = writeln!(out, "pub fn build() -> ::mxrs_ir::ProjectDecl {{");
    let _ = writeln!(out, "    ::mxrs_macros::project! {{");
    let _ = writeln!(out, "        {:?},", mendix_version);
    for module in modules {
        out.push_str(&render_module(module, &entity_qualified_name_by_id));
    }
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "}}");
    out
}

fn index_entities_by_id(modules: &[Module]) -> HashMap<String, String> {
    let mut by_id = HashMap::new();
    for module in modules {
        for entity in module.entities() {
            if let (Some(id), Some(qualified_name)) = (&entity.id, &entity.qualified_name) {
                by_id.insert(id.clone(), qualified_name.clone());
            }
        }
    }
    by_id
}

fn render_markers(modules: &[Module]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "#[allow(non_snake_case, dead_code)]");
    let _ = writeln!(out, "pub mod markers {{");
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        let _ = writeln!(out, "    pub mod {} {{", sanitize_ident(module_name));
        for entity in module.entities() {
            let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
            let ident = sanitize_ident(entity_name);
            let _ = writeln!(out, "        pub struct {ident};");
            let _ = writeln!(out, "        impl ::mxrs_ir::EntityMarker for {ident} {{");
            let _ = writeln!(
                out,
                "            const MODULE: &'static str = {:?};",
                module_name
            );
            let _ = writeln!(
                out,
                "            const NAME: &'static str = {:?};",
                entity_name
            );
            let _ = writeln!(out, "        }}");
        }
        let _ = writeln!(out, "    }}");
    }
    let _ = writeln!(out, "}}");
    out
}

fn render_module(module: &Module, entity_qualified_name_by_id: &HashMap<String, String>) -> String {
    let module_name = module.name.as_deref().unwrap_or("Unnamed");
    let mut out = String::new();
    let _ = writeln!(out, "        module {} {{", sanitize_ident(module_name));

    let Some(domain_model) = &module.domain_model else {
        let _ = writeln!(out, "        }}");
        return out;
    };
    let mut associations_by_entity_id: HashMap<&str, Vec<&Association>> = HashMap::new();
    for association in domain_model.all_associations() {
        if let Some(from_id) = association.from_entity_id.as_deref() {
            associations_by_entity_id
                .entry(from_id)
                .or_default()
                .push(association);
        }
    }

    let mut entities: Vec<&Entity> = module.entities().iter().collect();
    entities.sort_by(|a, b| a.name.cmp(&b.name));
    for entity in entities {
        out.push_str(&render_entity(
            entity,
            associations_by_entity_id
                .get(entity.id.as_deref().unwrap_or(""))
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            entity_qualified_name_by_id,
        ));
    }
    let _ = writeln!(out, "        }}");
    out
}

fn render_entity(
    entity: &Entity,
    associations: &[&Association],
    entity_qualified_name_by_id: &HashMap<String, String>,
) -> String {
    let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
    let mut out = String::new();
    let _ = writeln!(out, "            entity {} {{", sanitize_ident(entity_name));
    if !entity.documentation.is_empty() {
        let _ = writeln!(
            out,
            "                documentation {:?};",
            entity.documentation
        );
    }
    let _ = writeln!(out, "                persistable {};", entity.persistable);

    let mut attributes = entity.attributes.clone();
    attributes.sort_by(|a, b| a.name.cmp(&b.name));
    for attribute in &attributes {
        let attribute_name = attribute.name.as_deref().unwrap_or("Unnamed");
        match project_attr_keyword(attribute.attribute_type) {
            Some(keyword) => {
                let default = attribute
                    .default_value
                    .as_deref()
                    .filter(|v| !v.is_empty())
                    .map(|v| format!(" = {v:?}"))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "                {keyword} {}{default};",
                    sanitize_ident(attribute_name)
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "                // TODO: attribute {attribute_name:?} has type {:?}, not supported by project! {{}}'s grammar yet",
                    attribute.attribute_type
                );
            }
        }
    }

    let mut sorted_associations = associations.to_vec();
    sorted_associations.sort_by_key(|a| a.name.clone());
    for association in sorted_associations {
        let assoc_name = association.name.as_deref().unwrap_or("Unnamed");
        let target = association.to_entity_id.as_deref().and_then(|id_or_name| {
            if id_or_name.contains('.') {
                Some(id_or_name.to_string())
            } else {
                entity_qualified_name_by_id.get(id_or_name).cloned()
            }
        });
        let Some(target) = target else {
            let _ = writeln!(
                out,
                "                // TODO: association {assoc_name:?} has an unresolvable target, skipped"
            );
            continue;
        };
        let target_path = target
            .splitn(2, '.')
            .map(sanitize_ident)
            .collect::<Vec<_>>()
            .join("::");
        let assoc_type = match association.association_type {
            mxrs_model::association::AssociationType::Reference => "Reference",
            mxrs_model::association::AssociationType::ReferenceSet => "ReferenceSet",
        };
        let _ = writeln!(
            out,
            "                association {} -> markers::{target_path} as {assoc_type};",
            sanitize_ident(assoc_name)
        );
    }

    let _ = writeln!(out, "            }}");
    out
}

fn project_attr_keyword(attribute_type: AttributeType) -> Option<&'static str> {
    match attribute_type {
        AttributeType::String => Some("string"),
        AttributeType::Integer => Some("integer"),
        AttributeType::Long => Some("long"),
        AttributeType::Decimal => Some("decimal"),
        AttributeType::Boolean => Some("boolean"),
        AttributeType::DateTime => Some("datetime"),
        AttributeType::AutoNumber => Some("autonumber"),
        AttributeType::Float
        | AttributeType::HashString
        | AttributeType::Binary
        | AttributeType::Enum => None,
    }
}

/// A defensive, not exhaustive, Mendix-name → Rust-identifier sanitizer:
/// non-alphanumeric/underscore characters become `_`, and a leading digit
/// (or an empty name) gets an `_` prefix. Doesn't raw-identifier-escape
/// (`r#...`) a name that collides with a Rust keyword — Mendix module/
/// entity/attribute names are conventionally PascalCase and never do in
/// practice, so this is a known, narrow simplification, not a silent gap
/// with real-world consequences.
fn sanitize_ident(name: &str) -> String {
    let mut ident: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if ident.is_empty() || ident.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        ident.insert(0, '_');
    }
    ident
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_ident_replaces_invalid_characters() {
        assert_eq!(sanitize_ident("Order Total"), "Order_Total");
        assert_eq!(sanitize_ident("2FA"), "_2FA");
        assert_eq!(sanitize_ident(""), "_");
        assert_eq!(sanitize_ident("Order"), "Order");
    }

    #[test]
    fn project_attr_keyword_covers_the_grammars_seven_kinds_and_gaps_the_rest() {
        assert_eq!(project_attr_keyword(AttributeType::String), Some("string"));
        assert_eq!(
            project_attr_keyword(AttributeType::AutoNumber),
            Some("autonumber")
        );
        assert_eq!(project_attr_keyword(AttributeType::Float), None);
        assert_eq!(project_attr_keyword(AttributeType::Enum), None);
    }

    fn entity(name: &str, qualified_name: &str, extra: mxrs_bson::Document) -> Entity {
        let mut d = mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$QualifiedName": qualified_name,
            "name": name,
        };
        d.extend(extra);
        Entity::from_bson(&d)
    }

    fn bare_module(name: &str, entities: Vec<Entity>) -> Module {
        Module {
            id: uuid::Uuid::new_v4().to_string(),
            name: Some(name.to_string()),
            sort_index: None,
            from_app_store: false,
            app_store_guid: None,
            app_store_version: None,
            export_level: "Hidden".to_string(),
            domain_model: Some(mxrs_model::DomainModel {
                id: None,
                native_type: None,
                documentation: String::new(),
                entities,
                associations: vec![],
                cross_associations: vec![],
            }),
            pages: vec![],
            microflows: vec![],
            nanoflows: vec![],
            rules: vec![],
            menus: vec![],
            module_roles: vec![],
        }
    }

    #[test]
    fn an_unsupported_attribute_type_is_commented_out_not_dropped() {
        let mut order = entity("Order", "Sales.Order", mxrs_bson::doc! {});
        order
            .attributes
            .push(Attribute::from_bson(&mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "name": "Score",
                "type": { "$Type": "DomainModels$FloatAttributeType" },
            }));
        let module = bare_module("Sales", vec![order]);

        let source = render_module(&module, &HashMap::new());
        assert!(source.contains("TODO"));
        assert!(source.contains("Score"));
        assert!(!source.contains("float Score"));
    }

    #[test]
    fn safe_export_refuses_an_attribute_the_generated_source_would_delete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
        });
        mxrs_writer::write_project(&path, &builder.build()).unwrap();

        let project = Project::open(&path, true).unwrap();
        let module = project.modules().unwrap().remove(0);
        drop(project);
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let domain_unit = mpr
            .units_by_containment("DomainModel")
            .unwrap()
            .into_iter()
            .find(|unit| unit.container_id == module.id)
            .unwrap();
        let mut domain_doc = mpr.parse_contents(&domain_unit).unwrap();
        let entities = domain_doc.get_array_mut("entities").unwrap();
        let order = entities
            .iter_mut()
            .find_map(|value| value.as_document_mut())
            .unwrap();
        order
            .get_array_mut("attributes")
            .unwrap()
            .push(mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "DomainModels$Attribute",
                "name": "Score",
                "type": { "$Type": "DomainModels$FloatAttributeType" },
            }));
        mpr.update_unit(&domain_unit.unit_id, domain_doc).unwrap();
        drop(mpr);

        let error = export_project(&path).unwrap_err();
        assert_eq!(error.gaps().len(), 1);
        assert_eq!(error.gaps()[0].path, "Sales.Order.Score");
        assert!(error.to_string().contains("refusing a lossy Rust export"));

        let source = export_project_lossy(&path).unwrap();
        assert!(source.contains("TODO: attribute \"Score\""));
    }

    #[test]
    fn renders_an_entity_with_attributes_and_a_same_module_association() {
        let mut customer = entity("Customer", "Sales.Customer", mxrs_bson::doc! {});
        customer
            .attributes
            .push(Attribute::from_bson(&mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(), "name": "Name",
                "type": { "$Type": "DomainModels$StringAttributeType" },
            }));
        let mut order = entity(
            "Order",
            "Sales.Order",
            mxrs_bson::doc! { "persistable": true },
        );
        order
            .attributes
            .push(Attribute::from_bson(&mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(), "name": "Number",
                "type": { "$Type": "DomainModels$StringAttributeType" },
            }));
        let association = Association::from_bson(&mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "Name": "Order_Customer",
            "ParentID": order.id.clone().unwrap(),
            "ChildID": customer.id.clone().unwrap(),
            "Type": "Reference",
        });
        let mut module = bare_module("Sales", vec![customer, order]);
        module
            .domain_model
            .as_mut()
            .unwrap()
            .associations
            .push(association);

        let entity_ids = index_entities_by_id(std::slice::from_ref(&module));
        let source = render_module(&module, &entity_ids);

        assert!(source.contains("entity Customer"));
        assert!(source.contains("entity Order"));
        assert!(source.contains("string Name"));
        assert!(source.contains("string Number"));
        assert!(source.contains("persistable true"));
        assert!(
            source.contains("association Order_Customer -> markers::Sales::Customer as Reference")
        );
    }

    #[test]
    fn export_project_round_trips_a_real_written_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");

        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |m| {
            m.entity("Customer", |e| {
                e.string("Name");
            });
            m.entity("Order", |e| {
                e.persistable(true);
                e.string("Number").default_value = Some("A-0".to_string());
                e.association(
                    "Order_Customer",
                    mxrs_ir::Ref::<sales_markers::Customer>::new(),
                    mxrs_model::association::AssociationType::Reference,
                );
            });
        });
        mxrs_writer::write_project(&path, &builder.build()).unwrap();

        let source = export_project(&path).unwrap();
        assert!(source.contains("pub mod markers"));
        assert!(source.contains("pub struct Order"));
        assert!(source.contains("pub struct Customer"));
        assert!(source.contains("string Number = \"A-0\""));
        assert!(
            source.contains("association Order_Customer -> markers::Sales::Customer as Reference")
        );
        assert!(source.contains("::mxrs_macros::project!"));
    }

    #[allow(dead_code, non_snake_case)]
    mod sales_markers {
        pub struct Customer;
        impl mxrs_ir::EntityMarker for Customer {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Customer";
        }
    }
}
