use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, ItemStruct, LitStr, Path, Result, Token};

pub struct ApplicationArgs {
    version: LitStr,
    project: Option<Path>,
}

impl Parse for ApplicationArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut version = None;
        let mut project = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "version" => version = Some(input.parse()?),
                "project" => project = Some(input.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown application option `{other}`"),
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            version: version.ok_or_else(|| input.error("missing `version = \"...\"`"))?,
            project,
        })
    }
}

pub fn expand(args: &ApplicationArgs, item: &ItemStruct) -> Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "mxrs applications cannot be generic",
        ));
    }
    let ident = &item.ident;
    let version = &args.version;
    // An explicit entry point contributes whatever it builds by hand; the
    // declarations the crate registered are folded in either way, so a
    // project can move to self-registering declarations one file at a time.
    let base = match &args.project {
        Some(project) => quote! { (#project)() },
        None => quote! { ::mxrs::ProjectBuilder::new(Self::MENDIX_VERSION).build() },
    };
    Ok(quote! {
        #item

        impl ::mxrs::ApplicationDefinition for #ident {
            const MENDIX_VERSION: &'static str = #version;

            fn declaration() -> ::mxrs::ProjectDecl {
                let mut declaration = #base;
                declaration.mendix_version = Self::MENDIX_VERSION.to_string();
                ::mxrs::registry::apply(
                    ::mxrs::registry::crate_of(::core::module_path!()),
                    &mut declaration,
                );
                declaration
            }
        }

        impl #ident {
            pub fn build() -> ::mxrs::ProjectDecl {
                <Self as ::mxrs::ApplicationDefinition>::declaration()
            }
        }
    })
}
