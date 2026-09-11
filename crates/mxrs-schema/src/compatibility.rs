//! Projects the version-neutral document tree onto an exact Studio Pro
//! schema. Only the 11.12.1 strategy is ported (locked MVP scope is
//! Mendix 11.x only) — mxrb's 9.6.1/10.24.0 strategies exist to support
//! its full 5.21-11.12 matrix and are out of scope here.
//!
//! Ports `Mxrb::StudioCompatibility` + `Mendix11121Strategy` from
//! `lib/mxrb/studio_compatibility.rb`. Deliberately **not** ported yet:
//! `reconcile_project_conversion!`/`reconcile_system_texts!`, which
//! reconcile a document's `OneTimeConversions`/`SystemTexts` lists against
//! a seed template project read from disk — that's a new-project-scaffolding
//! concern (needs `mxrs-scaffold`, not yet built) rather than a per-document
//! compatibility fix, and every other projection here works with no
//! external data dependency.

use mxrs_bson::{Bson, Document};

/// `{SHA256}...`-prefixed schema hash Studio Pro stamps into `_MetaData`.
pub const SCHEMA_HASH_11_12_1: &str = "{SHA256}ex8TFkjI5tikVWCC05OxODFVHlheQtT9XpJxPAwVGY0=";

/// Returns the expected schema hash for `version`, or `None` for versions
/// outside the locked MVP scope (11.12.1 only).
pub fn schema_hash(version: &str) -> Option<&'static str> {
    match version {
        "11.12.1" => Some(SCHEMA_HASH_11_12_1),
        _ => None,
    }
}

/// Applies the schema migrations Studio Pro 11.12.1 performs when it opens
/// an older project, recursively across the whole document tree. A no-op
/// for any other version (mirrors Ruby's `STRATEGIES.fetch(version,
/// DefaultStrategy.new)`).
pub fn apply_document(version: &str, document: &mut Document) {
    if version == "11.12.1" {
        visit(document);
    }
}

fn visit(doc: &mut Document) {
    project_node(doc);
    let keys: Vec<String> = doc.keys().cloned().collect();
    for key in keys {
        if let Some(value) = doc.get_mut(&key) {
            visit_value(value);
        }
    }
}

fn visit_value(value: &mut Bson) {
    match value {
        Bson::Document(doc) => visit(doc),
        Bson::Array(items) => items.iter_mut().for_each(visit_value),
        _ => {}
    }
}

fn project_node(node: &mut Document) {
    let type_name = node.get_str("$Type").unwrap_or("").to_string();

    if type_name == "Settings$ProjectSettings" {
        project_settings(node);
    }
    if type_name == "Settings$TracingConfiguration" {
        project_tracing_configuration(node);
    }

    let renamed_type = type_rename(&type_name);
    if renamed_type != type_name {
        node.insert("$Type", renamed_type.to_string());
    }
    if type_name == "DomainModels$EntityImpl" {
        delete_empty_event_handlers(node);
    }
    if let Some((from, to)) = rename_for(&type_name) {
        if node.contains_key(from)
            && !node.contains_key(to)
            && let Some(value) = node.get(from).cloned()
        {
            node.insert(to, value);
        }
        node.remove(from);
    }
    for field in deletions_for(&type_name) {
        node.remove(*field);
    }
    if let Some((field, mapping)) = replacement_for(&type_name)
        && let Some(Bson::String(current)) = node.get(field)
        && let Some((_, to)) = mapping.iter().find(|(from, _)| *from == current)
    {
        let to = (*to).to_string();
        node.insert(field, Bson::String(to));
    }
    for (key, value) in defaults_for(&type_name) {
        if !node.contains_key(key) {
            node.insert(key, value);
        }
    }
}

fn delete_empty_event_handlers(node: &mut Document) {
    let Some(Bson::Array(items)) = node.get("EventHandlers") else {
        return;
    };
    if mxrs_bson::parse_array(Some(items)).items.is_empty() {
        node.remove("EventHandlers");
    }
}

