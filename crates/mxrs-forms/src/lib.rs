//! Typed, immutable view of the canonical Mendix Forms metamodel (pages,
//! widgets, actions), and a lossless BSON codec for it.
//!
//! Ports `lib/mxrb/forms/{catalog,values,node,storage_naming,mpr_codec}.rb`
//! from mxrb. Deliberately **not** ported: `SourceEmitter` (Ruby DSL
//! codegen — mxrs has no Ruby DSL to target), `coverage.rb` (a dev-only
//! catalog-coverage report for mxrb's own test suite), the
//! `method_missing`-based Ruby ergonomics on `Node` (`mxrs-dsl`'s job in
//! Phase 3, from-scratch Rust API), and `CustomWidgets$CustomWidget`
//! (pluggable-widget) support (`mxrs-pluggable` is deferred post-MVP) —
//! see `mpr_codec.rs` and `decisions/mxrs-rust-rewrite-plan.md` in this
//! project's ai-memory for the full rationale.

pub mod catalog;
pub mod error;
pub mod mpr_codec;
pub mod node;
pub mod storage_naming;
pub mod values;

pub use catalog::{Catalog, Property, Type};
pub use error::{FormsError, Result};
pub use mpr_codec::MprCodec;
pub use node::{Assignment, Node, Value};
