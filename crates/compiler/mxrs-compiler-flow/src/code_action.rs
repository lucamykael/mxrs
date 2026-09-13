//! Compiles `JavaActions$JavaAction`/`JavaScriptActions$JavaScriptAction`
//! editor documents into Runtime shape — ports `lib/mxrb/compiler/
//! code_action_document_compiler.rb` (64 lines). Depends only on
//! [`crate::types::code_action_type`], no project-wide state.

use mxrs_bson::{Document, doc};

use crate::CompilerError;
use crate::support::{array_docs, get_str_any};
use crate::types::code_action_type;

pub struct CodeActionCompiler;

impl CodeActionCompiler {
    /// `module_name` is `None` when the action lives outside a module —
    /// mirrors `qualified_name`'s own `raise CompilationError` guard
    /// (`code_action_document_compiler.rb:36-38`), a code action must
    /// belong to a module to be addressable by qualified name.
    pub fn compile(
        source: &Document,
        module_name: Option<&str>,
    ) -> Result<Document, CompilerError> {
        match get_str_any(source, &["$Type"]).as_deref() {
            Some("JavaActions$JavaAction") => Self::java_action(source, module_name),
            Some("JavaScriptActions$JavaScriptAction") => {
                Self::javascript_action(source, module_name)
            }
            other => Err(CompilerError::UnsupportedCodeAction {
                type_name: other.unwrap_or_default().to_string(),
            }),
        }
    }

    fn java_action(
        source: &Document,
        module_name: Option<&str>,
    ) -> Result<Document, CompilerError> {
        let name = get_str_any(source, &["Name"]).unwrap_or_default();
        let parameters: Result<Vec<Document>, CompilerError> = array_docs(source, &["Parameters"])
            .iter()
            .map(Self::java_parameter)
            .collect();
        let return_type =
            code_action_type(crate::support::get_doc_any(source, &["JavaReturnType"]).as_ref())?;
        Ok(doc! {
            "$ID": get_str_any(source, &["$ID"]).unwrap_or_default(),
            "$Type": "JavaActions$JavaAction",
            "Parameters": parameters?,
            "Name": name.clone(),
            "QualifiedName": qualified_name(module_name, &name)?,
            "ReturnType": return_type,
        })
    }

    fn java_parameter(source: &Document) -> Result<Document, CompilerError> {
        let name = get_str_any(source, &["Name"]).unwrap_or_default();
        let parameter_type_doc = crate::support::get_doc_any(source, &["ParameterType"]);
        // `JavaActions$MicroflowJavaActionParameterType` always compiles to
        // `String` — a microflow-typed Java parameter is passed by
        // qualified name, not by its own type tree
        // (`code_action_document_compiler.rb:56-60`).
        let is_microflow_parameter = parameter_type_doc
            .as_ref()
            .and_then(|d| get_str_any(d, &["$Type"]))
            .as_deref()
            == Some("JavaActions$MicroflowJavaActionParameterType");
        let parameter_type = if is_microflow_parameter {
            "String".to_string()
        } else {
            let inner = parameter_type_doc
                .as_ref()
                .and_then(|d| crate::support::get_doc_any(d, &["Type"]));
            code_action_type(inner.as_ref())?
        };
        Ok(doc! {
            "$ID": get_str_any(source, &["$ID"]).unwrap_or_default(),
            "$Type": get_str_any(source, &["$Type"]).unwrap_or_default(),
            "Name": name,
            "Type": parameter_type,
        })
    }

    /// No `ReturnType` field at all (JavaScript actions have no declared
    /// return type) and parameters carry no `Type` field either (untyped)
    /// — both deliberate omissions, not truncated ports
    /// (`code_action_document_compiler.rb:29-34`).
    fn javascript_action(
        source: &Document,
        module_name: Option<&str>,
    ) -> Result<Document, CompilerError> {
        let name = get_str_any(source, &["Name"]).unwrap_or_default();
        let parameters: Vec<Document> = array_docs(source, &["Parameters"])
            .iter()
            .map(|p| {
                doc! {
                    "$ID": get_str_any(p, &["$ID"]).unwrap_or_default(),
                    "$Type": get_str_any(p, &["$Type"]).unwrap_or_default(),
                    "Name": get_str_any(p, &["Name"]).unwrap_or_default(),
                }
            })
            .collect();
        Ok(doc! {
            "$ID": get_str_any(source, &["$ID"]).unwrap_or_default(),
            "$Type": "JavaScriptActions$JavaScriptAction",
            "Parameters": parameters,
            "Name": name.clone(),
            "QualifiedName": qualified_name(module_name, &name)?,
        })
    }
}

