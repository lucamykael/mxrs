//! Function-shaped declarations: `#[mxrs::microflow]` and `#[mxrs::nanoflow]`.
//!
//! A flow is written as the function that builds it. The attribute leaves
//! that function alone, registers the flow with the application, and
//! declares a unit type carrying the flow's Mendix name — so the flow can be
//! named wherever the model refers to one (a call activity, an event
//! handler, a button) exactly as Studio Pro names it. There is no separate
//! marker to generate and no aggregator to edit.
//!
//! ```text
//! /// Creates an animal from its name.
//! #[microflow(ACT, module = "VetClinic")]
//! pub fn create_animal(flow: &mut FlowBuilder) {
//!     // Declares `VetClinic.ACT_CreateAnimal`, nameable as
//!     // `ACT_CreateAnimal`.
//! }
//! ```

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;

use crate::derive::to_pascal_case;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    Microflow,
    Nanoflow,
}

/// Arguments of `#[mxrs::microflow(...)]` and `#[mxrs::nanoflow(...)]`.
pub struct FlowArgs {
    /// The naming-convention prefix (`ACT`, `SUB`, `DS`, ...), when the flow
    /// states one.
    prefix: Option<syn::Ident>,
    module: syn::LitStr,
    name: Option<syn::LitStr>,
}

impl syn::parse::Parse for FlowArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut prefix = None;
        let mut module = None;
        let mut name = None;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            if input.peek(syn::Token![=]) {
                input.parse::<syn::Token![=]>()?;
                match key.to_string().as_str() {
                    "module" => module = Some(input.parse()?),
                    "name" => name = Some(input.parse()?),
                    _ => {
                        return Err(syn::Error::new(
                            key.span(),
                            "unknown flow option; expected `module` or `name`",
                        ));
                    }
                }
            } else {
                let text = key.to_string();
                let is_prefix = text
                    .chars()
                    .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
                    && text.starts_with(|character: char| character.is_ascii_uppercase());
                if !is_prefix {
                    return Err(syn::Error::new(
                        key.span(),
                        "expected a naming prefix in capitals (`ACT`, `SUB`, `DS`, ...), `module = \"...\"`, or `name = \"...\"`",
                    ));
                }
                if prefix.replace(key.clone()).is_some() {
                    return Err(syn::Error::new(key.span(), "a flow has one naming prefix"));
                }
            }
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
        }
        Ok(Self {
            prefix,
            module: module.ok_or_else(|| input.error("missing `module = \"ModuleName\"`"))?,
            name,
        })
    }
}

impl FlowArgs {
    /// The Mendix name: stated outright, or the prefix joined to the
    /// function's name in the convention's casing
    /// (`ACT` + `create_animal` → `ACT_CreateAnimal`).
    fn mendix_name(&self, function: &syn::Ident) -> syn::Result<String> {
        let function_name = function.to_string();
        let function_name = function_name.strip_prefix("r#").unwrap_or(&function_name);
        let Some(name) = &self.name else {
            let base = to_pascal_case(function_name);
            return Ok(match &self.prefix {
                Some(prefix) => format!("{prefix}_{base}"),
                None => base,
            });
        };
        let value = name.value();
        if value.is_empty() || value.contains('.') {
            return Err(syn::Error::new(
                name.span(),
                "a flow name is unqualified; its module is `module = \"...\"`",
            ));
        }
        if let Some(prefix) = &self.prefix
            && !value.starts_with(&format!("{prefix}_"))
        {
            return Err(syn::Error::new(
                name.span(),
                format!("`{value}` does not carry the `{prefix}_` prefix this flow declares"),
            ));
        }
        Ok(value)
    }
}

pub fn expand_flow(
    args: &FlowArgs,
    kind: FlowKind,
    item: &syn::ItemFn,
) -> syn::Result<TokenStream> {
    let signature = &item.sig;
    if let Some(token) = &signature.asyncness {
        return Err(syn::Error::new(
            token.span(),
            "a flow declaration is not async",
        ));
    }
    if !signature.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &signature.generics,
            "a flow declaration cannot be generic",
        ));
    }
    if !matches!(signature.output, syn::ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            &signature.output,
            "a flow declares its result with `flow.return_value(...)`, not a Rust return type",
        ));
    }
    if signature.inputs.len() != 1 || matches!(signature.inputs[0], syn::FnArg::Receiver(_)) {
        return Err(syn::Error::new_spanned(
            &signature.inputs,
            "a flow declaration takes the builder alone: `fn name(flow: &mut FlowBuilder)`",
        ));
    }

    let ident = &signature.ident;
    let visibility = &item.vis;
    let module = &args.module;
    let name = args.mendix_name(ident)?;
    let marker = marker_ident(&name, ident.span());
    if marker == *ident {
        return Err(syn::Error::new(
            ident.span(),
            format!(
                "the function cannot share the flow's own name `{name}`; name the function differently and keep `name = \"{name}\"`"
            ),
        ));
    }
    let docs = item
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .collect::<Vec<_>>();
    let documentation = doc_text(&docs).map(|text| quote! { __mxrs_flow.documentation(#text); });
    let (marker_trait, module_builder, method, stage, noun) = match kind {
        FlowKind::Microflow => (
            quote!(::mxrs::MicroflowMarker),
            quote!(::mxrs::MicroflowModuleBuilder),
            quote!(microflow),
            quote!(::mxrs::registry::Stage::Microflow),
            "microflow",
        ),
        FlowKind::Nanoflow => (
            quote!(::mxrs::NanoflowMarker),
            quote!(::mxrs::NanoflowModuleBuilder),
            quote!(nanoflow),
            quote!(::mxrs::registry::Stage::Nanoflow),
            "nanoflow",
        ),
    };
    let summary = format!(
        "The `{}.{name}` {noun}, declared by [`{ident}`].",
        module.value()
    );

    Ok(quote! {
        #item

        #[doc = #summary]
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, Copy)]
        #visibility struct #marker;

        impl #marker_trait for #marker {
            const MODULE: &'static str = #module;
            const NAME: &'static str = #name;
        }

        ::mxrs::inventory::submit! {
            ::mxrs::registry::Declaration::new(
                #stage,
                ::core::module_path!(),
                ::core::file!(),
                ::core::line!(),
                |project| {
                    let mut module = #module_builder::new(#module);
                    module.#method(#name, |__mxrs_flow| {
                        #documentation
                        #ident(__mxrs_flow);
                    });
                    project.merge_module(module.into_decl());
                },
            )
        }
    })
}

