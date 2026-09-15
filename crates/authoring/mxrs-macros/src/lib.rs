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
mod derive_enumeration;
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

/// Implementation entry point for the facade's hygienic `project!` wrapper.
#[doc(hidden)]
#[proc_macro]
pub fn project_facade(input: TokenStream) -> TokenStream {
    let mut tokens = proc_macro2::TokenStream::from(input).into_iter();
    let Some(proc_macro2::TokenTree::Group(root)) = tokens.next() else {
        return syn::Error::new(proc_macro2::Span::call_site(), "expected facade path")
            .to_compile_error()
            .into();
    };
    let result =
        syn::parse2::<ProjectInput>(tokens.collect()).and_then(|input| expand::expand(&input));
    match result {
        Ok(expanded) => facade_paths(expanded, &root.stream()).into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn facade_paths(
    tokens: proc_macro2::TokenStream,
    root: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    use proc_macro2::{Group, TokenTree};
    let tokens: Vec<_> = tokens.into_iter().collect();
    let mut output = proc_macro2::TokenStream::new();
    let mut index = 0;
    while index < tokens.len() {
        if let [
            TokenTree::Punct(first),
            TokenTree::Punct(second),
            TokenTree::Ident(name),
            ..,
        ] = &tokens[index..]
            && starts_absolute_path(&tokens, index)
            && first.as_char() == ':'
            && second.as_char() == ':'
            && matches!(
                name.to_string().as_str(),
                "mxrs_dsl" | "mxrs_expr" | "mxrs_ir"
            )
        {
            output.extend(root.clone());
            index += 3;
            continue;
        }
        let token = match &tokens[index] {
            TokenTree::Group(group) => {
                let mut replacement =
                    Group::new(group.delimiter(), facade_paths(group.stream(), root));
                replacement.set_span(group.span());
                TokenTree::Group(replacement)
            }
            token => token.clone(),
        };
        output.extend([token]);
        index += 1;
    }
    output
}

#[proc_macro_derive(MxEntity, attributes(mx_entity, mx_attribute, mxrs))]
pub fn derive_mx_entity(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as syn::DeriveInput);
    match derive::expand_derive(&parsed) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(MxEnumeration, attributes(mxrs))]
pub fn derive_mx_enumeration(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as syn::DeriveInput);
    match derive_enumeration::expand_derive(&parsed) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

// Do not reinterpret a user path such as `helpers::mxrs_expr::value` as
// one of the absolute framework paths emitted by the expander.
fn starts_absolute_path(tokens: &[proc_macro2::TokenTree], index: usize) -> bool {
    use proc_macro2::TokenTree;
    match index
        .checked_sub(1)
        .and_then(|previous| tokens.get(previous))
    {
        Some(TokenTree::Ident(ident)) => matches!(
            ident.to_string().as_str(),
            "impl" | "as" | "for" | "return" | "break" | "yield" | "dyn" | "mut" | "const" | "in"
        ),
        Some(TokenTree::Punct(punct)) if punct.as_char() == '>' => {
            index >= 2
                && matches!(&tokens[index - 2], TokenTree::Punct(previous) if previous.as_char() == '-')
        }
        Some(TokenTree::Group(_)) => false,
        _ => true,
    }
}
