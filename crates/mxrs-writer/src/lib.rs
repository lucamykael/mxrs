//! Persists an `mxrs_ir::ProjectDecl` (built via `mxrs-dsl`) into a fresh
//! `.mpr`. Ports the fresh-project-creation slice of `writer.rb`, scoped to
//! Phase 3's domain-model + microflow subset — see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory.
//!
//! **Partially ported**: `writer.rb`'s incremental `synchronize_ruby_*!`
//! re-sync machinery (rewriting an *existing* project while preserving
//! unrelated content/GUIDs across regenerations) now covers the full
//! domain model — entities (add/rename-by-declaring-a-new-name/remove,
//! attribute add/remove with `$ID` preservation on a name match) and
//! associations — via `domain::synchronize_domain_model`, mirroring mxrb's
//! own `write_domain_model` (the method mxrb itself reuses for both fresh
//! creation and incremental resync). Narrower than mxrb's full entity
//! surface: indexes, access rules, lifecycle callbacks, validation rules,
//! and generalization targets have no `EntityDecl` DSL surface yet, so an
//! existing entity's values for those survive untouched rather than being
//! reconciled (see `domain`'s module doc). The microflow slice of
//! `synchronize_ruby_documents!` — upserting a `Documents` unit by name
//! instead of always inserting a new one — is ported too, in
//! `documents::synchronize_microflows` (upsert-only, unlike the domain
//! model: a microflow absent from a given call isn't deleted, matching
//! mxrb's own `write_documents`). **Not ported at all**: pages/widgets,
//! menus, enumerations/constants, and every
//! other document type `Module` supports beyond microflows; DSL-level
//! customization of security/navigation content (`scaffold` writes sane,
//! empty defaults for both, matching what `mxrb generate` produces for a
//! brand-new project, but there's no DSL surface yet to configure user
//! roles, guest access, or navigation profiles). Cross-module associations
//! and the default `ProjectSettings`/`Security$ProjectSecurity`/
//! `Navigation$NavigationDocument` scaffold *are* ported (see `domain` and
//! `scaffold` module docs).

pub mod documents;
pub mod domain;
pub mod error;
pub mod flow_compiler;
pub mod module;
pub mod project;
pub mod scaffold;

pub use error::{Result, WriterError};
pub use project::{synchronize_project, write_project};
