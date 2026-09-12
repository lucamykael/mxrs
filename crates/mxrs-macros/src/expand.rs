//! Lowers `project! {}` to the public `mxrs-dsl` builder surface and emits
//! the marker types needed by typed flow expressions in the same expansion.

use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, Path, Result};

use crate::parse::{
    AssociationInput, AttrKind, AttributeInput, EntityInput, FlowItem, MappingInput, MemberInput,
    MicroflowInput, ModuleInput, ProjectInput,
};

#[derive(Clone)]
enum Binding {
    Object(Path),
    List(Path),
}

pub fn expand(input: &ProjectInput) -> Result<TokenStream> {
    let version = &input.version;
    let marker_modules: Vec<TokenStream> = input.modules.iter().map(expand_marker_module).collect();
    let extension_imports = input.modules.iter().flat_map(|module| {
        module.entities.iter().map(move |entity| {
            let module_name = &module.name;
            let extension = format_ident!("{}VarExt", entity.name);
            quote! { use __mxrs_markers::#module_name::#extension as _; }
        })
    });
    let module_aliases = input.modules.iter().map(|module| {
        let module_name = &module.name;
        quote! { use __mxrs_markers::#module_name as #module_name; }
    });
    let module_stmts: Vec<TokenStream> = input
        .modules
        .iter()
        .map(expand_module)
        .collect::<Result<_>>()?;
    Ok(quote! {
        {
            mod __mxrs_markers {
                #(#marker_modules)*
            }
            #(#module_aliases)*
            #(#extension_imports)*
            let mut __mxrs_project = ::mxrs_dsl::ProjectBuilder::new(#version);
            #(#module_stmts)*
            __mxrs_project.build()
        }
    })
}

fn expand_marker_module(module: &ModuleInput) -> TokenStream {
    let module_ident = &module.name;
    let module_name = module.name.to_string();
    let entities = module
        .entities
        .iter()
        .map(|entity| expand_entity_markers(entity, &module_name));
    quote! {
        #[allow(non_snake_case, non_camel_case_types, dead_code)]
        pub mod #module_ident {
            #(#entities)*
        }
    }
}

fn expand_entity_markers(entity: &EntityInput, module_name: &str) -> TokenStream {
    let entity_ident = &entity.name;
    let entity_name = entity.name.to_string();
    let extension_ident = format_ident!("{}VarExt", entity.name);
    let attributes = entity.attributes.iter().map(|attribute| {
        let marker = format_ident!("{}_{}", entity.name, attribute.name);
        let attribute_name = attribute.name.to_string();
        let value_type = expression_type(&attribute.kind);
        quote! {
            pub struct #marker;
            impl ::mxrs_ir::AttributeMarker for #marker {
                type Entity = #entity_ident;
                const NAME: &'static str = #attribute_name;
            }
            impl ::mxrs_expr::TypedAttributeMarker for #marker {
                type Value = #value_type;
            }
        }
    });
    let associations = entity.associations.iter().map(|association| {
        let marker = format_ident!("{}_{}", entity.name, association.name);
        let association_name = association.name.to_string();
        let association_type = &association.association_type;
        let target = &association.target;
        quote! {
            pub struct #marker;
            impl ::mxrs_ir::AssociationMarker for #marker {
                type From = #entity_ident;
                type To = super::#target;
                const NAME: &'static str = #association_name;
                const ASSOCIATION_TYPE: ::mxrs_ir::AssociationType =
                    ::mxrs_ir::AssociationType::#association_type;
            }
        }
    });
    let methods = entity.attributes.iter().map(|attribute| {
        let method = format_ident!("{}", to_snake_case(&attribute.name.to_string()));
        let value_type = expression_type(&attribute.kind);
        quote! { fn #method(&self) -> ::mxrs_expr::Expr<#value_type>; }
    });
    let method_impls = entity.attributes.iter().map(|attribute| {
        let marker = format_ident!("{}_{}", entity.name, attribute.name);
        let method = format_ident!("{}", to_snake_case(&attribute.name.to_string()));
        quote! {
            fn #method(&self) -> ::mxrs_expr::Expr<
                <#marker as ::mxrs_expr::TypedAttributeMarker>::Value
            > {
                self.attribute::<#marker>()
            }
        }
    });
    quote! {
        pub struct #entity_ident;
        impl ::mxrs_ir::EntityMarker for #entity_ident {
            const MODULE: &'static str = #module_name;
            const NAME: &'static str = #entity_name;
        }
        #(#attributes)*
        #(#associations)*
        pub trait #extension_ident {
            #(#methods)*
        }
        impl #extension_ident for ::mxrs_expr::Var<#entity_ident> {
            #(#method_impls)*
        }
    }
}

