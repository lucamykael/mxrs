//! Renders each Mendix entity as the struct that declares it.
//!
//! Every entity — persistable, non-persistable or an OQL view — becomes one
//! `#[entity]`/`#[dto]`/`#[view]` struct in its own file. The struct is the
//! whole declaration: attributes and associations are its fields, the model's
//! documentation is its `///` comment, and indexes and event handlers are
//! `#[mxrs(...)]` options. There is no second, imperative form to fall back
//! to: a name Rust cannot spell is spelled differently and stated with
//! `name = "..."`, and the two things a struct cannot restate — an index over
//! a member it does not declare, an event the builder has no word for — are
//! kept from the imported model with `preserve(...)`, in the open.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

use mxrs_model::attribute::AttributeType;
use mxrs_model::entity::{Entity, IndexMemberKind};
use mxrs_model::{Association, Module};

use crate::{
    DerivedEnumeration, GeneratedModule, ModuleRoot, derive_pascal_case, enum_type_shadows_scalar,
    generated_module, inner_file_stem, known_microflows, module_root, module_stem, rust_keyword,
    sanitize_ident, snake_ident,
};

/// Where one entity's struct lives and how its members are spelled.
pub(crate) struct TypedEntityTarget {
    pub(crate) root: ModuleRoot,
    pub(crate) module_stem: String,
    pub(crate) file_stem: String,
    pub(crate) type_name: String,
    pub(crate) dto: bool,
    /// Mendix attribute name → the accessor the struct generates for it.
    pub(crate) attributes: HashMap<String, String>,
}

impl TypedEntityTarget {
    /// The module the entity's struct is importable from — under `dto/` when
    /// the entity is non-persistent, under `domain/entities/` otherwise, and
    /// under whichever module tree owns it.
    pub(crate) fn module_path(&self) -> String {
        let (authored, package) = if self.dto {
            ("domain::dtos", "dto")
        } else {
            ("domain::entities", "domain::entities")
        };
        format!(
            "{}::{}",
            self.root.path(&self.module_stem, authored, package),
            self.file_stem,
        )
    }

    /// The struct itself.
    pub(crate) fn type_path(&self) -> String {
        format!("{}::{}", self.module_path(), self.type_name)
    }
}

/// Project-wide context the entity layer renders against: who owns which
/// associations, how entity ids resolve to qualified names, and which
/// entities the project declares.
struct EntityLayerContext<'a> {
    associations_by_entity: HashMap<&'a str, Vec<&'a Association>>,
    qualified_by_id: HashMap<&'a str, String>,
    declared: HashSet<String>,
}

/// One association the entity's generated file must restate. The writer
/// preserves only associations whose target lives outside the declared
/// project; an association between declared entities that no entity file
/// re-declares would silently disappear from the rebuilt model.
struct DeclarableAssociation<'a> {
    association: &'a Association,
    name: &'a str,
    /// Qualified `Module.Entity` the association points at.
    target: String,
}

fn declarable_associations<'a>(
    entity: &Entity,
    ctx: &EntityLayerContext<'a>,
) -> Vec<DeclarableAssociation<'a>> {
    let Some(id) = entity.id.as_deref() else {
        return Vec::new();
    };
    let mut list = Vec::new();
    for association in ctx.associations_by_entity.get(id).into_iter().flatten() {
        let Some(name) = association.name.as_deref() else {
            continue;
        };
        let Some(raw_target) = association.to_entity_id.as_deref() else {
            continue;
        };
        let Some(target) = ctx.qualified_by_id.get(raw_target).cloned().or_else(|| {
            ctx.declared
                .contains(raw_target)
                .then(|| raw_target.to_string())
        }) else {
            continue;
        };
        if !ctx.declared.contains(&target) {
            // The writer's own unmodeled-external rule keeps this one.
            continue;
        }
        list.push(DeclarableAssociation {
            association,
            name,
            target,
        });
    }
    list.sort_by(|left, right| left.name.cmp(right.name));
    list.dedup_by(|left, right| left.name == right.name);
    list
}

/// The Rust type an entity declares. UpperCamelCase like any Rust type; a
/// name that would be empty, a keyword, or one of the types the struct's own
/// fields are written with gets an `Entity` suffix instead of a second
/// rendering form.
fn entity_type_name(entity_name: &str) -> String {
    let mut type_name = derive_pascal_case(&sanitize_ident(entity_name));
    if type_name.starts_with(|c: char| c.is_ascii_digit()) {
        type_name.insert(0, '_');
    }
    if type_name.is_empty()
        || rust_keyword(&type_name)
        || enum_type_shadows_scalar(&type_name)
        || matches!(type_name.as_str(), "Reference" | "ReferenceSet")
    {
        type_name.push_str("Entity");
    }
    type_name
}

/// Field identifiers the struct cannot use: Rust's keywords, the two
/// associated functions every entity already has, and the one word an
/// `index(...)` option reads as a flag rather than as a member.
fn reserved_field(ident: &str) -> bool {
    rust_keyword(ident) || matches!(ident, "mx_register" | "qualified_name" | "include_offline")
}

/// One Mendix member name as a struct field, unique within `seen`.
fn field_ident(mendix_name: &str, seen: &mut HashSet<String>) -> String {
    let mut ident = snake_ident(mendix_name);
    if ident.trim_matches('_').is_empty() {
        ident = "member".to_string();
    }
    if reserved_field(&ident) {
        ident.push('_');
    }
    if seen.insert(ident.clone()) {
        return ident;
    }
    (2..)
        .map(|suffix| format!("{ident}_{suffix}"))
        .find(|candidate| seen.insert(candidate.clone()))
        .expect("an unbounded suffix always finds a free name")
}

/// How every member of one entity is spelled: attributes by Mendix name,
/// in name order, then the associations the entity must restate.
struct EntityFields {
    attributes: Vec<String>,
    associations: Vec<String>,
}

fn sorted_attributes(entity: &Entity) -> Vec<&mxrs_model::attribute::Attribute> {
    let mut attributes = entity
        .attributes
        .iter()
        .filter(|attribute| {
            attribute
                .name
                .as_deref()
                .is_some_and(|name| !name.is_empty())
        })
        .collect::<Vec<_>>();
    attributes.sort_by(|left, right| left.name.cmp(&right.name));
    attributes
}

