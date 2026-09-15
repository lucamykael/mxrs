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
//! creation and incremental resync). Entity declarations cover inheritance,
//! system members, indexes, access rules, lifecycle callbacks, validation
//! rules, images, and stored/OQL view sources with identity-preserving
//! reconciliation. The microflow slice of
//! `synchronize_ruby_documents!` — upserting a `Documents` unit by name
//! instead of always inserting a new one — is ported too, in
//! `documents::synchronize_microflows` and `synchronize_nanoflows`
//! (upsert-only, unlike the domain model: a flow absent from a given call isn't deleted, matching
//! mxrb's own `write_documents`). Cargo-native enumerations are also written
//! and incrementally synchronized, including values, localized captions, and
//! stable identity preservation. Native/structural pages are written and
//! incrementally synchronized too, via `page_compiler` — see `mxrs_ir::page`'s
//! doc comment for exactly which pages/widgets that covers (containers/text/
//! buttons, data-bound native widgets, flow actions, and selected pluggable
//! widgets). Project/module security and modern navigation profiles now have
//! typed DSL/IR surfaces and loss-preserving incremental writers, including
//! reference validation and stable nested identities. **Not ported at all**:
//! menus as standalone module documents, constants, demo-user authoring, and
//! every other document type `Module` supports beyond flows and pages.
//! Cross-module associations
//! and the default `ProjectSettings`/`Security$ProjectSecurity`/
//! `Navigation$NavigationDocument` scaffold *are* ported (see `domain` and
//! `scaffold` module docs).

pub mod documents;
pub mod domain;
pub mod error;
pub mod flow_compiler;
mod flow_contract;
pub mod flow_graph;
mod flow_parameters;
pub mod instrumentation;
pub mod layout_compiler;
pub mod module;
mod navigation;
pub mod page_compiler;
pub mod project;
pub mod scaffold;
mod security;

pub use error::{Result, WriterError};
pub use instrumentation::{InstrumentationReport, instrument_functional_tests};
pub use project::{synchronize_project, synchronize_project_documents, write_project};
