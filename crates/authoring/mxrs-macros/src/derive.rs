//! Entity declarations: `#[mxrs::entity]`, `#[mxrs::dto]`, `#[mxrs::view]`
//! and the older `#[derive(MxEntity)]`.
//!
//! All four are front ends onto the same `mxrs-dsl` builder API that
//! `project! {}` targets. Primitive Rust/Mendix field types are inferred,
//! while `#[mxrs(...)]` carries only the metadata the Rust type system cannot
//! express.
//!
//! ```
//! # fn main() {
//! use mxrs_macros::MxEntity;
//!
//! #[derive(MxEntity)]
//! #[mxrs(module = "Sales", persistable)]
//! struct Order {
//!     #[mxrs(default = "A-0000", required, length = 80)]
//!     number: mxrs_expr::MxString,
//!     total: mxrs_expr::MxDecimal,
//! }
//!
//! let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
//! project.module("Sales", |m| {
//!     Order::mx_register(m);
//! });
//! let definition = project.build();
//! let order = &definition.modules[0].entities[0];
//! assert_eq!(order.name, "Order");
//! assert_eq!(order.attributes[0].name, "Number");
//! # }
//! ```
//!
//! The attribute forms are the application-facing surface. They resolve
//! every path through the `mxrs` facade, register the entity with the
//! application on their own, read `///` comments as documentation, and give
//! each field an accessor (`Order::number()`), so no separately generated
//! marker file is needed to name an attribute or an association. They are
//! also authoritative where the derive merely preserves: an entity declared
//! with `#[mxrs::entity]` has exactly the indexes, event handlers, parent
//! and system members its source states.
//!
//! The legacy `#[mx_entity]`/`#[mx_attribute]` spelling remains accepted for
//! source compatibility. Only plain structs with named fields are supported;
//! tuple/unit structs and enums are a clear compile error.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;

/// Which authoring surface an expansion serves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// `#[derive(MxEntity)]`: implementation-crate paths, preserving
    /// defaults, no accessors.
    Derive,
    /// `#[mxrs::entity]` and friends: facade paths, authoritative
    /// declaration, accessors and self-registration.
    Facade,
}

/// How a facade entity relates to the application's model.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// A persistable entity the application stores.
    Entity,
    /// A non-persistable entity: a data transfer object.
    Dto,
    /// A non-persistable entity backed by an OQL view source document.
    View,
}

/// Arguments of `#[mxrs::entity(...)]`, `#[mxrs::dto(...)]` and
/// `#[mxrs::view(...)]`.
pub struct EntityArgs {
    module: syn::LitStr,
    name: Option<syn::LitStr>,
    source: Option<syn::LitStr>,
    imported: bool,
}

impl syn::parse::Parse for EntityArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut module = None;
        let mut name = None;
        let mut source = None;
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
                "source" => {
                    input.parse::<syn::Token![=]>()?;
                    source = Some(input.parse()?);
                }
                "imported" => imported = true,
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown entity option; expected `module`, `name`, `source`, or `imported`",
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
            source,
            imported,
        })
    }
}

struct ParsedAttribute {
    kind: String,
    name: Option<String>,
    default: Option<syn::Expr>,
    /// `no_default`: the attribute states that it has no default value at
    /// all, where an unstated one would mean the platform's.
    no_default: bool,
    enumeration: Option<String>,
    enumeration_type: Option<syn::Type>,
    documentation: Option<String>,
    length: Option<i32>,
    localize_date: Option<bool>,
    required: bool,
    unique: bool,
}

struct ParsedEntity {
    name: String,
    module: Option<String>,
    documentation: Option<String>,
    persistable: Option<bool>,
    image: Option<String>,
    generalizes: Option<syn::Path>,
    /// `stores(owner, created_date, ...)`: the system members a root entity
    /// keeps for every object.
    stores: Option<Vec<syn::Ident>>,
    indexes: Vec<ParsedIndex>,
    lifecycle: Vec<ParsedLifecycle>,
    preserve_indexes: bool,
    preserve_lifecycle: bool,
    preserve_image: bool,
    preserve_inheritance: bool,
}

struct ParsedIndex {
    members: Vec<ParsedIndexMember>,
    include_offline: bool,
}

enum ParsedIndexMember {
    Attribute { field: syn::Ident, ascending: bool },
    System { member: syn::Ident, ascending: bool },
}

struct ParsedLifecycle {
    event: &'static str,
    handler: LifecycleHandler,
    pass_event_object: Option<bool>,
    raise_error_on_false: Option<bool>,
}

enum LifecycleHandler {
    Marker(syn::Path),
    Named(syn::LitStr),
}

struct ParsedAssociation {
    name: String,
    target: syn::Type,
    reference_set: bool,
    documentation: Option<String>,
    owner_both: bool,
    storage_table: bool,
}

/// `::mxrs_dsl`/`::mxrs_ir`/`::mxrs_expr` for the derive, `::mxrs` for the
/// facade — an application crate depends on the facade alone.
struct Roots {
    dsl: TokenStream,
    ir: TokenStream,
    expr: TokenStream,
}