fn plan_fields(
    entity_name: &str,
    entity: &Entity,
    declarable: &[DeclarableAssociation<'_>],
) -> EntityFields {
    let mut seen = HashSet::new();
    let attributes = sorted_attributes(entity)
        .into_iter()
        .map(|attribute| field_ident(attribute.name.as_deref().unwrap_or_default(), &mut seen))
        .collect();
    // The entity macro names an association `{Entity}_{PascalField}` by
    // default, so the field is the name without its entity prefix.
    let prefix = format!("{entity_name}_");
    let associations = declarable
        .iter()
        .map(|declared| {
            let base = declared
                .name
                .strip_prefix(&prefix)
                .filter(|rest| !rest.is_empty())
                .unwrap_or(declared.name);
            field_ident(base, &mut seen)
        })
        .collect();
    EntityFields {
        attributes,
        associations,
    }
}

/// `documentation` as `///` lines at `indent`, when a comment says exactly
/// what the model does.
///
/// A doc comment is markdown to rustdoc and to Clippy, and rustfmt trims its
/// trailing whitespace, so only plain prose travels this way: text that
/// could be read as a list, a quote, a heading, a table or a code block —
/// and would then be linted, reflowed or compiled as a doctest — is stated
/// with `documentation = "..."` instead, byte for byte.
pub(crate) fn doc_comment(documentation: &str, indent: &str) -> Option<String> {
    if documentation.is_empty() {
        return Some(String::new());
    }
    let plain = documentation.split('\n').all(|line| {
        let first = line.chars().next();
        line == line.trim()
            && !line.contains('\r')
            && !line.contains("```")
            && !line.contains("~~~")
            && !first.is_some_and(|c| matches!(c, '-' | '*' | '+' | '>' | '#' | '|' | '=' | '`'))
            && !line.split_once(['.', ')']).is_some_and(|(marker, _)| {
                !marker.is_empty() && marker.chars().all(|c| c.is_ascii_digit())
            })
    });
    // A leading or trailing blank line would be a doc attribute the comment
    // cannot show, and the model's text would no longer match what is read.
    if !plain || documentation.starts_with('\n') || documentation.ends_with('\n') {
        return None;
    }
    let mut out = String::new();
    for line in documentation.split('\n') {
        if line.is_empty() {
            let _ = writeln!(out, "{indent}///");
        } else {
            let _ = writeln!(out, "{indent}/// {line}");
        }
    }
    Some(out)
}

/// The value the entity macro already assumes for an attribute that states
/// no default — see `authoritative_default` in `mxrs-macros`.
fn implied_default(kind: &str, view: bool) -> Option<&'static str> {
    if view {
        return None;
    }
    match kind {
        "boolean" => Some("false"),
        "integer" | "long" | "float" | "decimal" => Some("0"),
        "autonumber" => Some("1"),
        _ => None,
    }
}

/// `default = ...` as the literal a person would write: `true`, `42`, `0.5`,
/// or the model's own text when it is none of those.
fn default_literal(kind: &str, value: &str) -> String {
    match kind {
        "boolean" if matches!(value, "true" | "false") => value.to_string(),
        "integer" | "long" | "autonumber"
            if value
                .parse::<i64>()
                .is_ok_and(|parsed| parsed.to_string() == value) =>
        {
            value.to_string()
        }
        "float" | "decimal"
            if value.contains('.')
                && !value.starts_with('-')
                && value
                    .parse::<f64>()
                    .is_ok_and(|parsed| parsed.is_finite() && parsed.to_string() == value) =>
        {
            value.to_string()
        }
        _ => format!("{value:?}"),
    }
}

fn attribute_kind(attribute_type: AttributeType) -> &'static str {
    match attribute_type {
        AttributeType::String => "string",
        AttributeType::Integer => "integer",
        AttributeType::Long => "long",
        AttributeType::Float => "float",
        AttributeType::Decimal => "decimal",
        AttributeType::Boolean => "boolean",
        AttributeType::DateTime => "datetime",
        AttributeType::AutoNumber => "autonumber",
        AttributeType::HashString => "hash_string",
        AttributeType::Binary => "binary",
        AttributeType::Enum => "enumeration",
    }
}

/// One `index(...)` option, or `None` when the index names a member the
/// struct does not declare.
fn render_index(
    index: &mxrs_model::entity::EntityIndex,
    fields: &HashMap<String, String>,
) -> Option<String> {
    let mut members = Vec::new();
    for member in &index.members {
        let rendered = match &member.kind {
            IndexMemberKind::Attribute(name) => {
                let name = name.rsplit('.').next().unwrap_or(name);
                fields.get(name)?.clone()
            }
            IndexMemberKind::System(system) => format!("system({})", system.native_name()),
            IndexMemberKind::Unresolved(_) => return None,
        };
        members.push(if member.ascending {
            rendered
        } else {
            format!("desc({rendered})")
        });
    }
    if members.is_empty() {
        return None;
    }
    if index.include_offline {
        members.push("include_offline".to_string());
    }
    Some(format!("index({})", members.join(", ")))
}

/// Where the entity stands in the hierarchy: its parent, the system members
/// a root keeps, nothing for a plain root — or `preserve(inheritance)` when
/// the model holds a shape the declaration cannot restate.
///
/// `parent` is the spelled type of a parent this project declares.
fn render_inheritance(entity: &Entity, parent: Option<String>) -> Option<String> {
    const PRESERVE: &str = "preserve(inheritance)";
    let Some(generalization) = &entity.generalization else {
        return Some(PRESERVE.to_string());
    };
    if let Some(target) = &generalization.target {
        let parent = parent.or_else(|| built_in_parent(target).map(str::to_string));
        return Some(match parent {
            Some(parent) => format!("generalizes = {parent}"),
            None => PRESERVE.to_string(),
        });
    }
    // A root states whether it persists; one that leaves it out inherits
    // the answer, which only the model itself can keep saying.
    if !generalization.native_type.ends_with("NoGeneralization")
        || generalization.persistable != Some(entity.persistable)
    {
        return Some(PRESERVE.to_string());
    }
    let members = &generalization.system_members;
    let stored = [
        ("owner", members.owner),
        ("created_date", members.created_date),
        ("changed_date", members.changed_date),
        ("changed_by", members.changed_by),
    ]
    .into_iter()
    .filter_map(|(name, stored)| stored.then_some(name))
    .collect::<Vec<_>>();
    (!stored.is_empty()).then(|| format!("stores({})", stored.join(", ")))
}

/// The marker `mxrs` ships for a System entity a project may specialize.
fn built_in_parent(target: &str) -> Option<&'static str> {
    match target {
        "System.User" => Some("mxrs::system::User"),
        "System.FileDocument" => Some("mxrs::system::FileDocument"),
        "System.Image" => Some("mxrs::system::Image"),
        _ => None,
    }
}

