//! The typed value space a [`crate::node::Node`] field can hold.
//!
//! Ports `Mxrb::Settings::{BinaryAsset,Collection}` from
//! `lib/mxrb/settings/model.rb`.

use mxrs_bson::BinarySubtype;

use crate::error::{Result, SettingsError};
use crate::node::Node;

/// A settings field value. `Node` isn't boxed: it's stored inside `Vec`
/// (via `Collection`) or a heap-allocated map slot everywhere it recurses,
/// so this enum's size doesn't grow unbounded.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Time(mxrs_bson::DateTime),
    Binary(BinaryAsset),
    Node(Node),
    Collection(Collection),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_string())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Boolean(value)
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Integer(value)
    }
}

/// An external binary value, primarily used for trusted certificates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryAsset {
    pub bytes: Vec<u8>,
    pub subtype: BinarySubtype,
    pub path: Option<String>,
}

impl BinaryAsset {
    pub fn from_bytes(bytes: Vec<u8>, subtype: BinarySubtype) -> Self {
        Self {
            bytes,
            subtype,
            path: None,
        }
    }

    pub fn empty(subtype: BinarySubtype) -> Self {
        Self::from_bytes(Vec::new(), subtype)
    }

    pub fn at(&self, path: impl Into<String>) -> Self {
        let mut clone = self.clone();
        clone.path = Some(path.into());
        clone
    }
}

/// A Mendix array with its leading marker (see `mxrs_bson::array`), holding
/// either scalar values or homogeneous [`Node`]s.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub items: Vec<Value>,
    pub marker: i32,
}

impl Collection {
    pub fn new(items: Vec<Value>, marker: i32) -> Result<Self> {
        if !(1..=3).contains(&marker) {
            return Err(SettingsError::InvalidMarker);
        }
        Ok(Self { items, marker })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_rejects_out_of_range_markers() {
        assert!(Collection::new(vec![], 0).is_err());
        assert!(Collection::new(vec![], 4).is_err());
        assert!(Collection::new(vec![], 1).is_ok());
    }
}