impl Roots {
    fn new(surface: Surface) -> Self {
        match surface {
            Surface::Derive => Self {
                dsl: quote!(::mxrs_dsl),
                ir: quote!(::mxrs_ir),
                expr: quote!(::mxrs_expr),
            },
            Surface::Facade => Self {
                dsl: quote!(::mxrs),
                ir: quote!(::mxrs),
                expr: quote!(::mxrs),
            },
        }
    }
}

pub fn expand_derive(input: &syn::DeriveInput) -> syn::Result<TokenStream> {
    let syn::Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "MxEntity can only be derived for a struct",
        ));
    };
    let syn::Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "MxEntity requires a struct with named fields",
        ));
    };
    let entity = parse_mx_entity(&input.attrs, &input.ident, Surface::Derive)?;
    expand(
        &input.ident,
        &input.vis,
        fields,
        entity,
        Surface::Derive,
        None,
    )
}

/// Expands `#[mxrs::entity]`, `#[mxrs::dto]` or `#[mxrs::view]`.
pub fn expand_entity(
    args: &EntityArgs,
    kind: EntityKind,
    item: &syn::ItemStruct,
) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "a Mendix entity cannot be generic",
        ));
    }
    let syn::Fields::Named(fields) = &item.fields else {
        return Err(syn::Error::new_spanned(
            &item.fields,
            "a Mendix entity is a struct with named fields",
        ));
    };
    let mut entity = parse_mx_entity(&item.attrs, &item.ident, Surface::Facade)?;
    if entity.module.is_some() || entity.persistable.is_some() {
        return Err(syn::Error::new_spanned(
            item,
            "`module` and `persistable` belong to the entity attribute itself: write `#[mxrs::entity(module = \"...\")]` or `#[mxrs::dto(module = \"...\")]`",
        ));
    }
    entity.module = Some(args.module.value());
    if let Some(name) = &args.name {
        entity.name = name.value();
    }
    entity.persistable = Some(kind == EntityKind::Entity);
    match (kind, &args.source) {
        (EntityKind::View, None) => {
            return Err(syn::Error::new_spanned(
                &args.module,
                "a view names the OQL view source it reads: `source = \"Module.Document\"`",
            ));
        }
        (EntityKind::Entity | EntityKind::Dto, Some(source)) => {
            return Err(syn::Error::new_spanned(
                source,
                "`source` is only valid on `#[mxrs::view]`",
            ));
        }
        _ => {}
    }

    let implementation = expand(
        &item.ident,
        &item.vis,
        fields,
        entity,
        Surface::Facade,
        Some(FacadeEntity {
            kind,
            source: args.source.as_ref().map(syn::LitStr::value),
            imported: args.imported,
        }),
    )?;
    let mut declaration = item.clone();
    strip_helper_attributes(&mut declaration.attrs);
    // The struct is a schema: its fields are read by the macro, never by code.
    declaration
        .attrs
        .push(syn::parse_quote!(#[allow(dead_code)]));
    if let syn::Fields::Named(fields) = &mut declaration.fields {
        for field in &mut fields.named {
            strip_helper_attributes(&mut field.attrs);
        }
    }
    Ok(quote! {
        #declaration
        #implementation
    })
}

struct FacadeEntity {
    kind: EntityKind,
    source: Option<String>,
    imported: bool,
}

fn strip_helper_attributes(attributes: &mut Vec<syn::Attribute>) {
    attributes.retain(|attribute| !attribute.path().is_ident("mxrs"));
}

struct AttributeField<'a> {
    ident: &'a syn::Ident,
    kind: String,
}