/// The type a flow is named by: its Mendix name, spelled as an identifier.
fn marker_ident(name: &str, span: proc_macro2::Span) -> syn::Ident {
    let mut ident: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if ident.starts_with(|character: char| character.is_ascii_digit()) {
        ident.insert(0, '_');
    }
    if syn::parse_str::<syn::Ident>(&ident).is_err() {
        ident.push('_');
    }
    syn::Ident::new(&ident, span)
}

/// A document a module declares through a builder of its own: the function
/// receives that builder, already named.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Constant,
    Layout,
    Menu,
    Page,
}

/// Arguments of `#[mxrs::page(...)]`, `#[mxrs::layout(...)]`,
/// `#[mxrs::constant(...)]` and `#[mxrs::menu(...)]`.
pub struct DocumentArgs {
    module: syn::LitStr,
    name: Option<syn::LitStr>,
}

impl syn::parse::Parse for DocumentArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut module = None;
        let mut name = None;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            match key.to_string().as_str() {
                "module" => module = Some(input.parse()?),
                "name" => name = Some(input.parse()?),
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown option; expected `module` or `name`",
                    ));
                }
            }
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
        }
        Ok(Self {
            module: module.ok_or_else(|| input.error("missing `module = \"ModuleName\"`"))?,
            name,
        })
    }
}

/// Expands a document declared by the function that builds it. The Mendix
/// name is the function's, in the model's casing (`order_overview` →
/// `OrderOverview`), unless `name = "..."` states it.
pub fn expand_document(
    args: &DocumentArgs,
    kind: DocumentKind,
    item: &syn::ItemFn,
) -> syn::Result<TokenStream> {
    let signature = &item.sig;
    if signature.inputs.len() != 1
        || matches!(signature.inputs[0], syn::FnArg::Receiver(_))
        || !matches!(signature.output, syn::ReturnType::Default)
        || !signature.generics.params.is_empty()
        || signature.asyncness.is_some()
    {
        return Err(syn::Error::new_spanned(
            signature,
            "a declaration takes its builder alone and returns nothing: `fn name(builder: &mut Builder)`",
        ));
    }
    let ident = &signature.ident;
    let module = &args.module;
    let name = match &args.name {
        Some(name) => name.value(),
        None => {
            let function = ident.to_string();
            to_pascal_case(function.strip_prefix("r#").unwrap_or(&function))
        }
    };
    let (method, stage) = match kind {
        DocumentKind::Constant => (quote!(constant), quote!(Document)),
        DocumentKind::Layout => (quote!(layout), quote!(Layout)),
        DocumentKind::Menu => (quote!(menu), quote!(Document)),
        DocumentKind::Page => (quote!(page), quote!(Page)),
    };
    let docs = item
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .collect::<Vec<_>>();
    // Layouts and menus carry no documentation of their own in the model.
    let documentation = match kind {
        DocumentKind::Constant | DocumentKind::Page => {
            doc_text(&docs).map(|text| quote! { __mxrs_builder.documentation(#text); })
        }
        DocumentKind::Layout | DocumentKind::Menu => None,
    };

    Ok(quote! {
        #item

        ::mxrs::inventory::submit! {
            ::mxrs::registry::Declaration::new(
                ::mxrs::registry::Stage::#stage,
                ::core::module_path!(),
                ::core::file!(),
                ::core::line!(),
                |project| {
                    let mut module = ::mxrs::ModuleBuilder::new(#module);
                    module.#method(#name, |__mxrs_builder| {
                        #documentation
                        #ident(__mxrs_builder);
                    });
                    project.merge_module(module.into_decl());
                },
            )
        }
    })
}

/// `///` lines as Mendix documentation, each without the comment's own space.
fn doc_text(attributes: &[&syn::Attribute]) -> Option<String> {
    let lines = attributes
        .iter()
        .filter_map(|attribute| match &attribute.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(text),
                        ..
                    }),
                ..
            }) => Some(text.value()),
            _ => None,
        })
        .map(|line| line.strip_prefix(' ').map(str::to_string).unwrap_or(line))
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}
