//! Names and collection contracts used by Studio Pro 11 project settings.
//!
//! Ports `Mxrb::Settings::Catalog` from `lib/mxrb/settings/model.rb`.

use std::sync::OnceLock;

use regex::Regex;

use crate::error::{Result, SettingsError};

/// `(storage $Type, DSL component method name)` pairs.
pub const TYPE_METHODS: &[(&str, &str)] = &[
    ("Settings$ProjectSettings", "project_settings"),
    ("Forms$WebUIProjectSettingsPart", "web_ui"),
    ("Settings$IntegrationProjectSettingsPart", "integration"),
    ("Settings$ConfigurationSettings", "configuration"),
    ("Settings$ModelSettings", "model"),
    ("Settings$ConventionSettings", "conventions"),
    ("Settings$LanguageSettings", "languages"),
    ("Settings$CertificateSettings", "certificates"),
    ("Settings$WorkflowsProjectSettingsPart", "workflows"),
    ("Settings$JarDeploymentSettings", "jar_deployment"),
    ("Settings$DistributionSettings", "distribution"),
    ("Settings$JavaActionsSettings", "java_actions"),
    ("Settings$ThemeModuleEntry", "theme_module"),
    ("Settings$ActionActivityDefaultColor", "action_activity_default_color"),
    ("Settings$Certificate", "certificate"),
    ("Settings$ServerConfiguration", "server"),
    ("Settings$CustomSetting", "custom_setting"),
    ("Settings$OpenTelemetryConfiguration", "open_telemetry_configuration"),
    ("Settings$TracingConfiguration", "tracing_configuration"),
    ("Texts$Language", "language"),
];

/// The subset of [`TYPE_METHODS`] valid as `Settings$ProjectSettings.Settings[]` parts.
pub const PART_TYPES: &[&str] = &[
    "Forms$WebUIProjectSettingsPart",
    "Settings$IntegrationProjectSettingsPart",
    "Settings$ConfigurationSettings",
    "Settings$ModelSettings",
    "Settings$ConventionSettings",
    "Settings$LanguageSettings",
    "Settings$CertificateSettings",
    "Settings$WorkflowsProjectSettingsPart",
    "Settings$JarDeploymentSettings",
    "Settings$DistributionSettings",
    "Settings$JavaActionsSettings",
];

pub const FIELDS: &[(&str, &[&str])] = &[
    ("Settings$ProjectSettings", &["Settings"]),
    (
        "Forms$WebUIProjectSettingsPart",
        &[
            "EnableDownloadResources",
            "EnableMicroflowReachabilityAnalysis",
            "EnableNewStringBehavior",
            "EnableNewWidgetGeneration",
            "EnableRspackBundler",
            "EnableWidgetBundling",
            "Theme",
            "ThemeModuleName",
            "ThemeModuleOrder",
            "UrlPrefix",
            "UseOptimizedClient",
        ],
    ),
    ("Settings$IntegrationProjectSettingsPart", &["ObsoleteEnableUrlEncoding"]),
    ("Settings$ConfigurationSettings", &["Configurations"]),
    (
        "Settings$ModelSettings",
        &[
            "AfterStartupMicroflow",
            "AllowUserMultipleSessions",
            "BcryptCost",
            "BeforeShutdownMicroflow",
            "DecimalScale",
            "DefaultTimeZoneCode",
            "EnableDataStorageNewQueryHandling",
            "EnableDataStorageOptimisticLocking",
            "EnforceDataStorageUniqueness",
            "FirstDayOfWeek",
            "HashAlgorithm",
            "HealthCheckMicroflow",
            "JavaMajorVersion",
            "JavaVersion",
            "RoundingMode",
            "ScheduledEventTimeZoneCode",
            "SslCertificateAlgorithm",
            "UseDatabaseForeignKeyConstraints",
            "UseDeprecatedClientForWebServiceCalls",
            "UseOQLVersion2",
            "UseSystemContextForBackgroundTasks",
        ],
    ),
    (
        "Settings$ConventionSettings",
        &["ActionActivityDefaultColors", "DefaultAssociationStorage", "DefaultSequenceFlowLineType", "LowerCaseMicroflowVariables"],
    ),
    ("Settings$LanguageSettings", &["DefaultLanguageCode", "Languages"]),
    ("Settings$CertificateSettings", &["Certificates"]),
    (
        "Settings$WorkflowsProjectSettingsPart",
        &[
            "DefaultTaskParallelism",
            "Groups",
            "OnWorkflowEvent",
            "UserEntity",
            "UsertaskOnStateChangeEvent",
            "WorkflowEngineParallelism",
            "WorkflowOnStateChangeEvent",
        ],
    ),
    ("Settings$JarDeploymentSettings", &["Exclusions"]),
    ("Settings$DistributionSettings", &["BasedOnVersion", "IsDistributable", "Version"]),
    ("Settings$JavaActionsSettings", &["GeneratePostfixesForParameters"]),
    ("Settings$ThemeModuleEntry", &["ModuleName"]),
    ("Settings$ActionActivityDefaultColor", &["ActionActivityType", "BackgroundColor"]),
    ("Settings$Certificate", &["Data", "Type"]),
    (
        "Settings$ServerConfiguration",
        &[
            "ApplicationRootUrl",
            "ConstantValues",
            "CustomSettings",
            "DatabaseName",
            "DatabasePassword",
            "DatabaseType",
            "DatabaseUrl",
            "DatabaseUseIntegratedSecurity",
            "DatabaseUserName",
            "EmulateCloudSecurity",
            "ExtraJvmParameters",
            "HttpPortNumber",
            "MaxJavaHeapSize",
            "Name",
            "OpenAdminPort",
            "OpenHttpPort",
            "OpenTelemetry",
            "ServerPortNumber",
            "Tracing",
        ],
    ),
    ("Settings$CustomSetting", &["Name", "Value"]),
    ("Settings$OpenTelemetryConfiguration", &["Enabled", "Endpoint", "Logs", "ServiceName", "Traces"]),
    ("Settings$TracingConfiguration", &["Enabled", "Endpoint", "ServiceName"]),
    (
        "Texts$Language",
        &["CheckCompleteness", "Code", "CustomDateFormat", "CustomDateTimeFormat", "CustomTimeFormat", "Description", "RightToLeft"],
    ),
];

