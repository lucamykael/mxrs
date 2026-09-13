//! The JSON schema manifest `mxrs-typegen` generates marker types from —
//! the single source of truth a user's own `build.rs` reads. Deliberately a
//! separate, hand-authored (or scripted) file rather than something scanned
//! out of Rust source: `mxrs-dsl`'s builder calls aren't statically
//! analyzable before the crate that calls them has compiled, and this
//! manifest is what breaks that chicken-and-egg problem (see the crate's
//! top-level doc comment for the alternatives this was weighed against).
//!
//! Narrow on purpose: only what's needed to generate a distinct marker type
//! per module/entity/attribute/association/microflow/nanoflow (names, plus an
//! association's target and type) — no attribute types or microflow bodies
//! yet (a microflow's manifest entry is just its name: `MicroflowMarker`
//! only needs to prove a name exists, not describe what the microflow does).
//! Widen incrementally as `mxrs-dsl` grows a typed surface that actually
//! consumes more of it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::TypegenError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub modules: Vec<ModuleManifest>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub name: String,
    #[serde(default)]
    pub entities: Vec<EntityManifest>,
    /// Microflow names owned by this module. Each generates a marker struct
    /// implementing `mxrs_ir::MicroflowMarker`, sharing the module's flat
    /// item namespace with its entities (a microflow named the same as an
    /// entity in the same module is a [`crate::TypegenError::ModuleItemNameCollision`]).
    #[serde(default)]
    pub microflows: Vec<String>,
    /// Nanoflow names owned by this module. These share the module item
    /// namespace with entities and microflows and implement
    /// `mxrs_ir::NanoflowMarker`.
    #[serde(default)]
    pub nanoflows: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityManifest {
    pub name: String,
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(default)]
    pub associations: Vec<AssociationManifest>,
}

/// `target` is `"Entity"` for a same-module target or `"Module.Entity"` for
/// cross-module — same convention `mxrs_ir::Ref<M>::qualified_name()`
/// produces at the `mxrs-dsl` layer. `association_type` is `"Reference"` or
/// `"ReferenceSet"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssociationManifest {
    pub name: String,
    pub target: String,
    #[serde(rename = "type")]
    pub association_type: String,
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
