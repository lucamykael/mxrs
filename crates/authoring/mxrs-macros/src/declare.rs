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
    Rule,
}

/// Arguments of `#[mxrs::microflow(...)]` and `#[mxrs::nanoflow(...)]`.
pub struct FlowArgs {
    /// The naming-convention prefix (`ACT`, `SUB`, `DS`, ...), when the flow
    /// states one.
    prefix: Option<syn::Ident>,
    /// The flow's module; a flow declared in a service takes the service's.
    module: Option<syn::LitStr>,
    name: Option<syn::LitStr>,
    /// `roles(...)`: the module roles that may run the flow.
    roles: Option<Vec<Related>>,
    /// `calls(...)`: the flows it calls.
    calls: Option<Vec<Related>>,
    /// `uses(...)`: the entities it works with.
    uses: Option<Vec<Related>>,
    /// `used_by(...)`: what refers to it.
    used_by: Option<Vec<Related>>,
    /// `folder = "..."`: the folder of its module it lives in.
    folder: Option<syn::LitStr>,
}

/// `folder = "Orders/Admin"`: a path of folder names inside the module.
pub(crate) fn folder_path(input: syn::parse::ParseStream<'_>) -> syn::Result<syn::LitStr> {
    let folder: syn::LitStr = input.parse()?;
    let value = folder.value();
    if value.split('/').any(str::is_empty) {
        return Err(syn::Error::new(
            folder.span(),
            "a folder is a path of folder names inside the module: `Orders/Admin` (a `/` in a name is `%2F`)",
        ));
    }
    Ok(folder)
}

