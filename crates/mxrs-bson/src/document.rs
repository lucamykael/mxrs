//! Whole-document BSON parse/serialize, with Mendix storage conventions:
//! `$ID`/`$Type` keys ordered first, UUID-shaped strings on known key names
//! stored as MS-GUID binary blobs, and optional int64 widening for Studio 11
//! object properties.
//!
//! Ports `Mxrb::IO::BsonCodec.parse` / `.serialize` / `.storage_value` /
//! `.storage_hash` from `lib/mxrb/io/bson_codec.rb`.

use bson::{spec::BinarySubtype, Binary, Bson, Document};

use crate::error::Result;
use crate::guid::{looks_like_uuid, uuid_to_blob};

/// Keys whose UUID-string values are stored as 16-byte MS-GUID binary blobs
/// rather than plain strings. Mirrors `BINARY_UUID_KEYS` in `bson_codec.rb`.
pub const BINARY_UUID_KEYS: &[&str] = &[
    "$ID",
    "ChildPointer",
    "CloseButtonPointer",
    "DataStorageGuid",
    "DefaultButtonPointer",
    "DefaultPagePointer",
    "DestinationPointer",
    "GUID",
    "OriginPointer",
    "ParentPointer",
    "StableId",
    "TypeParameterPointer",
    "TypePointer",
];

/// Parses raw BSON bytes into a [`Document`]. Empty/absent input parses to
/// an empty document, matching `bson_codec.rb`'s handling of `nil`/`""`.
pub fn parse(bytes: &[u8]) -> Result<Document> {
    if bytes.is_empty() {
        return Ok(Document::new());
    }
    Ok(Document::from_reader(bytes)?)
}

/// Serializes a [`Document`] to raw BSON bytes.
pub fn serialize(doc: &Document) -> Result<Vec<u8>> {
    Ok(doc.to_vec()?)
}

/// Applies Mendix storage conventions to `value` for the given field `key`
/// (`None` for array elements, which never receive keyed conversions):
/// known UUID-key strings become binary blobs, and keyed integers widen to
/// `Int64` when `int64_properties` is enabled. Recurses into documents and
/// arrays either way.
pub fn storage_value(value: &Bson, key: Option<&str>, int64_properties: bool) -> Bson {
    if let (Some(k), Bson::String(s)) = (key, value) {
        if BINARY_UUID_KEYS.contains(&k) && looks_like_uuid(s) {
            if let Ok(blob) = uuid_to_blob(s) {
                return Bson::Binary(Binary {
                    subtype: BinarySubtype::Generic,
                    bytes: blob.to_vec(),
                });
            }
        }
    }

    if int64_properties && key.is_some() {
        if let Bson::Int32(i) = value {
            return Bson::Int64(i64::from(*i));
        }
    }

    match value {
        Bson::Document(doc) => Bson::Document(storage_hash(doc, int64_properties)),
        Bson::Array(items) => Bson::Array(
            items
                .iter()
                .map(|item| storage_value(item, None, int64_properties))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Reorders `value`'s keys so `$ID` then `$Type` come first (original order
/// preserved for the rest), applying [`storage_value`] to every field.
pub fn storage_hash(value: &Document, int64_properties: bool) -> Document {
    let keys: Vec<&String> = value.keys().collect();
    let mut ordered: Vec<&String> = Vec::with_capacity(keys.len());
    if let Some(id_key) = keys.iter().find(|k| k.as_str() == "$ID") {
        ordered.push(id_key);
    }
    if let Some(type_key) = keys.iter().find(|k| k.as_str() == "$Type") {
        ordered.push(type_key);
    }
    for k in &keys {
        if k.as_str() != "$ID" && k.as_str() != "$Type" {
            ordered.push(k);
        }
    }

    let mut result = Document::new();
    for key in ordered {
        // Safe: `key` was just read from `value.keys()`.
        let v = value.get(key).expect("key came from value.keys()");
        result.insert(
            key.clone(),
            storage_value(v, Some(key.as_str()), int64_properties),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use bson::doc;

    #[test]
    fn parse_empty_bytes_yields_empty_document() {
        assert_eq!(parse(&[]).unwrap(), Document::new());
    }

    #[test]
    fn parse_and_serialize_round_trip() {
        let original = doc! { "$ID": "x", "Name": "Order", "Total": 5i32 };
        let bytes = serialize(&original).unwrap();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn storage_hash_orders_id_and_type_first() {
        let input = doc! { "Name": "Order", "$Type": "DomainModels$Entity", "$ID": "id-1" };
        let result = storage_hash(&input, false);
        let keys: Vec<&String> = result.keys().collect();
        assert_eq!(keys, vec!["$ID", "$Type", "Name"]);
    }

    #[test]
    fn storage_hash_converts_uuid_shaped_guid_field_to_binary() {
        let uuid = "c67c5271-da7d-45f1-81df-ceb6946b8abe";
        let input = doc! { "GUID": uuid, "Name": "keep-as-string" };
        let result = storage_hash(&input, false);
        assert!(matches!(result.get("GUID"), Some(Bson::Binary(_))));
        assert_eq!(result.get_str("Name").unwrap(), "keep-as-string");
    }

    #[test]
    fn storage_hash_leaves_non_uuid_strings_on_binary_keys_untouched() {
        let input = doc! { "GUID": "not-a-uuid" };
        let result = storage_hash(&input, false);
        assert_eq!(result.get_str("GUID").unwrap(), "not-a-uuid");
    }

    #[test]
    fn storage_hash_widens_keyed_integers_when_enabled() {
        let input = doc! { "Total": 5i32 };
        let result = storage_hash(&input, true);
        assert!(matches!(result.get("Total"), Some(Bson::Int64(5))));
    }

    #[test]
    fn storage_hash_does_not_widen_array_markers_even_when_int64_enabled() {
        // The leading marker of a Mendix array must stay Int32 even when
        // int64_properties is set for the surrounding document — array
        // elements receive key=None, so the widening rule can't apply to
        // them, exactly like `bson_codec.rb`'s comment documents.
        let input = doc! { "Items": [Bson::Int32(3), Bson::String("ref-1".into())] };
        let result = storage_hash(&input, true);
        let Some(Bson::Array(items)) = result.get("Items") else {
            panic!("expected array");
        };
        assert!(matches!(items[0], Bson::Int32(3)));
    }

    #[test]
    fn storage_hash_recurses_into_nested_documents() {
        let input = doc! { "Child": { "GUID": "c67c5271-da7d-45f1-81df-ceb6946b8abe" } };
        let result = storage_hash(&input, false);
        let Some(Bson::Document(child)) = result.get("Child") else {
            panic!("expected nested document");
        };
        assert!(matches!(child.get("GUID"), Some(Bson::Binary(_))));
    }
}