fn project_tracing_configuration(node: &mut Document) {
    node.insert("$ID", uuid::Uuid::new_v4().to_string());
    if let Some(Bson::String(endpoint)) = node.get("Endpoint").cloned() {
        node.insert("Endpoint", strip_v1_traces_suffix(&endpoint));
    }
    node.insert("Logs", Bson::Null);
    node.insert("Traces", Bson::Null);
}

/// Mirrors Ruby's `endpoint.sub(%r{/v1/traces/?\z}, '')`.
fn strip_v1_traces_suffix(endpoint: &str) -> String {
    endpoint
        .strip_suffix("/v1/traces/")
        .or_else(|| endpoint.strip_suffix("/v1/traces"))
        .unwrap_or(endpoint)
        .to_string()
}

fn project_settings(node: &mut Document) {
    let Some(Bson::Array(settings)) = node.get_mut("Settings") else {
        return;
    };
    let present: std::collections::HashSet<String> = mxrs_bson::parse_array(Some(settings))
        .items
        .iter()
        .filter_map(|part| {
            if let Bson::Document(d) = part {
                d.get_str("$Type").ok().map(str::to_string)
            } else {
                None
            }
        })
        .collect();

    for (type_name, defaults) in project_setting_defaults() {
        if present.contains(type_name) {
            continue;
        }
        let mut part = Document::new();
        part.insert("$ID", uuid::Uuid::new_v4().to_string());
        part.insert("$Type", type_name.to_string());
        for (key, value) in defaults {
            part.insert(key, value);
        }
        settings.push(Bson::Document(part));
    }
}

fn project_setting_defaults() -> Vec<(&'static str, Vec<(&'static str, Bson)>)> {
    vec![
        (
            "Settings$JarDeploymentSettings",
            vec![("Exclusions", Bson::Array(vec![Bson::Int32(2)]))],
        ),
        (
            "Settings$DistributionSettings",
            vec![
                ("BasedOnVersion", Bson::String(String::new())),
                ("IsDistributable", Bson::Boolean(false)),
                ("Version", Bson::String(String::new())),
            ],
        ),
    ]
}

fn type_rename(type_name: &str) -> &str {
    match type_name {
        "Settings$TracingConfiguration" => "Settings$OpenTelemetryConfiguration",
        other => other,
    }
}

fn rename_for(type_name: &str) -> Option<(&'static str, &'static str)> {
    match type_name {
        "Settings$ServerConfiguration" => Some(("Tracing", "OpenTelemetry")),
        _ => None,
    }
}

fn deletions_for(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "DomainModels$EntityImpl" => &["IsRemote", "RemoteSource"],
        "Enumerations$EnumerationValue" => &["ExportLevel"],
        "Microflows$Nanoflow" => &["ApplyEntityAccess"],
        "Projects$ModuleImpl" => &["AppStorePackageId"],
        "Settings$IntegrationProjectSettingsPart" => &["ObsoleteEnableUrlEncoding"],
        "Settings$ModelSettings" => &["JavaVersion"],
        "Settings$WorkflowsProjectSettingsPart" => {
            &["UsertaskOnStateChangeEvent", "WorkflowOnStateChangeEvent"]
        }
        _ => &[],
    }
}

fn replacement_for(
    type_name: &str,
) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    match type_name {
        "Forms$Page" => Some(("ExportLevel", &[("Public", "Hidden")])),
        _ => None,
    }
}

