//! Ports `lib/mxrb/compiler/navigation_document_compiler.rb` (93 lines):
//! compiles server-visible navigation profiles while preserving Runtime
//! widget inventories mxrs-dsl/mxrs-writer have no way to produce yet
//! (`Grids`/`CustomWidgetModules`/`PluginWidgets`, offline-sync configs) —
//! these survive re-compilation only because `RuntimeModelSchema::counterpart`
//! remembers the previously-compiled Runtime value. See
//! `mxrs_schema::RuntimeModelSchema` for that lookup itself.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document, doc};
use mxrs_schema::RuntimeModelSchema;

use crate::CompilerError;
use crate::support::{
    array_items, build_array, get, plain_array_field, plain_document_field, to_s,
};

pub struct NavigationCompiler<'a> {
    schema: &'a RuntimeModelSchema,
}

impl<'a> NavigationCompiler<'a> {
    pub fn new(schema: &'a RuntimeModelSchema) -> Self {
        NavigationCompiler { schema }
    }

    pub fn compile(&self, source: &Document) -> Result<Document, CompilerError> {
        let existing = self.schema.counterpart(source).cloned().unwrap_or_default();
        let mut result = Document::new();
        for field in self.schema.fields_for(source)? {
            let value = match field.as_str() {
                "Profiles" => {
                    let profiles = array_items(source, "Profiles")
                        .into_iter()
                        .filter_map(|v| v.as_document().cloned())
                        .map(|p| self.profile(&p).map(Bson::Document))
                        .collect::<Result<Vec<_>, _>>()?;
                    build_array(profiles)
                }
                "$ID" | "$Type" => get(source, &field),
                _ => existing
                    .get(field.as_str())
                    .cloned()
                    .or_else(|| source.get(field.as_str()).cloned())
                    .unwrap_or_else(|| Bson::Array(vec![])),
            };
            result.insert(field, value);
        }
        Ok(result)
    }

    fn profile(&self, source: &Document) -> Result<Document, CompilerError> {
        let existing = self.schema.counterpart(source).cloned().unwrap_or_default();

        let kind = get(source, "Kind");
        let is_offline = matches!(&kind, Bson::String(s) if s == "Offline");
        let throw_partial_sync_error =
            matches!(get(source, "ThrowPartialSyncError"), Bson::Boolean(true));
        let app_icon = source
            .get("AppIcon")
            .cloned()
            .or_else(|| existing.get("AppIcon").cloned())
            .unwrap_or_else(|| Bson::String(String::new()));

        let mut values: HashMap<&str, Bson> = HashMap::new();
        values.insert(
            "OfflineEntityConfigsRuntime",
            self.offline_configs(source, &existing),
        );
        values.insert("HomePage", home_page(source.get_document("HomePage").ok()));
        values.insert("HomeItems", plain_array_field(source, "HomeItems"));
        values.insert(
            "AppTitle",
            self.text_reference(source.get_document("AppTitle").ok())?,
        );
        values.insert(
            "LoginPageSettings",
            self.form_settings(source.get_document("LoginPageSettings").ok())?,
        );
        values.insert(
            "ProgressiveWebAppSettings",
            plain_document_field(source, "ProgressiveWebAppSettings"),
        );
        values.insert(
            "NotFoundHomepage",
            plain_document_field(source, "NotFoundHomepage"),
        );
        values.insert("Name", get(source, "Name"));
        values.insert("IsOffline", Bson::Boolean(is_offline));
        values.insert(
            "ThrowPartialSyncError",
            Bson::Boolean(throw_partial_sync_error),
        );
        values.insert("Kind", kind);
        values.insert("AppIcon", app_icon);

        let mut result = Document::new();
        for field in self.schema.fields_for(source)? {
            let value = values
                .get(field.as_str())
                .cloned()
                .or_else(|| source.get(field.as_str()).cloned())
                .or_else(|| existing.get(field.as_str()).cloned())
                .unwrap_or(Bson::Null);
            result.insert(field, value);
        }
        Ok(result)
    }

    fn offline_configs(&self, source: &Document, existing: &Document) -> Bson {
        if let Some(v) = existing.get("OfflineEntityConfigsRuntime") {
            return v.clone();
        }
        plain_array_field(source, "OfflineEntityConfigs")
    }

