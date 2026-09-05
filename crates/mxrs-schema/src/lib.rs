//! Mendix version/schema registry, scoped to 11.x per the locked MVP
//! decision — see `decisions/mxrs-rust-rewrite-plan.md` (milestone M1.6)
//! in this project's ai-memory. Widening to the full 5.21-11.12 matrix
//! mxrb supports means adding version entries to each module here, not
//! restructuring them.
//!
//! Ports `lib/mxrb/schema/tables.rb`, `lib/mxrb/studio_compatibility.rb`,
//! and `lib/mxrb/compiler/schemas/{runtime-11.json,system-model-11.12.1.b64}`
//! from mxrb.

pub mod assets;
pub mod compatibility;
pub mod tables;

pub use assets::{RUNTIME_SCHEMA_11, SYSTEM_MODEL_SEED_11_12_1};
pub use compatibility::{apply_document, schema_hash, SCHEMA_HASH_11_12_1};
pub use tables::{table_info, TableInfo, ATTRIBUTE_TYPES, TABLES, UNIT_TYPES};