/// One event handler option, or `None` for an event the builder cannot name
/// or a handler the project does not have.
fn render_lifecycle(
    callback: &mxrs_model::entity::LifecycleCallback,
    microflows: &HashSet<String>,
) -> Option<String> {
    if !matches!(
        callback.event.as_str(),
        "before_commit" | "after_commit" | "before_delete" | "after_delete"
    ) || !microflows.contains(&callback.handler)
    {
        return None;
    }
    let raises_by_default = callback.event.starts_with("before_");
    let mut options = Vec::new();
    if !callback.pass_event_object {
        options.push("pass_event_object = false".to_string());
    }
    if callback.raise_error_on_false != raises_by_default {
        options.push(format!(
            "raise_error_on_false = {}",
            callback.raise_error_on_false
        ));
    }
    Some(if options.is_empty() {
        format!("{} = {:?}", callback.event, callback.handler)
    } else {
        format!(
            "{}({:?}, {})",
            callback.event,
            callback.handler,
            options.join(", ")
        )
    })
}

/// What every entity file is rendered against.
struct RenderContext<'a> {
    derived_enums: &'a HashMap<String, DerivedEnumeration>,
    typed: &'a HashMap<String, TypedEntityTarget>,
    microflows: &'a HashSet<String>,
}

/// One entity as its `#[entity]`/`#[dto]`/`#[view]` struct.
fn render_entity_file(
    module_name: &str,
    entity: &Entity,
    declarable: &[DeclarableAssociation<'_>],
    fields: &EntityFields,
    context: &RenderContext<'_>,
) -> String {
    let RenderContext {
        derived_enums,
        typed,
        microflows,
    } = context;
    let entity_name = entity.name.as_deref().unwrap_or_default();
    let target = &typed[&format!("{module_name}.{entity_name}")];
    let type_name = &target.type_name;
    let own_qualified = format!("{module_name}.{entity_name}");
    let imported_module = target.root == ModuleRoot::Package;
    let view = entity.oql_view();
    let attributes = sorted_attributes(entity);

    // A referenced Rust type — an enumeration, or the target struct of an
    // association — is imported once by its short name; a name that
    // collides with another import, with this entity, or with a type the
    // fields are written in is spelled by its full path instead.
    let mut requests: Vec<(&str, String)> = Vec::new();
    for attribute in &attributes {
        let Some(derived) = attribute
            .enumeration
            .as_deref()
            .and_then(|qualified| derived_enums.get(qualified))
        else {
            continue;
        };
        if !enum_type_shadows_scalar(&derived.type_name) {
            requests.push((&derived.type_name, derived.module_path()));
        }
    }
    for declared in declarable {
        if declared.target == own_qualified {
            continue;
        }
        if let Some(target) = typed.get(&declared.target) {
            requests.push((&target.type_name, target.module_path()));
        }
    }
    // Only an entity this project declares states its parent; an installed
    // module's entity is named, and its hierarchy stays the module's own.
    let parent = (!imported_module && !view)
        .then(|| entity.generalization_target())
        .flatten()
        .and_then(|parent| typed.get(&parent));
    if let Some(parent) = parent {
        requests.push((&parent.type_name, parent.module_path()));
    }
    let mut imported: BTreeMap<&str, &str> = BTreeMap::new();
    let mut colliding = HashSet::new();
    for (name, path) in &requests {
        if let Some(existing) = imported.insert(name, path.as_str())
            && existing != path.as_str()
        {
            colliding.insert(*name);
        }
    }
    imported.retain(|name, _| {
        !colliding.contains(name) && *name != type_name && !crate::names::prelude_name(name)
    });
    let spell = |name: &str, path: &str| -> String {
        if imported.get(name).is_some_and(|found| *found == path) {
            name.to_string()
        } else {
            format!("{path}::{name}")
        }
    };

    let mut body = String::new();
    let mut field_by_attribute = HashMap::new();
    for (attribute, field) in attributes.iter().zip(&fields.attributes) {
        let mendix_name = attribute.name.as_deref().unwrap_or_default();
        field_by_attribute.insert(mendix_name.to_string(), field.clone());
        let kind = attribute_kind(attribute.attribute_type);
        let mut options = Vec::new();
        if derive_pascal_case(field) != mendix_name {
            options.push(format!("name = {mendix_name:?}"));
        }
        let field_type = match kind {
            "string" => "MxString".to_string(),
            "integer" => "MxInteger".to_string(),
            "long" => "MxLong".to_string(),
            "float" => "MxFloat".to_string(),
            "decimal" => "MxDecimal".to_string(),
            "boolean" => "MxBool".to_string(),
            "datetime" => "MxDateTime".to_string(),
            "binary" => "MxBinary".to_string(),
            "autonumber" => {
                options.push("kind = \"autonumber\"".to_string());
                "MxLong".to_string()
            }
            "hash_string" => {
                options.push("kind = \"hash_string\"".to_string());
                "MxString".to_string()
            }
            _ => {
                let enumeration = attribute.enumeration.as_deref().unwrap_or_default();
                match derived_enums
                    .get(enumeration)
                    .filter(|derived| !enum_type_shadows_scalar(&derived.type_name))
                {
                    Some(derived) => spell(&derived.type_name, &derived.module_path()),
                    None => {
                        options.push("kind = \"enumeration\"".to_string());
                        options.push(format!("enumeration = {enumeration:?}"));
                        "MxString".to_string()
                    }
                }
            }
        };
        let implied = implied_default(kind, view);
        match attribute.default_value.as_deref() {
            Some(default) if implied.map_or(!default.is_empty(), |implied| implied != default) => {
                options.push(format!("default = {}", default_literal(kind, default)));
            }
            Some(_) => {}
            // The model stores no default at all, where an unstated one
            // would mean the platform's: say so rather than invent it.
            None if implied.is_some() => options.push("no_default".to_string()),
            None => {}
        }
        // A stored attribute assumes the platform's length and
        // localization, so only a difference is worth stating. A view
        // assumes nothing: what its model states, the source states.
        if kind == "string" {
            match attribute.length {
                Some(0) => options.push("unlimited".to_string()),
                Some(length) if view || length != 200 => {
                    options.push(format!("length = {length}"));
                }
                _ => {}
            }
        }
        match attribute.localize_date {
            Some(false) => options.push("localize_date = false".to_string()),
            Some(true) if view && kind == "datetime" => {
                options.push("localize_date = true".to_string());
            }
            _ => {}
        }
        if attribute.required {
            options.push("required".to_string());
        }
        if attribute.unique {
            options.push("unique".to_string());
        }
        match doc_comment(&attribute.documentation, "    ") {
            Some(comment) => body.push_str(&comment),
            None => options.push(format!("documentation = {:?}", attribute.documentation)),
        }
        if !options.is_empty() {
            let _ = writeln!(body, "    #[mxrs({})]", options.join(", "));
        }
        let _ = writeln!(body, "    pub {field}: {field_type},");
    }

    for (declared, field) in declarable.iter().zip(&fields.associations) {
        let spelling = if declared.target == own_qualified {
            type_name.clone()
        } else {
            match typed.get(&declared.target) {
                Some(target) => spell(&target.type_name, &target.module_path()),
                None => continue,
            }
        };
        let container = match declared.association.association_type {
            mxrs_model::association::AssociationType::Reference => "Reference",
            mxrs_model::association::AssociationType::ReferenceSet => "ReferenceSet",
        };
        let mut options = Vec::new();
        if format!("{entity_name}_{}", derive_pascal_case(field)) != declared.name {
            options.push(format!("association = {:?}", declared.name));
        }
        if matches!(
            declared.association.owner,
            mxrs_model::association::Owner::Both
        ) {
            options.push("owner = \"Both\"".to_string());
        }
        if matches!(
            declared.association.storage_format,
            mxrs_model::association::StorageFormat::Table
        ) {
            options.push("storage = \"Table\"".to_string());
        }
        match doc_comment(&declared.association.documentation, "    ") {
            Some(comment) => body.push_str(&comment),
            None => options.push(format!(
                "documentation = {:?}",
                declared.association.documentation
            )),
        }
        if !options.is_empty() {
            let _ = writeln!(body, "    #[mxrs({})]", options.join(", "));
        }
        let _ = writeln!(body, "    pub {field}: {container}<{spelling}>,");
    }

    let mut out = String::from("use mxrs::prelude::*;\n");
    if !imported.is_empty() {
        out.push('\n');
        for (name, path) in &imported {
            let _ = writeln!(out, "use {path}::{name};");
        }
    }
    out.push('\n');

    let mut entity_options = Vec::new();
    match doc_comment(&entity.documentation, "") {
        Some(comment) => out.push_str(&comment),
        None => entity_options.push(format!("documentation = {:?}", entity.documentation)),
    }
    let mut arguments = vec![format!("module = {module_name:?}")];
    if type_name != entity_name {
        arguments.push(format!("name = {entity_name:?}"));
    }
    let attribute = if view {
        arguments.push(format!(
            "source = {:?}",
            entity.oql_source_document().unwrap_or_default()
        ));
        "view"
    } else if entity.persistable {
        "entity"
    } else {
        "dto"
    };
    if imported_module {
        arguments.push("imported".to_string());
    }
    let _ = writeln!(out, "#[{attribute}({})]", arguments.join(", "));

    // An installed module's entity is named, not declared: what it indexes
    // and which handlers it runs stay the module's own business.
    if !imported_module {
        if let Some(image) = entity.image.as_deref().filter(|image| !image.is_empty()) {
            entity_options.push(format!("image = {image:?}"));
        }
        if !view {
            let parent = parent.map(|parent| spell(&parent.type_name, &parent.module_path()));
            entity_options.extend(render_inheritance(entity, parent));
        }
        let indexes = entity
            .indexes
            .iter()
            .map(|index| render_index(index, &field_by_attribute))
            .collect::<Option<Vec<_>>>();
        match indexes {
            Some(indexes) => entity_options.extend(indexes),
            None => entity_options.push("preserve(indexes)".to_string()),
        }
        let lifecycle = entity
            .lifecycle
            .iter()
            .map(|callback| render_lifecycle(callback, microflows))
            .collect::<Option<Vec<_>>>();
        match lifecycle {
            Some(lifecycle) => entity_options.extend(lifecycle),
            None => entity_options.push("preserve(lifecycle)".to_string()),
        }
    }
    for option in &entity_options {
        let _ = writeln!(out, "#[mxrs({option})]");
    }
    if body.is_empty() {
        let _ = writeln!(out, "pub struct {type_name} {{}}");
    } else {
        let _ = writeln!(out, "pub struct {type_name} {{\n{body}}}");
    }
    out
}

/// A file stem no sibling in the same folder already uses.
fn unique_stem(stem: String, taken: &mut HashSet<String>) -> String {
    if taken.insert(stem.clone()) {
        return stem;
    }
    (2..)
        .map(|suffix| format!("{stem}_{suffix}"))
        .find(|candidate| taken.insert(candidate.clone()))
        .expect("an unbounded suffix always finds a free name")
}

/// Collects each module's entity declarations: persisted entities into the
/// module's `domain/entities`, everything non-persistent (including OQL
/// view entities) into its DTO folder. Returns where every entity's struct
/// lives, so flows, pages and ports can name it.
pub(crate) fn collect_entity_layer(
    modules: &[Module],
    packages: &std::collections::BTreeSet<String>,
    derived_enums: &HashMap<String, DerivedEnumeration>,
    generated: &mut BTreeMap<String, GeneratedModule>,
) -> HashMap<String, TypedEntityTarget> {
    let microflows = known_microflows(modules);

    let mut ctx = EntityLayerContext {
        associations_by_entity: HashMap::new(),
        qualified_by_id: HashMap::new(),
        declared: HashSet::new(),
    };
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        for entity in module.entities() {
            let Some(entity_name) = entity.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            let qualified = format!("{module_name}.{entity_name}");
            ctx.declared.insert(qualified.clone());
            if let Some(id) = entity.id.as_deref() {
                ctx.qualified_by_id.insert(id, qualified);
            }
        }
        for association in module.associations() {
            if let Some(from) = association.from_entity_id.as_deref() {
                ctx.associations_by_entity
                    .entry(from)
                    .or_default()
                    .push(association);
            }
        }
    }

    // Every name is decided before any file is rendered: an association
    // field is typed by its target's struct, wherever that is declared.
    let mut typed = HashMap::new();
    let mut plans = Vec::new();
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        let mut module_entities = module
            .entities()
            .iter()
            .filter(|entity| entity.name.as_deref().is_some_and(|name| !name.is_empty()))
            .collect::<Vec<_>>();
        module_entities.sort_by(|left, right| left.name.cmp(&right.name));
        let mut entity_stems = HashSet::new();
        let mut dto_stems = HashSet::new();
        for entity in module_entities {
            let entity_name = entity.name.as_deref().unwrap_or_default();
            let qualified = format!("{module_name}.{entity_name}");
            let dto = !entity.persistable || entity.oql_view();
            let file_stem = unique_stem(
                inner_file_stem(entity_name),
                if dto {
                    &mut dto_stems
                } else {
                    &mut entity_stems
                },
            );
            let declarable = declarable_associations(entity, &ctx);
            let fields = plan_fields(entity_name, entity, &declarable);
            // A member is found by its Mendix name: an attribute by its own,
            // an association by its qualified one (`Sales.Order_Customer`).
            let attributes = sorted_attributes(entity)
                .into_iter()
                .zip(&fields.attributes)
                .map(|(attribute, field)| {
                    (attribute.name.clone().unwrap_or_default(), field.clone())
                })
                .chain(
                    declarable
                        .iter()
                        .zip(&fields.associations)
                        .map(|(declared, field)| {
                            (format!("{module_name}.{}", declared.name), field.clone())
                        }),
                )
                .collect();
            typed.insert(
                qualified.clone(),
                TypedEntityTarget {
                    root: module_root(&module_stem(module_name), packages),
                    module_stem: module_stem(module_name),
                    file_stem,
                    type_name: entity_type_name(entity_name),
                    dto,
                    attributes,
                },
            );
            plans.push((module_name, entity, qualified, declarable, fields));
        }
    }

    let context = RenderContext {
        derived_enums,
        typed: &typed,
        microflows: &microflows,
    };
    for (module_name, entity, qualified, declarable, fields) in plans {
        let source = render_entity_file(module_name, entity, &declarable, &fields, &context);
        let target = &typed[&qualified];
        let module = generated_module(generated, module_name);
        if target.dto {
            module.dtos.push((target.file_stem.clone(), source));
        } else {
            module.entities.push((target.file_stem.clone(), source));
        }
    }
    typed
}

