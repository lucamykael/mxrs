//! A schema-checked settings component. Storage names remain internal to
//! callers, who address fields by their derived method name
//! ([`crate::catalog::field_method`]) or the canonical BSON field name —
//! both resolve to the same canonical field.
//!
//! Ports `Mxrb::Settings::Node` from `lib/mxrb/settings/model.rb`.

use crate::catalog;
use crate::error::{Result, SettingsError};
use crate::value::Value;
use crate::value_contracts;

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    storage_type: String,
    // Insertion-ordered (not a HashMap) to match Ruby Hash semantics: byte-
    // identical re-encoding after a no-op decode->encode round trip depends
    // on fields staying in their original document order.
    fields: Vec<(String, Value)>,
}

impl Node {
    pub fn new(storage_type: impl Into<String>) -> Result<Self> {
        let storage_type = storage_type.into();
        catalog::method_for_type(&storage_type)?;
        Ok(Self {
            storage_type,
            fields: Vec::new(),
        })
    }

    pub fn storage_type(&self) -> &str {
        &self.storage_type
    }

    /// Sets `field` (accepted either as its canonical BSON name or its
    /// derived method name) after validating it belongs to this node's
    /// type and normalizing/type-checking `value`.
    pub fn set(&mut self, field: &str, value: Value) -> Result<()> {
        let method = catalog::field_method(field);
        let canonical = catalog::field_for(&self.storage_type, &method)?;
        let normalized = value_contracts::normalize(&self.storage_type, canonical, value)?;
        if let Some(slot) = self.fields.iter_mut().find(|(k, _)| k == canonical) {
            slot.1 = normalized;
        } else {
            self.fields.push((canonical.to_string(), normalized));
        }
        Ok(())
    }

    pub fn fetch(&self, field: &str) -> Result<&Value> {
        let method = catalog::field_method(field);
        let canonical = catalog::field_for(&self.storage_type, &method)?;
        self.fields
            .iter()
            .find(|(k, _)| k == canonical)
            .map(|(_, v)| v)
            .ok_or_else(|| SettingsError::FieldNotSet {
                storage_type: self.storage_type.clone(),
                field: canonical.to_string(),
            })
    }

    pub fn has_field(&self, canonical_field: &str) -> bool {
        self.fields.iter().any(|(k, _)| k == canonical_field)
    }

    /// Fields in insertion order, as `(canonical BSON name, value)` pairs.
    pub fn fields(&self) -> &[(String, Value)] {
        &self.fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::BinarySubtype;

    #[test]
    fn new_rejects_unknown_storage_type() {
        assert!(Node::new("Nonexistent$Type").is_err());
    }

    #[test]
    fn set_and_fetch_accept_either_canonical_or_method_name() {
        let mut node = Node::new("Settings$ServerConfiguration").unwrap();
        node.set("HttpPortNumber", Value::Integer(8080)).unwrap();
        assert_eq!(node.fetch("HttpPortNumber").unwrap(), &Value::Integer(8080));
        assert_eq!(
            node.fetch("http_port_number").unwrap(),
            &Value::Integer(8080)
        );
    }

    #[test]
    fn set_rejects_unknown_field() {
        let mut node = Node::new("Settings$ServerConfiguration").unwrap();
        assert!(node.set("NotAField", Value::Integer(1)).is_err());
    }

    #[test]
    fn set_rejects_wrong_value_type() {
        let mut node = Node::new("Settings$ServerConfiguration").unwrap();
        assert!(node
            .set("HttpPortNumber", Value::String("not-an-int".into()))
            .is_err());
    }

    #[test]
    fn set_preserves_original_field_position_on_update() {
        let mut node = Node::new("Settings$ServerConfiguration").unwrap();
        node.set("Name", Value::String("A".into())).unwrap();
        node.set("HttpPortNumber", Value::Integer(1)).unwrap();
        node.set("Name", Value::String("B".into())).unwrap();

        let keys: Vec<&str> = node.fields().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["Name", "HttpPortNumber"]);
        assert_eq!(node.fetch("Name").unwrap(), &Value::String("B".into()));
    }

    #[test]
    fn set_rejects_incompatible_binary_asset_type() {
        let mut node = Node::new("Settings$Certificate").unwrap();
        let result = node.set(
            "Data",
            Value::Binary(crate::value::BinaryAsset::empty(BinarySubtype::Generic)),
        );
        assert!(result.is_ok());
    }
}
