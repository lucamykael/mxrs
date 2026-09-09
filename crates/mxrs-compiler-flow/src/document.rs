//! Compiles a microflow/nanoflow/rule's **root** document — ports
//! `lib/mxrb/compiler/microflow_document_compiler.rb` (85 lines).
//! Delegates every nested graph node to [`FlowNodeCompiler`].

use std::collections::HashMap;

use mxrs_bson::{doc, Bson, Document};
use mxrs_schema::RuntimeModelSchema;

use crate::node::{FlowNodeCompiler, VariableTypes};
use crate::support::{derived_id, get_any, get_doc_any, get_str_any, stable_dedup};
use crate::types::data_type;
use crate::CompilerError;

const ROOT_TYPES: &[&str] = &[
    "Microflows$Microflow",
    "Microflows$Nanoflow",
    "Microflows$Rule",
];

pub struct FlowDocumentCompiler<'a> {
    schema: &'a RuntimeModelSchema,
    nodes: FlowNodeCompiler<'a>,
    role_map: &'a HashMap<String, Vec<String>>,
}

impl<'a> FlowDocumentCompiler<'a> {
    pub fn new(
        schema: &'a RuntimeModelSchema,
        nodes: FlowNodeCompiler<'a>,
        role_map: &'a HashMap<String, Vec<String>>,
    ) -> Self {
        FlowDocumentCompiler {
            schema,
            nodes,
            role_map,
        }
    }

    pub fn compile(&self, source: &Document, module_name: &str) -> Result<Document, CompilerError> {
        let type_name = get_str_any(source, &["$Type"]).unwrap_or_default();
        if !ROOT_TYPES.contains(&type_name.as_str()) {
            return Err(CompilerError::UnsupportedFlowRoot { type_name });
        }
        let vars = FlowNodeCompiler::variable_types_for(source);
        let fields = self.schema.fields_for(source)?;
        let mut result = Document::new();
        for field in fields {
            result.insert(
                field.clone(),
                self.root_value(source, &field, module_name, &vars)?,
            );
        }
        Ok(result)
    }

    fn root_value(
        &self,
        source: &Document,
        field: &str,
        module_name: &str,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        match field {
            "$ID" | "$Type" | "Name" => {
                Ok(get_any(source, &[field]).cloned().unwrap_or(Bson::Null))
            }
            "ObjectCollection" => {
                let value = get_any(source, &["ObjectCollection"])
                    .cloned()
                    .unwrap_or(Bson::Null);
                self.nodes.compile(&value, vars)
            }
            "SequenceFlows" => {
                // Field rename: editor `Flows` becomes Runtime `SequenceFlows`.
                let value = get_any(source, &["Flows"]).cloned().unwrap_or(Bson::Null);
                self.nodes.compile(&value, vars)
            }
            "QualifiedName" => {
                let name = get_str_any(source, &["Name"]).unwrap_or_default();
                Ok(Bson::String(format!("{module_name}.{name}")))
            }
            "ReturnType" => {
                let return_type = get_doc_any(source, &["MicroflowReturnType"]);
                Ok(Bson::String(data_type(return_type.as_ref())?))
            }
            "ModelerAllowedUserRoles" => Ok(Bson::Array(
                self.allowed_user_roles(source)
                    .into_iter()
                    .map(Bson::String)
                    .collect(),
            )),
            "UrlSegments" => Ok(Bson::Array(vec![])),
            _ => self.generic_field(source, field, vars),
        }
    }

    fn generic_field(
        &self,
        source: &Document,
        field: &str,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        if let Some(value) = source.get(field) {
            return self.nodes.compile(value, vars);
        }
        if let Some(existing) = self.schema.counterpart(source) {
            if let Some(value) = existing.get(field) {
                return Ok(value.clone());
            }
        }
        self.root_default(source, field)
    }

    fn root_default(&self, source: &Document, field: &str) -> Result<Bson, CompilerError> {
        Ok(match field {
            // Mendix's own field-name typo ("Concurreny", not "Concurrency")
            // — reproduced verbatim, not corrected
            // (`microflow_document_compiler.rb:56-57`).
            "ConcurrenyErrorMessage" => {
                let id = get_str_any(source, &["$ID"]).unwrap_or_default();
                Bson::Document(doc! {
                    "$ID": derived_id(&id, field),
                    "$Type": "Texts$Text",
                })
            }
            "ApplyEntityAccess" | "AllowConcurrentExecution" => Bson::Boolean(false),
            "ConcurrencyErrorMicroflow" | "Url" => Bson::String(String::new()),
            "UrlSearchParameters" => Bson::Array(vec![]),
            other => {
                return Err(CompilerError::CannotDeriveRuntimeRootField {
                    type_name: get_str_any(source, &["$Type"]).unwrap_or_default(),
                    field: other.to_string(),
                })
            }
        })
    }

    fn allowed_user_roles(&self, flow: &Document) -> Vec<String> {
        let module_roles = crate::support::string_list(flow, &["AllowedModuleRoles"]);
        stable_dedup(
            module_roles
                .into_iter()
                .flat_map(|role| self.role_map.get(&role).cloned().unwrap_or_default()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsupported_root_type_is_a_loud_error() {
        let schema = RuntimeModelSchema::for_11(&[]).unwrap();
        let associations = HashMap::new();
        let role_map = HashMap::new();
        let nodes = FlowNodeCompiler::new(&schema, &associations, None);
        let compiler = FlowDocumentCompiler::new(&schema, nodes, &role_map);
        let source = doc! { "$ID": "x", "$Type": "Microflows$SomethingElse" };
        let err = compiler.compile(&source, "Sales").unwrap_err();
        assert!(matches!(err, CompilerError::UnsupportedFlowRoot { .. }));
    }

    #[test]
    fn qualified_name_and_sequence_flows_rename_are_applied() {
        let schema = RuntimeModelSchema::for_11(&[]).unwrap();
        let associations = HashMap::new();
        let role_map = HashMap::new();
        let nodes = FlowNodeCompiler::new(&schema, &associations, None);
        let compiler = FlowDocumentCompiler::new(&schema, nodes, &role_map);
        let source = doc! {
            "$ID": "11111111-1111-1111-1111-111111111111",
            "$Type": "Microflows$Microflow",
            "Name": "ACT_Do",
            "Flows": mxrs_bson::build_array(vec![], 3),
            "ObjectCollection": { "Objects": mxrs_bson::build_array(vec![], 3) },
        };
        let compiled = compiler.compile(&source, "Sales").unwrap();
        assert_eq!(compiled.get_str("QualifiedName").unwrap(), "Sales.ACT_Do");
        assert!(compiled.get_array("SequenceFlows").is_ok());
    }

    #[test]
    fn allowed_user_roles_expands_module_roles_and_dedups() {
        let schema = RuntimeModelSchema::for_11(&[]).unwrap();
        let associations = HashMap::new();
        let mut role_map = HashMap::new();
        role_map.insert(
            "Sales.Manager".to_string(),
            vec!["Administrator".to_string(), "SalesLead".to_string()],
        );
        let nodes = FlowNodeCompiler::new(&schema, &associations, None);
        let compiler = FlowDocumentCompiler::new(&schema, nodes, &role_map);
        let flow = doc! {
            "AllowedModuleRoles": mxrs_bson::build_array(
                vec![mxrs_bson::Bson::String("Sales.Manager".to_string())], 1,
            ),
        };
        let roles = compiler.allowed_user_roles(&flow);
        assert_eq!(
            roles,
            vec!["Administrator".to_string(), "SalesLead".to_string()]
        );
    }
}
