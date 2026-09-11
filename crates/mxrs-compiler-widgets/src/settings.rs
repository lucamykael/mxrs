//! Ports `lib/mxrb/compiler/settings_document_compiler.rb` (45 lines):
//! compiles project settings using the ordered, version-matched Runtime
//! schema. Unlike [`crate::navigation::NavigationCompiler`]'s three-level
//! `computed → source → existing` fallback, this compiler's per-field
//! fallback is `source → existing → null` — do not share fallback logic
//! between the two, they're genuinely different (confirmed against both
//! Ruby source files independently).

use mxrs_bson::{Bson, Document, doc};
use mxrs_schema::RuntimeModelSchema;

use crate::CompilerError;
use crate::support::{array_items, build_array, get};

pub struct SettingsCompiler<'a> {
    schema: &'a RuntimeModelSchema,
}

impl<'a> SettingsCompiler<'a> {
    pub fn new(schema: &'a RuntimeModelSchema) -> Self {
        SettingsCompiler { schema }
    }

    pub fn compile(&self, source: &Document) -> Result<Document, CompilerError> {
        let type_name = source.get_str("$Type").unwrap_or_default();
        if type_name != "Settings$ProjectSettings" {
            return Err(CompilerError::UnsupportedSettingsRoot {
                type_name: type_name.to_string(),
            });
        }
        let settings = array_items(source, "Settings")
            .into_iter()
            .filter_map(|v| v.as_document().cloned())
            .map(|node| self.compile_node(&node).map(Bson::Document))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(doc! {
            "$ID": get(source, "$ID"),
            "$Type": get(source, "$Type"),
            "Settings": build_array(settings),
        })
    }

    fn compile_value(&self, value: Bson) -> Result<Bson, CompilerError> {
        match value {
            Bson::Document(d) => {
                if d.contains_key("$Type") {
                    self.compile_node(&d).map(Bson::Document)
                } else {
                    let mut result = Document::new();
                    for (key, value) in d {
                        result.insert(key, self.compile_value(value)?);
                    }
                    Ok(Bson::Document(result))
                }
            }
            Bson::Array(items) => {
                let compiled = mxrs_bson::parse_array(Some(&items))
                    .items
                    .into_iter()
                    .map(|v| self.compile_value(v))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(build_array(compiled))
            }
            other => Ok(other),
        }
    }

    fn compile_node(&self, source: &Document) -> Result<Document, CompilerError> {
        let existing = self.schema.counterpart(source);
        let mut result = Document::new();
        for field in self.schema.fields_for(source)? {
            let value = match source.get(field.as_str()) {
                Some(v) => self.compile_value(v.clone())?,
                None => existing
                    .and_then(|e| e.get(field.as_str()).cloned())
                    .unwrap_or(Bson::Null),
            };
            result.insert(field, value);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(existing: &[Document]) -> RuntimeModelSchema {
        RuntimeModelSchema::for_11(existing).unwrap()
    }

    #[test]
    fn a_non_project_settings_root_is_a_loud_error() {
        let source = doc! { "$ID": "1", "$Type": "Settings$ModelSettings" };
        let schema = schema(&[]);
        let err = SettingsCompiler::new(&schema).compile(&source).unwrap_err();
        assert!(matches!(
            err,
            CompilerError::UnsupportedSettingsRoot { type_name } if type_name == "Settings$ModelSettings"
        ));
    }

    #[test]
    fn compile_value_strips_the_array_marker_recursively() {
        let schema = schema(&[]);
        let compiler = SettingsCompiler::new(&schema);
        let nested = Bson::Array(mxrs_bson::build_array(
            vec![Bson::String("value".into())],
            3,
        ));
        let compiled = compiler.compile_value(nested).unwrap();
        let Bson::Array(items) = compiled else {
            panic!("expected an array");
        };
        // Re-marked with this crate's own default marker, but item content intact.
        assert!(items.contains(&Bson::String("value".into())));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn a_field_missing_from_both_source_and_any_counterpart_resolves_to_null() {
        // `Settings$LanguageSettings`'s builtin field list includes
        // `DefaultLanguageCode` — with no ID-matched counterpart at all and
        // the source document not setting it, `compile_node` must emit an
        // explicit `Bson::Null` for it (not omit the key, not error).
        let schema = schema(&[]);
        let source_part = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Settings$LanguageSettings",
        };
        let compiled = SettingsCompiler::new(&schema)
            .compile_node(&source_part)
            .unwrap();
        assert!(matches!(
            compiled.get("DefaultLanguageCode"),
            Some(Bson::Null)
        ));
    }

    #[test]
    fn compile_node_falls_back_to_existing_then_null_for_unset_fields() {
        let node_id = "22222222-2222-4222-8222-222222222222";
        let existing_part = doc! {
            "$ID": node_id,
            "$Type": "Settings$LanguageSettings",
            "CheckCompleteness": true,
        };
        let schema = schema(&[existing_part]);
        let compiler = SettingsCompiler::new(&schema);

        // Source omits `CheckCompleteness` entirely — should fall back to
        // the existing counterpart's value, not null.
        let source_part = doc! {
            "$ID": node_id,
            "$Type": "Settings$LanguageSettings",
        };
        let compiled = compiler.compile_node(&source_part).unwrap();
        assert!(compiled.get_bool("CheckCompleteness").unwrap());
    }

    #[test]
    fn compile_node_prefers_source_over_existing_when_both_present() {
        let node_id = "33333333-3333-4333-8333-333333333333";
        let existing_part = doc! {
            "$ID": node_id,
            "$Type": "Settings$LanguageSettings",
            "CheckCompleteness": true,
        };
        let schema = schema(&[existing_part]);
        let compiler = SettingsCompiler::new(&schema);

        let source_part = doc! {
            "$ID": node_id,
            "$Type": "Settings$LanguageSettings",
            "CheckCompleteness": false,
        };
        let compiled = compiler.compile_node(&source_part).unwrap();
        assert!(!compiled.get_bool("CheckCompleteness").unwrap());
    }

    #[test]
    fn full_settings_round_trip_drops_runtime_only_keys_not_in_schema() {
        let node_id = "44444444-4444-4444-8444-444444444444";
        let source = doc! {
            "$ID": "root",
            "$Type": "Settings$ProjectSettings",
            "Settings": mxrs_bson::build_array(
                vec![Bson::Document(doc! {
                    "$ID": node_id,
                    "$Type": "Settings$LanguageSettings",
                    "CheckCompleteness": true,
                    "RuntimeOnly": "should not survive, not in the schema field list",
                })],
                3,
            ),
        };
        let schema = schema(&[]);
        let compiled = SettingsCompiler::new(&schema).compile(&source).unwrap();
        let Some(Bson::Array(settings)) = compiled.get("Settings") else {
            panic!("expected Settings array");
        };
        let Bson::Document(part) = &settings[1] else {
            panic!("expected a document");
        };
        assert!(!part.contains_key("RuntimeOnly"));
    }
}
