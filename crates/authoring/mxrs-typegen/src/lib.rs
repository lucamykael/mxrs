//! Phase 4 (per the phased roadmap in `decisions/mxrs-rust-rewrite-plan.md`
//! in this project's ai-memory): build.rs codegen of per-module/entity/
//! attribute marker types from a JSON schema manifest, subsuming the
//! unresolved-reference slice of mxrb's `architecture/validator.rb` +
//! `semantic/analyzer.rb#unresolved_references` — a nonexistent or renamed
//! reference against generated markers fails `cargo build`, not a runtime
//! `mxrb evaluate` lint pass.
//!
//! # Why a manifest, not scanning `mxrs-dsl` calls
//!
//! `mxrs-dsl`'s builder (`ProjectBuilder::new(...).module("Sales", |m| {...})`)
//! *is* the schema — there's no separate declaration to scan, and scanning
//! Rust source for calls to a specific builder API is fragile (macros,
//! conditional compilation, values built in a loop, ...) even before
//! considering that the crate containing those calls hasn't compiled yet
//! when `build.rs` runs. A hand-authored (or scripted) JSON manifest sidesteps
//! that chicken-and-egg problem entirely: it's the one thing guaranteed to
//! exist *before* the crate using its generated markers compiles.
//!
//! This does mean the manifest and the `mxrs-dsl` builder calls that
//! actually persist the project are two things a user keeps in sync by
//! hand. `mxrs-dsl` *is* wired to require the generated `Ref<M>` markers in
//! place of raw strings for association targets (`EntityBuilder::association`
//! — see `mxrs_ir::markers`' doc comment), so a nonexistent/renamed entity
//! reference fails `cargo build` end to end, not just against the generated
//! markers in isolation — proven by this crate's `tests/reference_checking.rs`
//! (real throwaway `cargo build` invocations, not `trybuild` `.stderr`
//! snapshots — see that file's doc comment for why) and by
//! `mxrs-macros`' `tests/typegen_integration.rs`, which exercises the full
//! typegen → `project! {}` → `mxrs-writer` pipeline.
//!
//! Associations are also in the manifest (an entity's `associations` list:
//! `name`/`target`/`type`), each generating an `AssociationMarker` impl —
//! but `mxrs-dsl` does *not* require one of these for
//! `EntityBuilder::association`'s own `name`/`association_type` parameters
//! (only the target is marker-checked, via `EntityMarker`/`Ref<M>`); see
//! `mxrs_ir::markers::AssociationMarker`'s doc comment for why that's a
//! deliberately narrower wiring than the entity/attribute case.
//!
//! # Usage (in a downstream project's own `build.rs`)
//!
//! ```no_run
//! # #![allow(clippy::needless_doctest_main)]
//! fn main() {
//!     let manifest_path = "schema.json";
//!     println!("cargo:rerun-if-changed={manifest_path}");
//!     let out_dir = std::env::var("OUT_DIR").unwrap();
//!     let out_path = std::path::Path::new(&out_dir).join("mxrs_markers.rs");
//!     mxrs_typegen::generate_from_path(manifest_path, &out_path).unwrap();
//! }
//! ```
//! then, in the crate itself: `include!(concat!(env!("OUT_DIR"), "/mxrs_markers.rs"));`

pub mod codegen;
pub mod manifest;

use std::path::Path;

pub use manifest::{AssociationManifest, EntityManifest, Manifest, ModuleManifest};

#[derive(Debug, thiserror::Error)]
pub enum TypegenError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid manifest JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{1} {0:?} is not usable as a Rust identifier")]
    InvalidIdentifier(String, String),

    #[error("duplicate module name {0:?} in manifest")]
    DuplicateModule(String),

    #[error("duplicate entity name {0}.{1:?} in manifest")]
    DuplicateEntity(String, String),

    #[error("duplicate attribute name {0}.{1}.{2:?} in manifest")]
    DuplicateAttribute(String, String, String),

    #[error("duplicate association name {0}.{1}.{2:?} in manifest")]
    DuplicateAssociation(String, String, String),

    #[error("duplicate microflow name {0}.{1:?} in manifest")]
    DuplicateMicroflow(String, String),

    #[error("duplicate nanoflow name {0}.{1:?} in manifest")]
    DuplicateNanoflow(String, String),

    #[error(
        "module {0} has multiple entity/flow items named {1:?} — the generated marker names would collide"
    )]
    ModuleItemNameCollision(String, String),

    #[error(
        "association {0}.{1}.{2:?} has unknown type {3:?} (expected \"Reference\" or \"ReferenceSet\")"
    )]
    UnknownAssociationType(String, String, String, String),

    #[error("association {0}.{1}.{2:?} targets {3:?}, which no manifest entity declares")]
    UnknownAssociationTarget(String, String, String, String),

    #[error(
        "entity {0}.{1:?} has both an attribute and an association named {2:?} — the generated marker names would collide"
    )]
    MarkerNameCollision(String, String, String),
}

/// Renders `manifest` into Rust marker-type source (see [`codegen::generate`]).
pub fn generate(manifest: &Manifest) -> Result<String, TypegenError> {
    codegen::generate(manifest)
}

/// Reads a JSON manifest from `manifest_path`, generates marker-type Rust
/// source, and writes it to `out_path` — the convenience entry point a
/// downstream `build.rs` calls. Does not itself print
/// `cargo:rerun-if-changed` (a `build.rs` concern, not this library's).
pub fn generate_from_path(
    manifest_path: impl AsRef<Path>,
    out_path: impl AsRef<Path>,
) -> Result<(), TypegenError> {
    let manifest = Manifest::from_path(manifest_path)?;
    let source = generate(&manifest)?;
    std::fs::write(out_path, source)?;
    Ok(())
}