    /// Deep/recursive text-template compiler — distinct from
    /// [`crate::artifact`]'s shallow `{$ID, $Type}`-only `text_reference`
    /// (same name, deliberately different behavior, matching mxrb's own
    /// two separate private methods of the same name in two different
    /// compiler classes).
    fn text_reference(&self, source: Option<&Document>) -> Result<Bson, CompilerError> {
        let Some(source) = source else {
            return Ok(Bson::Null);
        };
        let mut result = Document::new();
        for field in self.schema.fields_for(source)? {
            let value = match field.as_str() {
                "Parameters" => {
                    let items = array_items(source, "Parameters")
                        .into_iter()
                        .map(|v| self.text_reference(v.as_document()))
                        .collect::<Result<Vec<_>, _>>()?;
                    build_array(items)
                }
                "Text" => self.text_reference(source.get_document("Text").ok())?,
                _ => get(source, &field),
            };
            result.insert(field, value);
        }
        Ok(Bson::Document(result))
    }

    fn form_settings(&self, source: Option<&Document>) -> Result<Bson, CompilerError> {
        let Some(source) = source else {
            return Ok(Bson::Null);
        };
        let title_override = self.text_reference(source.get_document("TitleOverride").ok())?;
        let location = source
            .get_str("Location")
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "Content".to_string());
        Ok(Bson::Document(doc! {
            "$ID": get(source, "$ID"),
            "$Type": get(source, "$Type"),
            "TitleOverride": title_override,
            "ParameterMappings": plain_array_field(source, "ParameterMappings"),
            "Form": to_s(&get(source, "Form")),
            "Location": location,
        }))
    }
}