#[cfg(test)]
mod tests {
    use mxrs_model::association::{AssociationType, Owner, StorageFormat};
    use mxrs_model::attribute::Attribute;
    use mxrs_model::entity::{
        EntityIndex, Generalization, IndexedAttribute, IndexedSystemMember, LifecycleCallback,
        Location, SystemMembers,
    };

    use super::*;

    fn bare_entity(name: &str) -> Entity {
        Entity {
            id: None,
            name: Some(name.to_string()),
            qualified_name: None,
            documentation: String::new(),
            persistable: true,
            location: Location { x: 0, y: 0 },
            data_storage_guid: None,
            image: None,
            export_level: String::new(),
            generalization: Some(root(true, SystemMembers::default())),
            access_rules: Vec::new(),
            indexes: Vec::new(),
            system_members: SystemMembers::default(),
            lifecycle: Vec::new(),
            validation_rules: Vec::new(),
            source: None,
            oql_query: None,
            native_type: None,
            attributes: Vec::new(),
        }
    }

    /// The hierarchy of an entity with no parent, as Studio Pro stores it.
    fn root(persistable: bool, system_members: SystemMembers) -> Generalization {
        Generalization {
            id: None,
            native_type: "DomainModels$NoGeneralization".to_string(),
            target: None,
            persistable: Some(persistable),
            system_members,
            raw: mxrs_bson::Document::new(),
        }
    }

