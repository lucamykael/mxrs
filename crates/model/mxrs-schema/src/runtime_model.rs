//! Ordered Runtime-model field registry backed by the embedded 11.x schema.
//!
//! Compiler passes need more than a list of legal types: BSON field order is
//! significant for deterministic packages, and an existing compiled model
//! may contain audited fields absent from the built-in table. This mirrors
//! mxrb's `RuntimeModelSchema`: prefer an ID-matched counterpart, then the
//! embedded schema, then the widest observed document of the same type.

use std::collections::HashMap;

use mxrs_bson::{Bson, Document};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeModelError {
    #[error("invalid embedded Runtime schema: {0}")]
    InvalidSchema(#[from] serde_json::Error),
    #[error("no Runtime schema for {0}")]
    MissingType(String),
}

#[derive(Debug, Clone)]
pub struct RuntimeModelSchema {
    builtin_fields: HashMap<String, Vec<String>>,
    by_id: HashMap<String, Document>,
    by_qualified_name: HashMap<String, Document>,
    observed_fields: HashMap<String, Vec<Vec<String>>>,
}

impl RuntimeModelSchema {
    pub fn for_11(existing_documents: &[Document]) -> Result<Self, RuntimeModelError> {
        let builtin_fields = serde_json::from_str(crate::assets::RUNTIME_SCHEMA_11)?;
        let mut schema = Self {
            builtin_fields,
            by_id: HashMap::new(),
            by_qualified_name: HashMap::new(),
            observed_fields: HashMap::new(),
        };
        for document in existing_documents {
            schema.index_value(&Bson::Document(document.clone()));
        }
        Ok(schema)
    }

    pub fn fields_for(&self, source: &Document) -> Result<Vec<String>, RuntimeModelError> {
        let type_name = source.get_str("$Type").unwrap_or_default();
        if let Some(existing) = self.counterpart(source)
            && existing.get_str("$Type").ok() == Some(type_name)
        {
            return Ok(existing.keys().cloned().collect());
        }
        if let Some(fields) = self.builtin_fields.get(type_name) {
            return Ok(fields.clone());
        }
        self.observed_fields
            .get(type_name)
            .and_then(|candidates| candidates.iter().max_by_key(|fields| fields.len()))
            .cloned()
            .ok_or_else(|| RuntimeModelError::MissingType(type_name.to_string()))
    }

    pub fn counterpart(&self, source: &Document) -> Option<&Document> {
        source
            .get("$ID")
            .and_then(mxrs_bson::extract_id)
            .and_then(|id| self.by_id.get(&id))
    }

    pub fn counterpart_id(&self, id: &Bson) -> Option<&Document> {
        mxrs_bson::extract_id(id).and_then(|id| self.by_id.get(&id))
    }

    pub fn named(&self, qualified_name: &str) -> Option<&Document> {
        self.by_qualified_name.get(qualified_name)
    }

    fn index_value(&mut self, value: &Bson) {
        match value {
            Bson::Document(document) => self.index_document(document),
            Bson::Array(values) => {
                for value in values {
                    self.index_value(value);
                }
            }
            _ => {}
        }
    }

    fn index_document(&mut self, document: &Document) {
        if let Ok(type_name) = document.get_str("$Type") {
            let fields: Vec<String> = document.keys().cloned().collect();
            let candidates = self
                .observed_fields
                .entry(type_name.to_string())
                .or_default();
            if !candidates.contains(&fields) {
                candidates.push(fields);
            }
            if let Some(id) = document.get("$ID").and_then(mxrs_bson::extract_id) {
                self.by_id.insert(id, document.clone());
            }
            if let Ok(name) = document.get_str("QualifiedName") {
                self.by_qualified_name
                    .insert(name.to_string(), document.clone());
            }
        }
        for value in document.values() {
            self.index_value(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    #[test]
    fn uses_builtin_order_and_id_matched_counterparts() {
        let existing = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Microflows$ActionActivity",
            "RuntimeOnly": 7,
        };
        let schema = RuntimeModelSchema::for_11(std::slice::from_ref(&existing)).unwrap();
        let builtin = doc! {
            "$ID": "22222222-2222-4222-8222-222222222222",
            "$Type": "Microflows$ActionActivity",
        };
        assert_eq!(
            schema.fields_for(&builtin).unwrap(),
            ["$ID", "$Type", "Action", "Caption", "Disabled"]
        );
        assert_eq!(
            schema.fields_for(&existing).unwrap(),
            ["$ID", "$Type", "RuntimeOnly"]
        );
    }

    #[test]
    fn indexes_nested_ids_names_and_observed_fallback_fields() {
        let nested = doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Custom$Root",
            "Child": {
                "$ID": "22222222-2222-4222-8222-222222222222",
                "$Type": "Custom$Child",
                "QualifiedName": "Demo.Child",
                "Value": true,
            },
        };
        let schema = RuntimeModelSchema::for_11(&[nested]).unwrap();
        let child = doc! {
            "$ID": "33333333-3333-4333-8333-333333333333",
            "$Type": "Custom$Child",
        };
        assert_eq!(
            schema.fields_for(&child).unwrap(),
            ["$ID", "$Type", "QualifiedName", "Value"]
        );
        assert!(
            schema
                .named("Demo.Child")
                .unwrap()
                .get_bool("Value")
                .unwrap()
        );
    }
}