fn expression_type(kind: &AttrKind) -> TokenStream {
    match kind {
        AttrKind::String | AttrKind::HashString => quote!(::mxrs_expr::MxString),
        AttrKind::Integer => quote!(::mxrs_expr::MxInteger),
        AttrKind::Long | AttrKind::AutoNumber => quote!(::mxrs_expr::MxLong),
        AttrKind::Float => quote!(::mxrs_expr::MxFloat),
        AttrKind::Decimal => quote!(::mxrs_expr::MxDecimal),
        AttrKind::Boolean => quote!(::mxrs_expr::MxBool),
        AttrKind::DateTime => quote!(::mxrs_expr::MxDateTime),
        AttrKind::Binary => quote!(::mxrs_expr::MxBinary),
        AttrKind::Enumeration => quote!(::mxrs_expr::MxEnumeration),
    }
}

fn expand_module(module: &ModuleInput) -> Result<TokenStream> {
    let name = module.name.to_string();
    let entity_stmts: Vec<TokenStream> = module
        .entities
        .iter()
        .map(|entity| expand_entity(entity, &module.name))
        .collect();
    let microflow_stmts: Vec<TokenStream> = module
        .microflows
        .iter()
        .map(expand_microflow)
        .collect::<Result<_>>()?;
    Ok(quote! {
        __mxrs_project.module(#name, |m| {
            #(#entity_stmts)*
            #(#microflow_stmts)*
        });
    })
}