    fn specialization(target: &str) -> Generalization {
        Generalization {
            id: None,
            native_type: "DomainModels$Generalization".to_string(),
            target: Some(target.to_string()),
            persistable: None,
            system_members: SystemMembers::default(),
            raw: mxrs_bson::Document::new(),
        }
    }

    fn attribute(name: &str, attribute_type: AttributeType) -> Attribute {
        Attribute {
            id: None,
            name: Some(name.to_string()),
            documentation: String::new(),
            attribute_type,
            default_value: None,
            data_storage_guid: None,
            export_level: String::new(),
            raw_type_doc: None,
            raw_value_doc: None,
            length: None,
            localize_date: None,
            enumeration: None,
            required: false,
            unique: false,
        }
    }

    fn association(name: &str, to: &str, set: bool) -> Association {
        Association {
            id: None,
            name: Some(name.to_string()),
            documentation: String::new(),
            from_entity_id: Some("self".to_string()),
            to_entity_id: Some(to.to_string()),
            association_type: if set {
                AssociationType::ReferenceSet
            } else {
                AssociationType::Reference
            },
            owner: Owner::Default,
            storage_format: StorageFormat::Column,
            source: None,
            guid: None,
            delete_behavior: None,
            export_level: "Hidden".to_string(),
        }
    }

    fn derived(module: &str, file: &str, type_name: &str) -> DerivedEnumeration {
        DerivedEnumeration {
            root: ModuleRoot::Authored,
            module_stem: module.to_string(),
            file_stem: file.to_string(),
            type_name: type_name.to_string(),
            variants: HashMap::new(),
        }
    }

    /// Renders `entity` of `module_name` in a project that also declares
    /// `others` (`(qualified name, module stem, file stem, type name)`),
    /// owning `associations` (each pointing at a qualified name).
    fn render(
        module_name: &str,
        root: ModuleRoot,
        entity: &Entity,
        derived_enums: &HashMap<String, DerivedEnumeration>,
        others: &[(&str, &str, &str, &str)],
        associations: &[(Association, &str)],
        microflows: &[&str],
    ) -> String {
        let entity_name = entity.name.as_deref().unwrap();
        let declarable = associations
            .iter()
            .map(|(association, target)| DeclarableAssociation {
                association,
                name: association.name.as_deref().unwrap(),
                target: (*target).to_string(),
            })
            .collect::<Vec<_>>();
        let fields = plan_fields(entity_name, entity, &declarable);
        let mut typed = HashMap::new();
        typed.insert(
            format!("{module_name}.{entity_name}"),
            TypedEntityTarget {
                root,
                module_stem: module_stem(module_name),
                file_stem: inner_file_stem(entity_name),
                type_name: entity_type_name(entity_name),
                dto: !entity.persistable || entity.oql_view(),
                attributes: HashMap::new(),
            },
        );
        for (qualified, module, file, type_name) in others {
            typed.insert(
                (*qualified).to_string(),
                TypedEntityTarget {
                    root: ModuleRoot::Authored,
                    module_stem: (*module).to_string(),
                    file_stem: (*file).to_string(),
                    type_name: (*type_name).to_string(),
                    dto: false,
                    attributes: HashMap::new(),
                },
            );
        }
        let microflows = microflows.iter().map(|name| (*name).to_string()).collect();
        render_entity_file(
            module_name,
            entity,
            &declarable,
            &fields,
            &RenderContext {
                derived_enums,
                typed: &typed,
                microflows: &microflows,
            },
        )
    }

    fn render_alone(module_name: &str, entity: &Entity) -> String {
        render(
            module_name,
            ModuleRoot::Authored,
            entity,
            &HashMap::new(),
            &[],
            &[],
            &[],
        )
    }

