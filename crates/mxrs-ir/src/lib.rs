//! Storage-independent declaration types shared by `mxrs-dsl` (which
//! constructs them from a user-authored Rust program) and `mxrs-writer`
//! (which compiles them into `.mpr` BSON). See `decisions/mxrs-rust-rewrite-plan.md`
//! in this project's ai-memory for the surrounding Phase 3 plan.
//!
//! Deviation from the original crate table worth flagging: the plan places
//! `mxrs-ir` upstream of `mxrs-model` (depending only on `mxrs-schema` +
//! `mxrs-fragment-store`). In practice `mxrs-model`'s `Entity`/`Attribute`
//! are already both-directions BSON structs (`from_bson`/`to_bson`), so this
//! crate reuses them directly for entity/attribute declarations instead of
//! duplicating near-identical types — only associations and microflow bodies
//! get their own declaration shape here, since those need name-based
//! resolution (entity/microflow references, not yet-assigned UUIDs) that
//! `mxrs-model`'s storage-oriented structs don't carry.

pub mod declaration;
pub mod flow;

pub use declaration::{AssociationDecl, EntityDecl, ModuleDecl, ProjectDecl};
pub use flow::{Activity, Member, MicroflowCallMapping, MicroflowDecl};
