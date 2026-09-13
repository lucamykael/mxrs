//! Mendix `ContentsHash`: `Base64(SHA256(bson_bytes))`.
//!
//! Ports `Mxrb::IO::BsonCodec.contents_hash` from `lib/mxrb/io/bson_codec.rb`.

use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

pub fn contents_hash(bson_bytes: &[u8]) -> String {
    let digest = Sha256::digest(bson_bytes);
    STANDARD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_empty_input() {
        // echo -n "" | sha256sum | then base64 of the raw digest bytes.
        assert_eq!(
            contents_hash(b""),
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
    }
}
