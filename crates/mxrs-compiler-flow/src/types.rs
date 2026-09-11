//! Editor `DataTypes$*`/`CodeActions$*`/`JavaActions$*` type documents ->
//! Runtime type-identifier strings. Ports `lib/mxrb/compiler/
//! runtime_data_types.rb` (31 lines) and `lib/mxrb/compiler/
//! code_action_type_compiler.rb` (32 lines) — kept in one small module
//! since both are pure `&Document -> Result<String, CompilerError>`
//! functions with no other state, unlike every other file in this crate.

use mxrs_bson::Document;
use mxrs_compiler_support::runtime_data_type;

use crate::CompilerError;
use crate::support::get_str_any;

/// `document` is `None` for a void return (`runtime_data_types.rb:16-17`).
pub fn data_type(document: Option<&Document>) -> Result<String, CompilerError> {
    runtime_data_type(document)
        .map_err(|type_name| CompilerError::UnsupportedDataType { type_name })
}

/// `true` iff `value` is a document whose `$Type` starts with
/// `"DataTypes$"` — mirrors `RuntimeDataTypes#data_type_document?`, used
/// by the node compiler to decide whether a field's raw value needs
/// [`data_type`] conversion instead of a plain recursive compile.
pub fn is_data_type_document(value: &Document) -> bool {
    get_str_any(value, &["$Type"]).is_some_and(|type_name| type_name.starts_with("DataTypes$"))
}

/// `source` is `None` for a void return. Normalizes `JavaActions$X` to
/// `CodeActions$X` before lookup, since Java action parameter/return types
/// and CodeActions type trees share one scalar table
/// (`code_action_type_compiler.rb:15-19`).
///
/// **Intentional lossy narrowing**, reproduced verbatim from the Ruby, not
/// "obviously correct": `Long` collapses into `Integer` and `Float`
/// collapses into `Decimal` — Runtime's code-action type system doesn't
/// distinguish the wider variants, so this loses precision information a
/// naive reader might expect to survive. Flagging it here rather than
/// letting it look like an oversight.
pub fn code_action_type(source: Option<&Document>) -> Result<String, CompilerError> {
    let Some(source) = source else {
        return Ok("Void".to_string());
    };
    let raw_type = get_str_any(source, &["$Type"]).unwrap_or_default();
    let type_name = raw_type
        .strip_prefix("JavaActions$")
        .map(|rest| format!("CodeActions${rest}"))
        .unwrap_or(raw_type);
    Ok(match type_name.as_str() {
        "CodeActions$VoidType" => "Void".to_string(),
        "CodeActions$StringType" => "String".to_string(),
        "CodeActions$BooleanType" => "Boolean".to_string(),
        "CodeActions$IntegerType" => "Integer".to_string(),
        "CodeActions$LongType" => "Integer".to_string(),
        "CodeActions$DecimalType" => "Decimal".to_string(),
        "CodeActions$FloatType" => "Decimal".to_string(),
        "CodeActions$DateTimeType" => "DateTime".to_string(),
        "CodeActions$ConcreteEntityType" => get_str_any(source, &["Entity"]).unwrap_or_default(),
        "CodeActions$EnumerationType" => {
            format!(
                "#{}",
                get_str_any(source, &["Enumeration"]).unwrap_or_default()
            )
        }
        // Generic entity types aren't resolved — narrow gap, matches the Ruby.
        "CodeActions$ParameterizedEntityType" => "Unknown".to_string(),
        "CodeActions$ListType" => {
            let parameter = crate::support::get_doc_any(source, &["Parameter"]);
            format!("[{}]", code_action_type(parameter.as_ref())?)
        }
        other => {
            return Err(CompilerError::UnsupportedCodeActionType {
                type_name: other.to_string(),
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    #[test]
    fn scalar_data_types_map_to_runtime_names() {
        for (mendix_type, expected) in [
            ("DataTypes$VoidType", "Void"),
            ("DataTypes$StringType", "String"),
            ("DataTypes$BooleanType", "Boolean"),
            ("DataTypes$IntegerType", "Integer"),
            ("DataTypes$DecimalType", "Decimal"),
            ("DataTypes$DateTimeType", "DateTime"),
            ("DataTypes$UnknownType", "Unknown"),
        ] {
            let d = doc! { "$Type": mendix_type };
            assert_eq!(data_type(Some(&d)).unwrap(), expected);
        }
    }

    #[test]
    fn a_missing_document_is_void() {
        assert_eq!(data_type(None).unwrap(), "Void");
    }

    #[test]
    fn object_and_list_types_carry_the_entity_name() {
        let object = doc! { "$Type": "DataTypes$ObjectType", "Entity": "Sales.Order" };
        assert_eq!(data_type(Some(&object)).unwrap(), "Sales.Order");
        let list = doc! { "$Type": "DataTypes$ListType", "Entity": "Sales.Order" };
        assert_eq!(data_type(Some(&list)).unwrap(), "[Sales.Order]");
    }

    #[test]
    fn enumeration_type_is_hash_prefixed() {
        let d = doc! { "$Type": "DataTypes$EnumerationType", "Enumeration": "Sales.Status" };
        assert_eq!(data_type(Some(&d)).unwrap(), "#Sales.Status");
    }

    #[test]
    fn an_unsupported_data_type_is_a_loud_error() {
        let d = doc! { "$Type": "DataTypes$Bogus" };
        let err = data_type(Some(&d)).unwrap_err();
        assert!(
            matches!(err, CompilerError::UnsupportedDataType { type_name } if type_name == "DataTypes$Bogus")
        );
    }

    #[test]
    fn java_action_types_share_the_code_actions_scalar_table() {
        let d = doc! { "$Type": "JavaActions$StringType" };
        assert_eq!(code_action_type(Some(&d)).unwrap(), "String");
    }

    #[test]
    fn long_and_float_narrow_into_integer_and_decimal() {
        let long = doc! { "$Type": "CodeActions$LongType" };
        assert_eq!(code_action_type(Some(&long)).unwrap(), "Integer");
        let float = doc! { "$Type": "CodeActions$FloatType" };
        assert_eq!(code_action_type(Some(&float)).unwrap(), "Decimal");
    }

    #[test]
    fn parameterized_entity_type_is_unknown() {
        let d = doc! { "$Type": "CodeActions$ParameterizedEntityType" };
        assert_eq!(code_action_type(Some(&d)).unwrap(), "Unknown");
    }

    #[test]
    fn list_type_recurses() {
        let d = doc! {
            "$Type": "CodeActions$ListType",
            "Parameter": { "$Type": "CodeActions$ConcreteEntityType", "Entity": "Sales.Order" },
        };
        assert_eq!(code_action_type(Some(&d)).unwrap(), "[Sales.Order]");
    }

    #[test]
    fn a_missing_code_action_source_is_void() {
        assert_eq!(code_action_type(None).unwrap(), "Void");
    }

    #[test]
    fn an_unsupported_code_action_type_is_a_loud_error() {
        let d = doc! { "$Type": "CodeActions$Bogus" };
        let err = code_action_type(Some(&d)).unwrap_err();
        assert!(
            matches!(err, CompilerError::UnsupportedCodeActionType { type_name } if type_name == "CodeActions$Bogus")
        );
    }
}
