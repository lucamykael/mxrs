//! `$ID` extraction from any of its three Mendix BSON representations.
//!
//! Ports `Mxrb::IO::BsonCodec.extract_id` / `.extended_binary` from
//! `lib/mxrb/io/bson_codec.rb`.

use base64::{engine::general_purpose::STANDARD, Engine};
use bson::{spec::BinarySubtype, Binary, Bson, Document};

use crate::guid::blob_to_uuid;

/// Extracts the Mendix `$ID` value from any of its three representations:
///   1. `Bson::String` — already a UUID string.
///   2. `Bson::Binary` — a 16-byte MS-GUID blob, converted to a UUID string.
///   3. `Bson::Document` — either an extended-JSON-shaped `{"$binary": {...}}`
///      sub-document, or a `{"Data": "<base64>", ...}` map.
///
/// Returns `None` if `value` doesn't match any of these shapes, or the
/// payload can't be decoded.
pub fn extract_id(value: &Bson) -> Option<String> {
    match value {
        Bson::String(s) => Some(s.clone()),
        Bson::Binary(bin) => blob_to_uuid(&bin.bytes),
        Bson::Document(doc) => {
            if let Some(binary) = extended_binary(doc) {
                return blob_to_uuid(&binary.bytes);
            }
            let data = doc.get_str("Data").ok()?;
            let bytes = STANDARD.decode(data).ok()?;
            blob_to_uuid(&bytes)
        }
        _ => None,
    }
}

/// Recognizes MongoDB canonical extended-JSON's `{"$binary": {"base64": ...,
/// "subType": ...}}` shape embedded in an already-decoded BSON document, and
/// decodes it into a `Binary` value.
pub fn extended_binary(value: &Document) -> Option<Binary> {
    let payload = value.get_document("$binary").ok()?;
    let base64_str = payload.get_str("base64").ok()?;
    let subtype_str = payload.get_str("subType").ok().unwrap_or("00");

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
    use bson::doc;

    const UUID: &str = "c67c5271-da7d-45f1-81df-ceb6946b8abe";

    #[test]
    fn extracts_from_plain_string() {
        assert_eq!(
            extract_id(&Bson::String(UUID.to_string())),
            Some(UUID.to_string())
        );
    }

    #[test]
    fn extracts_from_bson_binary() {
        let blob = crate::guid::uuid_to_blob(UUID).unwrap();
        let value = Bson::Binary(Binary {
            subtype: BinarySubtype::Generic,
            bytes: blob.to_vec(),
        });
        assert_eq!(extract_id(&value), Some(UUID.to_string()));
    }

    #[test]
    fn extracts_from_extended_json_binary_map() {
        let blob = crate::guid::uuid_to_blob(UUID).unwrap();
        let base64_str = STANDARD.encode(blob);
        let value = Bson::Document(doc! {
            "$binary": { "base64": base64_str, "subType": "04" }
        });
        assert_eq!(extract_id(&value), Some(UUID.to_string()));
    }

    #[test]
    fn extracts_from_data_subtype_map() {
        let blob = crate::guid::uuid_to_blob(UUID).unwrap();
        let base64_str = STANDARD.encode(blob);
        let value = Bson::Document(doc! { "Data": base64_str, "Subtype": 3 });
        assert_eq!(extract_id(&value), Some(UUID.to_string()));
    }

    #[test]
    fn returns_none_for_unrecognized_shapes() {
        assert_eq!(extract_id(&Bson::Int32(5)), None);
        assert_eq!(extract_id(&Bson::Document(doc! { "Foo": "bar" })), None);
    }
}
