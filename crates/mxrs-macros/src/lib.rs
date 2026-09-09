//! Phase 8 (M8.1, per the phased roadmap in
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory):
//! `project! {}` proc-macro sugar over `mxrs-dsl`'s builder API.
//!
//! ```
//! # fn main() {
//! // A marker type per referenceable entity — hand-written here; in a real
//! // project these come from `mxrs-typegen`'s build.rs codegen. An
//! // association's target (after `->`) is this kind of Rust path, not a
//! // Mendix-style dotted name — see `parse`'s doc comment.
//! mod markers {
//!     pub struct Customer;
//!     impl mxrs_ir::EntityMarker for Customer {
//!         const MODULE: &'static str = "Sales";
//!         const NAME: &'static str = "Customer";
//!     }
//! }
//!
//! let definition = mxrs_macros::project! {
//!     "11.12.1",
//!     module Sales {
//!         entity Customer {
//!             string Name;
//!         }
//!         entity Order {
//!             string Number = "A-0000";
//!             decimal Total;
//!             association Order_Customer -> markers::Customer as Reference;
//!         }
//!     }
//! };
//! assert_eq!(definition.modules[0].name, "Sales");
//! # }
//! ```
//!
//! See `parse`'s doc comment for the full grammar and what's deliberately
//! not covered yet, and `expand`'s doc comment for the non-negotiable rule
//! this crate follows: every expansion lowers to calls against `mxrs-dsl`
//! pub fns a caller could already reach by hand.
//!
//! `#[derive(MxEntity)]` (M8.3) is a second, independent front end onto the
//! same `mxrs-dsl` surface — see `derive`'s doc comment.

mod derive;
mod expand;
mod parse;

use proc_macro::TokenStream;
use syn::parse_macro_input;

use crate::parse::ProjectInput;

#[proc_macro]
pub fn project(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as ProjectInput);
    expand::expand(&parsed).into()
}

#[proc_macro_derive(MxEntity, attributes(mx_entity, mx_attribute))]
pub fn derive_mx_entity(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as syn::DeriveInput);
    match derive::expand_derive(&parsed) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
