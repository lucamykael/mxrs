//! Typed, storage-independent representation of Mendix project settings.
//!
//! Ports `lib/mxrb/settings/{model,mpr_codec,value_contracts}.rb` from mxrb.
//! Deliberately **not** ported: `SourceEmitter` (renders settings as Ruby
//! DSL source text — mxrs has no Ruby DSL to target) and the
//! `CollectionBuilder`/`NodeBuilder`/`ProjectBuilder` `method_missing`-based
//! Ruby block builders (that's `mxrs-dsl`'s job in Phase 3, with a
//! from-scratch Rust API shape, not ported Ruby block syntax) — see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory.

pub mod catalog;
pub mod error;
pub mod mpr_codec;
pub mod node;
pub mod value;
pub mod value_contracts;

pub use error::{Result, SettingsError};
pub use mpr_codec::{decode, encode};
pub use node::Node;
pub use value::{BinaryAsset, Collection, Value};
