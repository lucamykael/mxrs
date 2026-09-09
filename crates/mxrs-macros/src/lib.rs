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
//!             association Order_Customer -> Customer as Reference;
//!         }
//!     }
//! };
//! assert_eq!(definition.modules[0].name, "Sales");
//! # }
//! ```
//!
//! See `parse`'s doc comment for the full grammar and what's deliberately
//! not covered yet (entity documentation/`persistable`, microflows,
//! cross-module targets beyond one dotted `Module.Entity` path), and
//! `expand`'s doc comment for the non-negotiable rule this crate follows:
//! every expansion lowers to calls against `mxrs-dsl` pub fns a caller
//! could already reach by hand.

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