    /// The struct is the authoring surface a hand author uses: the whole
    /// file is pinned so a regression in eloquence — a stray absolute path,
    /// a restated default, a lost option — fails loudly.
    #[test]
    fn an_entity_file_is_its_struct_pinned_whole() {
        let mut entity = bare_entity("Parameter");
        entity.documentation = "Catalog parameter.".to_string();
        let mut limit = attribute("Limit", AttributeType::Enum);
        limit.enumeration = Some("Catalogs.ENUM_Limit".to_string());
        limit.default_value = Some(String::new());
        entity.attributes.push(limit);
        let mut measure = attribute("ParameterToMeasure", AttributeType::String);
        measure.length = Some(50);
        measure.required = true;
        measure.documentation = "What is measured.".to_string();
        entity.attributes.push(measure);
        let mut sequence = attribute("APIKey", AttributeType::AutoNumber);
        sequence.default_value = Some("1".to_string());
        sequence.unique = true;
        entity.attributes.push(sequence);
        let mut updated = attribute("UpdatedAt", AttributeType::DateTime);
        updated.localize_date = Some(false);
        entity.attributes.push(updated);
        let mut created = attribute("CreatedAt", AttributeType::DateTime);
        created.localize_date = Some(true);
        created.default_value = Some("[%CurrentDateTime%]".to_string());
        entity.attributes.push(created);
        let mut name = attribute("Name", AttributeType::String);
        name.length = Some(200);
        name.default_value = Some(String::new());
        entity.attributes.push(name);
        let mut notes = attribute("Notes", AttributeType::String);
        notes.length = Some(0);
        notes.documentation = "- a list\n- of notes".to_string();
        entity.attributes.push(notes);
        let mut active = attribute("Active", AttributeType::Boolean);
        active.default_value = Some("true".to_string());
        entity.attributes.push(active);
        let mut archived = attribute("Archived", AttributeType::Boolean);
        archived.default_value = Some("false".to_string());
        entity.attributes.push(archived);
        let mut count = attribute("Count", AttributeType::Integer);
        count.default_value = Some("0".to_string());
        entity.attributes.push(count);
        let mut limit_value = attribute("Maximum", AttributeType::Decimal);
        limit_value.default_value = Some("2.5".to_string());
        entity.attributes.push(limit_value);
        let mut unset = attribute("Minimum", AttributeType::Integer);
        unset.default_value = Some(String::new());
        entity.attributes.push(unset);
        entity
            .attributes
            .push(attribute("Type", AttributeType::Binary));

        assert_eq!(
            render_alone("Catalogs", &entity),
            r#"use mxrs::prelude::*;

/// Catalog parameter.
#[entity(module = "Catalogs")]
pub struct Parameter {
    #[mxrs(name = "APIKey", kind = "autonumber", unique)]
    pub api_key: MxLong,
    #[mxrs(default = true)]
    pub active: MxBool,
    pub archived: MxBool,
    pub count: MxInteger,
    #[mxrs(default = "[%CurrentDateTime%]")]
    pub created_at: MxDateTime,
    #[mxrs(kind = "enumeration", enumeration = "Catalogs.ENUM_Limit")]
    pub limit: MxString,
    #[mxrs(default = 2.5)]
    pub maximum: MxDecimal,
    #[mxrs(default = "")]
    pub minimum: MxInteger,
    pub name: MxString,
    #[mxrs(unlimited, documentation = "- a list\n- of notes")]
    pub notes: MxString,
    /// What is measured.
    #[mxrs(length = 50, required)]
    pub parameter_to_measure: MxString,
    pub type_: MxBinary,
    #[mxrs(localize_date = false)]
    pub updated_at: MxDateTime,
}
"#
        );
    }

    #[test]
    fn the_attribute_says_what_kind_of_entity_it_is() {
        let mut dto = bare_entity("AccountPasswordData");
        dto.persistable = false;
        dto.generalization = Some(root(false, SystemMembers::default()));
        assert_eq!(
            render_alone("Administration", &dto),
            "use mxrs::prelude::*;\n\n#[dto(module = \"Administration\")]\npub struct AccountPasswordData {}\n"
        );

        let mut view = bare_entity("Order_Totals");
        view.persistable = false;
        view.source = Some(mxrs_bson::doc! {
            "$Type": "DomainModels$OqlViewEntitySource",
            "SourceDocument": "Sales.OrderTotals",
        });
        let mut total = attribute("Total", AttributeType::Decimal);
        total.default_value = None;
        view.attributes.push(total);
        assert_eq!(
            render_alone("Sales", &view),
            r#"use mxrs::prelude::*;

#[view(module = "Sales", name = "Order_Totals", source = "Sales.OrderTotals")]
pub struct OrderTotals {
    pub total: MxDecimal,
}
"#
        );

        // An installed module's entity is named, not declared.
        let mut installed = bare_entity("Log");
        installed.indexes.push(EntityIndex {
            id: None,
            guid: None,
            include_offline: false,
            members: Vec::new(),
            raw: mxrs_bson::Document::new(),
        });
        let rendered = render(
            "CommunityCommons",
            ModuleRoot::Package,
            &installed,
            &HashMap::new(),
            &[],
            &[],
            &[],
        );
        assert_eq!(
            rendered,
            "use mxrs::prelude::*;\n\n#[entity(module = \"CommunityCommons\", imported)]\npub struct Log {}\n"
        );
    }

    /// Associations between declared entities become Reference<T> fields: a
    /// self-reference names the entity's own struct, a default-shaped name
    /// needs no restatement, and non-default name/owner/storage/
    /// documentation are restated as field options.
    #[test]
    fn association_fields_reference_the_target_structs() {
        let parent = association("Ticket_Parent", "Sales.Ticket", false);
        let mut assigned = association("Assigned", "Sales.Order", true);
        assigned.owner = Owner::Both;
        assigned.storage_format = StorageFormat::Table;
        assigned.documentation = "Who.".to_string();
        let mut entity = bare_entity("Ticket");
        entity
            .attributes
            .push(attribute("Assigned", AttributeType::Boolean));

        let rendered = render(
            "Sales",
            ModuleRoot::Authored,
            &entity,
            &HashMap::new(),
            &[("Sales.Order", "sales", "order", "Order")],
            &[(assigned, "Sales.Order"), (parent, "Sales.Ticket")],
            &[],
        );
        assert_eq!(
            rendered,
            r#"use mxrs::prelude::*;

use crate::domain::entities::sales::order::Order;

#[entity(module = "Sales")]
pub struct Ticket {
    #[mxrs(no_default)]
    pub assigned: MxBool,
    /// Who.
    #[mxrs(association = "Assigned", owner = "Both", storage = "Table")]
    pub assigned_2: ReferenceSet<Order>,
    pub parent: Reference<Ticket>,
}
"#
        );
    }

