//! Microflows, nanoflows and rules — all three share this shape in mxrb
//! (differentiated only by their Unit's `$Type`/containment at the Module
//! layer). Ports `lib/mxrb/model/microflow.rb`.
//!
//! Object/flow graph nodes are kept as raw BSON documents: mxrb's own
//! `compare.rb#flow_summary` only needs generic `$ID`/pointer/case-value
//! shape, not a typed activity catalog (that belongs to a future
//! `mxrs-compiler-flow`, per the phased plan) — see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory.

use mxrs_bson::Document;

use crate::support::{docs_any, get_bool_any, get_doc_any, get_id_any, get_str_any};

#[derive(Debug, Clone)]
pub struct Microflow {
    pub id: Option<String>,
    pub name: Option<String>,
    pub documentation: String,
    pub return_variable_name: String,
    pub allow_concurrent_execution: bool,
    pub apply_entity_access: bool,
    pub mark_as_used: bool,
    pub excluded: bool,
    pub export_level: String,
    pub allowed_module_roles: Vec<String>,
    pub parameters: Vec<Document>,
    /// Raw `MicroflowReturnType`/`ReturnType` sub-document.
    pub return_type_document: Option<Document>,
    pub return_type: Option<String>,
    pub objects: Vec<Document>,
    pub flows: Vec<Document>,
}

impl Microflow {
    pub fn from_bson(doc: &Document) -> Self {
        let return_doc = get_doc_any(doc, &["MicroflowReturnType", "ReturnType"]);
        let return_type = return_doc
            .as_ref()
            .and_then(|r| get_str_any(r, &["$Type", "Type"]));

        let obj_col = get_doc_any(doc, &["ObjectCollection"]).unwrap_or_default();
        let objects = docs_any(&obj_col, &["Objects", "objects"]);
        let flows = {
            let from_doc = docs_any(doc, &["Flows", "flows"]);
            if from_doc.is_empty() {
                docs_any(&obj_col, &["Flows", "flows"])
            } else {
                from_doc
            }
        };

        Microflow {
            id: get_id_any(doc, &["$ID"]),
            name: get_str_any(doc, &["Name", "name"]),
            documentation: get_str_any(doc, &["Documentation", "documentation"])
                .unwrap_or_default(),
            return_variable_name: get_str_any(doc, &["ReturnVariableName"])
                .unwrap_or_else(|| "ReturnValue".into()),
            allow_concurrent_execution: get_bool_any(doc, &["AllowConcurrentExecution"])
                .unwrap_or(true),
            apply_entity_access: get_bool_any(doc, &["ApplyEntityAccess"]).unwrap_or(false),
            mark_as_used: get_bool_any(doc, &["MarkAsUsed"]).unwrap_or(false),
            excluded: get_bool_any(doc, &["Excluded"]).unwrap_or(false),
            export_level: get_str_any(doc, &["ExportLevel"]).unwrap_or_else(|| "Hidden".into()),
            allowed_module_roles: string_items(doc, &["AllowedModuleRoles"]),
            parameters: extract_parameters(doc, &obj_col),
            return_type_document: return_doc,
            return_type,
            objects,
            flows,
        }
    }

    pub fn to_bson(&self) -> Document {
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        mxrs_bson::doc! {
            "$ID": id,
            "$Type": "Microflows$Microflow",
            "Name": self.name.clone(),
            "Documentation": self.documentation.clone(),
            "ReturnVariableName": self.return_variable_name.clone(),
            "AllowConcurrentExecution": self.allow_concurrent_execution,
            "ApplyEntityAccess": self.apply_entity_access,
            "MarkAsUsed": self.mark_as_used,
            "Excluded": self.excluded,
            "AllowedModuleRoles": mxrs_bson::build_array(
                self.allowed_module_roles.iter().cloned().map(mxrs_bson::Bson::String).collect(), 1,
            ),
            "MicroflowParameterCollection": mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Microflows$MicroflowParameterCollection",
                "Parameters": mxrs_bson::build_array(self.parameters.iter().cloned().map(mxrs_bson::Bson::Document).collect(), 3),
            },
            "MicroflowReturnType": self.return_type_document.clone().unwrap_or_else(default_void_return),
            "ObjectCollection": mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Microflows$MicroflowObjectCollection",
                "Objects": mxrs_bson::build_array(self.objects.iter().cloned().map(mxrs_bson::Bson::Document).collect(), 3),
            },
            "Flows": mxrs_bson::build_array(self.flows.iter().cloned().map(mxrs_bson::Bson::Document).collect(), 3),
        }
    }
}

fn string_items(doc: &Document, keys: &[&str]) -> Vec<String> {
    crate::support::items_any(doc, keys)
        .into_iter()
        .filter_map(|b| match b {
            mxrs_bson::Bson::String(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn default_void_return() -> Document {
    mxrs_bson::doc! {
        "$ID": uuid::Uuid::new_v4().to_string(),
        "$Type": "Microflows$MicroflowReturnType",
        "Type": mxrs_bson::Bson::Null,
        "AllowedModuleRoles": mxrs_bson::build_array(vec![], 1),
    }
}

/// A microflow's parameters normally live in `MicroflowParameterCollection`,
/// but mxrb's own writer historically embedded them inside
/// `ObjectCollection.Objects` instead — mirrors `extract_parameters`'s
/// fallback.
fn extract_parameters(doc: &Document, obj_col: &Document) -> Vec<Document> {
    if let Some(pc) = get_doc_any(doc, &["MicroflowParameterCollection", "Parameters"]) {
        let list = docs_any(&pc, &["Parameters"]);
        if !list.is_empty() {
            return list;
        }
    }
    docs_any(obj_col, &["Objects", "objects"])
        .into_iter()
        .filter(|o| get_str_any(o, &["$Type"]).as_deref() == Some("Microflows$MicroflowParameter"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    #[test]
    fn decodes_name_and_defaults() {
        let d = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "Name": "ACT_CreateOrder" };
        let mf = Microflow::from_bson(&d);
        assert_eq!(mf.name.as_deref(), Some("ACT_CreateOrder"));
        assert!(mf.allow_concurrent_execution);
        assert_eq!(mf.return_variable_name, "ReturnValue");
    }

    #[test]
    fn falls_back_to_object_collection_for_parameters() {
        let param = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$MicroflowParameter", "Name": "Order" };
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "ObjectCollection": { "Objects": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(param)], 3) },
        };
        let mf = Microflow::from_bson(&d);
        assert_eq!(mf.parameters.len(), 1);
    }

    #[test]
    fn round_trips_name_through_to_bson() {
        let d = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "Name": "ACT_X" };
        let mf = Microflow::from_bson(&d);
        assert_eq!(mf.to_bson().get_str("Name").unwrap(), "ACT_X");
    }
}