fn defaults_for(type_name: &str) -> Vec<(&'static str, Bson)> {
    match type_name {
        "CustomWidgets$WidgetValueType" => vec![("AllowUpload", Bson::Boolean(false))],
        "Forms$CallNanoflowClientAction" => vec![
            ("ConfirmationInfo", Bson::Null),
            ("DisabledDuringExecution", Bson::Boolean(true)),
            ("Nanoflow", Bson::String(String::new())),
            ("OutputMappings", Bson::Array(vec![Bson::Int32(3)])),
            ("ParameterMappings", Bson::Array(vec![Bson::Int32(2)])),
            ("ProgressBar", Bson::String("None".to_string())),
            ("ProgressMessage", Bson::Null),
        ],
        "Forms$MicroflowAction" => vec![("DisabledDuringExecution", Bson::Boolean(true))],
        "Forms$MicroflowSettings" => vec![
            ("Asynchronous", Bson::Boolean(false)),
            ("ConfirmationInfo", Bson::Null),
            ("FormValidations", Bson::String("All".to_string())),
            ("OutputMappings", Bson::Array(vec![Bson::Int32(3)])),
            ("ParameterMappings", Bson::Array(vec![Bson::Int32(2)])),
            ("ProgressBar", Bson::String("None".to_string())),
            ("ProgressMessage", Bson::Null),
        ],
        "Forms$NoAction" => vec![("DisabledDuringExecution", Bson::Boolean(true))],
        "Forms$Page" => vec![("Autofocus", Bson::String("Off".to_string()))],
        "Forms$PageParameter" => {
            vec![
                ("DefaultValue", Bson::String(String::new())),
                ("IsRequired", Bson::Boolean(true)),
            ]
        }
        "Forms$PageVariable" => vec![("SubKey", Bson::String(String::new()))],
        "Forms$SnippetParameterMapping" => vec![("Argument", Bson::String(String::new()))],
        "Forms$WebUIProjectSettingsPart" => {
            vec![
                ("EnableNewStringBehavior", Bson::Boolean(false)),
                ("EnableRspackBundler", Bson::Boolean(false)),
            ]
        }
        "Menus$MenuItem" => vec![("AlternativeText", Bson::Null)],
        "Microflows$Nanoflow" => vec![("UseListParameterByReference", Bson::Boolean(true))],
        "Projects$ModuleImpl" => vec![("AppStorePackageIdString", Bson::String(String::new()))],
        "Projects$ModuleSettings" => vec![
            ("Checksum", Bson::String(String::new())),
            ("ConvertedChecksum", Bson::String(String::new())),
            ("EnableDetailedTroubleshooting", Bson::Boolean(true)),
            ("ModuleDependencies", Bson::Null),
            ("OriginalPackageId", Bson::String(String::new())),
            ("PackageId", Bson::String(String::new())),
        ],
        "Settings$ModelSettings" => {
            vec![
                ("DecimalScale", Bson::Int32(8)),
                ("JavaMajorVersion", Bson::String("21".to_string())),
            ]
        }
        "Settings$ServerConfiguration" => vec![("OpenTelemetry", Bson::Null)],
        "Settings$WorkflowsProjectSettingsPart" => {
            vec![("Groups", Bson::Array(vec![Bson::Int32(2)]))]
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    #[test]
    fn schema_hash_covers_only_11_12_1() {
        assert_eq!(schema_hash("11.12.1"), Some(SCHEMA_HASH_11_12_1));
        assert_eq!(schema_hash("9.6.1.29396"), None);
    }

    #[test]
    fn is_a_no_op_for_versions_outside_scope() {
        let mut document = doc! { "$Type": "Forms$Page" };
        let before = document.clone();
        apply_document("9.6.1.29396", &mut document);
        assert_eq!(document, before);
    }

    #[test]
    fn adds_missing_default_fields() {
        let mut document = doc! { "$Type": "Forms$Page", "Name": "Home" };
        apply_document("11.12.1", &mut document);
        assert_eq!(document.get_str("Autofocus").unwrap(), "Off");
    }

    #[test]
    fn does_not_override_an_already_present_field() {
        let mut document = doc! { "$Type": "Forms$Page", "Autofocus": "First" };
        apply_document("11.12.1", &mut document);
        assert_eq!(document.get_str("Autofocus").unwrap(), "First");
    }

    #[test]
    fn deletes_removed_fields() {
        let mut document =
            doc! { "$Type": "DomainModels$EntityImpl", "IsRemote": true, "RemoteSource": "x" };
        apply_document("11.12.1", &mut document);
        assert!(!document.contains_key("IsRemote"));
        assert!(!document.contains_key("RemoteSource"));
    }

    #[test]
    fn removes_empty_event_handlers_but_keeps_non_empty() {
        let mut empty =
            doc! { "$Type": "DomainModels$EntityImpl", "EventHandlers": [Bson::Int32(3)] };
        apply_document("11.12.1", &mut empty);
        assert!(!empty.contains_key("EventHandlers"));

        let mut non_empty = doc! {
            "$Type": "DomainModels$EntityImpl",
            "EventHandlers": [Bson::Int32(3), Bson::String("x".into())]
        };
        apply_document("11.12.1", &mut non_empty);
        assert!(non_empty.contains_key("EventHandlers"));
    }

    #[test]
    fn renames_field_only_when_target_absent() {
        let mut document = doc! { "$Type": "Settings$ServerConfiguration", "Tracing": "a" };
        apply_document("11.12.1", &mut document);
        assert!(!document.contains_key("Tracing"));
        assert_eq!(document.get_str("OpenTelemetry").unwrap(), "a");

        let mut both_present =
            doc! { "$Type": "Settings$ServerConfiguration", "Tracing": "a", "OpenTelemetry": "b" };
        apply_document("11.12.1", &mut both_present);
        assert!(!both_present.contains_key("Tracing"));
        assert_eq!(both_present.get_str("OpenTelemetry").unwrap(), "b");
    }

    #[test]
    fn renames_type_and_projects_tracing_configuration_fields() {
        let mut document = doc! {
            "$ID": "old-id",
            "$Type": "Settings$TracingConfiguration",
            "Endpoint": "https://collector.example/v1/traces"
        };
        apply_document("11.12.1", &mut document);
        assert_eq!(
            document.get_str("$Type").unwrap(),
            "Settings$OpenTelemetryConfiguration"
        );
        assert_ne!(document.get_str("$ID").unwrap(), "old-id");
        assert_eq!(
            document.get_str("Endpoint").unwrap(),
            "https://collector.example"
        );
        assert!(matches!(document.get("Logs"), Some(Bson::Null)));
    }

    #[test]
    fn replaces_enum_value_only_for_the_mapped_case() {
        let mut mapped = doc! { "$Type": "Forms$Page", "ExportLevel": "Public" };
        apply_document("11.12.1", &mut mapped);
        assert_eq!(mapped.get_str("ExportLevel").unwrap(), "Hidden");

        let mut unmapped = doc! { "$Type": "Forms$Page", "ExportLevel": "Private" };
        apply_document("11.12.1", &mut unmapped);
        assert_eq!(unmapped.get_str("ExportLevel").unwrap(), "Private");
    }

    #[test]
    fn adds_missing_project_setting_parts() {
        let mut document =
            doc! { "$Type": "Settings$ProjectSettings", "Settings": [Bson::Int32(3)] };
        apply_document("11.12.1", &mut document);
        let Some(Bson::Array(settings)) = document.get("Settings") else {
            panic!("expected array")
        };
        let types: Vec<&str> = settings
            .iter()
            .filter_map(|b| {
                if let Bson::Document(d) = b {
                    d.get_str("$Type").ok()
                } else {
                    None
                }
            })
            .collect();
        assert!(types.contains(&"Settings$JarDeploymentSettings"));
        assert!(types.contains(&"Settings$DistributionSettings"));
    }

    #[test]
    fn recurses_into_nested_documents_and_arrays() {
        let mut document = doc! {
            "$Type": "Wrapper",
            "Children": [{ "$Type": "Forms$Page", "Name": "Nested" }]
        };
        apply_document("11.12.1", &mut document);
        let Some(Bson::Array(children)) = document.get("Children") else {
            panic!("expected array")
        };
        let Bson::Document(child) = &children[0] else {
            panic!("expected document")
        };
        assert_eq!(child.get_str("Autofocus").unwrap(), "Off");
    }
}
