//! Lowers a parsed `project! {}` invocation into calls against
//! `mxrs-dsl`'s existing builder API — the non-negotiable rule this crate
//! is required to follow (per the plan's DSL/macro track section): every
//! expansion must be something a caller could have written by hand against
//! `mxrs-dsl`'s already-public surface. No capability exists only through
//! this macro.
//!
//! A `project! {}` invocation expands to a block expression evaluating to
//! an `mxrs_ir::ProjectDecl` (the same value `ProjectBuilder::build()`
//! returns), so it's usable directly wherever one is needed —
//! `mxrs_writer::write_project(path, &project! { ... })`.
//!
//! References `::mxrs_model::association::AssociationType` directly rather
//! than going through an `mxrs-dsl` re-export (there isn't one — existing
//! hand-written `mxrs-dsl` usage already imports `mxrs_model::association`
//! types directly, e.g. in `mxrs-writer`'s own test suite), so a crate
//! using `project! {}` needs `mxrs-model` as a direct dependency too, same
//! as hand-written `mxrs-dsl` usage already does.

use proc_macro2::TokenStream;
use quote::quote;

use crate::parse::{AssociationInput, AttributeInput, EntityInput, ModuleInput, ProjectInput};

pub fn expand(input: &ProjectInput) -> TokenStream {
    let version = &input.version;
    let module_stmts: Vec<TokenStream> = input.modules.iter().map(expand_module).collect();
    quote! {
        {
            let mut __mxrs_project = ::mxrs_dsl::ProjectBuilder::new(#version);
            #(#module_stmts)*
            __mxrs_project.build()
        }
    }
}

fn expand_module(module: &ModuleInput) -> TokenStream {
    let name = module.name.to_string();
    let entity_stmts: Vec<TokenStream> = module.entities.iter().map(expand_entity).collect();
    quote! {
        __mxrs_project.module(#name, |m| {
            #(#entity_stmts)*
        });
    }
}

fn expand_entity(entity: &EntityInput) -> TokenStream {
    let name = entity.name.to_string();
    let attr_stmts: Vec<TokenStream> = entity.attributes.iter().map(expand_attribute).collect();
    let assoc_stmts: Vec<TokenStream> = entity.associations.iter().map(expand_association).collect();
    quote! {
        m.entity(#name, |e| {
            #(#attr_stmts)*
            #(#assoc_stmts)*
        });
    }
}

fn expand_attribute(attribute: &AttributeInput) -> TokenStream {
    let name = attribute.name.to_string();
    let method = syn::Ident::new(attribute.kind.builder_method(), attribute.name.span());
    match &attribute.default {
        Some(default) => quote! {
            e.#method(#name).default_value = Some((#default).to_string());
        },
        None => quote! {
            e.#method(#name);
        },
    }
}

fn expand_association(association: &AssociationInput) -> TokenStream {
    let name = association.name.to_string();
    let target = &association.target;
    let association_type = &association.association_type;
    quote! {
        e.association(#name, #target, ::mxrs_model::association::AssociationType::#association_type);
    }
}
