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

/// Checks the one shape every builder-taking declaration has.
fn builder_function(item: &syn::ItemFn) -> syn::Result<&syn::Ident> {
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
    Ok(&signature.ident)
}

fn registration(stage: TokenStream, apply: TokenStream) -> TokenStream {
    quote! {
        ::mxrs::inventory::submit! {
            ::mxrs::registry::Declaration::new(
                ::mxrs::registry::Stage::#stage,
                ::core::module_path!(),
                ::core::file!(),
                ::core::line!(),
                |project| { #apply },
            )
        }
    }
}

/// `key = "value"` pairs, each key at most once and only from `allowed`.
struct NamedStrings(Vec<(syn::Ident, syn::LitStr)>);

impl NamedStrings {
    fn parse(input: syn::parse::ParseStream<'_>, allowed: &[&str]) -> syn::Result<Self> {
        let mut pairs: Vec<(syn::Ident, syn::LitStr)> = Vec::new();
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            if !allowed.iter().any(|name| key == name) {
                return Err(syn::Error::new(
                    key.span(),
                    format!("unknown option; expected one of: {}", allowed.join(", ")),
                ));
            }
            if pairs.iter().any(|(existing, _)| *existing == key) {
                return Err(syn::Error::new(key.span(), format!("duplicate `{key}`")));
            }
            input.parse::<syn::Token![=]>()?;
            pairs.push((key, input.parse()?));
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
        }
        Ok(Self(pairs))
    }

    fn get(&self, key: &str) -> Option<&syn::LitStr> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    fn require(&self, key: &str) -> syn::Result<&syn::LitStr> {
        self.get(key).ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing `{key} = \"...\"`"),
            )
        })
    }
}

/// Arguments of `#[mxrs::declaration(module = "...")]`: the module the
/// function declares into, and optionally the stage it is assembled at.
pub struct DeclarationArgs {
    module: syn::LitStr,
    stage: Option<syn::Ident>,
}

impl syn::parse::Parse for DeclarationArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut module = None;
        let mut stage = None;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            match key.to_string().as_str() {
                "module" => module = Some(input.parse()?),
                "stage" => stage = Some(input.parse()?),
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown option; expected `module` or `stage`",
                    ));
                }
            }
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
        }
        Ok(Self {
            module: module.ok_or_else(|| input.error("missing `module = \"ModuleName\"`"))?,
            stage,
        })
    }
}

/// Expands `#[mxrs::declaration]`: a function that declares into one module
/// through its `ModuleBuilder` — whatever the more specific attributes do
/// not cover.
pub fn expand_declaration(args: &DeclarationArgs, item: &syn::ItemFn) -> syn::Result<TokenStream> {
    let ident = builder_function(item)?;
    let module = &args.module;
    let stage = match &args.stage {
        Some(stage) => quote!(#stage),
        None => quote!(Document),
    };
    let registration = registration(
        stage,
        quote! {
            let mut module = ::mxrs::ModuleBuilder::new(#module);
            #ident(&mut module);
            project.merge_module(module.into_decl());
        },
    );
    Ok(quote! {
        #item
        #registration
    })
}

/// A declaration the project has one of.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    Security,
    Navigation,
}

/// Expands `#[mxrs::security]` and `#[mxrs::navigation]`.
pub fn expand_project(kind: ProjectKind, item: &syn::ItemFn) -> syn::Result<TokenStream> {
    let ident = builder_function(item)?;
    let (stage, method, field) = match kind {
        ProjectKind::Security => (quote!(Security), quote!(security), quote!(security)),
        ProjectKind::Navigation => (quote!(Navigation), quote!(navigation), quote!(navigation)),
    };
    let registration = registration(
        stage,
        quote! {
            let mut builder = ::mxrs::ProjectBuilder::new(project.mendix_version.clone());
            builder.#method(|__mxrs_builder| #ident(__mxrs_builder));
            project.#field = builder.build().#field;
        },
    );
    Ok(quote! {
        #item
        #registration
    })
}

/// Arguments of `#[mxrs::navigation_item(profile = "...", caption = "...")]`.
pub struct NavigationItemArgs(NamedStrings);

impl syn::parse::Parse for NavigationItemArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        NamedStrings::parse(input, &["profile", "caption"]).map(Self)
    }
}

