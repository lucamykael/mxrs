//! Enumeration declarations: `#[mxrs::enumeration]` and the older
//! `#[derive(MxEnumeration)]`. Both turn a Rust enum into a typed qualified
//! name and an editable Mendix enumeration document; the attribute form also
//! registers it with the application and reads `///` as documentation.

use proc_macro2::TokenStream;
use quote::quote;

struct EnumerationOptions {
    name: String,
    module: Option<String>,
    documentation: Option<String>,
}

struct ValueOptions {
    name: String,
    captions: Vec<(String, String)>,
}

/// Arguments of `#[mxrs::enumeration(...)]`.
pub struct EnumerationArgs {
    module: syn::LitStr,
    name: Option<syn::LitStr>,
    imported: bool,
}

impl syn::parse::Parse for EnumerationArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut module = None;
        let mut name = None;
        let mut imported = false;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            match key.to_string().as_str() {
                "module" => {
                    input.parse::<syn::Token![=]>()?;
                    module = Some(input.parse()?);
                }
                "name" => {
                    input.parse::<syn::Token![=]>()?;
                    name = Some(input.parse()?);
                }
                "imported" => imported = true,
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown enumeration option; expected `module`, `name`, or `imported`",
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
            imported,
        })
    }
}

pub fn expand_derive(input: &syn::DeriveInput) -> syn::Result<TokenStream> {
    let syn::Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "MxEnumeration can only be derived for an enum",
        ));
    };
    let options = parse_options(&input.attrs, &input.ident, false)?;
    let module = options.module.clone().ok_or_else(|| {
        syn::Error::new_spanned(
            &input.ident,
            "MxEnumeration requires #[mxrs(module = \"ModuleName\")]",
        )
    })?;
    expand(
        &input.ident,
        &data.variants,
        &options,
        &module,
        &quote!(::mxrs_dsl),
        &quote!(::mxrs_ir),
    )
}

/// Expands `#[mxrs::enumeration]`.
pub fn expand_enumeration(
    args: &EnumerationArgs,
    item: &syn::ItemEnum,
) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "a Mendix enumeration cannot be generic",
        ));
    }
    let mut options = parse_options(&item.attrs, &item.ident, true)?;
    if options.module.is_some() {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "`module` belongs to the attribute itself: `#[mxrs::enumeration(module = \"...\")]`",
        ));
    }
    if let Some(name) = &args.name {
        options.name = name.value();
    }
    let module = args.module.value();
    let facade = quote!(::mxrs);
    let implementation = expand(
        &item.ident,
        &item.variants,
        &options,
        &module,
        &facade,
        &facade,
    )?;
    let mut declaration = item.clone();
    declaration
        .attrs
        .retain(|attribute| !attribute.path().is_ident("mxrs"));
    for variant in &mut declaration.variants {
        variant
            .attrs
            .retain(|attribute| !attribute.path().is_ident("mxrs"));
    }
    let ident = &item.ident;
    let registration = (!args.imported).then(|| {
        quote! {
            ::mxrs::inventory::submit! {
                ::mxrs::registry::Declaration::new(
                    ::mxrs::registry::Stage::Enumeration,
                    ::core::module_path!(),
                    ::core::file!(),
                    ::core::line!(),
                    |project| {
                        let mut module = ::mxrs::ModuleBuilder::new(#module);
                        #ident::mx_register(&mut module);
                        project.merge_module(module.into_decl());
                    },
                )
            }
        }
    });
    Ok(quote! {
        #[allow(dead_code)]
        #declaration
        #implementation
        #registration
    })
}

