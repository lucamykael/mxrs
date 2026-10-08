//! Parameter lowering shared by fresh writes and synchronization.
//!
//! A flow's parameters are objects of its graph, as Studio Pro stores them:
//! what names one — a REST operation's parameter, a call's argument —
//! finds it there, as `Module.Flow.Parameter`.
use std::collections::HashMap;

use mxrs_bson::{Bson, Document, build_array, doc, extract_id, parse_array};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::MicroflowDecl;
use mxrs_model::Microflow;

use crate::{Result, WriterError, flow_compiler};

/// The parameter objects `flow` declares, each keeping the identity, the
/// place in the drawing and whatever else the one of its name in `previous`
/// has.
pub(crate) fn lower(
    flow: &MicroflowDecl,
    flow_id: &str,
    previous: Option<&Document>,
    identity: ProjectIdentity,
) -> Result<Vec<Bson>> {
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
    for (index, parameter) in flow.parameters.iter().enumerate() {
        let key = format!("{flow_id}.{}", parameter.name);
        let old = by_name.get(parameter.name.as_str()).copied();
        let id = old
            .and_then(|doc| doc.get("$ID"))
            .and_then(extract_id)
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::FlowParameter, &key));
        // A new parameter sits above the flow, beside the ones before it.
        let mut document = old.cloned().unwrap_or_else(|| {
            doc! {
                "$ID": id.clone(), "$Type": "Microflows$MicroflowParameter",
                "HasVariableNameBeenChanged": false,
                "RelativeMiddlePoint": format!("{};25", 50 + 80 * index), "Size": "30;30",
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
    Ok(parameters)
}

/// Places `parameters` among the objects of `flow`'s graph, first, instead
/// of any it had — there or in the parameter collection an earlier mxrs
/// wrote, which Studio Pro does not read.
pub(crate) fn place(flow: &mut Document, parameters: Vec<Bson>) {
    flow.remove("MicroflowParameterCollection");
    flow.remove("Parameters");
    let collection = flow
        .get_document_mut("ObjectCollection")
        .expect("a flow document holds its graph's objects");
    let parsed = parse_array(collection.get_array("Objects").ok().map(Vec::as_slice));
    let objects = parameters
        .into_iter()
        .chain(parsed.items.into_iter().filter(|object| {
            object.as_document().is_none_or(|object| {
                object.get_str("$Type").ok() != Some("Microflows$MicroflowParameter")
            })
        }))
        .collect();
    collection.insert("Objects", build_array(objects, parsed.marker));
}

/// Whether `flow` keeps its parameters in a collection rather than in its
/// graph.
pub(crate) fn collected(flow: &Document) -> bool {
    flow.contains_key("MicroflowParameterCollection") || flow.get_document("Parameters").is_ok()
}
