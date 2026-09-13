//! Contracts evidenced by the shipped Studio project templates and settings
//! fixtures. Unknown collection element schemas stay typed but unrestricted.
//!
//! Ports `Mxrb::Settings::ValueContracts` from
//! `lib/mxrb/settings/value_contracts.rb`.

use crate::catalog;
use crate::error::{Result, SettingsError};
use crate::value::{Collection, Value};

const BOOLEAN_FIELDS: &[&str] = &[
    "EnableDownloadResources",
    "EnableMicroflowReachabilityAnalysis",
    "EnableNewStringBehavior",
    "EnableNewWidgetGeneration",
    "EnableRspackBundler",
    "EnableWidgetBundling",
    "ObsoleteEnableUrlEncoding",
    "AllowUserMultipleSessions",
    "EnableDataStorageNewQueryHandling",
    "EnableDataStorageOptimisticLocking",
    "EnforceDataStorageUniqueness",
    "UseDatabaseForeignKeyConstraints",
    "UseDeprecatedClientForWebServiceCalls",
    "UseOQLVersion2",
    "UseSystemContextForBackgroundTasks",
    "LowerCaseMicroflowVariables",
    "IsDistributable",
    "GeneratePostfixesForParameters",
    "DatabaseUseIntegratedSecurity",
    "EmulateCloudSecurity",
    "OpenAdminPort",
    "OpenHttpPort",
    "Enabled",
    "CheckCompleteness",
    "RightToLeft",
];

const INTEGER_FIELDS: &[&str] = &[
    "BcryptCost",
    "DecimalScale",
    "DefaultTaskParallelism",
    "WorkflowEngineParallelism",
    "HttpPortNumber",
    "MaxJavaHeapSize",
    "ServerPortNumber",
];

const NULLABLE_FIELDS: &[&str] = &[
    "OpenTelemetry",
    "Tracing",
    "Logs",
    "Traces",
    "UsertaskOnStateChangeEvent",
    "WorkflowOnStateChangeEvent",
];

const COMPONENTS: &[(&str, &str)] = &[
    ("OpenTelemetry", "Settings$OpenTelemetryConfiguration"),
    ("Tracing", "Settings$TracingConfiguration"),
];

const COLLECTION_COMPONENTS: &[(&str, &str)] = &[
    ("ThemeModuleOrder", "Settings$ThemeModuleEntry"),
    ("Configurations", "Settings$ServerConfiguration"),
    (
        "ActionActivityDefaultColors",
        "Settings$ActionActivityDefaultColor",
    ),
    ("Languages", "Texts$Language"),
    ("Certificates", "Settings$Certificate"),
    ("CustomSettings", "Settings$CustomSetting"),
];

enum Expected {
    Boolean,
    Integer,
    Binary,
    Component(&'static str),
    TypedValue,
    Str,
}

impl Expected {
    fn label(&self) -> &'static str {
        match self {
            Expected::Boolean => "a boolean",
            Expected::Integer => "an integer",
            Expected::Binary => "a BinaryAsset",
            Expected::Component(t) => t,
            Expected::TypedValue => "a typed value",
            Expected::Str => "a string",
        }
    }
}

pub fn normalize(storage_type: &str, field: &str, value: Value) -> Result<Value> {
    if matches!(value, Value::Null) && NULLABLE_FIELDS.contains(&field) {
        return Ok(Value::Null);
    }
    if catalog::is_collection_field(field) {
        return collection(storage_type, field, value);
    }

    let expected = expected_type(field);
    if !is_valid(&expected, &value) {
        return Err(SettingsError::TypeMismatch {
            storage_type: storage_type.to_string(),
            field: field.to_string(),
            expected: expected.label().to_string(),
        });
    }
    Ok(value)
}

fn expected_type(field: &str) -> Expected {
    if BOOLEAN_FIELDS.contains(&field) {
        return Expected::Boolean;
    }
    if INTEGER_FIELDS.contains(&field) {
        return Expected::Integer;
    }
    if field == "Data" {
        return Expected::Binary;
    }
    if let Some((_, component_type)) = COMPONENTS.iter().find(|(f, _)| *f == field) {
        return Expected::Component(component_type);
    }
    if field == "Logs" || field == "Traces" {
        return Expected::TypedValue;
    }
    Expected::Str
}

