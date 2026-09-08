//! Persists an `mxrs_ir::ProjectDecl` (built via `mxrs-dsl`) into a fresh
//! `.mpr`. Ports the fresh-project-creation slice of `writer.rb`, scoped to
//! Phase 3's domain-model + microflow subset — see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory.
//!
//! **Not ported** (deliberately, for this pass): `writer.rb`'s incremental
//! `synchronize_ruby_*!` re-sync machinery (rewriting an *existing* project
//! while preserving unrelated content/GUIDs across regenerations — this
//! crate only creates brand-new projects); `ProjectSettings`/
//! `Security$ProjectSecurity`/`Navigation$NavigationDocument`/default
//! application layout scaffolding (mxrb's `create_project!` writes these,
//! but they're a real Studio-Pro-openability requirement this pass doesn't
//! yet meet — flagged as a known gap, not silently glossed over); pages/
//! widgets, menus, enumerations/constants, security, and every other
//! document type `Module` supports beyond microflows; cross-module
//! associations (see `domain` module doc).

pub mod domain;
pub mod error;
pub mod flow_compiler;
pub mod module;
pub mod project;

pub use error::{Result, WriterError};
pub use project::write_project;
