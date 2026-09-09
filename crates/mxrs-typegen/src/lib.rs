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
//! hand, for now — `mxrs-dsl` is not yet wired to require the generated
//! `Ref<M>` markers in place of raw strings (see `mxrs_ir::markers`' doc
//! comment for the follow-up that closes this gap). Until then, this
//! crate's value is exactly what Phase 4's done-definition asks for: a
//! nonexistent/renamed reference against the generated markers themselves
//! fails to compile — proven by this crate's `trybuild` test.
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

pub use manifest::{EntityManifest, Manifest, ModuleManifest};

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
