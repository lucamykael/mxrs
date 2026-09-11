//! Runtime page metadata compilation: identity, parameters, title, URL
//! placeholders, popup settings, and resolved user roles.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document, doc};
use mxrs_compiler_support::{
    array_docs, build_array, get, get_doc_any, get_str_any, runtime_data_type, stable_dedup,
    string_list, to_s,
};
use mxrs_model::Project;

use crate::CompilerError;

pub struct PageDocumentCompiler {
    role_map: HashMap<String, Vec<String>>,
}

impl PageDocumentCompiler {
    pub fn new(project: &Project) -> Result<Self, CompilerError> {
        Ok(Self {
            role_map: mxrs_compiler_support::project_role_map(project)?,
        })
    }

    pub fn without_security() -> Self {
        Self {
            role_map: HashMap::new(),
        }
    }

    pub fn compile(&self, source: &Document, module_name: &str) -> Result<Document, CompilerError> {
        let type_name = get_str_any(source, &["$Type"]).unwrap_or_default();
        if type_name != "Forms$Page" {
            return Err(CompilerError::UnsupportedPageRoot { type_name });
        }
        let parameters = array_docs(source, &["Parameters"])
            .iter()
            .map(compile_parameter)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(Bson::Document)
            .collect();
        let name = to_s(&get(source, "Name"));
        let allowed_roles = stable_dedup(
            string_list(source, &["AllowedModuleRoles"])
                .into_iter()
                .flat_map(|role| self.role_map.get(&role).cloned().unwrap_or_default()),
        )
        .into_iter()
        .map(Bson::String)
        .collect();
        let url = to_s(&get(source, "Url"));
        let segments = url
            .split('/')
            .filter_map(|segment| {
                segment
                    .strip_prefix('{')
                    .and_then(|value| value.strip_suffix('}'))
                    .map(|value| Bson::String(value.to_string()))
            })
            .collect();
        Ok(doc! {
            "$ID": get(source, "$ID"),
            "$Type": get(source, "$Type"),
            "Parameters": build_array(parameters),
            "Title": text_reference(get_doc_any(source, &["Title"])),
            "UrlSegments": build_array(segments),
            "Name": name.clone(),
            "QualifiedName": format!("{module_name}.{name}"),
            "ModelerAllowedUserRoles": build_array(allowed_roles),
            "PopupWidth": to_i(source.get("PopupWidth")),
            "PopupHeight": to_i(source.get("PopupHeight")),
            "PopupResizable": source.get_bool("PopupResizable").unwrap_or(false),
            "Url": url,
        })
    }
}

fn compile_parameter(source: &Document) -> Result<Document, CompilerError> {
    let parameter_type = get_doc_any(source, &["ParameterType"]);
    let runtime_type = runtime_data_type(parameter_type.as_ref())
        .map_err(|type_name| CompilerError::UnsupportedDataType { type_name })?;
    Ok(doc! {
        "$ID": get(source, "$ID"),
        "$Type": get(source, "$Type"),
        "Name": get(source, "Name"),
        "ParameterTypeRuntime": runtime_type,
        "IsRequired": source.get_bool("IsRequired").unwrap_or(false),
    })
}

fn text_reference(source: Option<Document>) -> Bson {
    match source {
        Some(source) => Bson::Document(doc! {
            "$ID": get(&source, "$ID"),
            "$Type": get(&source, "$Type"),
        }),
        None => Bson::Null,
    }
}

fn to_i(value: Option<&Bson>) -> i32 {
    match value {
        Some(Bson::Int32(value)) => *value,
        Some(Bson::Int64(value)) => i32::try_from(*value).unwrap_or_default(),
        Some(Bson::Double(value)) => *value as i32,
        Some(Bson::String(value)) => value.parse().unwrap_or_default(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_parameters_url_segments_popup_fields_and_roles() {
        let compiler = PageDocumentCompiler {
            role_map: HashMap::from([(
                "Sales.User".to_string(),
                vec!["Operator".to_string(), "Auditor".to_string()],
            )]),
        };
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Forms$Page",
            "Name": "EditOrder",
            "Url": "orders/{Order}/edit/{Mode}",
            "PopupWidth": 640,
            "PopupHeight": "480",
            "PopupResizable": true,
            "AllowedModuleRoles": mxrs_bson::build_array(vec![Bson::String("Sales.User".into())], 3),
            "Parameters": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "$ID": "22222222-2222-4222-8222-222222222222",
                "$Type": "Forms$PageParameter",
                "Name": "Order",
                "ParameterType": {
                    "$Type": "DataTypes$ObjectType",
                    "Entity": "Sales.Order",
                },
            })], 3),
        };
        let page = compiler.compile(&source, "Sales").unwrap();
        assert_eq!(page.get_str("QualifiedName").unwrap(), "Sales.EditOrder");
        assert_eq!(page.get_i32("PopupWidth").unwrap(), 640);
        assert_eq!(page.get_i32("PopupHeight").unwrap(), 480);
        let segments =
            mxrs_bson::parse_array(page.get_array("UrlSegments").ok().map(Vec::as_slice));
        assert_eq!(
            segments.items,
            vec![Bson::String("Order".into()), Bson::String("Mode".into())]
        );
        let parameters =
            mxrs_bson::parse_array(page.get_array("Parameters").ok().map(Vec::as_slice));
        assert_eq!(
            parameters.items[0]
                .as_document()
                .unwrap()
                .get_str("ParameterTypeRuntime")
                .unwrap(),
            "Sales.Order"
        );
    }

    #[test]
    fn rejects_non_page_roots() {
        let error = PageDocumentCompiler::without_security()
            .compile(&doc! { "$Type": "Forms$Snippet" }, "Sales")
            .unwrap_err();
        assert!(
            matches!(error, CompilerError::UnsupportedPageRoot { type_name } if type_name == "Forms$Snippet")
        );
    }
}
