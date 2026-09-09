//! Restores BSON values that were serialized through MongoDB's canonical
//! extended-JSON form (`{"$binary": {"base64": ..., "subType": ...}}`).
//!
//! Ports `Mxrb::IO::BsonCodec.restore_extended_json` / `.extended_binary`
//! from `lib/mxrb/io/bson_codec.rb`. Architecture metadata is stored as
//! plain JSON, so this conversion is needed before a stored native document
//! is written again during a version transition.

use base64::{engine::general_purpose::STANDARD, Engine};
use bson::{spec::BinarySubtype, Binary};
use serde_json::Value;

/// A restored value: either a decoded binary blob, or a JSON value with any
/// nested extended-JSON binary markers restored.
#[derive(Debug, Clone, PartialEq)]
pub enum Restored {
    Binary(Binary),
    Json(Value),
}

pub fn restore_extended_json(value: &Value) -> Restored {
    match value {
        Value::Object(map) => {
            if let Some(binary) = extended_binary(value) {
                return Restored::Binary(binary);
            }
            let mut result = serde_json::Map::with_capacity(map.len());
            for (key, child) in map {
                result.insert(key.clone(), restored_to_json(restore_extended_json(child)));
            }
            Restored::Json(Value::Object(result))
        }
        Value::Array(items) => Restored::Json(Value::Array(
            items
                .iter()
                .map(|item| restored_to_json(restore_extended_json(item)))
                .collect(),
        )),
        other => Restored::Json(other.clone()),
    }
}

fn restored_to_json(restored: Restored) -> Value {
    match restored {
        // A restored binary embedded inside a larger JSON tree has no
        // faithful JSON representation left; callers that need the `Binary`
        // itself should call `restore_extended_json` directly on that
        // sub-value rather than through a parent container.
        Restored::Binary(binary) => Value::String(format!("<binary:{} bytes>", binary.bytes.len())),
        Restored::Json(value) => value,
    }
}

/// Recognizes `{"$binary": {"base64": ..., "subType": ...}}` and decodes it.
pub fn extended_binary(value: &Value) -> Option<Binary> {
    let payload = value.get("$binary")?.as_object()?;
    let base64_str = payload.get("base64")?.as_str()?;
    let subtype_str = payload
        .get("subType")
        .and_then(Value::as_str)
        .unwrap_or("00");

    let cleaned: String = base64_str.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = STANDARD.decode(cleaned).ok()?;
    let subtype_byte = u8::from_str_radix(subtype_str.trim(), 16).unwrap_or(0);

    Some(Binary {
        subtype: BinarySubtype::from(subtype_byte),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn restores_a_top_level_binary_marker() {
        let value = json!({ "$binary": { "base64": "aGVsbG8=", "subType": "00" } });
        match restore_extended_json(&value) {
            Restored::Binary(binary) => assert_eq!(binary.bytes, b"hello"),
            Restored::Json(_) => panic!("expected Binary"),
        }
    }

    #[test]
    fn passes_through_plain_json_untouched() {
        let value = json!({ "Name": "Order", "Total": 5 });
        match restore_extended_json(&value) {
            Restored::Json(v) => assert_eq!(v, value),
            Restored::Binary(_) => panic!("expected Json"),
        }
    }

    #[test]
    fn recurses_into_nested_objects_and_arrays() {
        let value = json!({ "Items": [{ "Name": "leaf" }] });
        match restore_extended_json(&value) {
            Restored::Json(v) => assert_eq!(v, value),
            Restored::Binary(_) => panic!("expected Json"),
        }
    }
}
