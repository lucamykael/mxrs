use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, ItemStruct, LitStr, Path, Result, Token};

pub struct ApplicationArgs {
    version: LitStr,
    project: Path,
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
            project: project.unwrap_or_else(|| syn::parse_quote!(crate::domain::build)),
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
    let project = &args.project;
    Ok(quote! {
        #item

        impl ::mxrs::ApplicationDefinition for #ident {
            const MENDIX_VERSION: &'static str = #version;

            fn declaration() -> ::mxrs::ProjectDecl {
                let mut declaration = (#project)();
                declaration.mendix_version = Self::MENDIX_VERSION.to_string();
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