fn home_page(source: Option<&Document>) -> Bson {
    match source {
        None => Bson::Null,
        Some(source) => Bson::Document(doc! {
            "$ID": get(source, "$ID"),
            "$Type": get(source, "$Type"),
            "Page": to_s(&get(source, "Page")),
            "Microflow": to_s(&get(source, "Microflow")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    fn schema(existing: &[Document]) -> RuntimeModelSchema {
        RuntimeModelSchema::for_11(existing).unwrap()
    }

    #[test]
    fn missing_counterpart_defaults_widget_inventories_to_empty_arrays() {
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Navigation$NavigationDocument",
            "Profiles": mxrs_bson::build_array(vec![], 3),
        };
        let schema = schema(&[]);
        let compiled = NavigationCompiler::new(&schema).compile(&source).unwrap();
        for field in ["Grids", "CustomWidgetModules", "PluginWidgets"] {
            assert_eq!(
                compiled.get(field),
                Some(&Bson::Array(vec![])),
                "expected {field} to default to []"
            );
        }
    }

    #[test]
    fn existing_widget_inventories_survive_recompilation() {
        let existing_doc = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Navigation$NavigationDocument",
            "Profiles": mxrs_bson::build_array(vec![], 3),
            "Grids": mxrs_bson::build_array(vec![Bson::String("StudioInjected".into())], 3),
            "CustomWidgetModules": mxrs_bson::build_array(vec![], 3),
            "PluginWidgets": mxrs_bson::build_array(vec![], 3),
        };
        let source = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Navigation$NavigationDocument",
            "Profiles": mxrs_bson::build_array(vec![], 3),
        };
        let schema = schema(&[existing_doc]);
        let compiled = NavigationCompiler::new(&schema).compile(&source).unwrap();
        let Some(Bson::Array(grids)) = compiled.get("Grids") else {
            panic!("expected Grids to be preserved from the counterpart");
        };
        assert_eq!(
            grids,
            &[Bson::Int32(3), Bson::String("StudioInjected".into())]
        );
    }

    fn offline_profile(kind: &str) -> Document {
        doc! {
            "$ID": "22222222-2222-4222-8222-222222222222",
            "$Type": "Navigation$NavigationProfile",
            "Kind": kind,
        }
    }

    fn profile_source(profile: Document) -> Document {
        doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Navigation$NavigationDocument",
            "Profiles": mxrs_bson::build_array(vec![Bson::Document(profile)], 3),
        }
    }

    fn compiled_profile(source: &Document, schema: &RuntimeModelSchema) -> Document {
        let compiled = NavigationCompiler::new(schema).compile(source).unwrap();
        let Some(Bson::Array(profiles)) = compiled.get("Profiles") else {
            panic!("expected Profiles array");
        };
        let Bson::Document(profile) = &profiles[1] else {
            panic!("expected a compiled profile document");
        };
        profile.clone()
    }

    #[test]
    fn is_offline_is_derived_from_kind() {
        let schema = schema(&[]);
        let source = profile_source(offline_profile("Offline"));
        let profile = compiled_profile(&source, &schema);
        assert!(profile.get_bool("IsOffline").unwrap());

        let source = profile_source(offline_profile("Responsive"));
        let profile = compiled_profile(&source, &schema);
        assert!(!profile.get_bool("IsOffline").unwrap());
    }

    #[test]
    fn nil_home_page_and_login_settings_pass_through_as_null() {
        let schema = schema(&[]);
        let source = profile_source(offline_profile("Responsive"));
        let profile = compiled_profile(&source, &schema);
        assert!(matches!(profile.get("HomePage"), Some(Bson::Null)));
        assert!(matches!(profile.get("LoginPageSettings"), Some(Bson::Null)));
    }

    #[test]
    fn offline_entity_configs_runtime_prefers_existing_over_recomputing() {
        let existing_profile = doc! {
            "$ID": "22222222-2222-4222-8222-222222222222",
            "$Type": "Navigation$NavigationProfile",
            "Kind": "Offline",
            "OfflineEntityConfigsRuntime": mxrs_bson::build_array(
                vec![Bson::String("computed-by-offline-sync".into())],
                3,
            ),
        };
        let existing_doc = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Navigation$NavigationDocument",
            "Profiles": mxrs_bson::build_array(vec![Bson::Document(existing_profile)], 3),
        };
        let source = profile_source(offline_profile("Offline"));
        let schema = schema(&[existing_doc]);
        let profile = compiled_profile(&source, &schema);
        let Some(Bson::Array(runtime_configs)) = profile.get("OfflineEntityConfigsRuntime") else {
            panic!("expected an array");
        };
        assert_eq!(
            runtime_configs,
            &[
                Bson::Int32(3),
                Bson::String("computed-by-offline-sync".into())
            ]
        );
    }

    #[test]
    fn app_icon_falls_back_to_empty_string_when_absent_everywhere() {
        let schema = schema(&[]);
        let source = profile_source(offline_profile("Responsive"));
        let profile = compiled_profile(&source, &schema);
        assert_eq!(profile.get_str("AppIcon").unwrap(), "");
    }

    #[test]
    fn nested_app_title_text_reference_recurses_through_parameters() {
        // `Texts$Text`'s builtin schema entry is just `[$ID, $Type]` — real
        // richer field sets (`Parameters`/`Text`) only appear once an
        // ID-matched counterpart establishes them, exactly like the
        // widget-inventory preservation above. Seed one so this test
        // exercises the real "recompiling against a previous compile"
        // path rather than an unreachable-in-practice bare builtin case.
        let app_title_id = "33333333-3333-4333-8333-333333333333";
        let existing_app_title = doc! {
            "$ID": app_title_id,
            "$Type": "Texts$Text",
            "Parameters": mxrs_bson::build_array(vec![], 3),
            "Text": Bson::Null,
        };
        let schema = schema(&[existing_app_title]);

        let app_title = doc! {
            "$ID": app_title_id,
            "$Type": "Texts$Text",
            "Parameters": mxrs_bson::build_array(
                vec![Bson::Document(doc! {
                    "$ID": "44444444-4444-4444-8444-444444444444",
                    "$Type": "Texts$Text",
                })],
                3,
            ),
            "Text": Bson::Null,
        };
        let mut profile = offline_profile("Responsive");
        profile.insert("AppTitle", app_title);
        let source = profile_source(profile);
        let profile = compiled_profile(&source, &schema);
        let Some(Bson::Document(compiled_title)) = profile.get("AppTitle") else {
            panic!("expected AppTitle to compile to a document");
        };
        let Some(Bson::Array(params)) = compiled_title.get("Parameters") else {
            panic!("expected Parameters to compile to an array");
        };
        assert_eq!(params.len(), 2); // marker + one nested text reference
    }
}
