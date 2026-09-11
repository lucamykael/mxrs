//! Mendix-specific BSON codec conventions:
//!
//! - UUIDs stored as 16-byte blobs with partial little-endian byte-swap
//!   (MS-GUID format) — see [`guid`].
//! - `$ID` can appear as a string, a BSON binary, or a `{Data, Subtype}`/
//!   extended-JSON binary map — see [`id`].
//! - Arrays carry a 4-byte int32 marker as their first element
//!   (1 = by-name, 2 = part-secondary, 3 = part-primary) — see [`array`].
//! - `$Type` uses storage names (`"DomainModels$Entity"`), not SDK
//!   qualified names — see [`containment`].
//!
//! Ports `lib/mxrb/io/bson_codec.rb` from mxrb (Ruby). See
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory,
//! milestone M1.1, for the surrounding phased plan.

pub mod array;
pub mod containment;
pub mod document;
pub mod error;
pub mod extjson;
pub mod guid;
pub mod hash;
pub mod id;

pub use array::{ParsedArray, build_array, parse_array};
pub use containment::containment_to_type;
pub use document::{BINARY_UUID_KEYS, parse, serialize, storage_hash, storage_value};
pub use error::{BsonCodecError, Result};
pub use extjson::{Restored, restore_extended_json};
pub use guid::{blob_to_uuid, looks_like_uuid, uuid_to_blob};
pub use hash::contents_hash;
pub use id::extract_id;

// Re-export the underlying BSON types so downstream crates (mxrs-mpr,
// mxrs-model, ...) depend on a single, pinned `bson` version through this
// crate rather than declaring their own.
pub use bson::{Binary, Bson, DateTime, Document, doc, spec::BinarySubtype};