    /// An attribute typed by an enumeration names the enum itself; two enums
    /// of one name, one sharing the entity's name, and one a scalar would
    /// hide are spelled so the file still means one thing.
    #[test]
    fn enumeration_attributes_are_typed_by_their_enum() {
        let mut enums = HashMap::new();
        enums.insert(
            "Catalogs.ENUM_Limit".to_string(),
            derived("catalogs", "enum_limit", "ENUMLimit"),
        );
        enums.insert(
            "Sales.Status".to_string(),
            derived("sales", "status", "Status"),
        );
        enums.insert(
            "Support.Status".to_string(),
            derived("support", "status", "Status"),
        );
        enums.insert(
            "Sales.MxString".to_string(),
            derived("sales", "mx_string", "MxString"),
        );

        let mut entity = bare_entity("Parameter");
        for (name, enumeration, default) in [
            ("Limit", "Catalogs.ENUM_Limit", Some("Ten")),
            ("SalesStatus", "Sales.Status", None),
            ("SupportStatus", "Support.Status", None),
            ("Opaque", "Catalogs.Unknown", None),
            ("Shadowing", "Sales.MxString", None),
        ] {
            let mut attribute = attribute(name, AttributeType::Enum);
            attribute.enumeration = Some(enumeration.to_string());
            attribute.default_value = default.map(str::to_string);
            entity.attributes.push(attribute);
        }
        let rendered = render(
            "Catalogs",
            ModuleRoot::Authored,
            &entity,
            &enums,
            &[],
            &[],
            &[],
        );
        assert_eq!(
            rendered,
            r#"use mxrs::prelude::*;

use crate::domain::enumerations::catalogs::enum_limit::ENUMLimit;

#[entity(module = "Catalogs")]
pub struct Parameter {
    #[mxrs(default = "Ten")]
    pub limit: ENUMLimit,
    #[mxrs(kind = "enumeration", enumeration = "Catalogs.Unknown")]
    pub opaque: MxString,
    pub sales_status: crate::domain::enumerations::sales::status::Status,
    #[mxrs(kind = "enumeration", enumeration = "Sales.MxString")]
    pub shadowing: MxString,
    pub support_status: crate::domain::enumerations::support::status::Status,
}
"#
        );

        // An enum sharing the entity's own type name keeps its full path.
        let mut status = bare_entity("Status");
        let mut current = attribute("Current", AttributeType::Enum);
        current.enumeration = Some("Sales.Status".to_string());
        status.attributes.push(current);
        let rendered = render(
            "Sales",
            ModuleRoot::Authored,
            &status,
            &enums,
            &[],
            &[],
            &[],
        );
        assert!(
            rendered.contains("pub current: crate::domain::enumerations::sales::status::Status,"),
            "{rendered}"
        );
        assert!(!rendered.contains("use crate::"), "{rendered}");
    }

    fn index(members: Vec<(IndexMemberKind, bool)>, include_offline: bool) -> EntityIndex {
        EntityIndex {
            id: None,
            guid: None,
            include_offline,
            members: members
                .into_iter()
                .map(|(kind, ascending)| IndexedAttribute {
                    id: None,
                    kind,
                    attribute_pointer: None,
                    ascending,
                    raw: mxrs_bson::Document::new(),
                })
                .collect(),
            raw: mxrs_bson::Document::new(),
        }
    }

    fn callback(event: &str, handler: &str, pass: bool, raise: bool) -> LifecycleCallback {
        LifecycleCallback {
            id: None,
            event: event.to_string(),
            handler: handler.to_string(),
            pass_event_object: pass,
            raise_error_on_false: raise,
            raw: mxrs_bson::Document::new(),
        }
    }

    /// Indexes and event handlers are options of the struct that owns them,
    /// stated only where they differ from what the builder assumes.
    #[test]
    fn indexes_and_event_handlers_are_options_of_the_struct() {
        let mut entity = bare_entity("Order");
        entity.image = Some("Sales.OrderIcon".to_string());
        entity
            .attributes
            .push(attribute("Number", AttributeType::String));
        entity
            .attributes
            .push(attribute("Total", AttributeType::Decimal));
        entity.indexes.push(index(
            vec![(
                IndexMemberKind::Attribute("Sales.Order.Number".to_string()),
                true,
            )],
            false,
        ));
        entity.indexes.push(index(
            vec![
                (IndexMemberKind::Attribute("Total".to_string()), false),
                (
                    IndexMemberKind::System(IndexedSystemMember::ChangedDate),
                    true,
                ),
            ],
            true,
        ));
        entity
            .lifecycle
            .push(callback("before_commit", "Sales.VAL_Order", true, true));
        entity
            .lifecycle
            .push(callback("after_delete", "Sales.SUB_Audit", false, true));
        let rendered = render(
            "Sales",
            ModuleRoot::Authored,
            &entity,
            &HashMap::new(),
            &[],
            &[],
            &["Sales.VAL_Order", "Sales.SUB_Audit"],
        );
        assert_eq!(
            rendered,
            r#"use mxrs::prelude::*;

#[entity(module = "Sales")]
#[mxrs(image = "Sales.OrderIcon")]
#[mxrs(index(number))]
#[mxrs(index(desc(total), system(ChangedDate), include_offline))]
#[mxrs(before_commit = "Sales.VAL_Order")]
#[mxrs(after_delete("Sales.SUB_Audit", pass_event_object = false, raise_error_on_false = true))]
pub struct Order {
    pub number: MxString,
    #[mxrs(no_default)]
    pub total: MxDecimal,
}
"#
        );
    }