fn qualified_name(module_name: Option<&str>, name: &str) -> Result<String, CompilerError> {
    let module_name = module_name.ok_or_else(|| CompilerError::CodeActionOutsideModule {
        name: name.to_string(),
    })?;
    Ok(format!("{module_name}.{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_a_java_action_with_parameters_and_return_type() {
        let source = doc! {
            "$ID": "11111111-1111-1111-1111-111111111111",
            "$Type": "JavaActions$JavaAction",
            "Name": "DoWork",
            "JavaReturnType": { "$Type": "JavaActions$StringType" },
            "Parameters": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                "$ID": "22222222-2222-2222-2222-222222222222",
                "$Type": "JavaActions$JavaActionParameter",
                "Name": "input",
                "ParameterType": { "Type": { "$Type": "JavaActions$IntegerType" } },
            })], 3),
        };
        let compiled = CodeActionCompiler::compile(&source, Some("Sales")).unwrap();
        assert_eq!(compiled.get_str("QualifiedName").unwrap(), "Sales.DoWork");
        assert_eq!(compiled.get_str("ReturnType").unwrap(), "String");
        let params = compiled.get_array("Parameters").unwrap();
        let mxrs_bson::Bson::Document(param) = &params[0] else {
            panic!("expected a document")
        };
        assert_eq!(param.get_str("Type").unwrap(), "Integer");
    }

    #[test]
    fn a_microflow_typed_java_parameter_is_always_string() {
        let source = doc! {
            "$ID": "11111111-1111-1111-1111-111111111111",
            "$Type": "JavaActions$JavaAction",
            "Name": "CallMicroflow",
            "Parameters": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                "$ID": "22222222-2222-2222-2222-222222222222",
                "$Type": "JavaActions$JavaActionParameter",
                "Name": "flow",
                "ParameterType": { "$Type": "JavaActions$MicroflowJavaActionParameterType" },
            })], 3),
        };
        let compiled = CodeActionCompiler::compile(&source, Some("Sales")).unwrap();
        let params = compiled.get_array("Parameters").unwrap();
        let mxrs_bson::Bson::Document(param) = &params[0] else {
            panic!("expected a document")
        };
        assert_eq!(param.get_str("Type").unwrap(), "String");
    }

    #[test]
    fn compiles_a_javascript_action_with_no_return_type_and_untyped_parameters() {
        let source = doc! {
            "$ID": "11111111-1111-1111-1111-111111111111",
            "$Type": "JavaScriptActions$JavaScriptAction",
            "Name": "Notify",
            "Parameters": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                "$ID": "22222222-2222-2222-2222-222222222222",
                "$Type": "JavaScriptActions$JavaScriptActionParameter",
                "Name": "message",
            })], 3),
        };
        let compiled = CodeActionCompiler::compile(&source, Some("Sales")).unwrap();
        assert_eq!(compiled.get_str("QualifiedName").unwrap(), "Sales.Notify");
        assert!(compiled.get("ReturnType").is_none());
        let params = compiled.get_array("Parameters").unwrap();
        let mxrs_bson::Bson::Document(param) = &params[0] else {
            panic!("expected a document")
        };
        assert!(param.get("Type").is_none());
    }

    #[test]
    fn an_unsupported_code_action_type_is_a_loud_error() {
        let source = doc! { "$ID": "x", "$Type": "Bogus$Action", "Name": "X" };
        let err = CodeActionCompiler::compile(&source, Some("Sales")).unwrap_err();
        assert!(matches!(err, CompilerError::UnsupportedCodeAction { .. }));
    }

    #[test]
    fn a_code_action_outside_a_module_is_a_loud_error() {
        let source = doc! {
            "$ID": "x", "$Type": "JavaScriptActions$JavaScriptAction", "Name": "Notify",
        };
        let err = CodeActionCompiler::compile(&source, None).unwrap_err();
        assert!(matches!(err, CompilerError::CodeActionOutsideModule { name } if name == "Notify"));
    }
}
