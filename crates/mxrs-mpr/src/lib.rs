//! Low-level Mendix `.mpr` SQLite/`.mxunit` unit I/O.
//!
//! Ports `lib/mxrb/io/mpr_file.rb` and `lib/mxrb/io/mxunit_codec.rb` from
//! mxrb (Ruby). Targets the v2 (`.mxunit` content-addressed file) storage
//! format used by Studio Pro 10+ — see `decisions/mxrs-rust-rewrite-plan.md`
//! (milestones M1.2-M1.4) in this project's ai-memory for the locked MVP
//! scope and what's deliberately deferred (v1 storage, migration between
//! formats, sidecar tables owned by other future crates).

pub mod error;
pub mod format;
pub mod mpr_file;
pub mod mxunit;
pub mod transaction;

pub use error::{MprError, Result};
pub use format::StorageFormat;
pub use mpr_file::{MprFile, RawUnit, SqlCell, SqlResult, WriteStats};
