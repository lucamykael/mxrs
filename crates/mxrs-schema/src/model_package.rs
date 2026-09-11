//! Reads a Mendix Runtime deployment's `model.mdp`: an ordered stream of
//! BSON documents with no container header, each carrying its own int32
//! length prefix (same shape `mxrs-mpr` already reads per-`Unit`, and
//! identical to the compressed stream `system_model` decodes — duplicated
//! here rather than shared, per this crate's existing precedent, since a
//! deployment's `model.mdp` is plain bytes straight off disk while the
//! System model seed is a compressed embedded asset).
//!
//! Ports the read path of mxrb's `compiler/model_package.rb` — not the
//! write/upsert side, which mxrs has no consumer for yet. The intended use
//! is seeding [`crate::RuntimeModelSchema`] with real Runtime-shaped
//! documents for node types the embedded `RUNTIME_SCHEMA_11` table doesn't
//! cover: a from-scratch compile has nothing to reconcile against, but a
//! deployment package from any prior build of the *same* Mendix version
//! carries field shapes that apply project-wide, not just to that build's
//! own model.

use std::path::Path;

use mxrs_bson::Document;

#[derive(Debug, thiserror::Error)]
pub enum ModelPackageError {
    #[error("cannot read model package {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("model.mdp: truncated BSON length at byte {0}")]
    Truncated(usize),
    #[error("model.mdp: invalid BSON size {size} at byte {offset}")]
    InvalidSize { offset: usize, size: i32 },
    #[error("model.mdp: invalid BSON at byte {offset}: {source}")]
    Bson {
        offset: usize,
        #[source]
        source: mxrs_bson::BsonCodecError,
    },
}

/// Reads every document in a `model.mdp` deployment model package.
pub fn read_model_package(path: impl AsRef<Path>) -> Result<Vec<Document>, ModelPackageError> {
    let path_ref = path.as_ref();
    let bytes = std::fs::read(path_ref).map_err(|source| ModelPackageError::Io {
        path: path_ref.display().to_string(),
        source,
    })?;
    parse_package(&bytes)
}

fn parse_package(bytes: &[u8]) -> Result<Vec<Document>, ModelPackageError> {
    let mut documents = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes.len() - offset < 4 {
            return Err(ModelPackageError::Truncated(offset));
        }
        let size = i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        if size < 5 || size as usize > bytes.len() - offset {
            return Err(ModelPackageError::InvalidSize { offset, size });
        }
        let end = offset + size as usize;
        let document = mxrs_bson::parse(&bytes[offset..end])
            .map_err(|source| ModelPackageError::Bson { offset, source })?;
        documents.push(document);
        offset = end;
    }
    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_two_document_stream() {
        let one = mxrs_bson::doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "A",
        };
        let two = mxrs_bson::doc! {
            "$ID": "22222222-2222-4222-8222-222222222222",
            "$Type": "B",
        };
        let mut bytes = mxrs_bson::serialize(&one).unwrap();
        bytes.extend(mxrs_bson::serialize(&two).unwrap());
        let documents = parse_package(&bytes).unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].get_str("$Type").unwrap(), "A");
        assert_eq!(documents[1].get_str("$Type").unwrap(), "B");
    }

    #[test]
    fn rejects_a_truncated_length_prefix() {
        assert!(matches!(
            parse_package(&[1, 2, 3]),
            Err(ModelPackageError::Truncated(0))
        ));
    }

    #[test]
    fn rejects_an_out_of_range_size() {
        assert!(matches!(
            parse_package(&(-1i32).to_le_bytes()),
            Err(ModelPackageError::InvalidSize {
                offset: 0,
                size: -1
            })
        ));
    }
}