/// Expands `#[mxrs::navigation_item]`: an item declared apart from the
/// profile it joins, appended to that profile when the model is assembled.
pub fn expand_navigation_item(
    args: &NavigationItemArgs,
    item: &syn::ItemFn,
) -> syn::Result<TokenStream> {
    let ident = builder_function(item)?;
    let profile = args.0.require("profile")?;
    let caption = args.0.require("caption")?;
    let registration = registration(
        quote!(NavigationItem),
        quote! {
            let mut item = ::mxrs::NavigationItemBuilder::new(#caption);
            #ident(&mut item);
            project.navigation_item(#profile, item.into_decl());
        },
    );
    Ok(quote! {
        #item
        #registration
    })
}

/// Arguments of `#[mxrs::demo_user(name = "...")]`.
pub struct DemoUserArgs(NamedStrings);

impl syn::parse::Parse for DemoUserArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        NamedStrings::parse(input, &["name"]).map(Self)
    }
}

/// Expands `#[mxrs::demo_user]`: a demo account, named by its function
/// unless `name = "..."` states otherwise. It joins the security the project
/// declares; a project that declares none keeps its imported security as it
/// is, demo users included.
pub fn expand_demo_user(args: &DemoUserArgs, item: &syn::ItemFn) -> syn::Result<TokenStream> {
    let ident = builder_function(item)?;
    let name = match args.0.get("name") {
        Some(name) => name.value(),
        None => {
            let function = ident.to_string();
            function.strip_prefix("r#").unwrap_or(&function).to_string()
        }
    };
    let registration = registration(
        quote!(DemoUser),
        quote! {
            let mut user = ::mxrs::DemoUserBuilder::new(#name);
            #ident(&mut user);
            if let Some(security) = project.security.as_mut() {
                security.demo_users.push(user.into_decl());
            }
        },
    );
    Ok(quote! {
        #item
        #registration
    })
}

/// Arguments of `#[mxrs::module_roles(module = "...")]`.
pub struct ModuleRolesArgs(NamedStrings);

impl syn::parse::Parse for ModuleRolesArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        NamedStrings::parse(input, &["module"]).map(Self)
    }
}

/// Expands `#[mxrs::module_roles]`: a module's roles as an enum. Each
/// variant is a role, its `///` comment the role's description, and
/// `#[mxrs(name = "...")]` a name the variant does not spell. The roles are
/// authoritative — an enum with no variants declares that the module has
/// none.
pub fn expand_module_roles(
    args: &ModuleRolesArgs,
    item: &syn::ItemEnum,
) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "module roles cannot be generic",
        ));
    }
    let module = args.0.require("module")?;
    let ident = &item.ident;
    let mut roles = Vec::new();
    let mut names = Vec::new();
    let mut variants = Vec::new();
    for variant in &item.variants {
        if !matches!(variant.fields, syn::Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "a module role is a plain variant",
            ));
        }
        let mut name = variant.ident.to_string();
        let mut description = None;
        for attribute in variant
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("mxrs"))
        {
            attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    name = meta.value()?.parse::<syn::LitStr>()?.value();
                    Ok(())
                } else if meta.path.is_ident("description") {
                    description = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                    Ok(())
                } else {
                    Err(meta.error("unknown module role option; expected `name` or `description`"))
                }
            })?;
        }
        let docs = variant
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("doc"))
            .collect::<Vec<_>>();
        let description = description.or_else(|| doc_text(&docs)).unwrap_or_default();
        roles.push(quote! { module.role(#name, #description); });
        let variant_ident = &variant.ident;
        variants.push(variant_ident);
        names.push(name);
    }
    let mut declaration = item.clone();
    for variant in &mut declaration.variants {
        variant
            .attrs
            .retain(|attribute| !attribute.path().is_ident("mxrs"));
    }
    let registration = registration(
        quote!(ModuleSecurity),
        quote! {
            let mut module = ::mxrs::ModuleBuilder::new(#module);
            module.clear_roles();
            #(#roles)*
            project.merge_module(module.into_decl());
        },
    );
    let name_arms = if variants.is_empty() {
        quote! { match *self {} }
    } else {
        quote! { match self { #(Self::#variants => #names,)* } }
    };
    Ok(quote! {
        #[allow(dead_code)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #declaration

        #[allow(dead_code)]
        impl #ident {
            /// The module the roles belong to.
            pub const MODULE: &'static str = #module;

            /// The role's name within its module.
            pub fn name(&self) -> &'static str {
                #name_arms
            }

            /// `Module.Role`: the form a user role or a page names it by.
            pub fn qualified_name(&self) -> String {
                format!("{}.{}", Self::MODULE, self.name())
            }
        }

        #registration
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
