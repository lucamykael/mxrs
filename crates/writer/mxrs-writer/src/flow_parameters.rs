//! Parameter lowering shared by fresh writes and synchronization.
use std::collections::HashMap;

use mxrs_bson::{Bson, Document, build_array, doc, extract_id};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::MicroflowDecl;
use mxrs_model::Microflow;

use crate::{Result, WriterError, flow_compiler};

pub(crate) fn lower(
    flow: &MicroflowDecl,
    flow_id: &str,
    previous: Option<&Document>,
    identity: ProjectIdentity,
) -> Result<Document> {
    let previous_parameters = previous
        .map(Microflow::from_bson)
        .map(|flow| flow.parameters)
        .unwrap_or_default();
    let mut by_name = HashMap::new();
    for parameter in &previous_parameters {
        let name = parameter
            .get_str("Name")
            .map_err(|_| WriterError::InvalidFlowParameter {
                flow: flow.name.clone(),
                parameter: String::new(),
                reason: "native parameter has no name".into(),
            })?;
        if by_name.insert(name, parameter).is_some() {
            return Err(WriterError::InvalidFlowParameter {
                flow: flow.name.clone(),
                parameter: name.into(),
                reason: "duplicate native parameter".into(),
            });
        }
    }
    let mut parameters = Vec::new();
    for parameter in &flow.parameters {
        let key = format!("{flow_id}.{}", parameter.name);
        let old = by_name.get(parameter.name.as_str()).copied();
        let id = old
            .and_then(|doc| doc.get("$ID"))
            .and_then(extract_id)
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::FlowParameter, &key));
        let mut document = old.cloned().unwrap_or_else(|| {
            doc! {
                "$ID": id.clone(), "$Type": "Microflows$MicroflowParameter",
                "RelativeMiddlePoint": "100;100", "Size": "30;30",
            }
        });
        document.insert("$ID", id);
        document.insert("$Type", "Microflows$MicroflowParameter");
        document.insert("Name", parameter.name.clone());
        document.insert("Documentation", parameter.documentation.clone());
        document.insert(
            "DefaultValue",
            parameter.default_value.clone().unwrap_or_default(),
        );
        document.insert("IsRequired", parameter.required);
        let fresh_type = flow_compiler::return_type_document(Some(&parameter.value_type))
            .expect("parameter has type");
        let old_type = old.and_then(|doc| doc.get_document("VariableType").ok());
        let type_id = old_type
            .and_then(|doc| doc.get("$ID"))
            .and_then(extract_id)
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::FlowParameterType, &key));
        let mut value_type = old_type.cloned().unwrap_or_default();
        value_type.insert("$ID", type_id);
        value_type.insert("$Type", fresh_type.get_str("$Type").expect("type tag"));
        value_type.remove("Entity");
        if let Some(entity) = fresh_type.get("Entity") {
            value_type.insert("Entity", entity.clone());
        }
        document.insert("VariableType", value_type);
        parameters.push(Bson::Document(document));
    }
    let old_collection = previous.and_then(|doc| {
        doc.get_document("MicroflowParameterCollection")
            .ok()
            .or_else(|| doc.get_document("Parameters").ok())
    });
    let mut collection = old_collection.cloned().unwrap_or_default();
    let id = old_collection
        .and_then(|doc| doc.get("$ID"))
        .and_then(extract_id)
        .unwrap_or_else(|| identity.artifact_id(ArtifactKind::FlowParameterCollection, flow_id));
    collection.insert("$ID", id);
    collection.insert("$Type", "Microflows$MicroflowParameterCollection");
    collection.insert("Parameters", build_array(parameters, 3));
    Ok(collection)
}