fn is_valid(expected: &Expected, value: &Value) -> bool {
    match expected {
        Expected::Boolean => matches!(value, Value::Boolean(_)),
        Expected::Integer => matches!(value, Value::Integer(_)),
        Expected::Binary => matches!(value, Value::Binary(_)),
        Expected::Component(t) => matches!(value, Value::Node(n) if n.storage_type() == *t),
        Expected::TypedValue => typed_value(value),
        Expected::Str => matches!(value, Value::String(_)),
    }
}

fn collection(storage_type: &str, field: &str, value: Value) -> Result<Value> {
    let Value::Collection(collection) = value else {
        return Err(SettingsError::TypeMismatch {
            storage_type: storage_type.to_string(),
            field: field.to_string(),
            expected: "a Settings::Collection".to_string(),
        });
    };
    for (index, item) in collection.items.iter().enumerate() {
        if !collection_item(field, item) {
            return Err(SettingsError::InvalidCollectionItem {
                storage_type: storage_type.to_string(),
                field: field.to_string(),
                index,
            });
        }
    }
    Ok(Value::Collection(collection))
}

fn collection_item(field: &str, item: &Value) -> bool {
    if field == "Settings" {
        return matches!(item, Value::Node(n) if catalog::is_part_type(n.storage_type()));
    }
    if let Some((_, component_type)) = COLLECTION_COMPONENTS.iter().find(|(f, _)| *f == field) {
        return is_valid(&Expected::Component(component_type), item);
    }
    if field == "Exclusions" {
        return matches!(item, Value::String(_));
    }
    typed_value(item)
}

fn typed_value(value: &Value) -> bool {
    match value {
        Value::Node(_)
        | Value::Binary(_)
        | Value::String(_)
        | Value::Integer(_)
        | Value::Float(_)
        | Value::Time(_)
        | Value::Boolean(_)
        | Value::Null => true,
        Value::Collection(Collection { items, .. }) => items.iter().all(typed_value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Node;

    #[test]
    fn nullable_field_accepts_null() {
        assert_eq!(
            normalize("Settings$ServerConfiguration", "Tracing", Value::Null).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn non_nullable_field_rejects_null_unless_typed_value() {
        assert!(normalize("Settings$ServerConfiguration", "Name", Value::Null).is_err());
        // Logs/Traces are :typed_value AND nullable, and NilClass is a
        // recognized typed_value — both paths accept Null.
        assert_eq!(
            normalize("Settings$OpenTelemetryConfiguration", "Logs", Value::Null).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn boolean_field_rejects_non_boolean() {
        assert!(
            normalize(
                "Forms$WebUIProjectSettingsPart",
                "EnableRspackBundler",
                Value::Integer(1)
            )
            .is_err()
        );
        assert!(
            normalize(
                "Forms$WebUIProjectSettingsPart",
                "EnableRspackBundler",
                Value::Boolean(true)
            )
            .is_ok()
        );
    }

    #[test]
    fn component_field_requires_matching_storage_type() {
        let wrong = Node::new("Settings$TracingConfiguration").unwrap();
        let result = normalize(
            "Settings$ServerConfiguration",
            "OpenTelemetry",
            Value::Node(wrong),
        );
        assert!(result.is_err());

        let right = Node::new("Settings$OpenTelemetryConfiguration").unwrap();
        assert!(
            normalize(
                "Settings$ServerConfiguration",
                "OpenTelemetry",
                Value::Node(right)
            )
            .is_ok()
        );
    }

    #[test]
    fn collection_field_requires_a_collection_value() {
        assert!(
            normalize(
                "Settings$JarDeploymentSettings",
                "Exclusions",
                Value::String("x".into())
            )
            .is_err()
        );
        let collection =
            Value::Collection(Collection::new(vec![Value::String("x".into())], 2).unwrap());
        assert!(normalize("Settings$JarDeploymentSettings", "Exclusions", collection).is_ok());
    }

    #[test]
    fn collection_item_validates_against_declared_component_type() {
        let wrong_item = Value::Node(Node::new("Settings$Certificate").unwrap());
        let wrong_collection = Value::Collection(Collection::new(vec![wrong_item], 3).unwrap());
        assert!(normalize("Settings$LanguageSettings", "Languages", wrong_collection).is_err());

        let right_item = Value::Node(Node::new("Texts$Language").unwrap());
        let right_collection = Value::Collection(Collection::new(vec![right_item], 3).unwrap());
        assert!(normalize("Settings$LanguageSettings", "Languages", right_collection).is_ok());
    }
}
