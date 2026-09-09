//! Phase 5 domain compiler, per the phased roadmap in
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory):
//! compiles an `mxrs-model` domain model into Mendix Runtime shape. See
//! `domain`'s module doc for the compiler itself.

mod domain;
mod security;
mod support;

pub use domain::DomainCompiler;
pub use security::SecurityCompiler;

#[derive(Debug, thiserror::Error)]
pub enum CompilerError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),

    #[error("OQL view entity {entity:?} references missing source document {source_name:?}")]
    MissingOqlViewSource { entity: String, source_name: String },
}
