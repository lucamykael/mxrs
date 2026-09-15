//! Lowers `project! {}` to the public `mxrs-dsl` builder surface and emits
//! the marker types needed by typed flow expressions in the same expansion.

use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, Path, Result};

use crate::parse::{
    AccessMemberInput, AccessRuleInput, AssociationInput, AttrKind, AttributeInput,
    EntityImageInput, EntityInput, EntitySourceInput, FlowItem, FlowValueType, IndexInput,
    IndexMemberInput, InheritanceInput, LifecycleInput, MappingInput, MemberInput, MicroflowInput,
    ModuleInput, ProjectInput,
};

#[derive(Clone)]
enum Binding {
    Scalar,
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
    let signatures: HashMap<String, &MicroflowInput> = input
        .modules
        .iter()
        .flat_map(|module| {
            module
                .microflows
                .iter()
                .map(move |flow| (format!("{}.{}", module.name, flow.name), flow))
        })
        .collect();
    let module_stmts: Vec<TokenStream> = input
        .modules
        .iter()
        .map(|module| expand_module(module, &signatures))
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
    let microflows = module.microflows.iter().map(|flow| {
        let ident = &flow.name;
        let name = flow.name.to_string();
        quote! {
            pub struct #ident;
            impl ::mxrs_ir::MicroflowMarker for #ident {
                const MODULE: &'static str = #module_name;
                const NAME: &'static str = #name;
            }
        }
    });
    let nanoflows = module.nanoflows.iter().map(|flow| {
        let ident = &flow.name;
        let name = flow.name.to_string();
        quote! {
            pub struct #ident;
            impl ::mxrs_ir::NanoflowMarker for #ident {
                const MODULE: &'static str = #module_name;
                const NAME: &'static str = #name;
            }
        }
    });
    quote! {
        #[allow(non_snake_case, non_camel_case_types, dead_code)]
        pub mod #module_ident {
            #(#entities)*
            #(#microflows)*
            #(#nanoflows)*
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

fn expand_module(
    module: &ModuleInput,
    signatures: &HashMap<String, &MicroflowInput>,
) -> Result<TokenStream> {
    let name = module.name.to_string();
    let role_stmts = module.roles.iter().map(|role| {
        let role_name = role.name.to_string();
        let description = &role.description;
        quote! { m.role(#role_name, #description); }
    });
    let queue_stmts = module.task_queues.iter().map(|queue| {
        let name = queue.name.to_string();
        let config = if let Some(parallelism) = &queue.parallelism {
            quote! { ::mxrs_ir::TaskQueueConfig::Fixed { parallelism: #parallelism } }
        } else {
            let expression = queue
                .parallelism_expression
                .as_ref()
                .expect("parser requires a config");
            let scope = queue
                .scope
                .as_ref()
                .map_or_else(|| quote! { PerNode }, |scope| quote! { #scope });
            quote! { ::mxrs_ir::TaskQueueConfig::Dynamic {
                parallelism_expression: #expression.to_string(),
                scope: ::mxrs_ir::TaskQueueScope::#scope,
            } }
        };
        let documentation = queue
            .documentation
            .as_ref()
            .map(|value| quote! { queue.documentation(#value); });
        let excluded = queue
            .excluded
            .as_ref()
            .map(|value| quote! { queue.excluded(#value); });
        let export_level = queue
            .export_level
            .as_ref()
            .map(|value| quote! { queue.export_level(::mxrs_ir::ExportLevel::#value); });
        quote! {
            m.task_queue(#name, #config, |queue| {
                #documentation
                #excluded
                #export_level
            });
        }
    });
    let oql_source_stmts = module.oql_view_sources.iter().map(|source| {
        let source_name = source.name.to_string();
        let query = &source.query;
        let documentation = source
            .documentation
            .as_ref()
            .map(|value| quote! { source.documentation(#value); });
        let excluded = source
            .excluded
            .as_ref()
            .map(|value| quote! { source.excluded(#value); });
        let export_level = source.export_level.as_ref().map(|value| {
            quote! { source.export_level(::mxrs_ir::ExportLevel::#value); }
        });
        quote! {
            m.oql_view_source(#source_name, #query, |source| {
                #documentation
                #excluded
                #export_level
            });
        }
    });
    let entity_stmts: Vec<TokenStream> = module
        .entities
        .iter()
        .map(|entity| expand_entity(entity, &module.name))
        .collect();
    let microflow_stmts: Vec<TokenStream> = module
        .microflows
        .iter()
        .map(|flow| expand_microflow(flow, signatures))
        .collect::<Result<_>>()?;
    let nanoflow_stmts: Vec<TokenStream> = module
        .nanoflows
        .iter()
        .map(|flow| expand_microflow(flow, signatures))
        .collect::<Result<_>>()?;
    Ok(quote! {
        __mxrs_project.module(#name, |m| {
            #(#role_stmts)*
            #(#oql_source_stmts)*
            #(#queue_stmts)*
            #(#entity_stmts)*
            #(#microflow_stmts)*
            #(#nanoflow_stmts)*
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
    let image_stmt = entity.image.as_ref().map(|image| match image {
        EntityImageInput::None => quote! { e.clear_image(); },
        EntityImageInput::Reference(reference) => quote! { e.image(#reference); },
    });
    let source_stmt = entity.source.as_ref().map(|source| match source {
        EntitySourceInput::Stored => quote! { e.stored(); },
        EntitySourceInput::OqlView(path) => {
            let qualified = qualified_path(path, &module_name.to_string());
            quote! { e.oql_view(#qualified); }
        }
    });
    let attr_stmts = entity.attributes.iter().map(expand_attribute);
    let assoc_stmts = entity
        .associations
        .iter()
        .map(|association| expand_association(association, &entity.name, module_name));
    let inheritance_stmt = entity.inheritance.as_ref().map(expand_inheritance);
    let index_stmts: Vec<TokenStream> = match &entity.indexes {
        None => vec![],
        Some(indexes) if indexes.is_empty() => vec![quote! { e.clear_indexes(); }],
        Some(indexes) => indexes
            .iter()
            .map(|index| expand_index(index, &entity.name, module_name))
            .collect(),
    };
    let lifecycle_stmts: Vec<TokenStream> = match &entity.lifecycle {
        None => vec![],
        Some(callbacks) if callbacks.is_empty() => vec![quote! { e.clear_lifecycle(); }],
        Some(callbacks) => callbacks.iter().map(expand_lifecycle).collect(),
    };
    let access_stmts: Vec<TokenStream> = match &entity.access_rules {
        None => vec![],
        Some(rules) if rules.is_empty() => vec![quote! { e.clear_access_rules(); }],
        Some(rules) => rules
            .iter()
            .map(|rule| expand_access_rule(rule, &entity.name, module_name))
            .collect(),
    };
    quote! {
        m.entity(#name, |e| {
            #documentation_stmt
            #persistable_stmt
            #image_stmt
            #source_stmt
            #inheritance_stmt
            #(#attr_stmts)*
            #(#assoc_stmts)*
            #(#index_stmts)*
            #(#lifecycle_stmts)*
            #(#access_stmts)*
        });
    }
}

fn qualified_path(path: &Path, default_module: &str) -> String {
    let joined = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join(".");
    if joined.contains('.') {
        joined
    } else {
        format!("{default_module}.{joined}")
    }
}

fn expand_access_rule(
    rule: &AccessRuleInput,
    entity_name: &Ident,
    module_name: &Ident,
) -> TokenStream {
    let roles = &rule.roles;
    let documentation = rule
        .documentation
        .as_ref()
        .map(|value| quote! { r.documentation(#value); });
    let allow_create = rule
        .allow_create
        .as_ref()
        .map(|value| quote! { r.allow_create(#value); });
    let allow_delete = rule
        .allow_delete
        .as_ref()
        .map(|value| quote! { r.allow_delete(#value); });
    let default_rights = rule.default_rights.as_ref().map(|value| {
        quote! { r.default_rights(::mxrs_ir::MemberRights::#value); }
    });
    let xpath = rule.xpath.as_ref().map(|value| quote! { r.xpath(#value); });
    let xpath_caption = rule
        .xpath_caption
        .as_ref()
        .map(|value| quote! { r.xpath_caption(#value); });
    let members = rule.members.iter().map(|member| match member {
        AccessMemberInput::Attribute { name, rights } => {
            let marker = format_ident!("{}_{}", entity_name, name);
            quote! { r.attribute::<#module_name::#marker>(::mxrs_ir::MemberRights::#rights); }
        }
        AccessMemberInput::Association { name, rights } => {
            let marker = format_ident!("{}_{}", entity_name, name);
            quote! { r.association::<#module_name::#marker>(::mxrs_ir::MemberRights::#rights); }
        }
    });
    quote! {
        e.access_rule([#(#roles),*], |r| {
            #documentation
            #allow_create
            #allow_delete
            #default_rights
            #xpath
            #xpath_caption
            #(#members)*
        });
    }
}

fn expand_inheritance(inheritance: &InheritanceInput) -> TokenStream {
    match inheritance {
        InheritanceInput::Generalizes(target) => quote! { e.generalizes::<#target>(); },
        InheritanceInput::Root {
            owner,
            created_date,
            changed_date,
            changed_by,
        } => {
            let owner = owner.as_ref().map(|value| quote! { s.owner(#value); });
            let created_date = created_date
                .as_ref()
                .map(|value| quote! { s.created_date(#value); });
            let changed_date = changed_date
                .as_ref()
                .map(|value| quote! { s.changed_date(#value); });
            let changed_by = changed_by
                .as_ref()
                .map(|value| quote! { s.changed_by(#value); });
            quote! {
                e.system_members(|s| {
                    #owner
                    #created_date
                    #changed_date
                    #changed_by
                });
            }
        }
    }
}

fn expand_index(index: &IndexInput, entity_name: &Ident, module_name: &Ident) -> TokenStream {
    let members = index.members.iter().map(|member| match member {
        IndexMemberInput::Attribute { name, ascending } => {
            let marker = format_ident!("{}_{}", entity_name, name);
            if ascending.as_ref().is_some_and(|value| !value.value) {
                quote! { i.attribute_descending::<#module_name::#marker>(); }
            } else {
                quote! { i.attribute::<#module_name::#marker>(); }
            }
        }
        IndexMemberInput::System { member, ascending } => {
            if ascending.as_ref().is_some_and(|value| !value.value) {
                quote! { i.system_descending(::mxrs_ir::SystemMember::#member); }
            } else {
                quote! { i.system(::mxrs_ir::SystemMember::#member); }
            }
        }
    });
    let include_offline = index
        .include_offline
        .as_ref()
        .map(|value| quote! { i.include_offline(#value); });
    quote! {
        e.index(|i| {
            #(#members)*
            #include_offline
        });
    }
}

fn expand_lifecycle(callback: &LifecycleInput) -> TokenStream {
    let event = &callback.event;
    let handler = &callback.handler;
    let pass_event_object = callback
        .pass_event_object
        .as_ref()
        .map(|value| quote! { hook.pass_event_object(#value); });
    let raise_error_on_false = callback
        .raise_error_on_false
        .as_ref()
        .map(|value| quote! { hook.raise_error_on_false(#value); });
    quote! {
        e.#event::<#handler>(|hook| {
            #pass_event_object
            #raise_error_on_false
        });
    }
}

fn parameter_type(kind: &FlowValueType) -> TokenStream {
    match kind {
        FlowValueType::Scalar(kind) => expression_type(kind),
        FlowValueType::Object(entity) => quote! { ::mxrs_expr::MxObject<#entity> },
        FlowValueType::List(entity) => quote! { ::mxrs_expr::MxList<#entity> },
    }
}

fn expand_microflow(
    microflow: &MicroflowInput,
    signatures: &HashMap<String, &MicroflowInput>,
) -> Result<TokenStream> {
    let name = microflow.name.to_string();
    let mut bindings = HashMap::new();
    let parameter_stmts: Vec<_> = microflow.parameters.iter().map(|parameter| {
        let variable = &parameter.name;
        let name = variable.to_string();
        let documentation = parameter.documentation.as_ref().map(|value| quote! { p.documentation(#value); });
        let required = parameter.required.as_ref().map(|value| quote! { p.required(#value); });
        let default_value = parameter.default_value.as_ref().map(|value| quote! { p.default_value(#value); });
        let configure = quote! { |p| { #documentation #required #default_value } };
        let declaration = match &parameter.value_type {
            FlowValueType::Scalar(kind) => {
                bindings.insert(name.clone(), Binding::Scalar);
                let tag = expression_type(kind);
                quote! { f.parameter::<#tag>(#name, #configure) }
            }
            FlowValueType::Object(entity) => {
                bindings.insert(name.clone(), Binding::Object(entity.clone()));
                quote! { f.object_parameter(#name, ::mxrs_ir::Ref::<#entity>::new(), #configure) }
            }
            FlowValueType::List(entity) => {
                bindings.insert(name.clone(), Binding::List(entity.clone()));
                quote! { f.list_parameter(#name, ::mxrs_ir::Ref::<#entity>::new(), #configure) }
            }
        };
        quote! { let #variable = #declaration; let _ = &#variable; }
    }).collect();
    let activity_stmts = expand_flow_items(&microflow.activities, &mut bindings, signatures)?;
    let mut rescue_bindings = bindings.clone();
    let rescue_stmts = expand_flow_items(
        &microflow.rescue_activities,
        &mut rescue_bindings,
        signatures,
    )?;
    let rescue_stmt =
        (!rescue_stmts.is_empty()).then(|| quote! { f.rescue_all(|f| { #(#rescue_stmts)* }); });
    let return_stmt = microflow
        .return_expression
        .as_ref()
        .map(|expr| quote! { f.return_value(#expr); });
    let method = if microflow.is_nanoflow {
        format_ident!("nanoflow")
    } else {
        format_ident!("microflow")
    };
    Ok(quote! {
        m.#method(#name, |f| {
            #(#parameter_stmts)*
            #(#activity_stmts)*
            #rescue_stmt
            #return_stmt
        });
    })
}

fn expand_flow_items(
    items: &[FlowItem],
    bindings: &mut HashMap<String, Binding>,
    signatures: &HashMap<String, &MicroflowInput>,
) -> Result<Vec<TokenStream>> {
    items
        .iter()
        .map(|item| expand_flow_item(item, bindings, signatures))
        .collect()
}

fn expand_flow_item(
    item: &FlowItem,
    bindings: &mut HashMap<String, Binding>,
    signatures: &HashMap<String, &MicroflowInput>,
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
                Some(Binding::Scalar) => {
                    return Err(syn::Error::new(
                        variable.span(),
                        format!("`{variable}` is a scalar, not an object variable"),
                    ));
                }
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
            result_type,
        } => {
            let target = microflow
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join(".");
            let mut seen = std::collections::HashSet::new();
            let mut mapping_exprs = Vec::new();
            for mapping in mappings {
                let name = mapping.parameter.to_string();
                if !seen.insert(name.clone()) {
                    return Err(syn::Error::new(
                        mapping.parameter.span(),
                        "duplicate call argument",
                    ));
                }
                if let Some(signature) = signatures.get(&target) {
                    let parameter = signature
                        .parameters
                        .iter()
                        .find(|parameter| parameter.name == mapping.parameter)
                        .ok_or_else(|| {
                            syn::Error::new(
                                mapping.parameter.span(),
                                format!("unknown parameter `{name}` on `{target}`"),
                            )
                        })?;
                    let tag = parameter_type(&parameter.value_type);
                    let value = &mapping.value;
                    mapping_exprs
                        .push(quote! { ::mxrs_dsl::CallArgument::typed::<#tag>(#name, #value) });
                } else {
                    mapping_exprs.push(expand_mapping(mapping));
                }
            }
            if let Some(signature) = signatures.get(&target) {
                for parameter in &signature.parameters {
                    if !seen.contains(&parameter.name.to_string()) {
                        return Err(syn::Error::new(
                            microflow.segments.last().expect("path").ident.span(),
                            format!("missing argument `{}` for `{target}`", parameter.name),
                        ));
                    }
                }
            }
            if let (Some(variable), Some(value_type)) = (result_variable, result_type) {
                let name = variable.to_string();
                if bindings.contains_key(&name) {
                    return Err(syn::Error::new(
                        variable.span(),
                        format!("duplicate flow variable `{name}`"),
                    ));
                }
                if signatures
                    .get(&target)
                    .is_some_and(|flow| flow.return_expression.is_none())
                {
                    return Err(syn::Error::new(
                        variable.span(),
                        format!("flow `{target}` returns no value"),
                    ));
                }
                let tag = parameter_type(value_type);
                let binding = match value_type {
                    FlowValueType::Scalar(_) => Binding::Scalar,
                    FlowValueType::Object(entity) => Binding::Object(entity.clone()),
                    FlowValueType::List(entity) => Binding::List(entity.clone()),
                };
                bindings.insert(name.clone(), binding);
                quote! {
                    let #variable = f.call_microflow_result::<#tag>(
                        ::mxrs_ir::MicroflowRef::<#microflow>::new(), #name,
                        vec![#(#mapping_exprs),*],
                    );
                    let _ = &#variable;
                }
            } else {
                quote! {
                    f.call_microflow(::mxrs_ir::MicroflowRef::<#microflow>::new(),
                        None, false, vec![#(#mapping_exprs),*]);
                }
            }
        }
        FlowItem::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let mut then_bindings = bindings.clone();
            let mut else_bindings = bindings.clone();
            let then_stmts = expand_flow_items(then_branch, &mut then_bindings, signatures)?;
            let else_stmts = expand_flow_items(else_branch, &mut else_bindings, signatures)?;
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
                Some(Binding::Scalar) => {
                    return Err(syn::Error::new(
                        list.span(),
                        format!("`{list}` is a scalar, not a list variable"),
                    ));
                }
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
            let stmts = expand_flow_items(activities, &mut loop_bindings, signatures)?;
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
            let stmts = expand_flow_items(activities, &mut loop_bindings, signatures)?;
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
    let default = attribute
        .default
        .as_ref()
        .map(|value| quote! { __mxrs_attribute.default_value = Some((#value).to_string()); });
    let documentation = attribute
        .documentation
        .as_ref()
        .map(|value| quote! { __mxrs_attribute.documentation = (#value).to_string(); });
    let length = attribute
        .length
        .as_ref()
        .map(|value| quote! { __mxrs_attribute.length = Some(#value); });
    let localize_date = attribute
        .localize_date
        .as_ref()
        .map(|value| quote! { __mxrs_attribute.localize_date = Some(#value); });
    let required = attribute
        .required
        .as_ref()
        .map(|value| quote! { __mxrs_attribute.required = #value; });
    let unique = attribute
        .unique
        .as_ref()
        .map(|value| quote! { __mxrs_attribute.unique = #value; });
    quote! {{
        let __mxrs_attribute = #builder;
        #default
        #documentation
        #length
        #localize_date
        #required
        #unique
    }}
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
    let owner = association.owner.as_ref().map(|value| {
        quote! { __mxrs_association.owner = ::mxrs_ir::AssociationOwner::#value; }
    });
    let storage = association.storage.as_ref().map(|value| {
        quote! { __mxrs_association.storage = ::mxrs_ir::AssociationStorage::#value; }
    });
    let documentation = association
        .documentation
        .as_ref()
        .map(|value| quote! { __mxrs_association.documentation = (#value).to_string(); });
    quote! {
        {
            let __mxrs_association = e.association::<#module_name::#marker>();
            #owner
            #storage
            #documentation
        }
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
