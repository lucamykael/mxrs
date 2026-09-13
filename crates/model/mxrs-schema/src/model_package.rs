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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mxrs_bson::{Document, extract_id};

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
    #[error("cannot serialize model.mdp document: {0}")]
    Serialize(#[source] mxrs_bson::BsonCodecError),
    #[error("model document has no $ID")]
    MissingId,
    #[error("model document {0} not found")]
    DocumentNotFound(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelPackageEntry {
    pub offset: usize,
    pub size: usize,
    pub document: Document,
}

impl ModelPackageEntry {
    pub fn id(&self) -> Option<String> {
        self.document.get("$ID").and_then(extract_id)
    }

    pub fn type_name(&self) -> &str {
        self.document.get_str("$Type").unwrap_or_default()
    }
}

/// Ordered, editable representation of a Runtime `model.mdp` stream.
///
/// Updates preserve document order; a new document is appended. Every
/// mutation returns a reindexed package, and [`Self::write`] replaces the
/// destination atomically through a temporary sibling file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelPackage {
    entries: Vec<ModelPackageEntry>,
}

impl ModelPackage {
    pub fn read(path: impl AsRef<Path>) -> Result<Self, ModelPackageError> {
        let path_ref = path.as_ref();
        let bytes = std::fs::read(path_ref).map_err(|source| ModelPackageError::Io {
            path: path_ref.display().to_string(),
            source,
        })?;
        Self::decode(&bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ModelPackageError> {
        Ok(Self {
            entries: parse_package(bytes)?,
        })
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_documents(
        documents: impl IntoIterator<Item = Document>,
    ) -> Result<Self, ModelPackageError> {
        Self::reindex(documents.into_iter().collect())
    }

    pub fn entries(&self) -> &[ModelPackageEntry] {
        &self.entries
    }

    pub fn documents(&self) -> impl ExactSizeIterator<Item = &Document> {
        self.entries.iter().map(|entry| &entry.document)
    }

    pub fn into_documents(self) -> Vec<Document> {
        self.entries
            .into_iter()
            .map(|entry| entry.document)
            .collect()
    }

    pub fn types(&self) -> BTreeMap<String, usize> {
        let mut types = BTreeMap::new();
        for entry in &self.entries {
            *types.entry(entry.type_name().to_string()).or_default() += 1;
        }
        types
    }

    pub fn find(&self, id: &str) -> Option<&ModelPackageEntry> {
        self.entries
            .iter()
            .find(|entry| entry.id().as_deref() == Some(id))
    }

    pub fn replace(&self, id: &str, document: Document) -> Result<Self, ModelPackageError> {
        let Some(position) = self
            .entries
            .iter()
            .position(|entry| entry.id().as_deref() == Some(id))
        else {
            return Err(ModelPackageError::DocumentNotFound(id.to_string()));
        };
        let mut documents = self.documents().cloned().collect::<Vec<_>>();
        documents[position] = document;
        Self::reindex(documents)
    }

    pub fn upsert(&self, document: Document) -> Result<Self, ModelPackageError> {
        let id = document
            .get("$ID")
            .and_then(extract_id)
            .ok_or(ModelPackageError::MissingId)?;
        if self.find(&id).is_some() {
            return self.replace(&id, document);
        }
        self.with_appended(document)
    }

    pub fn with_appended(&self, document: Document) -> Result<Self, ModelPackageError> {
        let mut documents = self.documents().cloned().collect::<Vec<_>>();
        documents.push(document);
        Self::reindex(documents)
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<PathBuf, ModelPackageError> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|source| ModelPackageError::Io {
            path: parent.display().to_string(),
            source,
        })?;
        let mut bytes = Vec::new();
        for entry in &self.entries {
            bytes.extend(
                mxrs_bson::serialize(&entry.document).map_err(ModelPackageError::Serialize)?,
            );
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("model.mdp");
        let temporary = parent.join(format!(
            ".{name}.tmp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&temporary, bytes).map_err(|source| ModelPackageError::Io {
            path: temporary.display().to_string(),
            source,
        })?;
        if let Err(source) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(ModelPackageError::Io {
                path: path.display().to_string(),
                source,
            });
        }
        Ok(path.to_path_buf())
    }

    fn reindex(documents: Vec<Document>) -> Result<Self, ModelPackageError> {
        let mut offset = 0usize;
        let mut entries = Vec::with_capacity(documents.len());
        for document in documents {
            let size = mxrs_bson::serialize(&document)
                .map_err(ModelPackageError::Serialize)?
                .len();
            entries.push(ModelPackageEntry {
                offset,
                size,
                document,
            });
            offset += size;
        }
        Ok(Self { entries })
    }
}

/// Reads every document in a `model.mdp` deployment model package.
pub fn read_model_package(path: impl AsRef<Path>) -> Result<Vec<Document>, ModelPackageError> {
    Ok(ModelPackage::read(path)?.into_documents())
}

fn parse_package(bytes: &[u8]) -> Result<Vec<ModelPackageEntry>, ModelPackageError> {
    let mut entries = Vec::new();
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
        entries.push(ModelPackageEntry {
            offset,
            size: size as usize,
            document,
        });
        offset = end;
    }
    Ok(entries)
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
        let entries = parse_package(&bytes).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].document.get_str("$Type").unwrap(), "A");
        assert_eq!(entries[1].document.get_str("$Type").unwrap(), "B");
        assert_eq!(entries[0].offset, 0);
        assert_eq!(entries[1].offset, entries[0].size);
    }

    #[test]
    fn replaces_upserts_inventories_and_writes_atomically() {
        let first_id = "11111111-1111-4111-8111-111111111111";
        let second_id = "22222222-2222-4222-8222-222222222222";
        let first = mxrs_bson::doc! {
            "$ID": first_id,
            "$Type": "Projects$Project",
            "Name": "App",
        };
        let second = mxrs_bson::doc! {
            "$ID": second_id,
            "$Type": "Security$ProjectSecurity",
        };
        let package = ModelPackage::from_documents([first, second]).unwrap();
        assert_eq!(package.entries()[1].offset, package.entries()[0].size);
        assert_eq!(package.types().get("Projects$Project"), Some(&1));
        assert_eq!(
            package.find(first_id).unwrap().type_name(),
            "Projects$Project"
        );
        assert!(package.find("missing").is_none());

        let replaced = package
            .upsert(mxrs_bson::doc! {
                "$ID": first_id,
                "$Type": "Projects$Project",
                "Name": "Updated",
            })
            .unwrap();
        assert_eq!(replaced.entries().len(), 2);
        assert_eq!(
            replaced
                .find(first_id)
                .unwrap()
                .document
                .get_str("Name")
                .unwrap(),
            "Updated"
        );

        let third_id = "33333333-3333-4333-8333-333333333333";
        let inserted = replaced
            .upsert(mxrs_bson::doc! { "$ID": third_id, "$Type": "Constants$Constant" })
            .unwrap();
        assert_eq!(inserted.entries().len(), 3);
        assert!(inserted.entries().last().unwrap().offset > 0);

        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("nested/model.mdp");
        assert_eq!(inserted.write(&output).unwrap(), output);
        assert_eq!(ModelPackage::read(&output).unwrap(), inserted);
        let siblings = std::fs::read_dir(output.parent().unwrap())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(siblings.len(), 1, "temporary sibling must be removed");
    }

    #[test]
    fn rejects_missing_ids_and_unknown_replacements() {
        let package = ModelPackage::empty();
        assert!(matches!(
            package.upsert(mxrs_bson::doc! { "$Type": "MissingId" }),
            Err(ModelPackageError::MissingId)
        ));
        assert!(matches!(
            package.replace("missing", Document::new()),
            Err(ModelPackageError::DocumentNotFound(id)) if id == "missing"
        ));
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
