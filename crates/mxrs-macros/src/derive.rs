//! `#[derive(MxEntity)]` is the Cargo-native entity front end onto the same
//! `mxrs-dsl` builder API that `project! {}` targets. Primitive Rust/Mendix
//! field types are inferred, while `#[mxrs(...)]` carries only the metadata
//! the Rust type system cannot express.
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
//! The legacy `#[mx_entity]`/`#[mx_attribute]` spelling remains accepted for
//! source compatibility. Only plain structs with named fields are supported;
//! tuple/unit structs and enums are a clear compile error.

use quote::{format_ident, quote};
use syn::spanned::Spanned;

struct ParsedAttribute {
    kind: String,
    name: Option<String>,
    default: Option<syn::Expr>,
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
}

struct ParsedAssociation {
    name: String,
    target: syn::Type,
    reference_set: bool,
    documentation: Option<String>,
    owner_both: bool,
    storage_table: bool,
}

pub fn expand_derive(input: &syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let entity = parse_mx_entity(&input.attrs, struct_name)?;
    let entity_name = &entity.name;

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

    let mut attribute_stmts = Vec::with_capacity(fields.named.len());
    let mut association_stmts = Vec::new();
    let mut association_markers = Vec::new();
    for field in &fields.named {
        let field_ident = field
            .ident
            .as_ref()
            .expect("named field always has an ident");
        if let Some((reference_set, target)) = association_target(&field.ty) {
            if entity.module.is_none() {
                return Err(syn::Error::new_spanned(
                    field,
                    "Reference<T> and ReferenceSet<T> fields require #[mxrs(module = \"ModuleName\")] on the entity",
                ));
            }
            let association =
                parse_association(field, target, reference_set, entity_name, field_ident)?;
            let ParsedAssociation {
                name,
                target,
                reference_set,
                documentation,
                owner_both,
                storage_table,
            } = association;
            let marker = format_ident!(
                "__MxrsAssociation{}{}",
                struct_name,
                to_pascal_case(&field_ident.to_string())
            );
            let association_type = if reference_set {
                quote!(::mxrs_ir::AssociationType::ReferenceSet)
            } else {
                quote!(::mxrs_ir::AssociationType::Reference)
            };
            association_markers.push(quote! {
                #[doc(hidden)]
                struct #marker;
                impl ::mxrs_ir::AssociationMarker for #marker {
                    type From = #struct_name;
                    type To = #target;
                    const NAME: &'static str = #name;
                    const ASSOCIATION_TYPE: ::mxrs_ir::AssociationType = #association_type;
                }
            });
            let documentation = documentation.map(|value| {
                quote! { association.documentation = #value.to_string(); }
            });
            let owner = owner_both.then(|| {
                quote! { association.owner = ::mxrs_ir::AssociationOwner::Both; }
            });
            let storage = storage_table.then(|| {
                quote! { association.storage = ::mxrs_ir::AssociationStorage::Table; }
            });
            association_stmts.push(quote! {{
                let association = e.association::<#marker>();
                #documentation
                #owner
                #storage
            }});
            continue;
        }
        let attribute = parse_mx_attribute(field, entity.module.as_deref())?;
        let ParsedAttribute {
            kind,
            name,
            default,
            enumeration,
            enumeration_type,
            documentation,
            length,
            localize_date,
            required,
            unique,
        } = attribute;
        let method = attribute_kind_method(&kind, field)?;
        let mendix_name = name.unwrap_or_else(|| to_pascal_case(&field_ident.to_string()));
        let builder = if let Some(enumeration_type) = enumeration_type {
            quote! {
                e.#method(
                    #mendix_name,
                    <#enumeration_type as ::mxrs_ir::EnumerationMarker>::qualified_name(),
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
        let default =
            default.map(|expr| quote! { attribute.default_value = Some((#expr).to_string()); });
        let documentation =
            documentation.map(|value| quote! { attribute.documentation = #value.to_string(); });
        let length = length.map(|value| quote! { attribute.length = Some(#value); });
        let localize_date =
            localize_date.map(|value| quote! { attribute.localize_date = Some(#value); });
        attribute_stmts.push(quote! {{
            let attribute = #builder;
            #default
            #documentation
            #length
            attribute.required = #required;
            attribute.unique = #unique;
            #localize_date
        }});
    }

    let documentation = entity
        .documentation
        .as_ref()
        .map(|value| quote! { e.documentation(#value); });
    let persistable = entity
        .persistable
        .map(|value| quote! { e.persistable(#value); });
    let marker = entity.module.as_ref().map(|module| {
        quote! {
            impl ::mxrs_ir::EntityMarker for #struct_name {
                const MODULE: &'static str = #module;
                const NAME: &'static str = #entity_name;
            }
        }
    });

    Ok(quote! {
        impl #struct_name {
            /// Registers this entity on `m` — generated by `#[derive(MxEntity)]`.
            /// Lowers to exactly the `mxrs-dsl` calls a caller could write by
            /// hand; nothing here is reachable only through the derive.
            pub fn mx_register(m: &mut ::mxrs_dsl::ModuleBuilder) {
                m.entity(#entity_name, |e| {
                    #documentation
                    #persistable
                    #(#attribute_stmts)*
                    #(#association_stmts)*
                });
            }
        }

        #marker
        #(#association_markers)*
    })
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
    field_ident: &syn::Ident,
) -> syn::Result<ParsedAssociation> {
    let mut name = format!("{entity_name}_{}", to_pascal_case(&field_ident.to_string()));
    let mut documentation = None;
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
) -> syn::Result<ParsedEntity> {
    let mut entity = ParsedEntity {
        name: struct_name.to_string(),
        module: None,
        documentation: None,
        persistable: None,
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
            } else {
                Err(meta.error(
                    "unknown entity #[mxrs(...)] key; expected `name`, `module`, `documentation`, or `persistable`",
                ))
            }
        })?;
    }
    Ok(entity)
}

fn parse_mx_attribute(
    field: &syn::Field,
    entity_module: Option<&str>,
) -> syn::Result<ParsedAttribute> {
    let mut kind = None;
    let mut name = None;
    let mut default = None;
    let mut enumeration = None;
    let mut documentation = None;
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

/// `order_date` -> `OrderDate` — the Mendix attribute-naming convention.
fn to_pascal_case(field_name: &str) -> String {
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
}
