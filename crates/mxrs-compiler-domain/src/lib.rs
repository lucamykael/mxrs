//! Phase 5 (first slice, per the phased roadmap in
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory):
//! compiles an `mxrs-model` domain model into Mendix Runtime shape. See
//! `domain`'s module doc for the compiler itself and its known gaps.

mod domain;
mod security;
mod support;

pub use domain::DomainCompiler;
pub use security::SecurityCompiler;

#[derive(Debug, thiserror::Error)]
pub enum CompilerError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),

    /// `Entity::oql_view()` returned true — resolving an OQL view entity's
    /// backing `ViewEntitySourceDocument` needs a project-wide document
    /// lookup this first pass doesn't build yet. See `domain`'s module doc.
    #[error("entity {0:?} is an OQL view entity, which this compiler pass doesn't support yet")]
    UnsupportedOqlViewEntity(String),
}