fn expand(
    enum_ident: &syn::Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    options: &EnumerationOptions,
    module: &str,
    dsl: &TokenStream,
    ir: &TokenStream,
) -> syn::Result<TokenStream> {
    let name = &options.name;
    let documentation = options
        .documentation
        .as_ref()
        .map(|text| quote! { enumeration.documentation(#text); });
    let values = variants
        .iter()
        .map(|variant| {
            if !matches!(variant.fields, syn::Fields::Unit) {
                return Err(syn::Error::new_spanned(
                    variant,
                    "MxEnumeration variants cannot carry fields",
                ));
            }
            let value = parse_value(&variant.attrs, &variant.ident)?;
            let value_name = value.name;
            let captions = value.captions.iter().map(|(language, caption)| {
                quote! { (#language.to_string(), #caption.to_string()) }
            });
            Ok(quote! {
                enumeration.value(#value_name).captions = vec![#(#captions),*];
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        impl #enum_ident {
            pub fn mx_register(module: &mut #dsl::ModuleBuilder) {
                module.enumeration(#name, |enumeration| {
                    #documentation
                    #(#values)*
                });
            }
        }

        impl #ir::EnumerationMarker for #enum_ident {
            const MODULE: &'static str = #module;
            const NAME: &'static str = #name;
        }
    })
}

fn parse_options(
    attributes: &[syn::Attribute],
    enum_ident: &syn::Ident,
    read_docs: bool,
) -> syn::Result<EnumerationOptions> {
    let mut name = enum_ident.to_string();
    let mut module = None;
    let mut documentation = read_docs.then(|| doc_comment(attributes)).flatten();
    for attribute in attributes
        .iter()
        .filter(|attr| attr.path().is_ident("mxrs"))
    {
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = meta.value()?.parse::<syn::LitStr>()?.value();
            } else if meta.path.is_ident("module") {
                module = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            } else if meta.path.is_ident("documentation") {
                documentation = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            } else {
                return Err(meta.error(
                    "unknown enumeration #[mxrs(...)] key; expected `name`, `module`, or `documentation`",
                ));
            }
            Ok(())
        })?;
    }
    Ok(EnumerationOptions {
        name,
        module,
        documentation,
    })
}

fn doc_comment(attributes: &[syn::Attribute]) -> Option<String> {
    let lines = attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
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

/// A value's captions: `caption = "..."` (in `language`, `en_US` unless
/// stated) for the usual single one, or `captions(en_US = "...", nl_NL =
/// "...")` for every language — including none at all, `captions()`.
/// A value that states neither is captioned with its own name.
fn parse_value(
    attributes: &[syn::Attribute],
    variant_ident: &syn::Ident,
) -> syn::Result<ValueOptions> {
    let mut name = variant_ident.to_string();
    let mut caption = None;
    let mut language = "en_US".to_string();
    let mut captions = None;
    for attribute in attributes
        .iter()
        .filter(|attr| attr.path().is_ident("mxrs"))
    {
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = meta.value()?.parse::<syn::LitStr>()?.value();
            } else if meta.path.is_ident("caption") {
                caption = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            } else if meta.path.is_ident("language") {
                language = meta.value()?.parse::<syn::LitStr>()?.value();
            } else if meta.path.is_ident("captions") {
                // A language code is written bare (`en_US`); one that is not
                // an identifier is quoted (`"zh-Hans"`).
                let content;
                syn::parenthesized!(content in meta.input);
                let mut all = Vec::new();
                while !content.is_empty() {
                    let code = if content.peek(syn::LitStr) {
                        content.parse::<syn::LitStr>()?.value()
                    } else {
                        content.parse::<syn::Ident>()?.to_string()
                    };
                    content.parse::<syn::Token![=]>()?;
                    all.push((code, content.parse::<syn::LitStr>()?.value()));
                    if content.peek(syn::Token![,]) {
                        content.parse::<syn::Token![,]>()?;
                    }
                }
                captions = Some(all);
            } else {
                return Err(meta.error(
                    "unknown enumeration value #[mxrs(...)] key; expected `name`, `caption`, `language`, or `captions`",
                ));
            }
            Ok(())
        })?;
    }
    let captions = match captions {
        Some(_) if caption.is_some() => {
            return Err(syn::Error::new_spanned(
                variant_ident,
                "state either `caption` or `captions(...)`, not both",
            ));
        }
        Some(captions) => captions,
        None => vec![(language, caption.unwrap_or_else(|| name.clone()))],
    };
    Ok(ValueOptions { name, captions })
}
