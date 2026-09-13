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
    /// Raw `generalization`/`Generalization` sub-document.
    pub generalization: Option<Document>,
    pub access_rules: Vec<AccessRule>,
    /// Raw index docs, post-processed so member `Attribute` refs carry the
    /// resolved qualified attribute name — mirrors `normalize_indexes`.
    /// mxrb itself never round-trips indexes back into `to_bson`, so this
    /// crate doesn't either (informational only).
    pub indexes: Vec<Document>,
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
        );
        let persistable = generalization
            .as_ref()
            .and_then(|g| get_bool_any(g, &["persistable", "Persistable"]))
            .unwrap_or(true);
        let system_members = parse_system_members(generalization.as_ref());

        let mut attributes: Vec<Attribute> = docs_any(doc, &["attributes", "Attributes"])
            .iter()
            .map(Attribute::from_bson)
            .collect();
        let validation_rules = docs_any(doc, &["validationRules", "ValidationRules"]);
        apply_validation_rules(&mut attributes, &validation_rules);

        let qualified_name = get_str_any(doc, &["$QualifiedName"]);
        let raw_indexes = docs_any(doc, &["indexes", "Indexes"]);
        let indexes = normalize_indexes(raw_indexes, &attributes, qualified_name.as_deref());

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
        let g = self.generalization.as_ref()?;
        get_str_any(g, &["generalization", "Generalization"])
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
            "$Type": "DomainModels$EntityImpl",
            "$QualifiedName": self.qualified_name.clone(),
            "name": self.name.clone(),
            "documentation": self.documentation.clone(),
            "dataStorageGuid": self.data_storage_guid.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            "location": doc! { "x": self.location.x, "y": self.location.y },
            "generalization": self.serialize_generalization(),
            "attributes": mxrs_bson::build_array(self.attributes.iter().map(|a| mxrs_bson::Bson::Document(a.to_bson())).collect(), 3),
            "validationRules": mxrs_bson::build_array(self.validation_rules.iter().cloned().map(mxrs_bson::Bson::Document).collect(), 3),
            "eventHandlers": mxrs_bson::build_array(self.lifecycle.iter().map(lifecycle_bson).collect(), 3),
            "indexes": mxrs_bson::build_array(vec![], 3),
            "accessRules": mxrs_bson::build_array(vec![], 3),
            "source": mxrs_bson::Bson::Null,
            "exportLevel": self.export_level.clone(),
            "image": self.image.clone().unwrap_or_default(),
            "imageData": "",
        }
    }

    fn serialize_generalization(&self) -> Document {
        self.generalization.clone().unwrap_or_else(|| {
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

fn parse_system_members(generalization: Option<&Document>) -> SystemMembers {
    let Some(g) = generalization else {
        return SystemMembers::default();
    };
    SystemMembers {
        owner: get_bool_any(g, &["hasOwner", "HasOwnerAttr"]).unwrap_or(false),
        created_date: get_bool_any(g, &["hasCreatedDate", "HasCreatedDateAttr"]).unwrap_or(false),
        changed_date: get_bool_any(g, &["hasChangedDate", "HasChangedDateAttr"]).unwrap_or(false),
        changed_by: get_bool_any(g, &["hasChangedBy", "HasChangedByAttr"]).unwrap_or(false),
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

fn normalize_indexes(
    mut indexes: Vec<Document>,
    attributes: &[Attribute],
    qualified_name: Option<&str>,
) -> Vec<Document> {
    let names_by_id: std::collections::HashMap<&str, &str> = attributes
        .iter()
        .filter_map(|a| Some((a.id.as_deref()?, a.name.as_deref()?)))
        .collect();

    for index in &mut indexes {
        let members = items_any(index, &["Attributes"]);
        let mut updated = Vec::with_capacity(members.len());
        for member in members {
            let mxrs_bson::Bson::Document(mut member) = member else {
                updated.push(member);
                continue;
            };
            if get_any(&member, &["Attribute", "attribute"]).is_none()
                && let Some(id) = get_id_any(&member, &["AttributePointer"])
                && let Some(name) = names_by_id.get(id.as_str())
            {
                let full = match qualified_name {
                    Some(q) => format!("{q}.{name}"),
                    None => (*name).to_string(),
                };
                member.insert("Attribute", full);
            }
            updated.push(mxrs_bson::Bson::Document(member));
        }
        index.insert("Attributes", mxrs_bson::build_array(updated, 3));
    }
    indexes
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
    if !callback.raw.is_empty() {
        return mxrs_bson::Bson::Document(callback.raw.clone());
    }
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
    mxrs_bson::Bson::Document(doc! {
        "$ID": callback.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        "$Type": "DomainModels$EventHandler",
        "Event": capitalize(event),
        "Moment": capitalize(moment),
        "Microflow": callback.handler.clone(),
        "PassEventObject": callback.pass_event_object,
        "RaiseErrorOnFalse": callback.raise_error_on_false,
    })
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
}
