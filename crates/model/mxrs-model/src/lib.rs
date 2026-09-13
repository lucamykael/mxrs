//! The `Project`/`Module`/`Entity`/`Page`/`Microflow` model graph tying
//! `mxrs-mpr`, `mxrs-forms` and `mxrs-settings` together. Ports
//! `lib/mxrb/model/{project,unit}.rb` and subclasses from mxrb — see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory
//! (Phase 2) for the surrounding plan and per-module scope notes.
//!
//! Deliberately not ported here (out of this crate's scope, per the plan):
//! `DesignSystem`/`DesignMigration`/`DesignMaterializer` (design-token asset
//! scanning, a separate feature not part of the module graph), and the
//! semantic-index/OQL/migration/mutation surface on `Project` (later
//! phases' `mxrs-semantic`/`mxrs-oql`/writer own that).

pub mod association;
pub mod attribute;
pub mod domain_model;
pub mod entity;
pub mod error;
pub mod menu;
pub mod microflow;
pub mod module;
pub mod navigation;
pub mod page;
pub mod project;
mod support;

pub use association::Association;
pub use attribute::{Attribute, AttributeType};
pub use domain_model::DomainModel;
pub use entity::Entity;
pub use error::{ModelError, Result};
pub use menu::{Menu, MenuItem};
pub use microflow::Microflow;
pub use module::Module;
pub use navigation::Navigation;
pub use page::{Page, Widget};
pub use project::Project;
