//! Domain model entities. Embedded inside `DomainModel` BSON — not separate
//! Unit rows. Ports `lib/mxrb/model/entity.rb` from mxrb.

use mxrs_bson::{Document, doc};

use crate::attribute::{Attribute, apply_validation_rules};
use crate::support::{
    docs_any, get_any, get_bool_any, get_doc_any, get_i32_any, get_id_any, get_str_any, items_any,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemMembers {
    pub owner: bool,
    pub created_date: bool,
    pub changed_date: bool,
    pub changed_by: bool,
}

#[derive(Debug, Clone)]
pub struct Generalization {
    pub id: Option<String>,
    pub native_type: String,
    /// `None` means `DomainModels$NoGeneralization`; `Some` is the qualified
    /// parent entity name.
    pub target: Option<String>,
    /// Key-presence-sensitive because runtime compilation inherits absent
    /// values from the parent.
    pub persistable: Option<bool>,
    pub system_members: SystemMembers,
    /// Lossless storage sidecar. Typed fields drive authoring and inspection;
    /// this preserves editor fields unknown to the current schema adapter.
    pub raw: Document,
}

impl Generalization {
    pub fn to_bson(&self) -> Document {
        generalization_bson(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexedSystemMember {
    CreatedDate,
    ChangedDate,
    Owner,
    ChangedBy,
}

impl IndexedSystemMember {
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::CreatedDate => "CreatedDate",
            Self::ChangedDate => "ChangedDate",
            Self::Owner => "Owner",
            Self::ChangedBy => "ChangedBy",
        }
    }

    fn from_native(value: &str) -> Option<Self> {
        match value {
            "CreatedDate" => Some(Self::CreatedDate),
            "ChangedDate" => Some(Self::ChangedDate),
            "Owner" => Some(Self::Owner),
            "ChangedBy" => Some(Self::ChangedBy),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexMemberKind {
    Attribute(String),
    System(IndexedSystemMember),
    /// Malformed or future-schema member retained losslessly. It is explicit
    /// in inspection APIs and prevents exporters from claiming a typed
    /// round-trip for a member they cannot name.
    Unresolved(String),
}

#[derive(Debug, Clone)]
pub struct IndexedAttribute {
    pub id: Option<String>,
    pub kind: IndexMemberKind,
    pub attribute_pointer: Option<String>,
    pub ascending: bool,
    pub raw: Document,
}

#[derive(Debug, Clone)]
pub struct EntityIndex {
    pub id: Option<String>,
    pub guid: Option<String>,
    pub include_offline: bool,
    pub members: Vec<IndexedAttribute>,
    pub raw: Document,
}

impl EntityIndex {
    pub fn to_bson(&self) -> Document {
        match index_bson(self) {
            mxrs_bson::Bson::Document(document) => document,
            _ => unreachable!("index_bson always returns a document"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMemberKind {
    Attribute,
    Association,
}

#[derive(Debug, Clone)]
pub struct AccessMember {
    pub id: Option<String>,
    pub name: String,
    pub reference: String,
    pub rights: String,
    pub kind: AccessMemberKind,
    pub raw: Document,
}

#[derive(Debug, Clone)]
pub struct AccessRule {
    pub id: Option<String>,
    pub roles: Vec<String>,
    pub create: bool,
    pub delete: bool,
    pub documentation: String,
    pub default_rights: String,
    pub members: Vec<AccessMember>,
    pub xpath: String,
    pub xpath_caption: Option<String>,
    pub raw: Document,
}

#[derive(Debug, Clone)]
pub struct LifecycleCallback {
    pub id: Option<String>,
    /// e.g. `"before_create"`, `"after_commit"` — `{moment}_{event}` lowercased.
    pub event: String,
    pub handler: String,
    pub pass_event_object: bool,
    pub raise_error_on_false: bool,
    /// Original editor document. The summarized fields above power the
    /// ergonomic API; compiler passes use this copy so unknown Runtime
    /// fields and nested text references survive lowering.
    pub raw: Document,
}

impl LifecycleCallback {
    pub fn to_bson(&self) -> Document {
        match lifecycle_bson(self) {
            mxrs_bson::Bson::Document(document) => document,
            _ => unreachable!("lifecycle_bson always returns a document"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub id: Option<String>,
    pub name: Option<String>,
    pub qualified_name: Option<String>,
    pub documentation: String,
    pub persistable: bool,
    pub location: Location,
    pub data_storage_guid: Option<String>,
    pub image: Option<String>,
    pub export_level: String,
    pub generalization: Option<Generalization>,
    pub access_rules: Vec<AccessRule>,
    pub indexes: Vec<EntityIndex>,
    pub system_members: SystemMembers,
    pub lifecycle: Vec<LifecycleCallback>,
    pub validation_rules: Vec<Document>,
    /// Raw `source`/`Source` sub-document (OQL view entities).
    pub source: Option<Document>,
    pub oql_query: Option<String>,
    /// The BSON `$Type` this entity was decoded from (e.g.
    /// `DomainModels$Entity`, `DomainModels$ViewEntity`).
    pub native_type: Option<String>,
    pub attributes: Vec<Attribute>,
}

impl Entity {
    pub fn from_bson(doc: &Document) -> Self {
        let native_type = get_str_any(doc, &["$Type"]);
        let source = get_doc_any(doc, &["source", "Source"]);
        let oql_query = get_str_any(doc, &["oqlQuery", "OqlQuery", "OQLQuery"])
            .or_else(|| source.as_ref().and_then(|s| get_str_any(s, &["Oql"])));

        let generalization = get_doc_any(
            doc,
            &[
                "generalization",
                "Generalization",
                "maybeGeneralization",
                "MaybeGeneralization",
            ],
        )
        .map(parse_generalization);
        let persistable = generalization
            .as_ref()
            .and_then(|g| g.persistable)
            .unwrap_or(true);
        let system_members = generalization
            .as_ref()
            .map(|g| g.system_members)
            .unwrap_or_default();

        let mut attributes: Vec<Attribute> = docs_any(doc, &["attributes", "Attributes"])
            .iter()
            .map(Attribute::from_bson)
            .collect();
        let validation_rules = docs_any(doc, &["validationRules", "ValidationRules"]);
        apply_validation_rules(&mut attributes, &validation_rules);

        let qualified_name = get_str_any(doc, &["$QualifiedName"]);
        let raw_indexes = docs_any(doc, &["indexes", "Indexes"]);
        let indexes = parse_indexes(raw_indexes, &attributes, qualified_name.as_deref());

        let access_rules = docs_any(doc, &["accessRules", "AccessRules"])
            .iter()
            .map(parse_access_rule)
            .collect();
        let lifecycle = docs_any(doc, &["eventHandlers", "EventHandlers", "Events"])
            .iter()
            .map(parse_lifecycle)
            .collect();

        Entity {
            id: get_id_any(doc, &["$ID"]),
            name: get_str_any(doc, &["name", "Name"]),
            qualified_name,
            documentation: get_str_any(doc, &["documentation", "Documentation"])
                .unwrap_or_default(),
            persistable,
            location: parse_location(get_any(doc, &["location", "Location"])),
            data_storage_guid: get_id_any(doc, &["dataStorageGuid", "DataStorageGuid", "GUID"]),
            image: get_str_any(doc, &["image", "Image"]),
            export_level: get_str_any(doc, &["exportLevel", "ExportLevel"])
                .unwrap_or_else(|| "Hidden".into()),
            generalization,
            access_rules,
            indexes,
            system_members,
            lifecycle,
            validation_rules,
            source,
            oql_query,
            native_type,
            attributes,
        }
    }

    pub fn generalization_target(&self) -> Option<String> {
        self.generalization.as_ref()?.target.clone()
    }

    pub fn oql_view(&self) -> bool {
        let source_type = self
            .source
            .as_ref()
            .and_then(|s| get_str_any(s, &["$Type"]))
            .unwrap_or_default();
        let native_type = self.native_type.as_deref().unwrap_or("");
        native_type.to_lowercase().contains("viewentity")
            || source_type.to_lowercase().contains("oqlviewentitysource")
            || self.oql_query.as_deref().is_some_and(|q| !q.is_empty())
    }

    pub fn oql_source_document(&self) -> Option<String> {
        let s = self.source.as_ref()?;
        get_str_any(s, &["SourceDocument", "sourceDocument"])
    }

    pub fn to_bson(&self) -> Document {
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        doc! {
            "$ID": id,
            "$Type": self.native_type.clone().unwrap_or_else(|| "DomainModels$EntityImpl".to_string()),
            "$QualifiedName": self.qualified_name.clone(),
            "name": self.name.clone(),
            "documentation": self.documentation.clone(),
            "dataStorageGuid": self.data_storage_guid.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            "location": doc! { "x": self.location.x, "y": self.location.y },
            "generalization": self.serialize_generalization(),
            "attributes": mxrs_bson::build_array(self.attributes.iter().map(|a| mxrs_bson::Bson::Document(a.to_bson())).collect(), 3),
            "validationRules": mxrs_bson::build_array(self.validation_rules.iter().cloned().map(mxrs_bson::Bson::Document).collect(), 3),
            "eventHandlers": mxrs_bson::build_array(self.lifecycle.iter().map(lifecycle_bson).collect(), 3),
            "indexes": mxrs_bson::build_array(self.indexes.iter().map(index_bson).collect(), 3),
            "accessRules": mxrs_bson::build_array(self.access_rules.iter().map(access_rule_bson).collect(), 3),
            "source": self.source.clone().map(mxrs_bson::Bson::Document).unwrap_or(mxrs_bson::Bson::Null),
            "exportLevel": self.export_level.clone(),
            "image": self.image.clone().unwrap_or_default(),
            "imageData": "",
        }
    }

    fn serialize_generalization(&self) -> Document {
        self.generalization
            .as_ref()
            .map(generalization_bson)
            .unwrap_or_else(|| {
                doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "DomainModels$NoGeneralization",
                    "persistable": self.persistable,
                    "hasChangedDate": false,
                    "hasCreatedDate": false,
                    "hasOwner": false,
                    "hasChangedBy": false,
                }
            })
    }
}

fn parse_generalization(g: Document) -> Generalization {
    let reference =
        get_str_any(&g, &["generalization", "Generalization"]).filter(|value| !value.is_empty());
    let type_name = get_str_any(&g, &["$Type"]).unwrap_or_else(|| {
        if reference.is_some() {
            "DomainModels$Generalization".to_string()
        } else {
            "DomainModels$NoGeneralization".to_string()
        }
    });
    let target = (!type_name.ends_with("NoGeneralization"))
        .then_some(reference)
        .flatten();
    let system_members = SystemMembers {
        owner: get_bool_any(&g, &["hasOwner", "HasOwnerAttr"]).unwrap_or(false),
        created_date: get_bool_any(&g, &["hasCreatedDate", "HasCreatedDateAttr"]).unwrap_or(false),
        changed_date: get_bool_any(&g, &["hasChangedDate", "HasChangedDateAttr"]).unwrap_or(false),
        changed_by: get_bool_any(&g, &["hasChangedBy", "HasChangedByAttr"]).unwrap_or(false),
    };
    Generalization {
        id: get_id_any(&g, &["$ID"]),
        native_type: type_name,
        target,
        persistable: get_bool_any(&g, &["persistable", "Persistable"]),
        system_members,
        raw: g,
    }
}

fn parse_location(value: Option<&mxrs_bson::Bson>) -> Location {
    match value {
        Some(mxrs_bson::Bson::String(s)) => {
            let mut parts = s.splitn(2, ';');
            let x = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
            let y = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
            Location { x, y }
        }
        Some(mxrs_bson::Bson::Document(d)) => Location {
            x: get_i32_any(d, &["x"]).unwrap_or(0),
            y: get_i32_any(d, &["y"]).unwrap_or(0),
        },
        _ => Location { x: 0, y: 0 },
    }
}

fn parse_indexes(
    indexes: Vec<Document>,
    attributes: &[Attribute],
    qualified_name: Option<&str>,
) -> Vec<EntityIndex> {
    let names_by_id: std::collections::HashMap<&str, &str> = attributes
        .iter()
        .filter_map(|a| Some((a.id.as_deref()?, a.name.as_deref()?)))
        .collect();

    indexes
        .into_iter()
        .map(|index| parse_index(index, &names_by_id, qualified_name))
        .collect()
}

fn parse_index(
    index: Document,
    names_by_id: &std::collections::HashMap<&str, &str>,
    qualified_name: Option<&str>,
) -> EntityIndex {
    let members = docs_any(&index, &["Attributes"])
        .into_iter()
        .map(|member| {
            let member_type = get_str_any(&member, &["Type"]).unwrap_or_else(|| "Normal".into());
            let pointer = get_id_any(&member, &["AttributePointer"]);
            let kind = if member_type == "Normal" {
                get_str_any(&member, &["Attribute", "attribute"])
                    .or_else(|| {
                        let name = names_by_id.get(pointer.as_deref()?)?;
                        Some(match qualified_name {
                            Some(qualified) => format!("{qualified}.{name}"),
                            None => (*name).to_string(),
                        })
                    })
                    .map(IndexMemberKind::Attribute)
                    .unwrap_or_else(|| {
                        IndexMemberKind::Unresolved(
                            pointer
                                .clone()
                                .unwrap_or_else(|| "missing attribute pointer".into()),
                        )
                    })
            } else {
                IndexedSystemMember::from_native(&member_type)
                    .map(IndexMemberKind::System)
                    .unwrap_or_else(|| IndexMemberKind::Unresolved(member_type))
            };
            IndexedAttribute {
                id: get_id_any(&member, &["$ID"]),
                kind,
                attribute_pointer: pointer,
                ascending: get_bool_any(&member, &["Ascending"]).unwrap_or(true),
                raw: member,
            }
        })
        .collect();
    EntityIndex {
        id: get_id_any(&index, &["$ID"]),
        guid: get_id_any(&index, &["GUID"]),
        include_offline: get_bool_any(&index, &["IncludeInOffline"]).unwrap_or(false),
        members,
        raw: index,
    }
}

fn generalization_bson(generalization: &Generalization) -> Document {
    if !matches!(
        generalization.native_type.as_str(),
        "DomainModels$Generalization" | "DomainModels$NoGeneralization" | ""
    ) {
        return generalization.raw.clone();
    }
    let mut document = generalization.raw.clone();
    document.insert(
        "$ID",
        generalization
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
    );
    match &generalization.target {
        Some(target) => {
            document.insert("$Type", "DomainModels$Generalization");
            insert_native(&mut document, "generalization", "Generalization", target);
        }
        None => {
            document.insert("$Type", "DomainModels$NoGeneralization");
            document.remove("generalization");
            document.remove("Generalization");
            insert_native(
                &mut document,
                "persistable",
                "Persistable",
                generalization.persistable.unwrap_or(true),
            );
            insert_native(
                &mut document,
                "hasOwner",
                "HasOwnerAttr",
                generalization.system_members.owner,
            );
            insert_native(
                &mut document,
                "hasCreatedDate",
                "HasCreatedDateAttr",
                generalization.system_members.created_date,
            );
            insert_native(
                &mut document,
                "hasChangedDate",
                "HasChangedDateAttr",
                generalization.system_members.changed_date,
            );
            insert_native(
                &mut document,
                "hasChangedBy",
                "HasChangedByAttr",
                generalization.system_members.changed_by,
            );
        }
    }
    document
}

fn insert_native(
    document: &mut Document,
    lower: &'static str,
    upper: &'static str,
    value: impl Into<mxrs_bson::Bson>,
) {
    let key = if document.contains_key(lower) {
        lower
    } else {
        upper
    };
    document.insert(key, value);
}

fn index_bson(index: &EntityIndex) -> mxrs_bson::Bson {
    if (!index.raw.is_empty()
        && index.raw.get_str("$Type").ok() != Some("DomainModels$EntityIndex"))
        || index
            .members
            .iter()
            .any(|member| matches!(member.kind, IndexMemberKind::Unresolved(_)))
    {
        return mxrs_bson::Bson::Document(index.raw.clone());
    }
    let mut document = index.raw.clone();
    document.insert(
        "$ID",
        index
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
    );
    document.insert("$Type", "DomainModels$EntityIndex");
    document.insert("IncludeInOffline", index.include_offline);
    if let Some(guid) = &index.guid {
        document.insert("GUID", guid.clone());
    }
    let marker = match document.get("Attributes") {
        Some(mxrs_bson::Bson::Array(values)) => mxrs_bson::parse_array(Some(values)).marker,
        _ => 3,
    };
    document.insert(
        "Attributes",
        mxrs_bson::build_array(
            index.members.iter().map(index_member_bson).collect(),
            marker,
        ),
    );
    mxrs_bson::Bson::Document(document)
}

fn index_member_bson(member: &IndexedAttribute) -> mxrs_bson::Bson {
    let mut document = member.raw.clone();
    document.insert(
        "$ID",
        member
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
    );
    document.insert("$Type", "DomainModels$IndexedAttribute");
    document.insert("Ascending", member.ascending);
    match &member.kind {
        IndexMemberKind::Attribute(_) => {
            document.insert("Type", "Normal");
            if let Some(pointer) = &member.attribute_pointer {
                document.insert("AttributePointer", pointer.clone());
            }
        }
        IndexMemberKind::System(system) => {
            document.insert("Type", system.native_name());
            document.insert(
                "AttributePointer",
                member
                    .attribute_pointer
                    .clone()
                    .unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".to_string()),
            );
        }
        IndexMemberKind::Unresolved(_) => {}
    }
    if !document.contains_key("AssociationPointer") {
        document.insert("AssociationPointer", "00000000-0000-0000-0000-000000000000");
    }
    mxrs_bson::Bson::Document(document)
}

fn parse_access_rule(doc: &Document) -> AccessRule {
    let roles = items_any(doc, &["ModuleRoles", "AllowedModuleRoles"])
        .into_iter()
        .filter_map(|b| match b {
            mxrs_bson::Bson::String(s) => Some(s),
            _ => None,
        })
        .collect();
    let members = docs_any(doc, &["MemberAccesses"])
        .iter()
        .map(|m| {
            let association_ref = get_str_any(m, &["Association"]).unwrap_or_default();
            let (attr_ref, kind) = if association_ref.is_empty() {
                (
                    get_str_any(m, &["Attribute"]).unwrap_or_default(),
                    AccessMemberKind::Attribute,
                )
            } else {
                (association_ref, AccessMemberKind::Association)
            };
            let name = attr_ref
                .rsplit(['/', '.'])
                .next()
                .unwrap_or(&attr_ref)
                .to_string();
            AccessMember {
                id: get_id_any(m, &["$ID"]),
                name,
                reference: attr_ref,
                rights: get_str_any(m, &["AccessRights"]).unwrap_or_else(|| "None".into()),
                kind,
                raw: m.clone(),
            }
        })
        .collect();

    AccessRule {
        id: get_id_any(doc, &["$ID"]),
        roles,
        create: get_bool_any(doc, &["AllowCreate"]).unwrap_or(false),
        delete: get_bool_any(doc, &["AllowDelete"]).unwrap_or(false),
        documentation: get_str_any(doc, &["Documentation"]).unwrap_or_default(),
        default_rights: get_str_any(doc, &["DefaultMemberAccessRights"])
            .unwrap_or_else(|| "None".into()),
        members,
        xpath: get_str_any(doc, &["XPathConstraint"]).unwrap_or_default(),
        xpath_caption: get_str_any(doc, &["XPathConstraintCaption"]),
        raw: doc.clone(),
    }
}

fn parse_lifecycle(doc: &Document) -> LifecycleCallback {
    let moment = get_str_any(doc, &["Moment"])
        .unwrap_or_default()
        .to_lowercase();
    let event = get_str_any(doc, &["Event"])
        .unwrap_or_default()
        .to_lowercase();
    let combined = [moment, event]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    LifecycleCallback {
        id: get_id_any(doc, &["$ID"]),
        event: combined,
        handler: get_str_any(doc, &["Microflow"]).unwrap_or_default(),
        pass_event_object: get_bool_any(doc, &["PassEventObject"]).unwrap_or(true),
        raise_error_on_false: get_bool_any(doc, &["RaiseErrorOnFalse"]).unwrap_or(false),
        raw: doc.clone(),
    }
}

fn lifecycle_bson(callback: &LifecycleCallback) -> mxrs_bson::Bson {
    if !callback.raw.is_empty()
        && callback.raw.get_str("$Type").ok() != Some("DomainModels$EventHandler")
    {
        return mxrs_bson::Bson::Document(callback.raw.clone());
    }
    let mut document = callback.raw.clone();
    let mut parts = callback.event.splitn(2, '_');
    let moment = parts.next().unwrap_or_default();
    let event = parts.next().unwrap_or_default();
    let capitalize = |s: &str| {
        let mut c = s.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    };
    document.insert(
        "$ID",
        callback
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
    );
    document.insert("$Type", "DomainModels$EventHandler");
    document.insert("Event", capitalize(event));
    document.insert("Moment", capitalize(moment));
    document.insert("Microflow", callback.handler.clone());
    document.insert("PassEventObject", callback.pass_event_object);
    document.insert("RaiseErrorOnFalse", callback.raise_error_on_false);
    mxrs_bson::Bson::Document(document)
}

/// Serializes one access rule back into its native `DomainModels$AccessRule`
/// shape, the inverse of [`parse_access_rule`] and the counterpart of mxrb's
/// `Writer#access_rule_doc`.
///
/// Until this existed `Entity::to_bson` wrote an empty `accessRules` array
/// unconditionally, so an entity read with rules and written back lost its
/// security. Roles are emitted under `AllowedModuleRoles` with array marker 1
/// (by-name references), which is the spelling both mxrb's writer and the
/// runtime's reader use.
pub fn access_rule_bson(rule: &AccessRule) -> mxrs_bson::Bson {
    if !rule.raw.is_empty() && rule.raw.get_str("$Type").ok() != Some("DomainModels$AccessRule") {
        return mxrs_bson::Bson::Document(rule.raw.clone());
    }
    let members = rule
        .members
        .iter()
        .map(|member| {
            let association = member.kind == AccessMemberKind::Association;
            if !member.raw.is_empty()
                && member.raw.get_str("$Type").ok() != Some("DomainModels$MemberAccess")
            {
                return mxrs_bson::Bson::Document(member.raw.clone());
            }
            let mut document = member.raw.clone();
            document.insert(
                "$ID",
                member
                    .id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            );
            document.insert("$Type", "DomainModels$MemberAccess");
            document.insert(
                "Association",
                if association {
                    member.reference.clone()
                } else {
                    String::new()
                },
            );
            document.insert(
                "Attribute",
                if association {
                    String::new()
                } else {
                    member.reference.clone()
                },
            );
            document.insert("AccessRights", member.rights.clone());
            mxrs_bson::Bson::Document(document)
        })
        .collect();
    let mut document = rule.raw.clone();
    document.insert(
        "$ID",
        rule.id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
    );
    document.insert("$Type", "DomainModels$AccessRule");
    document.insert("Documentation", rule.documentation.clone());
    document.insert(
        "AllowedModuleRoles",
        mxrs_bson::build_array(
            rule.roles
                .iter()
                .cloned()
                .map(mxrs_bson::Bson::String)
                .collect(),
            1,
        ),
    );
    document.insert("AllowCreate", rule.create);
    document.insert("AllowDelete", rule.delete);
    document.insert("DefaultMemberAccessRights", rule.default_rights.clone());
    document.insert("MemberAccesses", mxrs_bson::build_array(members, 3));
    document.insert("XPathConstraint", rule.xpath.clone());
    // Absent and empty are different to Studio Pro, so the key is written
    // only when the rule actually carries a caption — as mxrb does.
    if let Some(caption) = &rule.xpath_caption {
        document.insert("XPathConstraintCaption", caption.clone());
    }
    mxrs_bson::Bson::Document(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr_doc(name: &str, ty: &str) -> Document {
        doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "name": name,
            "type": { "$Type": ty },
        }
    }

    #[test]
    fn decodes_persistable_from_generalization() {
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "name": "Order",
            "generalization": { "persistable": false },
        };
        let e = Entity::from_bson(&d);
        assert!(!e.persistable);
    }

    #[test]
    fn defaults_persistable_true_without_generalization() {
        let d = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "name": "Order" };
        let e = Entity::from_bson(&d);
        assert!(e.persistable);
    }

    #[test]
    fn parses_location_string_form() {
        let d = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "location": "100;50" };
        let e = Entity::from_bson(&d);
        assert_eq!(e.location, Location { x: 100, y: 50 });
    }

    #[test]
    fn validation_rules_mark_required_and_unique() {
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "attributes": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(attr_doc("Number", "DomainModels$StringAttributeType"))], 3),
            "validationRules": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                "Attribute": "Sales.Order.Number",
                "RuleInfo": { "$Type": "DomainModels$RequiredRuleInfo" },
            })], 3),
        };
        let e = Entity::from_bson(&d);
        assert!(e.attributes[0].required);
        assert!(!e.attributes[0].unique);
    }

    #[test]
    fn oql_view_detected_from_native_type() {
        let d =
            doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "DomainModels$ViewEntity" };
        let e = Entity::from_bson(&d);
        assert!(e.oql_view());
    }

    #[test]
    fn retains_image_guid_and_raw_lifecycle_documents_for_compiler_passes() {
        let guid = uuid::Uuid::new_v4().to_string();
        let event = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "DomainModels$EventHandler",
            "Moment": "Before",
            "Event": "Create",
            "Microflow": "Sales.ACT_BeforeCreate",
            "OpaqueRuntimeField": true,
        };
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "Name": "Order",
            "GUID": guid.clone(),
            "Image": "Sales.OrderIcon",
            "Events": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(event)], 3),
        };
        let entity = Entity::from_bson(&d);
        assert_eq!(entity.data_storage_guid.as_deref(), Some(guid.as_str()));
        assert_eq!(entity.image.as_deref(), Some("Sales.OrderIcon"));
        assert_eq!(entity.lifecycle.len(), 1);
        assert!(
            entity.lifecycle[0]
                .raw
                .get_bool("OpaqueRuntimeField")
                .unwrap()
        );
    }

    #[test]
    fn access_rules_preserve_future_rule_and_member_fields() {
        let entity = Entity::from_bson(&doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "accessRules": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "DomainModels$AccessRule",
                "AllowedModuleRoles": mxrs_bson::build_array(vec![mxrs_bson::Bson::String("Sales.User".into())], 1),
                "DefaultMemberAccessRights": "None",
                "FutureRuleField": "kept",
                "MemberAccesses": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "DomainModels$MemberAccess",
                    "Attribute": "Sales.Order.Number",
                    "Association": "",
                    "AccessRights": "ReadOnly",
                    "FutureMemberField": "kept",
                })], 3),
            })], 3),
        });
        let serialized = entity.to_bson();
        let rules =
            mxrs_bson::parse_array(serialized.get_array("accessRules").ok().map(Vec::as_slice));
        let rule = rules.items[0].as_document().unwrap();
        assert_eq!(rule.get_str("FutureRuleField").unwrap(), "kept");
        let members =
            mxrs_bson::parse_array(rule.get_array("MemberAccesses").ok().map(Vec::as_slice));
        assert_eq!(
            members.items[0]
                .as_document()
                .unwrap()
                .get_str("FutureMemberField")
                .unwrap(),
            "kept"
        );
    }

    #[test]
    fn round_trips_name_and_documentation_through_to_bson() {
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "name": "Order",
            "documentation": "An order",
        };
        let e = Entity::from_bson(&d);
        let out = e.to_bson();
        assert_eq!(out.get_str("name").unwrap(), "Order");
        assert_eq!(out.get_str("documentation").unwrap(), "An order");
        assert_eq!(out.get_str("$Type").unwrap(), "DomainModels$EntityImpl");
    }

    #[test]
    fn indexes_decode_to_typed_members_without_polluting_the_native_document() {
        let attribute_id = uuid::Uuid::new_v4().to_string();
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$QualifiedName": "Sales.Order",
            "name": "Order",
            "attributes": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                "$ID": attribute_id.clone(),
                "name": "Number",
                "type": { "$Type": "DomainModels$StringAttributeType" },
            })], 3),
            "indexes": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "DomainModels$EntityIndex",
                "GUID": uuid::Uuid::new_v4().to_string(),
                "IncludeInOffline": true,
                "Attributes": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "DomainModels$IndexedAttribute",
                    "Type": "Normal",
                    "AttributePointer": attribute_id,
                    "Ascending": false,
                })], 7),
            })], 5),
        };
        let entity = Entity::from_bson(&d);
        assert_eq!(entity.indexes.len(), 1);
        assert!(entity.indexes[0].include_offline);
        assert!(matches!(
            &entity.indexes[0].members[0].kind,
            IndexMemberKind::Attribute(name) if name == "Sales.Order.Number"
        ));
        assert!(!entity.indexes[0].members[0].ascending);
        assert!(!entity.indexes[0].members[0].raw.contains_key("Attribute"));

        let output = entity.to_bson();
        let indexes = mxrs_bson::parse_array(output.get_array("indexes").ok().map(Vec::as_slice));
        assert_eq!(indexes.marker, 3);
        let index = indexes.items[0].as_document().unwrap();
        let members = mxrs_bson::parse_array(index.get_array("Attributes").ok().map(Vec::as_slice));
        assert_eq!(members.marker, 7);
        assert!(
            !members.items[0]
                .as_document()
                .unwrap()
                .contains_key("Attribute")
        );
    }

    #[test]
    fn typed_root_updates_the_existing_lowercase_generalization_shape() {
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "name": "Order",
            "generalization": {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "DomainModels$NoGeneralization",
                "persistable": true,
                "hasOwner": false,
            },
        };
        let mut entity = Entity::from_bson(&d);
        let generalization = entity.generalization.as_mut().unwrap();
        generalization.persistable = Some(false);
        generalization.system_members.owner = true;
        let output = entity.to_bson();
        let generalization = output.get_document("generalization").unwrap();
        assert!(!generalization.get_bool("persistable").unwrap());
        assert!(generalization.get_bool("hasOwner").unwrap());
        assert!(!generalization.contains_key("Persistable"));
        assert!(!generalization.contains_key("HasOwnerAttr"));
    }
}
