//! Decodes the Runtime-owned System module embedded with the schema assets.
//!
//! A Mendix application MPR does not contain the System module. Compiler
//! passes still need its entities, enumerations and module references, so the
//! compiler input includes a versioned gzip-compressed stream of concatenated
//! BSON documents. This is the Rust counterpart of mxrb's
//! `compiler/system_model_seed.rb` and `compiler/model_package.rb` readers.

use std::io::Read;

use base64::Engine;
use flate2::read::GzDecoder;
use mxrs_bson::Document;

use crate::assets::SYSTEM_MODEL_SEED_11_12_1;

pub const SYSTEM_MODULE_ID: &str = "6e3fe785-0e7d-42ec-a592-8bc1ea4ea87d";

#[derive(Debug, thiserror::Error)]
pub enum SystemModelError {
    #[error("no native System model seed for Mendix {0:?}")]
    UnsupportedVersion(String),
    #[error("invalid base64 in native System model seed: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("cannot decompress native System model seed: {0}")]
    Gzip(#[from] std::io::Error),
    #[error("native System model seed has a truncated BSON length at byte {0}")]
    Truncated(usize),
    #[error("native System model seed has invalid BSON size {size} at byte {offset}")]
    InvalidSize { offset: usize, size: i32 },
    #[error("invalid BSON in native System model seed at byte {offset}: {source}")]
    Bson {
        offset: usize,
        #[source]
        source: mxrs_bson::BsonCodecError,
    },
}

/// Returns the System model package documents for the supported 11.x family.
/// The audited 11.12.1 seed is also the compatibility seed used for other
/// 11.x patch releases, matching mxrb's version-band selection.
pub fn system_model_documents(version: &str) -> Result<Vec<Document>, SystemModelError> {
    let normalized = version.split(['-', '+']).next().unwrap_or(version);
    if !normalized.starts_with("11.") {
        return Err(SystemModelError::UnsupportedVersion(version.to_string()));
    }

    let cleaned: String = SYSTEM_MODEL_SEED_11_12_1
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let compressed = base64::engine::general_purpose::STANDARD.decode(cleaned)?;
    let mut bytes = Vec::new();
    GzDecoder::new(compressed.as_slice()).read_to_end(&mut bytes)?;
    parse_package(&bytes)
}

fn parse_package(bytes: &[u8]) -> Result<Vec<Document>, SystemModelError> {
    let mut documents = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes.len() - offset < 4 {
            return Err(SystemModelError::Truncated(offset));
        }
        let size = i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        if size < 5 || size as usize > bytes.len() - offset {
            return Err(SystemModelError::InvalidSize { offset, size });
        }
        let end = offset + size as usize;
        let document = mxrs_bson::parse(&bytes[offset..end])
            .map_err(|source| SystemModelError::Bson { offset, source })?;
        documents.push(document);
        offset = end;
    }
    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_system_module_domain_and_enumerations() {
        let documents = system_model_documents("11.12.1").unwrap();
        assert!(documents.iter().any(|document| {
            matches!(
                document.get_str("$Type").ok(),
                Some("Projects$Module" | "Projects$ModuleImpl")
            ) && mxrs_bson::extract_id(document.get("$ID").unwrap()).as_deref()
                == Some(SYSTEM_MODULE_ID)
        }));
        assert!(
            documents
                .iter()
                .any(|document| document.get_str("$Type").ok() == Some("DomainModels$DomainModel"))
        );
        assert!(
            documents
                .iter()
                .any(|document| document.get_str("$Type").ok() == Some("Enumerations$Enumeration"))
        );
    }

    #[test]
    fn uses_seed_for_the_supported_11_family_only() {
        assert!(system_model_documents("11.6.0").is_ok());
        assert!(matches!(
            system_model_documents("10.24.0"),
            Err(SystemModelError::UnsupportedVersion(_))
        ));
    }
}
