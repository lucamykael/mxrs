//! Parses the embedded per-version fresh-project bootstrap template into
//! ready-to-insert BSON documents. Ports the asset-loading half of
//! `Writer#apply_default_project_units` + `#load_native_units` from
//! `lib/mxrb/writer.rb`, narrowed to the fresh-project case: this template's
//! three units (`Settings$ProjectSettings`, `Texts$SystemTextCollection`,
//! `Projects$ProjectConversion`) are all flat children of the project root
//! in the donor project, so no container/GUID tree remap is needed — a
//! caller inserting them into a newly created `.mpr` just needs a fresh
//! unit id per document and the project root as the container.
//!
//! Sanitizing `Settings$ProjectSettings` (stripping donor-project lifecycle
//! callbacks etc. — `Writer#sanitize_project_settings!`) is left to the
//! caller: it's a fresh-project-authoring policy, not an asset-loading
//! concern, and belongs alongside `mxrs-writer`'s other scaffolding.

use base64::Engine;
use mxrs_bson::Document;

use crate::assets;

#[derive(Debug, Clone)]
pub struct TemplateUnit {
    pub containment: String,
    pub doc: Document,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectTemplateError {
    #[error("no fresh-project template embedded for Mendix version {0:?}")]
    UnsupportedVersion(String),
    #[error("project template JSON malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("project template unit {0:?} has non-base64 contents")]
    Base64(String),
    #[error("project template unit {0:?} has invalid BSON contents: {1}")]
    Bson(String, mxrs_bson::BsonCodecError),
}

/// Returns the fresh-project template units for `version`, or
/// `ProjectTemplateError::UnsupportedVersion` outside the locked MVP scope
/// (11.12.1 only, matching `schema_hash`/`apply_document`).
pub fn project_template_units(version: &str) -> Result<Vec<TemplateUnit>, ProjectTemplateError> {
    let raw = match version {
        "11.12.1" => assets::PROJECT_TEMPLATE_11_12_1,
        _ => {
            return Err(ProjectTemplateError::UnsupportedVersion(
                version.to_string(),
            ))
        }
    };

    let manifest: serde_json::Value = serde_json::from_str(raw)?;
    let units = manifest
        .get("units")
        .and_then(|u| u.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::with_capacity(units.len());
    for unit in units {
        let name = unit
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>")
            .to_string();
        let containment = unit
            .get("containment")
            .and_then(|v| v.as_str())
            .unwrap_or("ProjectDocuments")
            .to_string();
        let contents_b64 = unit
            .get("contents")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProjectTemplateError::Base64(name.clone()))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(contents_b64)
            .map_err(|_| ProjectTemplateError::Base64(name.clone()))?;
        let doc =
            mxrs_bson::parse(&bytes).map_err(|e| ProjectTemplateError::Bson(name.clone(), e))?;
        out.push(TemplateUnit { containment, doc });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_three_11_12_1_template_units() {
        let units = project_template_units("11.12.1").unwrap();
        let types: Vec<&str> = units
            .iter()
            .map(|u| u.doc.get_str("$Type").unwrap())
            .collect();
        assert!(types.contains(&"Settings$ProjectSettings"));
        assert!(types.contains(&"Texts$SystemTextCollection"));
        assert!(types.contains(&"Projects$ProjectConversion"));
        assert!(units
            .iter()
            .all(|u| u.containment == "ProjectDocuments" || u.containment == "ProjectConversion"));
    }

    #[test]
    fn unsupported_version_errors_instead_of_panicking() {
        let err = project_template_units("7.5.0").unwrap_err();
        assert!(matches!(err, ProjectTemplateError::UnsupportedVersion(v) if v == "7.5.0"));
    }
}