/// `((storage $Type, field), marker)` overrides; anything absent defaults to marker `3`.
pub const COLLECTION_MARKERS: &[((&str, &str), i32)] = &[
    (("Settings$ProjectSettings", "Settings"), 2),
    (("Settings$WorkflowsProjectSettingsPart", "Groups"), 2),
    (("Settings$WorkflowsProjectSettingsPart", "OnWorkflowEvent"), 2),
    (("Settings$JarDeploymentSettings", "Exclusions"), 2),
];

pub const COLLECTION_FIELDS: &[&str] = &[
    "Settings",
    "ThemeModuleOrder",
    "Configurations",
    "ActionActivityDefaultColors",
    "Languages",
    "Certificates",
    "Groups",
    "OnWorkflowEvent",
    "Exclusions",
    "ConstantValues",
    "CustomSettings",
];

pub fn method_for_type(storage_type: &str) -> Result<&'static str> {
    TYPE_METHODS
        .iter()
        .find(|(t, _)| *t == storage_type)
        .map(|(_, m)| *m)
        .ok_or_else(|| SettingsError::UnsupportedType(storage_type.to_string()))
}

pub fn type_for_method(method: &str) -> Result<&'static str> {
    TYPE_METHODS
        .iter()
        .find(|(_, m)| *m == method)
        .map(|(t, _)| *t)
        .ok_or_else(|| SettingsError::UnsupportedComponent(method.to_string()))
}

pub fn is_part_type(storage_type: &str) -> bool {
    PART_TYPES.contains(&storage_type)
}

pub fn fields_for(storage_type: &str) -> &'static [&'static str] {
    FIELDS.iter().find(|(t, _)| *t == storage_type).map(|(_, f)| *f).unwrap_or(&[])
}

/// Mirrors Ruby's two-pass regex camelCase -> snake_case conversion:
/// `field.gsub(/([A-Z]+)([A-Z][a-z])/, '\1_\2').gsub(/([a-z\d])([A-Z])/, '\1_\2').downcase`.
pub fn field_method(field: &str) -> String {
    static ACRONYM_BOUNDARY: OnceLock<Regex> = OnceLock::new();
    static WORD_BOUNDARY: OnceLock<Regex> = OnceLock::new();
    let acronym = ACRONYM_BOUNDARY.get_or_init(|| Regex::new(r"([A-Z]+)([A-Z][a-z])").unwrap());
    let word = WORD_BOUNDARY.get_or_init(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap());

    let step1 = acronym.replace_all(field, "${1}_${2}");
    let step2 = word.replace_all(&step1, "${1}_${2}");
    step2.to_lowercase()
}

pub fn field_for(storage_type: &str, method: &str) -> Result<&'static str> {
    fields_for(storage_type)
        .iter()
        .find(|f| field_method(f) == method)
        .copied()
        .ok_or_else(|| SettingsError::UnknownField { storage_type: storage_type.to_string(), method: method.to_string() })
}

pub fn is_collection_field(field: &str) -> bool {
    COLLECTION_FIELDS.contains(&field)
}

pub fn collection_marker(storage_type: &str, field: &str) -> i32 {
    COLLECTION_MARKERS.iter().find(|((t, f), _)| *t == storage_type && *f == field).map(|(_, m)| *m).unwrap_or(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_for_type_and_back() {
        assert_eq!(method_for_type("Settings$ProjectSettings").unwrap(), "project_settings");
        assert_eq!(type_for_method("project_settings").unwrap(), "Settings$ProjectSettings");
        assert!(method_for_type("Nonexistent$Type").is_err());
    }

    #[test]
    fn field_method_handles_plain_camel_case() {
        assert_eq!(field_method("EnableDownloadResources"), "enable_download_resources");
        assert_eq!(field_method("Name"), "name");
    }

    #[test]
    fn field_method_handles_acronym_boundaries() {
        assert_eq!(field_method("HttpPortNumber"), "http_port_number");
        assert_eq!(field_method("OpenAdminPort"), "open_admin_port");
        assert_eq!(field_method("SslCertificateAlgorithm"), "ssl_certificate_algorithm");
        assert_eq!(field_method("UseOQLVersion2"), "use_oql_version2");
    }

    #[test]
    fn field_for_resolves_known_field_by_its_method_name() {
        assert_eq!(field_for("Settings$ServerConfiguration", "http_port_number").unwrap(), "HttpPortNumber");
        assert!(field_for("Settings$ServerConfiguration", "not_a_field").is_err());
    }

    #[test]
    fn collection_marker_defaults_to_three() {
        assert_eq!(collection_marker("Settings$ProjectSettings", "Settings"), 2);
        assert_eq!(collection_marker("Settings$LanguageSettings", "Languages"), 3);
    }
}
