//! Entity associations. Embedded inside `DomainModel` BSON — not separate
//! Unit rows. Ports `lib/mxrb/model/association.rb` from mxrb.
//!
//! CRITICAL GOTCHA (from mxrb's own reverse engineering, preserved here):
//! `ParentID` is the FROM entity (the one carrying the foreign key) —
//! counter-intuitive naming. `ChildID` is the TO entity (the one referenced).
//! Member access should be added only to the FROM entity's pointer, never to
//! `ChildID`.

use mxrs_bson::{Document, doc};

use crate::support::{get_any, get_doc_any, get_id_any, get_str_any};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationType {
    Reference,
    ReferenceSet,
}

impl AssociationType {
    fn as_str(self) -> &'static str {
        match self {
            AssociationType::Reference => "Reference",
            AssociationType::ReferenceSet => "ReferenceSet",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "ReferenceSet" => AssociationType::ReferenceSet,
            _ => AssociationType::Reference,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Default,
    Both,
}

impl Owner {
    fn as_str(self) -> &'static str {
        match self {
            Owner::Default => "Default",
            Owner::Both => "Both",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "Both" => Owner::Both,
            _ => Owner::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageFormat {
    Column,
    Table,
}

impl StorageFormat {
    fn as_str(self) -> &'static str {
        match self {
            StorageFormat::Column => "Column",
            StorageFormat::Table => "Table",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "Table" => StorageFormat::Table,
            _ => StorageFormat::Column,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteAction {
    NoAction,
    Delete,
    DeleteMeIfNotUsed,
}

impl DeleteAction {
    fn from_str(s: &str) -> Self {
        match s {
            "Delete" => DeleteAction::Delete,
            "DeleteMeIfNotUsed" => DeleteAction::DeleteMeIfNotUsed,
            _ => DeleteAction::NoAction,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Association {
    pub id: Option<String>,
    pub name: Option<String>,
    pub documentation: String,
    /// FROM entity id (BSON `ParentID`/`ParentPointer`) — carries the FK.
    pub from_entity_id: Option<String>,
    /// TO entity id or qualified name (BSON `ChildID`/`ChildPointer`).
    pub to_entity_id: Option<String>,
    pub association_type: AssociationType,
    pub owner: Owner,
    pub storage_format: StorageFormat,
    /// Opaque editor fields copied verbatim into Runtime shape by the domain
    /// compiler. Their representation belongs to the versioned schema, not
    /// to association behavior.
    pub source: Option<mxrs_bson::Bson>,
    pub guid: Option<mxrs_bson::Bson>,
    /// Raw `DeleteBehavior` sub-document, kept for lossless inspection.
    pub delete_behavior: Option<Document>,
    pub export_level: String,
}

impl Association {
    pub fn from_bson(doc: &Document) -> Self {
        let child = get_any(doc, &["Child", "ChildPointer", "ChildID", "childId"]);
        let to_entity_id = match child {
            Some(mxrs_bson::Bson::String(s)) if s.contains('.') => Some(s.clone()),
            Some(v) => mxrs_bson::extract_id(v),
            None => None,
        };

        Association {
            id: get_id_any(doc, &["$ID"]),
            name: get_str_any(doc, &["Name", "name"]),
            documentation: get_str_any(doc, &["Documentation", "documentation"])
                .unwrap_or_default(),
            from_entity_id: get_id_any(doc, &["ParentPointer", "ParentID", "parentId"]),
            to_entity_id,
            association_type: AssociationType::from_str(
                &get_str_any(doc, &["Type", "type"]).unwrap_or_else(|| "Reference".into()),
            ),
            owner: Owner::from_str(
                &get_str_any(doc, &["Owner", "owner"]).unwrap_or_else(|| "Default".into()),
            ),
            storage_format: StorageFormat::from_str(
                &get_str_any(doc, &["StorageFormat"]).unwrap_or_else(|| "Column".into()),
            ),
            source: get_any(doc, &["Source", "source"]).cloned(),
            guid: get_any(doc, &["GUID", "guid", "DataStorageGuid", "dataStorageGuid"]).cloned(),
            delete_behavior: get_doc_any(doc, &["DeleteBehavior", "deleteBehavior"]),
            export_level: get_str_any(doc, &["ExportLevel"]).unwrap_or_else(|| "Hidden".into()),
        }
    }

    /// `to_entity_id` doubles as a dotted `"Module.Entity"` qualified name
    /// for a cross-module target (see `from_bson`'s `Child`/`ChildPointer`
    /// fallback) — detected here the same way, since `DomainModel` routes
    /// this association into its `crossAssociations` array either way and
    /// the BSON shape must match: `$Type: DomainModels$CrossAssociation`,
    /// a `Child` qualified-name field instead of `ChildID`, no editor
    /// connection-point fields (mirrors mxrb's `cross_association_doc`).
    pub fn is_cross_module(&self) -> bool {
        self.to_entity_id
            .as_deref()
            .is_some_and(|s| s.contains('.'))
    }

    pub fn to_bson(&self) -> Document {
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if self.is_cross_module() {
            let mut out = doc! {
                "$ID": id,
                "$Type": "DomainModels$CrossAssociation",
                "Name": self.name.clone(),
                "Documentation": self.documentation.clone(),
                "ParentID": self.from_entity_id.clone(),
                "Child": self.to_entity_id.clone(),
                "Type": self.association_type.as_str(),
                "Owner": self.owner.as_str(),
                "StorageFormat": self.storage_format.as_str(),
                "DeleteBehavior": self.delete_behavior.clone().unwrap_or_else(default_delete_behavior),
                "ExportLevel": self.export_level.clone(),
            };
            if let Some(source) = &self.source {
                out.insert("Source", source.clone());
            }
            if let Some(guid) = &self.guid {
                out.insert("GUID", guid.clone());
            }
            return out;
        }
        let mut out = doc! {
            "$ID": id,
            "$Type": "DomainModels$Association",
            "Name": self.name.clone(),
            "Documentation": self.documentation.clone(),
            "ParentID": self.from_entity_id.clone(),
            "ChildID": self.to_entity_id.clone(),
            "Type": self.association_type.as_str(),
            "Owner": self.owner.as_str(),
            "StorageFormat": self.storage_format.as_str(),
            "DeleteBehavior": self.delete_behavior.clone().unwrap_or_else(default_delete_behavior),
            "ExportLevel": self.export_level.clone(),
        };
        if let Some(source) = &self.source {
            out.insert("Source", source.clone());
        }
        if let Some(guid) = &self.guid {
            out.insert("GUID", guid.clone());
        }
        out
    }

    pub fn parent_delete_behavior(&self) -> DeleteAction {
        self.behavior_value("parentDeleteBehavior", "ParentDeleteBehavior")
    }

    pub fn child_delete_behavior(&self) -> DeleteAction {
        self.behavior_value("childDeleteBehavior", "ChildDeleteBehavior")
    }

    fn behavior_value(&self, lower: &str, upper: &str) -> DeleteAction {
        let Some(behavior) = &self.delete_behavior else {
            return DeleteAction::NoAction;
        };
        DeleteAction::from_str(
            &get_str_any(behavior, &[lower, upper]).unwrap_or_else(|| "NoAction".into()),
        )
    }
}

fn default_delete_behavior() -> Document {
    doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "DomainModels$DeleteBehavior",
        "parentDeleteBehavior": "NoAction",
        "childDeleteBehavior": "NoAction",
        "parentErrorMessage": mxrs_bson::Bson::Null,
        "childErrorMessage": mxrs_bson::Bson::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_parent_and_child_pointers_with_gotcha_preserved() {
        let from_id = uuid::Uuid::new_v4().to_string();
        let to_id = uuid::Uuid::new_v4().to_string();
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "Name": "Order_Customer",
            "ParentID": from_id.clone(),
            "ChildID": to_id.clone(),
            "Type": "Reference",
            "Owner": "Default",
        };
        let assoc = Association::from_bson(&d);
        assert_eq!(assoc.from_entity_id, Some(from_id));
        assert_eq!(assoc.to_entity_id, Some(to_id));
        assert_eq!(assoc.association_type, AssociationType::Reference);
    }

    #[test]
    fn round_trips_through_to_bson() {
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "Name": "Order_Customer",
            "ParentID": uuid::Uuid::new_v4().to_string(),
            "ChildID": uuid::Uuid::new_v4().to_string(),
            "Type": "ReferenceSet",
            "Owner": "Both",
            "StorageFormat": "Table",
        };
        let assoc = Association::from_bson(&d);
        let out = assoc.to_bson();
        assert_eq!(out.get_str("Type").unwrap(), "ReferenceSet");
        assert_eq!(out.get_str("Owner").unwrap(), "Both");
        assert_eq!(out.get_str("StorageFormat").unwrap(), "Table");
    }

    #[test]
    fn default_delete_behavior_is_no_action() {
        let d = doc! { "$ID": uuid::Uuid::new_v4().to_string() };
        let assoc = Association::from_bson(&d);
        assert_eq!(assoc.parent_delete_behavior(), DeleteAction::NoAction);
        assert_eq!(assoc.child_delete_behavior(), DeleteAction::NoAction);
    }

    #[test]
    fn behavior_value_reads_declared_delete_behavior() {
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "DeleteBehavior": { "parentDeleteBehavior": "Delete", "childDeleteBehavior": "DeleteMeIfNotUsed" },
        };
        let assoc = Association::from_bson(&d);
        assert_eq!(assoc.parent_delete_behavior(), DeleteAction::Delete);
        assert_eq!(
            assoc.child_delete_behavior(),
            DeleteAction::DeleteMeIfNotUsed
        );
    }

    #[test]
    fn to_entity_id_prefers_dotted_qualified_name_over_id_extraction() {
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "ChildID": "Sales.Customer",
        };
        let assoc = Association::from_bson(&d);
        assert_eq!(assoc.to_entity_id.as_deref(), Some("Sales.Customer"));
    }

    #[test]
    fn retains_opaque_source_and_guid_fields() {
        let guid = uuid::Uuid::new_v4().to_string();
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "Name": "Order_Customer",
            "Source": "Sales.Order",
            "GUID": guid.clone(),
        };
        let association = Association::from_bson(&d);
        assert_eq!(
            association.source,
            Some(mxrs_bson::Bson::String("Sales.Order".into()))
        );
        assert_eq!(association.guid, Some(mxrs_bson::Bson::String(guid)));
        let serialized = association.to_bson();
        assert_eq!(serialized.get_str("Source").unwrap(), "Sales.Order");
        assert!(serialized.contains_key("GUID"));
    }

    #[test]
    fn cross_module_association_serializes_as_cross_association_with_child_field() {
        let assoc = Association {
            id: None,
            name: Some("Order_Customer".into()),
            documentation: String::new(),
            from_entity_id: Some(uuid::Uuid::new_v4().to_string()),
            to_entity_id: Some("Sales.Customer".into()),
            association_type: AssociationType::Reference,
            owner: Owner::Default,
            storage_format: StorageFormat::Column,
            source: None,
            guid: None,
            delete_behavior: None,
            export_level: "Hidden".into(),
        };
        assert!(assoc.is_cross_module());
        let out = assoc.to_bson();
        assert_eq!(
            out.get_str("$Type").unwrap(),
            "DomainModels$CrossAssociation"
        );
        assert_eq!(out.get_str("Child").unwrap(), "Sales.Customer");
        assert!(!out.contains_key("ChildID"));

        let round_tripped = Association::from_bson(&out);
        assert_eq!(
            round_tripped.to_entity_id.as_deref(),
            Some("Sales.Customer")
        );
        assert_eq!(round_tripped.name.as_deref(), Some("Order_Customer"));
    }

    #[test]
    fn same_module_association_is_not_cross_module() {
        let assoc = Association {
            id: None,
            name: Some("Order_Line".into()),
            documentation: String::new(),
            from_entity_id: Some(uuid::Uuid::new_v4().to_string()),
            to_entity_id: Some(uuid::Uuid::new_v4().to_string()),
            association_type: AssociationType::Reference,
            owner: Owner::Default,
            storage_format: StorageFormat::Column,
            source: None,
            guid: None,
            delete_behavior: None,
            export_level: "Hidden".into(),
        };
        assert!(!assoc.is_cross_module());
        assert_eq!(
            assoc.to_bson().get_str("$Type").unwrap(),
            "DomainModels$Association"
        );
    }
}