fn expand(
    struct_name: &syn::Ident,
    visibility: &syn::Visibility,
    fields: &syn::FieldsNamed,
    entity: ParsedEntity,
    surface: Surface,
    facade: Option<FacadeEntity>,
) -> syn::Result<TokenStream> {
    let roots = Roots::new(surface);
    let Roots { dsl, ir, expr } = &roots;
    let entity_name = &entity.name;
    // A view's attributes carry no stored value of their own, so the
    // authoritative defaults a stored entity gets would only fight the view.
    let authoritative = facade
        .as_ref()
        .is_some_and(|facade| facade.kind != EntityKind::View);
    let members_module = format_ident!("__mxrs_{}", struct_name);

    let mut attribute_stmts = Vec::with_capacity(fields.named.len());
    let mut association_stmts = Vec::new();
    let mut association_markers = Vec::new();
    let mut attribute_fields = Vec::new();
    let mut member_types = Vec::new();
    let mut member_impls = Vec::new();
    let mut accessors = Vec::new();
    for field in &fields.named {
        let field_ident = field
            .ident
            .as_ref()
            .expect("named field always has an ident");
        let field_name = unraw(field_ident);
        // The member type is spelled as the field is, raw prefix included:
        // `r#type` is a field and a type name alike, `type` is neither.
        let member = field_ident.clone();
        if let Some((reference_set, target)) = association_target(&field.ty) {
            if entity.module.is_none() {
                return Err(syn::Error::new_spanned(
                    field,
                    "Reference<T> and ReferenceSet<T> fields require #[mxrs(module = \"ModuleName\")] on the entity",
                ));
            }
            let association = parse_association(
                field,
                target,
                reference_set,
                entity_name,
                &field_name,
                surface,
            )?;
            let ParsedAssociation {
                name,
                target,
                reference_set,
                documentation,
                owner_both,
                storage_table,
            } = association;
            let association_type = if reference_set {
                quote!(#ir::AssociationType::ReferenceSet)
            } else {
                quote!(#ir::AssociationType::Reference)
            };
            let marker = if surface == Surface::Facade {
                member_types.push(quote! { pub struct #member; });
                accessors.push(quote! {
                    #[doc = concat!("The `", #name, "` association.")]
                    pub const fn #field_ident() -> #ir::AssociationRef<#members_module::#member> {
                        #ir::AssociationRef::new()
                    }
                });
                quote!(#members_module::#member)
            } else {
                let marker = format_ident!(
                    "__MxrsAssociation{}{}",
                    struct_name,
                    to_pascal_case(&field_name)
                );
                association_markers.push(quote! {
                    #[doc(hidden)]
                    struct #marker;
                });
                quote!(#marker)
            };
            association_markers.push(quote! {
                impl #ir::AssociationMarker for #marker {
                    type From = #struct_name;
                    type To = #target;
                    const NAME: &'static str = #name;
                    const ASSOCIATION_TYPE: #ir::AssociationType = #association_type;
                }
            });
            let documentation = documentation.map(|value| {
                quote! { association.documentation = #value.to_string(); }
            });
            let owner = owner_both.then(|| {
                quote! { association.owner = #ir::AssociationOwner::Both; }
            });
            let storage = storage_table.then(|| {
                quote! { association.storage = #ir::AssociationStorage::Table; }
            });
            association_stmts.push(quote! {{
                let association = e.association::<#marker>();
                #documentation
                #owner
                #storage
            }});
            continue;
        }
        let attribute = parse_mx_attribute(field, entity.module.as_deref(), surface)?;
        let ParsedAttribute {
            kind,
            name,
            default,
            no_default,
            enumeration,
            enumeration_type,
            documentation,
            length,
            localize_date,
            required,
            unique,
        } = attribute;
        let method = attribute_kind_method(&kind, field)?;
        let mendix_name = name.unwrap_or_else(|| to_pascal_case(&field_name));
        let builder = if let Some(enumeration_type) = enumeration_type {
            quote! {
                e.#method(
                    #mendix_name,
                    <#enumeration_type as #ir::EnumerationMarker>::qualified_name(),
                )
            }
        } else if kind == "enumeration" {
            let enumeration = enumeration.ok_or_else(|| {
                syn::Error::new_spanned(
                    field,
                    "enumeration attributes require `enumeration = \"Module.Enum\"`",
                )
            })?;
            quote! { e.#method(#mendix_name, #enumeration) }
        } else {
            if enumeration.is_some() {
                return Err(syn::Error::new_spanned(
                    field,
                    "`enumeration` is only valid when `kind = \"enumeration\"`",
                ));
            }
            quote! { e.#method(#mendix_name) }
        };
        let default = match default {
            Some(expr) => Some(match integer_literal(&expr) {
                // A whole number is the model's text as written. Evaluating
                // it would give it Rust's `i32`, which a Long does not fit.
                Some(digits) => quote! { attribute.default_value = Some(#digits.to_string()); },
                None => quote! { attribute.default_value = Some((#expr).to_string()); },
            }),
            None => (authoritative && !no_default)
                .then(|| authoritative_default(&kind))
                .flatten()
                .map(|value| quote! { attribute.default_value = Some(#value.to_string()); }),
        };
        let documentation =
            documentation.map(|value| quote! { attribute.documentation = #value.to_string(); });
        let length = length
            .or((authoritative && kind == "string").then_some(DEFAULT_STRING_LENGTH))
            .map(|value| quote! { attribute.length = Some(#value); });
        let localize_date = localize_date
            .or((authoritative && kind == "datetime").then_some(true))
            .map(|value| quote! { attribute.localize_date = Some(#value); });
        attribute_stmts.push(quote! {{
            let attribute = #builder;
            #default
            #documentation
            #length
            attribute.required = #required;
            attribute.unique = #unique;
            #localize_date
        }});
        if surface == Surface::Facade {
            let value = attribute_value_type(&kind, expr);
            member_types.push(quote! { pub struct #member; });
            member_impls.push(quote! {
                impl #ir::AttributeMarker for #members_module::#member {
                    type Entity = #struct_name;
                    const NAME: &'static str = #mendix_name;
                }
                impl #expr::TypedAttributeMarker for #members_module::#member {
                    type Value = #value;
                }
            });
            accessors.push(quote! {
                #[doc = concat!("The `", #mendix_name, "` attribute.")]
                pub const fn #field_ident() -> #ir::AttributeRef<#members_module::#member> {
                    #ir::AttributeRef::new()
                }
            });
        }
        attribute_fields.push(AttributeField {
            ident: field_ident,
            kind,
        });
    }

    let documentation = entity
        .documentation
        .as_ref()
        .map(|value| quote! { e.documentation(#value); });
    let persistable = entity
        .persistable
        .map(|value| quote! { e.persistable(#value); });
    let source = facade.as_ref().map(|facade| match &facade.source {
        Some(source) => quote! { e.oql_view(#source); },
        None => quote! { e.stored(); },
    });
    let image = match (&entity.image, entity.preserve_image) {
        (Some(image), _) => Some(quote! { e.image(#image); }),
        (None, false) if facade.is_some() => Some(quote! { e.clear_image(); }),
        _ => None,
    };
    // A stored entity or a DTO states its place in the hierarchy the way it
    // states its indexes: a parent, the system members a root keeps, or
    // neither — a plain root. A view has no such place to state.
    let generalizes = match (&entity.generalizes, &entity.stores) {
        (Some(parent), _) => Some(quote! { e.generalizes::<#parent>(); }),
        (None, Some(members)) => Some(quote! {
            e.system_members(|s| {
                #(s.#members(true);)*
            });
        }),
        (None, None) if authoritative && !entity.preserve_inheritance => Some(quote! { e.root(); }),
        (None, None) => None,
    };
    let indexes = expand_indexes(&entity, &attribute_fields, &facade, &roots, &members_module)?;
    let lifecycle = expand_lifecycle(&entity, &facade, &roots);
    let marker = entity.module.as_ref().map(|module| {
        quote! {
            impl #ir::EntityMarker for #struct_name {
                const MODULE: &'static str = #module;
                const NAME: &'static str = #entity_name;
            }
        }
    });
    let members = (surface == Surface::Facade).then(|| {
        quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types, non_snake_case)]
            #visibility mod #members_module {
                #(#member_types)*
            }
            #(#member_impls)*
        }
    });
    let registration = match (&facade, &entity.module) {
        (Some(facade), Some(module)) if !facade.imported => Some(quote! {
            #dsl::inventory::submit! {
                #dsl::registry::Declaration::new(
                    #dsl::registry::Stage::Entity,
                    ::core::module_path!(),
                    ::core::file!(),
                    ::core::line!(),
                    |__mxrs_project| {
                        let mut __mxrs_module = #dsl::ModuleBuilder::new(#module);
                        #struct_name::mx_register(&mut __mxrs_module);
                        __mxrs_project.merge_module(__mxrs_module.into_decl());
                    },
                )
            }
        }),
        _ => None,
    };

    Ok(quote! {
        impl #struct_name {
            /// Registers this entity on `m`. Lowers to exactly the `mxrs-dsl`
            /// calls a caller could write by hand; nothing here is reachable
            /// only through the macro.
            pub fn mx_register(m: &mut #dsl::ModuleBuilder) {
                m.entity(#entity_name, |e| {
                    #documentation
                    #persistable
                    #source
                    #image
                    #generalizes
                    #(#attribute_stmts)*
                    #(#association_stmts)*
                    #indexes
                    #lifecycle
                });
            }

            #(#accessors)*
        }

        #marker
        #members
        #(#association_markers)*
        #registration
    })
}

const DEFAULT_STRING_LENGTH: i32 = 200;

/// `42` or `-42` as decimal digits, when `expr` is exactly an unsuffixed
/// integer literal.
fn integer_literal(expr: &syn::Expr) -> Option<String> {
    fn digits(expr: &syn::Expr) -> Option<&syn::LitInt> {
        match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(literal),
                ..
            }) if literal.suffix().is_empty() => Some(literal),
            _ => None,
        }
    }
    match expr {
        syn::Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Neg(_),
            expr,
            ..
        }) => digits(expr).map(|literal| format!("-{}", literal.base10_digits())),
        _ => digits(expr).map(|literal| literal.base10_digits().to_string()),
    }
}

/// The default value Studio Pro gives a new stored attribute of `kind`.
/// A facade entity states it implicitly, so deleting a `default = ...`
/// option returns the attribute to this value rather than to whatever an
/// imported model happened to hold.
fn authoritative_default(kind: &str) -> Option<&'static str> {
    match kind {
        "boolean" => Some("false"),
        "integer" | "long" | "float" | "decimal" => Some("0"),
        "autonumber" => Some("1"),
        _ => None,
    }
}

fn attribute_value_type(kind: &str, expr: &TokenStream) -> TokenStream {
    match kind {
        "integer" => quote!(#expr::MxInteger),
        "long" | "autonumber" => quote!(#expr::MxLong),
        "float" => quote!(#expr::MxFloat),
        "decimal" => quote!(#expr::MxDecimal),
        "boolean" => quote!(#expr::MxBool),
        "datetime" => quote!(#expr::MxDateTime),
        "binary" => quote!(#expr::MxBinary),
        "enumeration" => quote!(#expr::MxEnumeration),
        _ => quote!(#expr::MxString),
    }
}

fn expand_indexes(
    entity: &ParsedEntity,
    attributes: &[AttributeField<'_>],
    facade: &Option<FacadeEntity>,
    roots: &Roots,
    members_module: &syn::Ident,
) -> syn::Result<Option<TokenStream>> {
    let ir = &roots.ir;
    if entity.indexes.is_empty() {
        // An entity that names no index has none — unless it says the
        // imported ones are to be kept.
        return Ok(
            (facade.is_some() && !entity.preserve_indexes).then(|| quote! { e.clear_indexes(); })
        );
    }
    if facade.is_none() {
        // Index members are checked through the accessors only the
        // attribute forms generate.
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "`index(...)` needs the entity declared with `#[mxrs::entity]`",
        ));
    }
    let mut statements = Vec::new();
    for index in &entity.indexes {
        let mut members = Vec::new();
        for member in &index.members {
            match member {
                ParsedIndexMember::Attribute { field, ascending } => {
                    let attribute = attributes
                        .iter()
                        .find(|attribute| unraw(attribute.ident) == unraw(field))
                        .ok_or_else(|| {
                            syn::Error::new(
                                field.span(),
                                format!("`{field}` is not an attribute field of this entity"),
                            )
                        })?;
                    if matches!(attribute.kind.as_str(), "binary") {
                        return Err(syn::Error::new(
                            field.span(),
                            "a binary attribute cannot be indexed",
                        ));
                    }
                    let method = if *ascending {
                        format_ident!("attribute")
                    } else {
                        format_ident!("attribute_descending")
                    };
                    let member = attribute.ident;
                    members.push(quote! { i.#method::<#members_module::#member>(); });
                }
                ParsedIndexMember::System { member, ascending } => {
                    let method = if *ascending {
                        format_ident!("system")
                    } else {
                        format_ident!("system_descending")
                    };
                    members.push(quote! { i.#method(#ir::SystemMember::#member); });
                }
            }
        }
        let include_offline = index
            .include_offline
            .then(|| quote! { i.include_offline(true); });
        statements.push(quote! {
            e.index(|i| {
                #(#members)*
                #include_offline
            });
        });
    }
    Ok(Some(quote! { #(#statements)* }))
}

fn expand_lifecycle(
    entity: &ParsedEntity,
    facade: &Option<FacadeEntity>,
    roots: &Roots,
) -> Option<TokenStream> {
    let ir = &roots.ir;
    if entity.lifecycle.is_empty() {
        return (facade.is_some() && !entity.preserve_lifecycle)
            .then(|| quote! { e.clear_lifecycle(); });
    }
    let statements = entity.lifecycle.iter().map(|callback| {
        let pass_event_object = callback
            .pass_event_object
            .map(|value| quote! { l.pass_event_object(#value); });
        let raise_error_on_false = callback
            .raise_error_on_false
            .map(|value| quote! { l.raise_error_on_false(#value); });
        let configure = quote! {
            |l| {
                let _ = &l;
                #pass_event_object
                #raise_error_on_false
            }
        };
        match &callback.handler {
            LifecycleHandler::Marker(marker) => {
                let method = format_ident!("{}", callback.event);
                quote! { e.#method::<#marker>(#configure); }
            }
            LifecycleHandler::Named(name) => {
                let event = format_ident!("{}", to_pascal_case(callback.event));
                quote! { e.lifecycle_handler(#ir::LifecycleEvent::#event, #name, #configure); }
            }
        }
    });
    Some(quote! { #(#statements)* })
}

fn association_target(ty: &syn::Type) -> Option<(bool, syn::Type)> {
    let ty = unwrapped_type(ty);
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    let reference_set = match segment.ident.to_string().as_str() {
        "Reference" => false,
        "ReferenceSet" => true,
        _ => return None,
    };
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let mut types = arguments.args.iter().filter_map(|argument| match argument {
        syn::GenericArgument::Type(target) => Some(target.clone()),
        _ => None,
    });
    let target = types.next()?;
    types.next().is_none().then_some((reference_set, target))
}

fn parse_association(
    field: &syn::Field,
    target: syn::Type,
    reference_set: bool,
    entity_name: &str,
    field_name: &str,
    surface: Surface,
) -> syn::Result<ParsedAssociation> {
    let mut name = format!("{entity_name}_{}", to_pascal_case(field_name));
    let mut documentation = match surface {
        Surface::Facade => doc_comment(&field.attrs),
        Surface::Derive => None,
    };
    let mut owner_both = false;
    let mut storage_table = false;
    for attribute in field
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("mxrs"))
    {
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("association") {
                name = meta.value()?.parse::<syn::LitStr>()?.value();
            } else if meta.path.is_ident("documentation") {
                documentation = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            } else if meta.path.is_ident("owner") {
                let value = meta.value()?.parse::<syn::LitStr>()?.value();
                owner_both = match value.as_str() {
                    "Default" => false,
                    "Both" => true,
                    _ => return Err(meta.error("association owner must be `Default` or `Both`")),
                };
            } else if meta.path.is_ident("storage") {
                let value = meta.value()?.parse::<syn::LitStr>()?.value();
                storage_table = match value.as_str() {
                    "Column" => false,
                    "Table" => true,
                    _ => return Err(meta.error("association storage must be `Column` or `Table`")),
                };
            } else {
                return Err(meta.error(
                    "unknown association #[mxrs(...)] key; expected `association`, `documentation`, `owner`, or `storage`",
                ));
            }
            Ok(())
        })?;
    }
    Ok(ParsedAssociation {
        name,
        target,
        reference_set,
        documentation,
        owner_both,
        storage_table,
    })
}

fn parse_mx_entity(
    attrs: &[syn::Attribute],
    struct_name: &syn::Ident,
    surface: Surface,
) -> syn::Result<ParsedEntity> {
    let mut entity = ParsedEntity {
        name: struct_name.to_string(),
        module: None,
        documentation: match surface {
            Surface::Facade => doc_comment(attrs),
            Surface::Derive => None,
        },
        persistable: None,
        image: None,
        generalizes: None,
        stores: None,
        indexes: Vec::new(),
        lifecycle: Vec::new(),
        preserve_indexes: false,
        preserve_lifecycle: false,
        preserve_image: false,
        preserve_inheritance: false,
    };
    for attr in attrs {
        if attr.path().is_ident("mx_entity") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    entity.name = meta.value()?.parse::<syn::LitStr>()?.value();
                    Ok(())
                } else {
                    Err(meta.error("unknown #[mx_entity(...)] key, expected `name`"))
                }
            })?;
            continue;
        }
        if !attr.path().is_ident("mxrs") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                entity.name = meta.value()?.parse::<syn::LitStr>()?.value();
                Ok(())
            } else if meta.path.is_ident("module") {
                entity.module = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("documentation") {
                entity.documentation = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("persistable") {
                entity.persistable = Some(if meta.input.peek(syn::Token![=]) {
                    meta.value()?.parse::<syn::LitBool>()?.value()
                } else {
                    true
                });
                Ok(())
            } else if meta.path.is_ident("image") {
                entity.image = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("generalizes") {
                entity.generalizes = Some(meta.value()?.parse::<syn::Path>()?);
                Ok(())
            } else if meta.path.is_ident("stores") {
                let members = entity.stores.get_or_insert_with(Vec::new);
                meta.parse_nested_meta(|member| {
                    let ident = member
                        .path
                        .get_ident()
                        .filter(|ident| {
                            matches!(
                                ident.to_string().as_str(),
                                "owner" | "created_date" | "changed_date" | "changed_by"
                            )
                        })
                        .ok_or_else(|| {
                            member.error(
                                "unknown system member; expected `owner`, `created_date`, `changed_date`, or `changed_by`",
                            )
                        })?;
                    if members.contains(ident) {
                        return Err(member.error("this system member is already stored"));
                    }
                    members.push(ident.clone());
                    Ok(())
                })
            } else if meta.path.is_ident("index") {
                entity.indexes.push(parse_index(&meta)?);
                Ok(())
            } else if let Some(event) = lifecycle_event(&meta.path) {
                entity.lifecycle.push(parse_lifecycle(&meta, event)?);
                Ok(())
            } else if meta.path.is_ident("preserve") {
                meta.parse_nested_meta(|preserved| {
                    if preserved.path.is_ident("indexes") {
                        entity.preserve_indexes = true;
                    } else if preserved.path.is_ident("lifecycle") {
                        entity.preserve_lifecycle = true;
                    } else if preserved.path.is_ident("image") {
                        entity.preserve_image = true;
                    } else if preserved.path.is_ident("inheritance") {
                        entity.preserve_inheritance = true;
                    } else {
                        return Err(preserved.error(
                            "unknown `preserve(...)` member; expected `indexes`, `lifecycle`, `image`, or `inheritance`",
                        ));
                    }
                    Ok(())
                })
            } else {
                Err(meta.error(
                    "unknown entity #[mxrs(...)] key; expected `name`, `module`, `documentation`, `persistable`, `image`, `generalizes`, `stores`, `index`, `before_commit`, `after_commit`, `before_delete`, `after_delete`, or `preserve`",
                ))
            }
        })?;
    }
    if let (Some(parent), Some(_)) = (&entity.generalizes, &entity.stores) {
        return Err(syn::Error::new_spanned(
            parent,
            "a specialization inherits its parent's system members; state `generalizes` or `stores(...)`, not both",
        ));
    }
    if entity.preserve_inheritance && (entity.generalizes.is_some() || entity.stores.is_some()) {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "`preserve(inheritance)` keeps the imported hierarchy; it cannot be combined with `generalizes` or `stores(...)`",
        ));
    }
    Ok(entity)
}

fn lifecycle_event(path: &syn::Path) -> Option<&'static str> {
    [
        "before_commit",
        "after_commit",
        "before_delete",
        "after_delete",
    ]
    .into_iter()
    .find(|event| path.is_ident(event))
}

/// `index(number, desc(created), system(ChangedDate), include_offline)`.
fn parse_index(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<ParsedIndex> {
    let mut index = ParsedIndex {
        members: Vec::new(),
        include_offline: false,
    };
    meta.parse_nested_meta(|member| {
        if member.path.is_ident("include_offline") {
            index.include_offline = if member.input.peek(syn::Token![=]) {
                member.value()?.parse::<syn::LitBool>()?.value()
            } else {
                true
            };
            return Ok(());
        }
        if member.path.is_ident("desc") && member.input.peek(syn::token::Paren) {
            return member.parse_nested_meta(|descending| {
                index.members.push(parse_index_member(&descending, false)?);
                Ok(())
            });
        }
        index.members.push(parse_index_member(&member, true)?);
        Ok(())
    })?;
    if index.members.is_empty() {
        return Err(meta.error("an index names at least one member"));
    }
    Ok(index)
}

fn parse_index_member(
    meta: &syn::meta::ParseNestedMeta<'_>,
    ascending: bool,
) -> syn::Result<ParsedIndexMember> {
    // `system(...)` names a system member; a bare `system` is a field of
    // that name, like any other.
    if meta.path.is_ident("system") && meta.input.peek(syn::token::Paren) {
        let mut system = None;
        meta.parse_nested_meta(|member| {
            let ident = member
                .path
                .get_ident()
                .ok_or_else(|| member.error("expected a system member name"))?;
            if !matches!(
                ident.to_string().as_str(),
                "CreatedDate" | "ChangedDate" | "Owner" | "ChangedBy"
            ) {
                return Err(member.error(
                    "unknown system member; expected `CreatedDate`, `ChangedDate`, `Owner`, or `ChangedBy`",
                ));
            }
            system = Some(ident.clone());
            Ok(())
        })?;
        let member = system.ok_or_else(|| meta.error("`system(...)` names one system member"))?;
        return Ok(ParsedIndexMember::System { member, ascending });
    }
    let field = meta
        .path
        .get_ident()
        .ok_or_else(|| meta.error("expected an attribute field name"))?
        .clone();
    Ok(ParsedIndexMember::Attribute { field, ascending })
}

/// `before_commit = Handler`, `before_commit = "Module.Handler"`, or
/// `before_commit(Handler, pass_event_object = false)`.
fn parse_lifecycle(
    meta: &syn::meta::ParseNestedMeta<'_>,
    event: &'static str,
) -> syn::Result<ParsedLifecycle> {
    let mut callback = ParsedLifecycle {
        event,
        handler: LifecycleHandler::Named(syn::LitStr::new("", proc_macro2::Span::call_site())),
        pass_event_object: None,
        raise_error_on_false: None,
    };
    if meta.input.peek(syn::Token![=]) {
        callback.handler = parse_lifecycle_handler(meta.value()?)?;
        return Ok(callback);
    }
    let content;
    syn::parenthesized!(content in meta.input);
    callback.handler = parse_lifecycle_handler(&content)?;
    while !content.is_empty() {
        content.parse::<syn::Token![,]>()?;
        if content.is_empty() {
            break;
        }
        let key: syn::Ident = content.parse()?;
        content.parse::<syn::Token![=]>()?;
        let value = content.parse::<syn::LitBool>()?.value();
        match key.to_string().as_str() {
            "pass_event_object" => callback.pass_event_object = Some(value),
            "raise_error_on_false" => callback.raise_error_on_false = Some(value),
            _ => {
                return Err(syn::Error::new(
                    key.span(),
                    "unknown event handler option; expected `pass_event_object` or `raise_error_on_false`",
                ));
            }
        }
    }
    Ok(callback)
}

fn parse_lifecycle_handler(input: syn::parse::ParseStream<'_>) -> syn::Result<LifecycleHandler> {
    if input.peek(syn::LitStr) {
        let name: syn::LitStr = input.parse()?;
        if !name.value().contains('.') {
            return Err(syn::Error::new(
                name.span(),
                "a named event handler is qualified: `\"Module.Microflow\"`",
            ));
        }
        return Ok(LifecycleHandler::Named(name));
    }
    Ok(LifecycleHandler::Marker(input.parse()?))
}

/// The item's `///` comment as Mendix documentation: one leading space is
/// the comment's own, everything after it is the author's.
fn doc_comment(attrs: &[syn::Attribute]) -> Option<String> {
    let lines = attrs
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

fn parse_mx_attribute(
    field: &syn::Field,
    entity_module: Option<&str>,
    surface: Surface,
) -> syn::Result<ParsedAttribute> {
    let mut kind = None;
    let mut name = None;
    let mut default = None;
    let mut no_default = false;
    let mut enumeration = None;
    let mut documentation = match surface {
        Surface::Facade => doc_comment(&field.attrs),
        Surface::Derive => None,
    };
    let mut length = None;
    let mut localize_date = None;
    let mut required = false;
    let mut unique = false;
    for attr in &field.attrs {
        let legacy = attr.path().is_ident("mx_attribute");
        if !legacy && !attr.path().is_ident("mxrs") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("kind") {
                let lit: syn::LitStr = meta.value()?.parse()?;
                kind = Some(lit.value());
                Ok(())
            } else if meta.path.is_ident("default") {
                default = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("enumeration") {
                let lit: syn::LitStr = meta.value()?.parse()?;
                enumeration = Some(lit.value());
                Ok(())
            } else if !legacy && meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if !legacy && meta.path.is_ident("documentation") {
                documentation = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else if !legacy && meta.path.is_ident("length") {
                length = Some(meta.value()?.parse::<syn::LitInt>()?.base10_parse()?);
                Ok(())
            } else if !legacy && meta.path.is_ident("unlimited") {
                length = Some(0);
                Ok(())
            } else if !legacy && meta.path.is_ident("no_default") {
                no_default = true;
                Ok(())
            } else if !legacy && meta.path.is_ident("localize_date") {
                localize_date = Some(meta.value()?.parse::<syn::LitBool>()?.value());
                Ok(())
            } else if !legacy && meta.path.is_ident("required") {
                required = if meta.input.peek(syn::Token![=]) {
                    meta.value()?.parse::<syn::LitBool>()?.value()
                } else {
                    true
                };
                Ok(())
            } else if !legacy && meta.path.is_ident("unique") {
                unique = if meta.input.peek(syn::Token![=]) {
                    meta.value()?.parse::<syn::LitBool>()?.value()
                } else {
                    true
                };
                Ok(())
            } else {
                Err(meta.error("unknown attribute option"))
            }
        })?;
    }
    let kind_was_explicit = kind.is_some();
    let inferred_enumeration = type_ident(&field.ty).filter(|_| !kind_was_explicit);
    let kind = kind
        .or_else(|| infer_attribute_kind(&field.ty))
        .ok_or_else(|| {
            syn::Error::new_spanned(
                field,
                "cannot infer a Mendix attribute type; add #[mxrs(kind = \"...\")]",
            )
        })?;
    if no_default && default.is_some() {
        return Err(syn::Error::new_spanned(
            field,
            "state either `default = ...` or `no_default`, not both",
        ));
    }
    if length.is_some() && kind != "string" {
        return Err(syn::Error::new_spanned(
            field,
            "`length` and `unlimited` apply to string attributes only",
        ));
    }
    if kind == "enumeration" && enumeration.is_none() {
        enumeration = inferred_enumeration.map(|name| match entity_module {
            Some(module) => format!("{module}.{name}"),
            None => name,
        });
    }
    let enumeration_type = if kind == "enumeration" && !kind_was_explicit {
        Some(unwrapped_type(&field.ty).clone())
    } else {
        None
    };
    Ok(ParsedAttribute {
        kind,
        name,
        default,
        no_default,
        enumeration,
        enumeration_type,
        documentation,
        length,
        localize_date,
        required,
        unique,
    })
}

fn unwrapped_type(ty: &syn::Type) -> &syn::Type {
    let syn::Type::Path(path) = ty else {
        return ty;
    };
    let Some(segment) = path.path.segments.last() else {
        return ty;
    };
    if segment.ident == "Option"
        && let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner)) = arguments.args.first()
    {
        return unwrapped_type(inner);
    }
    ty
}

fn type_ident(ty: &syn::Type) -> Option<String> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident == "Option"
        && let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner)) = arguments.args.first()
    {
        return type_ident(inner);
    }
    Some(segment.ident.to_string())
}

fn infer_attribute_kind(ty: &syn::Type) -> Option<String> {
    let ident = type_ident(ty)?;
    Some(
        match ident.as_str() {
            "String" | "MxString" => "string",
            "i32" | "MxInteger" => "integer",
            "i64" | "MxLong" => "long",
            "f32" | "MxFloat" => "float",
            "f64" | "MxDecimal" => "decimal",
            "bool" | "MxBool" => "boolean",
            "MxDateTime" => "datetime",
            "MxBinary" | "Vec" => "binary",
            "MxEnumeration" => return None,
            _ => "enumeration",
        }
        .to_string(),
    )
}

fn attribute_kind_method(kind: &str, field: &syn::Field) -> syn::Result<syn::Ident> {
    let method = match kind {
        "string" => "string",
        "integer" => "integer",
        "long" => "long",
        "float" => "float",
        "decimal" => "decimal",
        "boolean" => "boolean",
        "datetime" => "datetime",
        "autonumber" => "autonumber",
        "hash_string" => "hash_string",
        "binary" => "binary",
        "enumeration" => "enumeration",
        other => {
            return Err(syn::Error::new_spanned(
                field,
                format!(
                    "unknown attribute kind `{other}` (expected one of: string, integer, long, float, decimal, boolean, datetime, autonumber, hash_string, binary, enumeration)"
                ),
            ));
        }
    };
    Ok(syn::Ident::new(method, field.span()))
}

/// A field identifier without its raw prefix: `r#type` names the Mendix
/// attribute `Type`, not `R#type`.
fn unraw(ident: &syn::Ident) -> String {
    let name = ident.to_string();
    name.strip_prefix("r#").map(str::to_string).unwrap_or(name)
}

/// `order_date` -> `OrderDate` — the Mendix attribute-naming convention.
pub(crate) fn to_pascal_case(field_name: &str) -> String {
    field_name
        .split('_')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_snake_case_field_names_to_pascal_case() {
        assert_eq!(to_pascal_case("number"), "Number");
        assert_eq!(to_pascal_case("order_date"), "OrderDate");
        assert_eq!(to_pascal_case("id"), "Id");
    }

    #[test]
    fn doc_comments_become_documentation_without_the_comment_space() {
        let item: syn::ItemStruct = syn::parse_quote! {
            /// First line.
            ///
            ///   indented
            struct Documented;
        };
        assert_eq!(
            doc_comment(&item.attrs).as_deref(),
            Some("First line.\n\n  indented")
        );
        let bare: syn::ItemStruct = syn::parse_quote! { struct Bare; };
        assert_eq!(doc_comment(&bare.attrs), None);
    }
}