/// What places a declaration's documents in the folder it states.
pub(crate) fn placed(folder: Option<&syn::LitStr>) -> TokenStream {
    match folder {
        Some(folder) => quote! { .in_folder(#folder) },
        None => TokenStream::new(),
    }
}

/// One item of a relation list: the Rust item that declares the related
/// thing — which the compiler resolves and an editor follows — or, for what
/// no Rust item declares, its qualified name.
enum Related {
    Item(syn::Path),
    Name(syn::LitStr),
}

impl syn::parse::Parse for Related {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(syn::LitStr) {
            input.parse().map(Related::Name)
        } else {
            input.parse().map(Related::Item)
        }
    }
}

fn related_list(input: syn::parse::ParseStream<'_>) -> syn::Result<Vec<Related>> {
    let content;
    syn::parenthesized!(content in input);
    Ok(content
        .parse_terminated(<Related as syn::parse::Parse>::parse, syn::Token![,])?
        .into_iter()
        .collect())
}

impl syn::parse::Parse for FlowArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut prefix = None;
        let mut module = None;
        let mut name = None;
        let mut folder = None;
        let (mut roles, mut calls, mut uses, mut used_by) = (None, None, None, None);
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            if input.peek(syn::token::Paren) {
                let slot = match key.to_string().as_str() {
                    "roles" => &mut roles,
                    "calls" => &mut calls,
                    "uses" => &mut uses,
                    "used_by" => &mut used_by,
                    _ => {
                        return Err(syn::Error::new(
                            key.span(),
                            "unknown flow relation; expected `roles(...)`, `calls(...)`, `uses(...)` or `used_by(...)`",
                        ));
                    }
                };
                if slot.replace(related_list(input)?).is_some() {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("`{key}(...)` is stated once"),
                    ));
                }
            } else if input.peek(syn::Token![=]) {
                input.parse::<syn::Token![=]>()?;
                match key.to_string().as_str() {
                    "module" => module = Some(input.parse()?),
                    "name" => name = Some(input.parse()?),
                    "folder" => folder = Some(folder_path(input)?),
                    _ => {
                        return Err(syn::Error::new(
                            key.span(),
                            "unknown flow option; expected `module`, `name` or `folder`",
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
                        "expected a naming prefix in capitals (`ACT`, `SUB`, `DS`, ...), `module = \"...\"`, `name = \"...\"`, or a relation such as `calls(...)`",
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
            module,
            name,
            roles,
            calls,
            uses,
            used_by,
            folder,
        })
    }
}

impl FlowArgs {
    /// The Mendix name: stated outright, or the prefix, the subject of the
    /// service declaring the flow and the function's name joined in the
    /// convention's casing (`ACT` + `create_animal` → `ACT_CreateAnimal`;
    /// `ACT` + `AssetType` + `edit` → `ACT_AssetType_Edit`).
    fn mendix_name(&self, function: &syn::Ident, subject: Option<&str>) -> syn::Result<String> {
        let function_name = function.to_string();
        let function_name = function_name.strip_prefix("r#").unwrap_or(&function_name);
        let Some(name) = &self.name else {
            let base = to_pascal_case(function_name);
            let parts = self
                .prefix
                .as_ref()
                .map(ToString::to_string)
                .into_iter()
                .chain(subject.map(str::to_string))
                .chain([base]);
            return Ok(parts.collect::<Vec<_>>().join("_"));
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

/// What a flow declaration's function must look like: the builder alone,
/// returning nothing, neither async nor generic.
fn check_flow_signature(signature: &syn::Signature) -> syn::Result<()> {
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
    Ok(())
}

pub fn expand_flow(
    args: &FlowArgs,
    kind: FlowKind,
    item: &syn::ItemFn,
) -> syn::Result<TokenStream> {
    check_flow_signature(&item.sig)?;
    let ident = &item.sig.ident;
    let module = args
        .module
        .as_ref()
        .ok_or_else(|| syn::Error::new(ident.span(), "missing `module = \"ModuleName\"`"))?;
    let name = args.mendix_name(ident, None)?;
    let declared = flow_declaration(FlowDeclaration {
        args,
        kind,
        module,
        name,
        ident,
        visibility: &item.vis,
        attributes: &item.attrs,
        call: quote!(#ident),
        doc_target: ident.to_string(),
    })?;
    Ok(quote! {
        #item
        #declared
    })
}

/// One flow, wherever it is declared: a function of its own or a method of
/// a service.
struct FlowDeclaration<'a> {
    args: &'a FlowArgs,
    kind: FlowKind,
    module: &'a syn::LitStr,
    name: String,
    ident: &'a syn::Ident,
    visibility: &'a syn::Visibility,
    attributes: &'a [syn::Attribute],
    /// How the registration calls the function that builds the flow.
    call: TokenStream,
    /// How the generated type's documentation links to that function.
    doc_target: String,
}

/// The unit type naming the flow, and its registration with the
/// application.
fn flow_declaration(declaration: FlowDeclaration<'_>) -> syn::Result<TokenStream> {
    let FlowDeclaration {
        args,
        kind,
        module,
        name,
        ident,
        visibility,
        attributes,
        call,
        doc_target,
    } = declaration;
    let marker = marker_ident(&name, ident.span());
    if marker == *ident {
        return Err(syn::Error::new(
            ident.span(),
            format!(
                "the function cannot share the flow's own name `{name}`; name the function differently and keep `name = \"{name}\"`"
            ),
        ));
    }
    let docs = attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .collect::<Vec<_>>();
    let documentation = doc_text(&docs).map(|text| quote! { __mxrs_flow.documentation(#text); });
    // A role is a variant of the enum its module declares its roles with,
    // a called flow the type its declaration generates, an entity its
    // struct: each resolves to the name the model knows it by.
    let relation = |list: &Option<Vec<Related>>,
                    method: TokenStream,
                    name_of: &dyn Fn(&syn::Path) -> TokenStream| {
        list.as_ref().map(|items| {
            let names = items.iter().map(|item| match item {
                Related::Name(name) => quote! { ::std::string::String::from(#name) },
                Related::Item(path) => name_of(path),
            });
            quote! {
                __mxrs_flow.#method(::std::vec::Vec::<::std::string::String>::from([#(#names),*]));
            }
        })
    };
    let flow_name = |path: &syn::Path| quote! { <#path as ::mxrs::FlowName>::flow_name() };
    let roles = relation(&args.roles, quote!(allowed_roles), &|path| {
        quote! { (#path).qualified_name() }
    });
    let calls = relation(&args.calls, quote!(declares_calls), &flow_name);
    let uses = relation(&args.uses, quote!(declares_uses), &|path| {
        quote! { <#path as ::mxrs::EntityMarker>::qualified_name() }
    });
    let used_by = relation(&args.used_by, quote!(declares_used_by), &flow_name);
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
        FlowKind::Rule => (
            quote!(::mxrs::RuleMarker),
            quote!(::mxrs::RuleModuleBuilder),
            quote!(rule),
            quote!(::mxrs::registry::Stage::Microflow),
            "rule",
        ),
    };
    let summary = format!(
        "The `{}.{name}` {noun}, declared by [`{doc_target}`].",
        module.value()
    );
    let placed = placed(args.folder.as_ref());

    Ok(quote! {
        #[doc = #summary]
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, Copy)]
        #visibility struct #marker;

        impl #marker_trait for #marker {
            const MODULE: &'static str = #module;
            const NAME: &'static str = #name;
        }

        impl ::mxrs::FlowName for #marker {
            fn flow_name() -> ::std::string::String {
                <Self as #marker_trait>::qualified_name()
            }
        }

        ::mxrs::inventory::submit! {
            ::mxrs::registry::Declaration::new(
                #stage,
                ::core::module_path!(),
                ::core::file!(),
                ::core::line!(),
                |__mxrs_project| {
                    let mut __mxrs_module = #module_builder::new(#module);
                    __mxrs_module.#method(#name, |__mxrs_flow| {
                        #documentation
                        #roles
                        #calls
                        #uses
                        #used_by
                        #call(__mxrs_flow);
                    });
                    __mxrs_project.merge_module(__mxrs_module.into_decl()#placed);
                },
            )
        }
    })
}

/// Arguments of `#[mxrs::service(...)]`.
pub struct ServiceArgs {
    module: syn::LitStr,
    /// What the service is about, named in each of its flows' names: an
    /// entity's struct, or the name as the model writes it.
    subject: Option<Subject>,
}

enum Subject {
    Item(syn::Path),
    Name(syn::LitStr),
}

impl Subject {
    fn text(&self) -> syn::Result<String> {
        match self {
            Subject::Name(name) => Ok(name.value()),
            Subject::Item(path) => path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
                .ok_or_else(|| syn::Error::new_spanned(path, "expected the subject's type")),
        }
    }
}

impl syn::parse::Parse for ServiceArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut module = None;
        let mut subject = None;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            let repeated = match key.to_string().as_str() {
                "module" => module.replace(input.parse()?).is_some(),
                "subject" if input.peek(syn::LitStr) => {
                    let name: syn::LitStr = input.parse()?;
                    let text = name.value();
                    if text.is_empty()
                        || !text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        return Err(syn::Error::new(
                            name.span(),
                            "a subject is a word of the flows' names: letters, digits and underscores",
                        ));
                    }
                    subject.replace(Subject::Name(name)).is_some()
                }
                "subject" => subject.replace(Subject::Item(input.parse()?)).is_some(),
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown service option; expected `module` or `subject`",
                    ));
                }
            };
            if repeated {
                return Err(syn::Error::new(
                    key.span(),
                    format!("`{key}` is stated once"),
                ));
            }
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
        }
        Ok(Self {
            module: module.ok_or_else(|| input.error("missing `module = \"ModuleName\"`"))?,
            subject,
        })
    }
}

