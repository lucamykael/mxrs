//! MS-GUID <-> UUID conversion.
//!
//! Ports `Mxrb::IO::BsonCodec.blob_to_uuid` / `.uuid_to_blob` from
//! `lib/mxrb/io/bson_codec.rb`. MS-GUID layout: bytes 0-3 little-endian,
//! 4-5 little-endian, 6-7 little-endian, bytes 8-15 big-endian (straight).

use crate::error::{BsonCodecError, Result};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Converts a 16-byte MS-GUID blob into a canonical lowercase UUID string.
/// Returns `None` if `blob` is not exactly 16 bytes.
pub fn blob_to_uuid(blob: &[u8]) -> Option<String> {
    if blob.len() != 16 {
        return None;
    }

    let mut p1 = [blob[0], blob[1], blob[2], blob[3]];
    p1.reverse();
    let mut p2 = [blob[4], blob[5]];
    p2.reverse();
    let mut p3 = [blob[6], blob[7]];
    p3.reverse();
    let p4 = &blob[8..10];
    let p5 = &blob[10..16];

    Some(format!(
        "{}-{}-{}-{}-{}",
        hex(&p1),
        hex(&p2),
        hex(&p3),
        hex(p4),
        hex(p5)
    ))
}

/// Converts a canonical UUID string into a 16-byte MS-GUID blob.
pub fn uuid_to_blob(uuid: &str) -> Result<[u8; 16]> {
    let hex_str: String = uuid.chars().filter(|c| *c != '-').collect();
    if hex_str.len() != 32 {
        return Err(BsonCodecError::InvalidUuid(uuid.to_string()));
    }

    let mut parts = [0u8; 16];
    for (i, part) in parts.iter_mut().enumerate() {
        let byte_str = hex_str
            .get(i * 2..i * 2 + 2)
            .ok_or_else(|| BsonCodecError::InvalidUuid(uuid.to_string()))?;
        *part = u8::from_str_radix(byte_str, 16)
            .map_err(|_| BsonCodecError::InvalidUuid(uuid.to_string()))?;
    }

    let mut blob = [0u8; 16];
    blob[0] = parts[3];
    blob[1] = parts[2];
    blob[2] = parts[1];
    blob[3] = parts[0];
    blob[4] = parts[5];
    blob[5] = parts[4];
    blob[6] = parts[7];
    blob[7] = parts[6];
    blob[8..16].copy_from_slice(&parts[8..16]);

    Ok(blob)
}

/// Loosely validates that `s` has the canonical
/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` UUID shape (hex digits + dashes in
/// the right positions), mirroring `UUID_PATTERN` in `bson_codec.rb`.
pub fn looks_like_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        let expect_dash = matches!(i, 8 | 13 | 18 | 23);
        if expect_dash {
            if *b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_known_uuid() {
        let uuid = "c67c5271-da7d-45f1-81df-ceb6946b8abe";
        let blob = uuid_to_blob(uuid).unwrap();
        assert_eq!(blob_to_uuid(&blob).unwrap(), uuid);
    }

    #[test]
    fn blob_to_uuid_rejects_wrong_length() {
        assert_eq!(blob_to_uuid(&[0u8; 15]), None);
        assert_eq!(blob_to_uuid(&[0u8; 17]), None);
    }

    #[test]
    fn uuid_to_blob_rejects_malformed_input() {
        assert!(uuid_to_blob("not-a-uuid").is_err());
        assert!(uuid_to_blob("zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz").is_err());
    }

    #[test]
    fn looks_like_uuid_validates_shape() {
        assert!(looks_like_uuid("c67c5271-da7d-45f1-81df-ceb6946b8abe"));
        assert!(!looks_like_uuid("c67c5271-da7d-45f1-81df-ceb6946b8ab")); // too short
        assert!(!looks_like_uuid("c67c5271_da7d_45f1_81df_ceb6946b8abe")); // wrong separators
        assert!(!looks_like_uuid("not-a-uuid-at-all-not-a-uuid-at-alll"));
    }

    // The MS-GUID byte layout matches .NET's `Guid` struct: the first three
    // groups are little-endian, the last two groups are big-endian/straight.
    #[test]
    fn matches_known_ms_guid_byte_layout() {
        // .NET `new Guid(0x01020304, 0x0506, 0x0708, 9,10,11,12,13,14,15,16)`
        // serializes to this exact 16-byte blob.
        let blob: [u8; 16] = [
            0x04, 0x03, 0x02, 0x01, 0x06, 0x05, 0x08, 0x07, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ];
        assert_eq!(
            blob_to_uuid(&blob).unwrap(),
            "01020304-0506-0708-090a-0b0c0d0e0f10"
        );
    }
}
