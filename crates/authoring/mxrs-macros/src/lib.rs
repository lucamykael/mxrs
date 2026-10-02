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
mod declare;
mod derive;
mod derive_enumeration;
mod expand;
mod parse;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

use crate::parse::ProjectInput;

/// Declares the root of a Cargo-native Mendix application.
///
/// The application's model is every declaration its crate registered —
/// `#[mxrs::entity]`, `#[mxrs::microflow]` and the rest. A crate that also
/// builds part of its model by hand passes that entry point as
/// `project = crate::build`; registered declarations are folded in after it.
#[proc_macro_attribute]
pub fn application(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as application::ApplicationArgs);
    let item = parse_macro_input!(item as syn::ItemStruct);
    match application::expand(&args, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Declares a persistable Mendix entity.
///
/// The struct is the entity: its fields are the attributes and
/// associations, `///` comments are the documentation, and `#[mxrs(...)]`
/// carries what the types cannot say. The entity registers itself with the
/// application and gets an accessor per field (`Order::number()`).
///
/// ```text
/// /// A customer order.
/// #[entity(module = "Sales")]
/// #[mxrs(index(number), before_commit = validate_order)]
/// pub struct Order {
///     #[mxrs(length = 80, required)]
///     pub number: MxString,
///     pub total: MxDecimal,
///     pub customer: Reference<Customer>,
/// }
/// ```
///
/// `imported` declares the type without registering it — for an entity an
/// installed module owns, which the application names but does not define.
#[proc_macro_attribute]
pub fn entity(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_entity(attributes, item, derive::EntityKind::Entity)
}

/// Declares a non-persistable Mendix entity: a data transfer object. Same
/// shape as [`macro@entity`].
#[proc_macro_attribute]
pub fn dto(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_entity(attributes, item, derive::EntityKind::Dto)
}

/// Declares an entity backed by an OQL view: `#[view(module = "Sales",
/// source = "Sales.OrderTotals")]`. Same shape as [`macro@entity`].
#[proc_macro_attribute]
pub fn view(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_entity(attributes, item, derive::EntityKind::View)
}

fn expand_entity(
    attributes: TokenStream,
    item: TokenStream,
    kind: derive::EntityKind,
) -> TokenStream {
    let args = parse_macro_input!(attributes as derive::EntityArgs);
    let item = parse_macro_input!(item as syn::ItemStruct);
    match derive::expand_entity(&args, kind, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Declares a Mendix enumeration from a Rust enum. Variants are the values;
/// `#[mxrs(caption = "...")]` captions one, and a value without a caption is
/// captioned with its own name.
#[proc_macro_attribute]
pub fn enumeration(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as derive_enumeration::EnumerationArgs);
    let item = parse_macro_input!(item as syn::ItemEnum);
    match derive_enumeration::expand_enumeration(&args, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Declares a server-side microflow as the function that builds it.
///
/// A naming prefix in capitals states the flow's kind and names it by
/// convention: `#[microflow(ACT, module = "Sales")] fn create_order` is
/// `Sales.ACT_CreateOrder`. `name = "..."` states the Mendix name outright
/// when it does not follow the convention. The function becomes a unit type
/// of the same name, so the flow is nameable wherever the model refers to
/// one, and registers itself with the application.
#[proc_macro_attribute]
pub fn microflow(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_flow(attributes, item, declare::FlowKind::Microflow)
}

/// Declares a service: an `impl` block whose methods marked
/// `#[microflow(...)]` are the flows of one subject.
///
/// ```text
/// pub struct AssetTypeService;
///
/// #[service(module = "Catalogs", subject = AssetType)]
/// impl AssetTypeService {
///     // Declares `Catalogs.ACT_AssetType_Edit`, nameable as
///     // `ACT_AssetType_Edit`.
///     #[microflow(ACT)]
///     pub fn edit(flow: &mut FlowBuilder) {}
/// }
/// ```
///
/// Every flow takes the service's module and is named `KIND_Subject_Method`
/// unless it states `name = "..."`. `subject` is an entity's struct, or the
/// subject as the model's names write it (`subject = "RubyCrud"`); a service
/// without one names its flows `KIND_Method`.
#[proc_macro_attribute]
pub fn service(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as declare::ServiceArgs);
    let item = parse_macro_input!(item as syn::ItemImpl);
    match declare::expand_service(&args, item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Declares a client-side nanoflow. Same shape as [`macro@microflow`].
#[proc_macro_attribute]
pub fn nanoflow(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_flow(attributes, item, declare::FlowKind::Nanoflow)
}

/// Declares a page as the function that builds it: `#[page(module =
/// "Sales")] fn order_overview(page: &mut PageBuilder)` is
/// `Sales.OrderOverview`. `name = "..."` states a Mendix name the function
/// does not spell, and `///` comments are the page's documentation.
#[proc_macro_attribute]
pub fn page(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_document(attributes, item, declare::DocumentKind::Page)
}

/// Declares a page layout. Same shape as [`macro@page`], with a
/// `LayoutBuilder`.
#[proc_macro_attribute]
pub fn layout(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_document(attributes, item, declare::DocumentKind::Layout)
}

/// Declares a constant. Same shape as [`macro@page`], with a
/// `ConstantBuilder`.
#[proc_macro_attribute]
pub fn constant(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_document(attributes, item, declare::DocumentKind::Constant)
}

/// Declares a standalone menu document. Same shape as [`macro@page`], with a
/// `MenuBuilder`.
#[proc_macro_attribute]
pub fn menu(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_document(attributes, item, declare::DocumentKind::Menu)
}

/// Declares into one module through its `ModuleBuilder`: the general form
/// for what the more specific attributes do not cover — a regular
/// expression, a scheduled event, a task queue.
///
/// ```text
/// #[declaration(module = "Sales")]
/// pub fn order_number_format(module: &mut ModuleBuilder) {
///     module.regular_expression("OrderNumberFormat", "^A-[0-9]{4}$", |_| {});
/// }
/// ```
#[proc_macro_attribute]
pub fn declaration(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as declare::DeclarationArgs);
    let item = parse_macro_input!(item as syn::ItemFn);
    match declare::expand_declaration(&args, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Declares the project's security from a function taking the
/// `SecurityBuilder`.
#[proc_macro_attribute]
pub fn security(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_project(attributes, item, declare::ProjectKind::Security)
}

/// Declares the project's navigation from a function taking the
/// `NavigationBuilder`.
#[proc_macro_attribute]
pub fn navigation(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_project(attributes, item, declare::ProjectKind::Navigation)
}

fn expand_project(
    attributes: TokenStream,
    item: TokenStream,
    kind: declare::ProjectKind,
) -> TokenStream {
    if !attributes.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "this attribute takes no options",
        )
        .to_compile_error()
        .into();
    }
    let item = parse_macro_input!(item as syn::ItemFn);
    match declare::expand_project(kind, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Adds one item to a navigation profile from where the item belongs — next
/// to the page it opens — instead of from the profile's own declaration.
#[proc_macro_attribute]
pub fn navigation_item(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as declare::NavigationItemArgs);
    let item = parse_macro_input!(item as syn::ItemFn);
    match declare::expand_navigation_item(&args, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Declares a local demo user from a function taking the `DemoUserBuilder`.
#[proc_macro_attribute]
pub fn demo_user(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as declare::DemoUserArgs);
    let item = parse_macro_input!(item as syn::ItemFn);
    match declare::expand_demo_user(&args, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Declares a module's roles as an enum: each variant a role, its `///`
/// comment the description.
#[proc_macro_attribute]
pub fn module_roles(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as declare::ModuleRolesArgs);
    let item = parse_macro_input!(item as syn::ItemEnum);
    match declare::expand_module_roles(&args, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_document(
    attributes: TokenStream,
    item: TokenStream,
    kind: declare::DocumentKind,
) -> TokenStream {
    let args = parse_macro_input!(attributes as declare::DocumentArgs);
    let item = parse_macro_input!(item as syn::ItemFn);
    match declare::expand_document(&args, kind, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_flow(attributes: TokenStream, item: TokenStream, kind: declare::FlowKind) -> TokenStream {
    let args = parse_macro_input!(attributes as declare::FlowArgs);
    let item = parse_macro_input!(item as syn::ItemFn);
    match declare::expand_flow(&args, kind, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Marks an HTTP controller operation generated from a Mendix Published REST
/// operation. Routing is still assembled by the selected web framework; this
/// annotation keeps method and path visible on the handler itself.
#[proc_macro_attribute]
pub fn route(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let parser =
        syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated;
    let attributes = parse_macro_input!(attributes with parser);
    let item = proc_macro2::TokenStream::from(item);
    let mut seen = std::collections::HashSet::new();
    let mut method = None;
    for attribute in attributes {
        let Some(name) = attribute.path.get_ident().map(ToString::to_string) else {
            return syn::Error::new_spanned(attribute.path, "expected a simple route key")
                .to_compile_error()
                .into();
        };
        if !matches!(name.as_str(), "method" | "path" | "service" | "operation") {
            return syn::Error::new_spanned(
                attribute.path,
                "unknown #[mxrs::route] key; expected `method`, `path`, `service`, or `operation`",
            )
            .to_compile_error()
            .into();
        }
        if !seen.insert(name.clone()) {
            return syn::Error::new_spanned(attribute.path, format!("duplicate `{name}`"))
                .to_compile_error()
                .into();
        }
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(value),
            ..
        }) = attribute.value
        else {
            return syn::Error::new_spanned(attribute.value, "route values must be strings")
                .to_compile_error()
                .into();
        };
        if name == "method" {
            method = Some(value);
        }
    }
    let Some(method) = method else {
        return syn::Error::new(proc_macro2::Span::call_site(), "missing required `method`")
            .to_compile_error()
            .into();
    };
    const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    if !METHODS.contains(&method.value().as_str()) {
        return syn::Error::new_spanned(method, "unsupported HTTP method")
            .to_compile_error()
            .into();
    }
    quote!(#item).into()
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
