//! `#[derive(MxEnumeration)]` turns a Rust enum into both a typed qualified
//! name and an editable Mendix enumeration document.

use quote::quote;

struct EnumerationOptions {
    name: String,
    module: String,
    documentation: Option<String>,
}

struct ValueOptions {
    name: String,
    caption: Option<String>,
    language: String,
}

pub fn expand_derive(input: &syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let enum_ident = &input.ident;
    let options = parse_options(&input.attrs, enum_ident)?;
    let syn::Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "MxEnumeration can only be derived for an enum",
        ));
    };

    let name = &options.name;
    let module = &options.module;
    let documentation = options
        .documentation
        .map(|text| quote! { enumeration.documentation(#text); });
    let values = data
        .variants
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
            let caption = value.caption.unwrap_or_else(|| value_name.clone());
            let language = value.language;
            Ok(quote! {
                enumeration.value(#value_name).captions = vec![(#language.to_string(), #caption.to_string())];
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        impl #enum_ident {
            pub fn mx_register(module: &mut ::mxrs_dsl::ModuleBuilder) {
                module.enumeration(#name, |enumeration| {
                    #documentation
                    #(#values)*
                });
            }
        }

        impl ::mxrs_ir::EnumerationMarker for #enum_ident {
            const MODULE: &'static str = #module;
            const NAME: &'static str = #name;
        }
    })
}

fn parse_options(
    attributes: &[syn::Attribute],
    enum_ident: &syn::Ident,
) -> syn::Result<EnumerationOptions> {
    let mut name = enum_ident.to_string();
    let mut module = None;
    let mut documentation = None;
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
    let module = module.ok_or_else(|| {
        syn::Error::new_spanned(
            enum_ident,
            "MxEnumeration requires #[mxrs(module = \"ModuleName\")]",
        )
    })?;
    Ok(EnumerationOptions {
        name,
        module,
        documentation,
    })
}

fn parse_value(
    attributes: &[syn::Attribute],
    variant_ident: &syn::Ident,
) -> syn::Result<ValueOptions> {
    let mut name = variant_ident.to_string();
    let mut caption = None;
    let mut language = "en_US".to_string();
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
            } else {
                return Err(meta.error(
                    "unknown enumeration value #[mxrs(...)] key; expected `name`, `caption`, or `language`",
                ));
            }
            Ok(())
        })?;
    }
    Ok(ValueOptions {
        name,
        caption,
        language,
    })
}
