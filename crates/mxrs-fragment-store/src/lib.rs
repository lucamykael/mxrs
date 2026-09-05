//! Content-addressed store for BSON sub-documents that have no typed model
//! yet, achieving losslessness for exotic/unmodeled Mendix constructs
//! without writing new codec code for each one.
//!
//! Ports `Mxrb::NativeFragmentStore` from `lib/mxrb/native_fragment_store.rb`
//! (milestone M1.5 in `decisions/mxrs-rust-rewrite-plan.md`) — see
//! `store.rs` for what was deliberately left out and why.

pub mod error;
pub mod store;

pub use error::{FragmentStoreError, Result};
pub use store::{FetchOptions, NativeFragmentStore};
