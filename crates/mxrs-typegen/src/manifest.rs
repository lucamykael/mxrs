//! The JSON schema manifest `mxrs-typegen` generates marker types from —
//! the single source of truth a user's own `build.rs` reads. Deliberately a
//! separate, hand-authored (or scripted) file rather than something scanned
//! out of Rust source: `mxrs-dsl`'s builder calls aren't statically
//! analyzable before the crate that calls them has compiled, and this
//! manifest is what breaks that chicken-and-egg problem (see the crate's
//! top-level doc comment for the alternatives this was weighed against).
//!
//! Narrow on purpose: only what's needed to generate a distinct marker type
//! per module/entity/attribute (names) — no attribute types, association
//! declarations, or microflow bodies yet. Widen incrementally as `mxrs-dsl`
//! grows a typed surface that actually consumes more of it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::TypegenError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub modules: Vec<ModuleManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub name: String,
    #[serde(default)]
    pub entities: Vec<EntityManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityManifest {
    pub name: String,
    #[serde(default)]
    pub attributes: Vec<String>,
}

impl Manifest {
    pub fn from_json(json: &str) -> Result<Self, TypegenError> {
        Ok(serde_json::from_str(json)?)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, TypegenError> {
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json)
    }
}
