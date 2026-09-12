//! Phase 8 (M8.1, per the phased roadmap in
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory):
//! `project! {}` proc-macro sugar over `mxrs-dsl`'s builder API.
//!
//! ```
//! # fn main() {
//! let definition = mxrs_macros::project! {
//!     "11.12.1",
//!     module Sales {
//!         entity Customer {
//!             string Name;
//!         }
//!         entity Order {
//!             string Number = "A-0000";
//!             decimal Total;
//!             association Order_Customer -> Sales::Customer as Reference;
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

mod application;
mod derive;
mod expand;
mod parse;

use proc_macro::TokenStream;
use syn::parse_macro_input;

use crate::parse::ProjectInput;

/// Declares the root of a Cargo-native Mendix application.
///
/// The default `project` function is `crate::domain::build`; applications
/// with a different composition root can pass
/// `project = crate::application_model` explicitly.
#[proc_macro_attribute]
pub fn application(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as application::ApplicationArgs);
    let item = parse_macro_input!(item as syn::ItemStruct);
    match application::expand(&args, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro]
pub fn project(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as ProjectInput);
    match expand::expand(&parsed) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(MxEntity, attributes(mx_entity, mx_attribute))]
pub fn derive_mx_entity(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as syn::DeriveInput);
    match derive::expand_derive(&parsed) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