/// A flow a service's method declares, as the service read it.
struct ServiceFlow {
    args: FlowArgs,
    kind: FlowKind,
    name: String,
    ident: syn::Ident,
    visibility: syn::Visibility,
    attributes: Vec<syn::Attribute>,
}

/// `#[service(module = "...", subject = ...)]` on an `impl` block: each
/// method marked `#[microflow(...)]` or `#[nanoflow(...)]` declares a flow
/// of the service's module, named `KIND_Subject_Method` unless it states
/// its name.
pub fn expand_service(args: &ServiceArgs, mut item: syn::ItemImpl) -> syn::Result<TokenStream> {
    if let Some((_, path, _)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "a service is an inherent `impl` block, not a trait implementation",
        ));
    }
    let subject = args.subject.as_ref().map(Subject::text).transpose()?;
    // A subject named by its struct is checked to exist, and an editor
    // follows it.
    let subject_check = match &args.subject {
        Some(Subject::Item(path)) => Some(quote! {
            const _: fn() = || {
                let _ = <#path as ::mxrs::EntityMarker>::qualified_name;
            };
        }),
        _ => None,
    };
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "a service is not generic: its flows are declared once",
        ));
    }
    let self_ty = item.self_ty.clone();
    let service_name = quote!(#self_ty).to_string().replace(' ', "");
    let mut flows = Vec::new();
    for member in &mut item.items {
        let syn::ImplItem::Fn(method) = member else {
            continue;
        };
        let kind_of = |attribute: &syn::Attribute| match attribute
            .path()
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .as_deref()
        {
            Some("microflow") => Some(FlowKind::Microflow),
            Some("nanoflow") => Some(FlowKind::Nanoflow),
            Some("rule") => Some(FlowKind::Rule),
            _ => None,
        };
        let Some(position) = method
            .attrs
            .iter()
            .position(|attribute| kind_of(attribute).is_some())
        else {
            continue;
        };
        let attribute = method.attrs.remove(position);
        let kind = kind_of(&attribute).expect("the attribute was found by its kind");
        let flow_args = match &attribute.meta {
            syn::Meta::Path(_) => syn::parse2::<FlowArgs>(TokenStream::new())?,
            _ => attribute.parse_args::<FlowArgs>()?,
        };
        if let Some(module) = &flow_args.module
            && module.value() != args.module.value()
        {
            return Err(syn::Error::new(
                module.span(),
                "a flow of a service belongs to the service's module",
            ));
        }
        check_flow_signature(&method.sig)?;
        let name = flow_args.mendix_name(&method.sig.ident, subject.as_deref())?;
        flows.push(ServiceFlow {
            args: flow_args,
            kind,
            name,
            ident: method.sig.ident.clone(),
            visibility: method.vis.clone(),
            attributes: method.attrs.clone(),
        });
    }
    let declarations = flows
        .iter()
        .map(|flow| {
            let ident = &flow.ident;
            flow_declaration(FlowDeclaration {
                args: &flow.args,
                kind: flow.kind,
                module: &args.module,
                name: flow.name.clone(),
                ident,
                visibility: &flow.visibility,
                attributes: &flow.attributes,
                call: quote!(<#self_ty>::#ident),
                doc_target: format!("{service_name}::{ident}"),
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(quote! {
        #item
        #subject_check
        #(#declarations)*
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
    if rust_keyword(&ident) || ident == "_" {
        ident.push('_');
    }
    syn::Ident::new(&ident, span)
}

/// Whether `ident` is a word Rust keeps for itself in any edition this
/// project builds with. The importer's `rust_keyword` is the same list: a
/// flow's type must be spelled identically by the macro that declares it and
/// by the source that names it.
fn rust_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "union"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    )
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
    folder: Option<syn::LitStr>,
}

impl syn::parse::Parse for DocumentArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut module = None;
        let mut name = None;
        let mut folder = None;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            match key.to_string().as_str() {
                "module" => module = Some(input.parse()?),
                "name" => name = Some(input.parse()?),
                "folder" => folder = Some(folder_path(input)?),
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown option; expected `module`, `name` or `folder`",
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
            folder,
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
    let placed = placed(args.folder.as_ref());
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
                |__mxrs_project| {
                    let mut __mxrs_module = ::mxrs::ModuleBuilder::new(#module);
                    __mxrs_module.#method(#name, |__mxrs_builder| {
                        #documentation
                        #ident(__mxrs_builder);
                    });
                    __mxrs_project.merge_module(__mxrs_module.into_decl()#placed);
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

/// Registers `apply`, which finds the project as `__mxrs_project`.
///
/// Everything an expansion binds is spelled `__mxrs_*`: the body calls the
/// user's own function by name, and a local called `project`, `module` or
/// `user` would be what that name meant for a function called the same.
fn registration(stage: TokenStream, apply: TokenStream) -> TokenStream {
    quote! {
        ::mxrs::inventory::submit! {
            ::mxrs::registry::Declaration::new(
                ::mxrs::registry::Stage::#stage,
                ::core::module_path!(),
                ::core::file!(),
                ::core::line!(),
                |__mxrs_project| { #apply },
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
    /// `folder = "..."`: the folder every document it declares lives in.
    folder: Option<syn::LitStr>,
}

impl syn::parse::Parse for DeclarationArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut module = None;
        let mut stage = None;
        let mut folder = None;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            match key.to_string().as_str() {
                "module" => module = Some(input.parse()?),
                "stage" => stage = Some(input.parse()?),
                "folder" => folder = Some(folder_path(input)?),
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown option; expected `module`, `stage` or `folder`",
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
            folder,
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
    let placed = placed(args.folder.as_ref());
    let registration = registration(
        stage,
        quote! {
            let mut __mxrs_module = ::mxrs::ModuleBuilder::new(#module);
            #ident(&mut __mxrs_module);
            __mxrs_project.merge_module(__mxrs_module.into_decl()#placed);
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
            let mut __mxrs_assembled =
                ::mxrs::ProjectBuilder::new(__mxrs_project.mendix_version.clone());
            __mxrs_assembled.#method(|__mxrs_builder| #ident(__mxrs_builder));
            __mxrs_project.#field = __mxrs_assembled.build().#field;
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
            let mut __mxrs_item = ::mxrs::NavigationItemBuilder::new(#caption);
            #ident(&mut __mxrs_item);
            __mxrs_project.navigation_item(#profile, __mxrs_item.into_decl());
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
/// declares; in a project that declares none it joins the security the model
/// already stores, which the writer leaves otherwise untouched — and refuses
/// to build when there is none to join.
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
            let mut __mxrs_user = ::mxrs::DemoUserBuilder::new(#name);
            #ident(&mut __mxrs_user);
            match __mxrs_project.security.as_mut() {
                Some(__mxrs_security) => __mxrs_security.demo_users.push(__mxrs_user.into_decl()),
                None => __mxrs_project.demo_users.push(__mxrs_user.into_decl()),
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
        roles.push(quote! { __mxrs_module.role(#name, #description); });
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
            let mut __mxrs_module = ::mxrs::ModuleBuilder::new(#module);
            __mxrs_module.clear_roles();
            #(#roles)*
            __mxrs_project.merge_module(__mxrs_module.into_decl());
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