fn expand_entity(entity: &EntityInput, module_name: &Ident) -> TokenStream {
    let name = entity.name.to_string();
    let documentation_stmt = entity
        .documentation
        .as_ref()
        .map(|doc| quote! { e.documentation(#doc); });
    let persistable_stmt = entity
        .persistable
        .as_ref()
        .map(|value| quote! { e.persistable(#value); });
    let attr_stmts = entity.attributes.iter().map(expand_attribute);
    let assoc_stmts = entity
        .associations
        .iter()
        .map(|association| expand_association(association, &entity.name, module_name));
    quote! {
        m.entity(#name, |e| {
            #documentation_stmt
            #persistable_stmt
            #(#attr_stmts)*
            #(#assoc_stmts)*
        });
    }
}

fn expand_microflow(microflow: &MicroflowInput) -> Result<TokenStream> {
    let name = microflow.name.to_string();
    let mut bindings = HashMap::new();
    let activity_stmts = expand_flow_items(&microflow.activities, &mut bindings)?;
    let mut rescue_bindings = bindings.clone();
    let rescue_stmts = expand_flow_items(&microflow.rescue_activities, &mut rescue_bindings)?;
    let rescue_stmt =
        (!rescue_stmts.is_empty()).then(|| quote! { f.rescue_all(|f| { #(#rescue_stmts)* }); });
    let return_stmt = microflow
        .return_expression
        .as_ref()
        .map(|expr| quote! { f.return_value(#expr); });
    Ok(quote! {
        m.microflow(#name, |f| {
            #(#activity_stmts)*
            #rescue_stmt
            #return_stmt
        });
    })
}

fn expand_flow_items(
    items: &[FlowItem],
    bindings: &mut HashMap<String, Binding>,
) -> Result<Vec<TokenStream>> {
    items
        .iter()
        .map(|item| expand_flow_item(item, bindings))
        .collect()
}

fn expand_flow_item(
    item: &FlowItem,
    bindings: &mut HashMap<String, Binding>,
) -> Result<TokenStream> {
    Ok(match item {
        FlowItem::Create {
            variable,
            entity,
            members,
            commit,
        } => {
            let name = variable.to_string();
            let member_exprs = expand_members(members, entity);
            bindings.insert(name.clone(), Binding::Object(entity.clone()));
            quote! {
                let #variable = f.create_object(
                    #name,
                    ::mxrs_ir::Ref::<#entity>::new(),
                    vec![#(#member_exprs),*],
                    #commit,
                );
            }
        }
        FlowItem::Change {
            variable,
            members,
            commit,
        } => {
            let entity = match bindings.get(&variable.to_string()) {
                Some(Binding::Object(entity)) => entity,
                Some(Binding::List(_)) => {
                    return Err(syn::Error::new(
                        variable.span(),
                        format!("`{variable}` is a list, not an object variable"),
                    ));
                }
                None => {
                    return Err(syn::Error::new(
                        variable.span(),
                        format!("unknown flow variable `{variable}`"),
                    ));
                }
            };
            let member_exprs = expand_members(members, entity);
            quote! { f.change_object(&#variable, vec![#(#member_exprs),*], #commit); }
        }
        FlowItem::CreateList { variable, entity } => {
            let name = variable.to_string();
            bindings.insert(name.clone(), Binding::List(entity.clone()));
            quote! {
                let #variable = f.create_list(#name, ::mxrs_ir::Ref::<#entity>::new());
            }
        }
        FlowItem::Delete { variable } => quote! { f.delete_object(&#variable); },
        FlowItem::Commit { variable } => quote! { f.commit(&#variable); },
        FlowItem::Call {
            microflow,
            mappings,
            result_variable,
        } => {
            let mapping_exprs = mappings.iter().map(expand_mapping);
            let (result_expr, use_return) = match result_variable {
                Some(variable) => {
                    let variable = variable.to_string();
                    (quote! { Some(#variable.to_string()) }, quote! { true })
                }
                None => (quote! { None }, quote! { false }),
            };
            quote! {
                f.call_microflow(
                    ::mxrs_ir::MicroflowRef::<#microflow>::new(),
                    #result_expr,
                    #use_return,
                    vec![#(#mapping_exprs),*],
                );
            }
        }
        FlowItem::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let mut then_bindings = bindings.clone();
            let mut else_bindings = bindings.clone();
            let then_stmts = expand_flow_items(then_branch, &mut then_bindings)?;
            let else_stmts = expand_flow_items(else_branch, &mut else_bindings)?;
            quote! {
                f.decision(
                    #condition,
                    |f| { #(#then_stmts)* },
                    |f| { #(#else_stmts)* },
                );
            }
        }
        FlowItem::LoopOver {
            iterator,
            list,
            activities,
        } => {
            let entity = match bindings.get(&list.to_string()) {
                Some(Binding::List(entity)) => entity.clone(),
                Some(Binding::Object(_)) => {
                    return Err(syn::Error::new(
                        list.span(),
                        format!("`{list}` is an object, not a list variable"),
                    ));
                }
                None => {
                    return Err(syn::Error::new(
                        list.span(),
                        format!("unknown list variable `{list}`"),
                    ));
                }
            };
            let mut loop_bindings = bindings.clone();
            loop_bindings.insert(iterator.to_string(), Binding::Object(entity.clone()));
            let stmts = expand_flow_items(activities, &mut loop_bindings)?;
            let iterator_name = iterator.to_string();
            quote! {
                f.loop_over(&#list, #iterator_name, |f, #iterator| {
                    #(#stmts)*
                });
            }
        }
        FlowItem::While {
            condition,
            activities,
        } => {
            let mut loop_bindings = bindings.clone();
            let stmts = expand_flow_items(activities, &mut loop_bindings)?;
            quote! { f.while_loop(#condition, |f| { #(#stmts)* }); }
        }
        FlowItem::Break => quote! { f.break_loop(); },
        FlowItem::Continue => quote! { f.continue_loop(); },
    })
}

fn expand_members(members: &[MemberInput], entity: &Path) -> Vec<TokenStream> {
    members
        .iter()
        .map(|member| {
            let marker = member_marker_path(entity, &member.name);
            let value = &member.value;
            if member.is_association {
                quote! { ::mxrs_expr::association::<#marker>(&#value) }
            } else {
                quote! { ::mxrs_expr::attribute::<#marker>(#value) }
            }
        })
        .collect()
}

fn member_marker_path(entity: &Path, member: &Ident) -> Path {
    let mut path = entity.clone();
    let entity_name = path
        .segments
        .last()
        .expect("entity path is non-empty")
        .ident
        .clone();
    path.segments
        .last_mut()
        .expect("entity path is non-empty")
        .ident = format_ident!("{}_{}", entity_name, member);
    path
}

fn expand_mapping(mapping: &MappingInput) -> TokenStream {
    let parameter = mapping.parameter.to_string();
    let value = &mapping.value;
    quote! { ::mxrs_dsl::CallArgument::new(#parameter, #value) }
}

fn expand_attribute(attribute: &AttributeInput) -> TokenStream {
    let name = attribute.name.to_string();
    let method = Ident::new(attribute.kind.builder_method(), attribute.name.span());
    let builder = match &attribute.enumeration {
        Some(enumeration) => quote! { e.#method(#name, #enumeration) },
        None => quote! { e.#method(#name) },
    };
    match &attribute.default {
        Some(default) => quote! { #builder.default_value = Some((#default).to_string()); },
        None => quote! { #builder; },
    }
}

fn expand_association(
    association: &AssociationInput,
    entity_name: &Ident,
    module_name: &Ident,
) -> TokenStream {
    // Mirrors the exact marker this same association statement caused
    // `expand_entity_markers` to generate — see that function for the
    // struct/impl this path resolves to.
    let marker = format_ident!("{}_{}", entity_name, association.name);
    quote! {
        e.association::<#module_name::#marker>();
    }
}

fn to_snake_case(value: &str) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() && index != 0 {
            output.push('_');
        }
        output.push(ch.to_ascii_lowercase());
    }
    output
}
