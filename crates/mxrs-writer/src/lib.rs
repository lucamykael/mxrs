//! Persists an `mxrs_ir::ProjectDecl` (built via `mxrs-dsl`) into a fresh
//! `.mpr`. Ports the fresh-project-creation slice of `writer.rb`, scoped to
//! Phase 3's domain-model + microflow subset — see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory.
//!
//! **Not ported** (deliberately, for this pass): `writer.rb`'s incremental
//! `synchronize_ruby_*!` re-sync machinery (rewriting an *existing* project
//! while preserving unrelated content/GUIDs across regenerations — this
//! crate only creates brand-new projects); pages/widgets, menus,
//! enumerations/constants, and every other document type `Module` supports
//! beyond microflows; DSL-level customization of security/navigation
//! content (`scaffold` writes sane, empty defaults for both, matching what
//! `mxrb generate` produces for a brand-new project, but there's no DSL
//! surface yet to configure user roles, guest access, or navigation
//! profiles). Cross-module associations and the default `ProjectSettings`/
//! `Security$ProjectSecurity`/`Navigation$NavigationDocument` scaffold
//! *are* ported (see `domain` and `scaffold` module docs).

pub mod domain;
pub mod error;
pub mod flow_compiler;
pub mod module;
pub mod project;
pub mod scaffold;

pub use error::{Result, WriterError};
pub use project::write_project;