    /// What a struct cannot restate is kept from the imported model, and
    /// says so.
    #[test]
    fn an_entity_states_its_parent_or_the_system_members_it_stores() {
        let options = |entity: &Entity, typed: &[(&str, &str, &str, &str)]| {
            let rendered = render(
                "Sales",
                ModuleRoot::Authored,
                entity,
                &HashMap::new(),
                typed,
                &[],
                &[],
            );
            rendered
                .lines()
                .filter(|line| line.starts_with("#[mxrs(") || line.starts_with("use crate"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        let mut entity = bare_entity("Order");
        // A plain root is what a struct is without saying anything.
        assert!(options(&entity, &[]).is_empty());

        entity.generalization = Some(root(
            true,
            SystemMembers {
                owner: true,
                created_date: false,
                changed_date: true,
                changed_by: false,
            },
        ));
        assert_eq!(
            options(&entity, &[]),
            ["#[mxrs(stores(owner, changed_date))]"]
        );

        // A parent the project declares is named by its struct.
        entity.generalization = Some(specialization("Sales.Document"));
        assert_eq!(
            options(
                &entity,
                &[("Sales.Document", "sales", "document", "Document")]
            ),
            [
                "use crate::domain::entities::sales::document::Document;",
                "#[mxrs(generalizes = Document)]",
            ]
        );
        // A System entity `mxrs` ships a marker for.
        entity.generalization = Some(specialization("System.FileDocument"));
        assert_eq!(
            options(&entity, &[]),
            ["#[mxrs(generalizes = mxrs::system::FileDocument)]"]
        );

        // What cannot be restated is kept, and says so: a parent nothing
        // here names, a root that leaves persistability to be inherited, and
        // a model with no hierarchy stored at all.
        entity.generalization = Some(specialization("System.Session"));
        assert_eq!(options(&entity, &[]), ["#[mxrs(preserve(inheritance))]"]);
        let mut inherited = root(true, SystemMembers::default());
        inherited.persistable = None;
        entity.generalization = Some(inherited);
        assert_eq!(options(&entity, &[]), ["#[mxrs(preserve(inheritance))]"]);
        entity.generalization = None;
        assert_eq!(options(&entity, &[]), ["#[mxrs(preserve(inheritance))]"]);
    }

    #[test]
    fn a_view_states_what_its_model_states() {
        let mut view = bare_entity("Order_Totals");
        view.persistable = false;
        view.source = Some(mxrs_bson::doc! {
            "$Type": "DomainModels$OqlViewEntitySource",
            "SourceDocument": "Sales.OrderTotals",
        });
        let mut label = attribute("Label", AttributeType::String);
        label.length = Some(200);
        let mut day = attribute("Day", AttributeType::DateTime);
        day.localize_date = Some(true);
        view.attributes.extend([label, day]);
        let rendered = render_alone("Sales", &view);
        // A stored entity would leave both unsaid; a view assumes nothing.
        assert!(
            rendered.contains("    #[mxrs(localize_date = true)]\n    pub day: MxDateTime,"),
            "{rendered}"
        );
        assert!(
            rendered.contains("    #[mxrs(length = 200)]\n    pub label: MxString,"),
            "{rendered}"
        );
        assert!(!rendered.contains("inheritance"), "{rendered}");
    }

    #[test]
    fn an_attribute_named_like_an_index_flag_is_spelled_apart_from_it() {
        let mut entity = bare_entity("Device");
        entity
            .attributes
            .push(attribute("IncludeOffline", AttributeType::Boolean));
        entity.indexes.push(index(
            vec![(
                IndexMemberKind::Attribute("IncludeOffline".to_string()),
                true,
            )],
            false,
        ));
        let rendered = render_alone("Sales", &entity);
        assert!(
            rendered.contains("#[mxrs(index(include_offline_))]"),
            "{rendered}"
        );
        assert!(
            rendered.contains("    pub include_offline_: MxBool,"),
            "{rendered}"
        );
    }

    #[test]
    fn what_the_struct_cannot_restate_is_preserved_in_the_open() {
        let mut entity = bare_entity("Order");
        entity
            .attributes
            .push(attribute("Number", AttributeType::String));
        // An index over a member this struct does not declare.
        entity.indexes.push(index(
            vec![(IndexMemberKind::Attribute("Inherited".to_string()), true)],
            false,
        ));
        // An event the builder has no word for, and a handler the project
        // does not have.
        entity
            .lifecycle
            .push(callback("before_rollback", "Sales.SUB_Audit", true, true));
        let rendered = render(
            "Sales",
            ModuleRoot::Authored,
            &entity,
            &HashMap::new(),
            &[],
            &[],
            &["Sales.SUB_Audit"],
        );
        assert!(
            rendered.contains("#[mxrs(preserve(indexes))]\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains("#[mxrs(preserve(lifecycle))]\n"),
            "{rendered}"
        );
        assert!(!rendered.contains("index("), "{rendered}");

        entity.lifecycle.clear();
        entity
            .lifecycle
            .push(callback("before_commit", "Sales.Missing", true, true));
        let rendered = render_alone("Sales", &entity);
        assert!(
            rendered.contains("#[mxrs(preserve(lifecycle))]\n"),
            "{rendered}"
        );
    }

    #[test]
    fn plain_prose_becomes_a_doc_comment() {
        assert_eq!(doc_comment("", "    ").as_deref(), Some(""));
        assert_eq!(
            doc_comment("A customer order.", "").as_deref(),
            Some("/// A customer order.\n")
        );
        assert_eq!(
            doc_comment("First.\n\nSecond.", "    ").as_deref(),
            Some("    /// First.\n    ///\n    /// Second.\n")
        );
    }

    #[test]
    fn text_a_comment_would_change_is_stated_as_an_option() {
        for documentation in [
            "trailing space ",
            " leading space",
            "- a list item",
            "1. a numbered item",
            "> a quote",
            "# a heading",
            "| a | table |",
            "```\ncode\n```",
            "windows\r\nline",
            "\nleading blank",
            "trailing blank\n",
            "    indented code",
        ] {
            assert_eq!(doc_comment(documentation, ""), None, "{documentation:?}");
        }
    }

    #[test]
    fn a_default_reads_as_the_literal_a_person_would_write() {
        assert_eq!(default_literal("boolean", "true"), "true");
        assert_eq!(default_literal("integer", "42"), "42");
        assert_eq!(default_literal("integer", "042"), "\"042\"");
        assert_eq!(default_literal("decimal", "0.5"), "0.5");
        assert_eq!(default_literal("decimal", "1.50"), "\"1.50\"");
        assert_eq!(default_literal("string", "A-0000"), "\"A-0000\"");
        assert_eq!(
            default_literal("datetime", "[%CurrentDateTime%]"),
            "\"[%CurrentDateTime%]\""
        );
    }

    #[test]
    fn a_member_name_rust_cannot_use_is_spelled_differently() {
        let mut seen = HashSet::new();
        assert_eq!(field_ident("Type", &mut seen), "type_");
        assert_eq!(field_ident("OrderDate", &mut seen), "order_date");
        assert_eq!(field_ident("Order_Date", &mut seen), "order_date_2");
        assert_eq!(field_ident("QualifiedName", &mut seen), "qualified_name_");
        assert_eq!(entity_type_name("String"), "StringEntity");
        assert_eq!(entity_type_name("Order_Line"), "OrderLine");
    }
}
