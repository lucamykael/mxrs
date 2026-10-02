//! One macro per activity of a flow, written the way Rust writes data.
//!
//! ```text
//! let order = create_object!(flow, Order {
//!     number: "A-1",
//!     total: 10.50,
//!     status: OrderStatus::Open,
//!     customer: customer,
//! }, commit, refresh);
//! ```
//!
//! Every argument is a Rust expression, so rustfmt lays the call out like
//! any other. A value is the Mendix expression it reads as: a string
//! literal is a Mendix string (`"A-1"` → `'A-1'`), a number or `true` is
//! itself, a variable is `$` and its name, an enumeration's variant is its
//! qualified value, and `mx("...")` is any expression Rust cannot check.
//! Each macro expands to the builder call the activity is, so a flow says
//! the same thing either way.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, Token};

/// Which activity a macro declares.
#[derive(Clone, Copy)]
pub enum Activity {
    CreateObject,
    ChangeObject,
    CommitObject,
    DeleteObject,
    RollbackObject,
    Retrieve,
    CreateList,
    ChangeList,
    AggregateList,
    CreateVariable,
    ChangeVariable,
    CallMicroflow,
    Log,
}

pub fn expand(activity: Activity, input: TokenStream) -> syn::Result<TokenStream> {
    let arguments =
        syn::parse::Parser::parse2(Punctuated::<Expr, Token![,]>::parse_terminated, input)?;
    let mut arguments = arguments.into_iter();
    let flow = arguments
        .next()
        .ok_or_else(|| syn::Error::new(Span::call_site(), "expected the flow builder first"))?;
    let rest: Vec<Expr> = arguments.collect();
    match activity {
        Activity::CreateObject => create_object(&flow, &rest),
        Activity::ChangeObject => change_object(&flow, &rest),
        Activity::CommitObject => commit_object(&flow, &rest),
        Activity::DeleteObject => refresh_only(&flow, &rest, quote!(delete_with), "delete_object"),
        Activity::RollbackObject => {
            refresh_only(&flow, &rest, quote!(rollback_with), "rollback_object")
        }
        Activity::Retrieve => retrieve(&flow, &rest),
        Activity::CreateList => create_list(&flow, &rest),
        Activity::ChangeList => change_list(&flow, &rest),
        Activity::AggregateList => aggregate_list(&flow, &rest),
        Activity::CreateVariable => create_variable(&flow, &rest),
        Activity::ChangeVariable => change_variable(&flow, &rest),
        Activity::CallMicroflow => call_microflow(&flow, &rest),
        Activity::Log => log(&flow, &rest),
    }
}

/// The Mendix expression `value` reads as.
pub fn value(value: &Expr) -> TokenStream {
    match value {
        Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Str(text) => quote! { ::mxrs::Mx::from(::mxrs::string(#text)) },
            syn::Lit::Int(number) => {
                let text = number.base10_digits();
                quote! { ::mxrs::mx(#text) }
            }
            syn::Lit::Float(number) => {
                let text = number.base10_digits();
                quote! { ::mxrs::mx(#text) }
            }
            syn::Lit::Bool(flag) => {
                let text = if flag.value { "true" } else { "false" };
                quote! { ::mxrs::mx(#text) }
            }
            _ => quote! { ::std::convert::Into::<::mxrs::Mx>::into(#value) },
        },
        Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Neg(_),
            expr,
            ..
        }) if matches!(&**expr, Expr::Lit(literal) if matches!(literal.lit, syn::Lit::Int(_) | syn::Lit::Float(_))) =>
        {
            let Expr::Lit(literal) = &**expr else {
                unreachable!("matched above")
            };
            let digits = match &literal.lit {
                syn::Lit::Int(number) => number.base10_digits().to_string(),
                syn::Lit::Float(number) => number.base10_digits().to_string(),
                _ => unreachable!("matched above"),
            };
            let text = format!("-{digits}");
            quote! { ::mxrs::mx(#text) }
        }
        // A name is what it stands for: a variable, or a text constant.
        Expr::Path(path) if path.path.get_ident().is_some() && path.qself.is_none() => {
            quote! { ::mxrs::ActivityValue::activity_value(&#value) }
        }
        _ => quote! { ::std::convert::Into::<::mxrs::Mx>::into(#value) },
    }
}

/// The options after an activity's main arguments: bare words (`commit`)
/// and `key = value` pairs, each at most once.
struct Options {
    words: Vec<syn::Ident>,
    pairs: Vec<(syn::Ident, Expr)>,
}

impl Options {
    fn parse(arguments: &[Expr], words: &[&str], pairs: &[&str]) -> syn::Result<Self> {
        let mut options = Options {
            words: Vec::new(),
            pairs: Vec::new(),
        };
        for argument in arguments {
            match argument {
                Expr::Path(path) if path.path.get_ident().is_some() => {
                    let word = path.path.get_ident().expect("checked").clone();
                    if !words.iter().any(|known| word == known) {
                        return Err(syn::Error::new(
                            word.span(),
                            format!(
                                "unknown option `{word}`; expected one of: {}",
                                expected(words, pairs)
                            ),
                        ));
                    }
                    if options.words.contains(&word) {
                        return Err(syn::Error::new(
                            word.span(),
                            format!("`{word}` is stated once"),
                        ));
                    }
                    options.words.push(word);
                }
                Expr::Assign(assign) => {
                    let Expr::Path(path) = &*assign.left else {
                        return Err(syn::Error::new_spanned(
                            &assign.left,
                            "expected an option name",
                        ));
                    };
                    let Some(key) = path.path.get_ident().cloned() else {
                        return Err(syn::Error::new_spanned(
                            &assign.left,
                            "expected an option name",
                        ));
                    };
                    if !pairs.iter().any(|known| key == known) {
                        return Err(syn::Error::new(
                            key.span(),
                            format!(
                                "unknown option `{key}`; expected one of: {}",
                                expected(words, pairs)
                            ),
                        ));
                    }
                    if options.pairs.iter().any(|(seen, _)| *seen == key) {
                        return Err(syn::Error::new(
                            key.span(),
                            format!("`{key}` is stated once"),
                        ));
                    }
                    options.pairs.push((key, (*assign.right).clone()));
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        format!("expected an option; one of: {}", expected(words, pairs)),
                    ));
                }
            }
        }
        Ok(options)
    }

    fn has(&self, word: &str) -> bool {
        self.words.iter().any(|seen| seen == word)
    }

    fn get(&self, key: &str) -> Option<&Expr> {
        self.pairs
            .iter()
            .find(|(seen, _)| seen == key)
            .map(|(_, value)| value)
    }

    /// The variable name stated with `name = ...`, or `default`.
    fn name_or(&self, default: TokenStream) -> TokenStream {
        match self.get("name") {
            Some(name) => quote! { #name },
            None => default,
        }
    }
}

fn expected(words: &[&str], pairs: &[&str]) -> String {
    words
        .iter()
        .map(|word| format!("`{word}`"))
        .chain(pairs.iter().map(|pair| format!("`{pair} = ...`")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `Entity { member: value, ... }`: the entity and what is set.
fn members(expression: &Expr, what: &str) -> syn::Result<(syn::Path, Vec<TokenStream>)> {
    let Expr::Struct(literal) = expression else {
        return Err(syn::Error::new_spanned(
            expression,
            format!("expected the entity and what {what} sets: `Entity {{ member: value, ... }}`"),
        ));
    };
    if let Some(rest) = &literal.rest {
        return Err(syn::Error::new_spanned(
            rest,
            "a change sets only the members it names",
        ));
    }
    let entity = literal.path.clone();
    let sets = literal
        .fields
        .iter()
        .map(|field| {
            let syn::Member::Named(member) = &field.member else {
                return Err(syn::Error::new_spanned(
                    &field.member,
                    "expected a member's name",
                ));
            };
            let value = value(&field.expr);
            Ok(quote! { __mxrs_change.set(#entity::#member(), #value); })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    Ok((entity, sets))
}

/// `commit`, `commit_without_events` and `refresh` of a create or change.
fn change_options(options: &Options) -> TokenStream {
    let commit = if options.has("commit_without_events") {
        Some(quote! { __mxrs_change.commit(::mxrs::Commit::WithoutEvents); })
    } else if options.has("commit") {
        Some(quote! { __mxrs_change.commit(::mxrs::Commit::Yes); })
    } else {
        None
    };
    let refresh = options
        .has("refresh")
        .then(|| quote! { __mxrs_change.refresh_in_client(true); });
    quote! { #commit #refresh }
}

fn first<'a>(arguments: &'a [Expr], what: &str) -> syn::Result<&'a Expr> {
    arguments
        .first()
        .ok_or_else(|| syn::Error::new(Span::call_site(), format!("expected {what}")))
}

/// `create_object!(flow, Entity { member: value, ... }, name = "...",
/// commit, refresh)` — named `New<Entity>` unless it says otherwise.
fn create_object(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let (entity, sets) = members(first(arguments, "the entity it creates")?, "it")?;
    let options = Options::parse(
        &arguments[1..],
        &["commit", "commit_without_events", "refresh"],
        &["name"],
    )?;
    let name = options.name_or(quote! {
        ::std::format!("New{}", <#entity as ::mxrs::EntityMarker>::NAME)
    });
    let after = change_options(&options);
    Ok(quote! {
        (#flow).create(#name, ::mxrs::Ref::<#entity>::new(), |__mxrs_change| {
            #(#sets)*
            #after
        })
    })
}

/// `change_object!(flow, &order, Order { member: value, ... }, commit,
/// refresh)`.
fn change_object(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let variable = first(arguments, "the object it changes")?;
    let (_, sets) = members(
        arguments.get(1).ok_or_else(|| {
            syn::Error::new_spanned(
                variable,
                "expected `Entity { member: value, ... }` after the object",
            )
        })?,
        "the change",
    )?;
    let options = Options::parse(
        &arguments[2..],
        &["commit", "commit_without_events", "refresh"],
        &[],
    )?;
    let after = change_options(&options);
    Ok(quote! {
        (#flow).change(#variable, |__mxrs_change| {
            #(#sets)*
            #after
        })
    })
}

/// `commit_object!(flow, &order, without_events, refresh)`.
fn commit_object(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let variable = first(arguments, "the object it commits")?;
    let options = Options::parse(&arguments[1..], &["without_events", "refresh"], &[])?;
    let events = options
        .has("without_events")
        .then(|| quote! { __mxrs_commit.with_events(false); });
    let refresh = options
        .has("refresh")
        .then(|| quote! { __mxrs_commit.refresh_in_client(true); });
    Ok(quote! {
        (#flow).commit_with(#variable, |__mxrs_commit| {
            #events
            #refresh
        })
    })
}

/// `delete_object!(flow, &order, refresh)`, `rollback_object!(...)`.
fn refresh_only(
    flow: &Expr,
    arguments: &[Expr],
    method: TokenStream,
    what: &str,
) -> syn::Result<TokenStream> {
    let variable = first(arguments, &format!("the object {what} acts on"))?;
    let options = Options::parse(&arguments[1..], &["refresh"], &[])?;
    let refresh = options
        .has("refresh")
        .then(|| quote! { __mxrs_options.refresh_in_client(true); });
    Ok(quote! {
        (#flow).#method(#variable, |__mxrs_options| {
            #refresh
        })
    })
}

/// `[(member, Ascending), ...]`: what a list is ordered by.
fn sortings(
    expression: &Expr,
    method: TokenStream,
    receiver: TokenStream,
) -> syn::Result<Vec<TokenStream>> {
    let Expr::Array(array) = expression else {
        return Err(syn::Error::new_spanned(
            expression,
            "expected the orderings: `[(member, Ascending), ...]`",
        ));
    };
    array
        .elems
        .iter()
        .map(|element| {
            let Expr::Tuple(pair) = element else {
                return Err(syn::Error::new_spanned(
                    element,
                    "expected `(member, Ascending)`",
                ));
            };
            let [member, order] = [pair.elems.first(), pair.elems.get(1)];
            let (Some(member), Some(order), 2) = (member, order, pair.elems.len()) else {
                return Err(syn::Error::new_spanned(
                    element,
                    "expected `(member, Ascending)`",
                ));
            };
            let order = sort_order(order)?;
            Ok(quote! { #receiver.#method(#member, #order); })
        })
        .collect()
}

fn sort_order(order: &Expr) -> syn::Result<TokenStream> {
    match order {
        Expr::Path(path) if path.path.is_ident("Ascending") => {
            Ok(quote! { ::mxrs::SortOrder::Ascending })
        }
        Expr::Path(path) if path.path.is_ident("Descending") => {
            Ok(quote! { ::mxrs::SortOrder::Descending })
        }
        other => Ok(other.to_token_stream()),
    }
}

/// `retrieve!(flow, Entity, xpath = "...", sort = [...], first, range =
/// (limit, offset), name = "...")` — named `<Entity>List`, or `<Entity>`
/// when it retrieves the first object, unless it says otherwise.
fn retrieve(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let entity = first(arguments, "the entity it retrieves")?;
    let options = Options::parse(
        &arguments[1..],
        &["first"],
        &["xpath", "sort", "range", "name", "by"],
    )?;
    // `retrieve!(flow, &order, by = Order::customer(), name = "...")`: what
    // an object is associated with, rather than what the database holds.
    if let Some(association) = options.get("by") {
        if options.words.len() + options.pairs.len() != 2 {
            return Err(syn::Error::new(
                Span::call_site(),
                "a retrieve over an association takes `by` and `name` alone",
            ));
        }
        let name = options.get("name").ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                "name what it retrieves with `name = \"...\"`",
            )
        })?;
        return Ok(quote! { (#flow).retrieve_associated(#name, #entity, #association) });
    }
    let default = if options.has("first") {
        quote! { ::std::string::String::from(<#entity as ::mxrs::EntityMarker>::NAME) }
    } else {
        quote! { ::std::format!("{}List", <#entity as ::mxrs::EntityMarker>::NAME) }
    };
    let name = options.name_or(default);
    let xpath = options
        .get("xpath")
        .map(|xpath| quote! { __mxrs_retrieve.xpath(#xpath); });
    let sort = options
        .get("sort")
        .map(|sort| sortings(sort, quote!(sort_by), quote!(__mxrs_retrieve)))
        .transpose()?
        .unwrap_or_default();
    let first = options
        .has("first")
        .then(|| quote! { __mxrs_retrieve.first(); });
    let range = match options.get("range") {
        Some(Expr::Tuple(pair)) if pair.elems.len() == 2 => {
            let limit = value(&pair.elems[0]);
            let offset = value(&pair.elems[1]);
            Some(quote! { __mxrs_retrieve.range(#limit, #offset); })
        }
        Some(other) => {
            return Err(syn::Error::new_spanned(
                other,
                "expected `range = (limit, offset)`",
            ));
        }
        None => None,
    };
    Ok(quote! {
        (#flow).retrieve(#name, ::mxrs::Ref::<#entity>::new(), |__mxrs_retrieve| {
            #xpath
            #(#sort)*
            #first
            #range
        })
    })
}

/// `create_list!(flow, Entity, name = "...")` — named `<Entity>List`
/// unless it says otherwise.
fn create_list(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let entity = first(arguments, "the entity of the list")?;
    let options = Options::parse(&arguments[1..], &[], &["name"])?;
    let name = options.name_or(quote! {
        ::std::format!("{}List", <#entity as ::mxrs::EntityMarker>::NAME)
    });
    Ok(quote! { (#flow).create_list_of(#name, ::mxrs::Ref::<#entity>::new()) })
}

/// `change_list!(flow, &list, ...)`: changes the list (`add = value`,
/// `remove = value`, `replace = value`, `clear`), or makes another of it
/// (`head`, `tail`, `find = (member, value)`, `find_by = expression`,
/// `filter = (member, value)`, `filter_by = expression`, `sort = [...]`,
/// `range = (limit, offset)`, `union = &other`, `intersect = &other`,
/// `subtract = &other`, `contains = &object`, `equals = &other`), which
/// is named with `name = "..."`.
fn change_list(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let list = first(arguments, "the list")?;
    let options = Options::parse(
        &arguments[1..],
        &["clear", "head", "tail"],
        &[
            "add",
            "remove",
            "replace",
            "find",
            "find_by",
            "filter",
            "filter_by",
            "sort",
            "range",
            "union",
            "intersect",
            "subtract",
            "contains",
            "equals",
            "name",
        ],
    )?;
    let operations = options.words.len()
        + options
            .pairs
            .iter()
            .filter(|(key, _)| key != "name")
            .count();
    if operations != 1 {
        return Err(syn::Error::new(
            Span::call_site(),
            "a change_list! does one thing: `add = ...`, `clear`, `sort = [...]`, ...",
        ));
    }
    let name = options.get("name");
    let named = |operation: &str| {
        name.map(|name| quote! { #name }).ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                format!("`{operation}` makes another list: name it with `name = \"...\"`"),
            )
        })
    };
    let mutate = |change: TokenStream, value: TokenStream| {
        if name.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "a change to the list itself makes no other variable: drop `name`",
            ));
        }
        Ok(quote! { (#flow).change_list(#list, ::mxrs::ListChange::#change, #value) })
    };
    let pair = |expression: &Expr, what: &str| -> syn::Result<(Expr, Expr)> {
        match expression {
            Expr::Tuple(tuple) if tuple.elems.len() == 2 => {
                Ok((tuple.elems[0].clone(), tuple.elems[1].clone()))
            }
            other => Err(syn::Error::new_spanned(other, format!("expected `{what}`"))),
        }
    };
    if options.has("clear") {
        return mutate(quote!(Clear), quote! { ::mxrs::mx("") });
    }
    if options.has("head") {
        let name = named("head")?;
        return Ok(quote! { (#flow).list_head(#name, #list) });
    }
    if options.has("tail") {
        let name = named("tail")?;
        return Ok(quote! { (#flow).list_tail(#name, #list) });
    }
    let (key, argument) = options
        .pairs
        .iter()
        .find(|(key, _)| key != "name")
        .expect("one operation is stated");
    let key = key.to_string();
    match key.as_str() {
        "add" => mutate(quote!(Add), value(argument)),
        "remove" => mutate(quote!(Remove), value(argument)),
        "replace" => mutate(quote!(Set), value(argument)),
        "find" | "filter" => {
            let (member, wanted) = pair(argument, &format!("{key} = (member, value)"))?;
            let name = named(&key)?;
            let method = syn::Ident::new(&format!("list_{key}"), Span::call_site());
            let wanted = value(&wanted);
            Ok(quote! { (#flow).#method(#name, #list, #member, #wanted) })
        }
        "find_by" | "filter_by" => {
            let name = named(&key)?;
            let method = syn::Ident::new(&format!("list_{key}"), Span::call_site());
            let expression = value(argument);
            Ok(quote! { (#flow).#method(#name, #list, #expression) })
        }
        "sort" => {
            let name = named("sort")?;
            let sorts = sortings(argument, quote!(by), quote!(__mxrs_sort))?;
            Ok(quote! {
                (#flow).list_sort(#name, #list, |__mxrs_sort| {
                    #(#sorts)*
                })
            })
        }
        "range" => {
            let (limit, offset) = pair(argument, "range = (limit, offset)")?;
            let name = named("range")?;
            let (limit, offset) = (value(&limit), value(&offset));
            Ok(quote! { (#flow).list_range(#name, #list, #limit, #offset) })
        }
        "union" | "intersect" | "subtract" | "contains" | "equals" => {
            let name = named(&key)?;
            let method = syn::Ident::new(&format!("list_{key}"), Span::call_site());
            Ok(quote! { (#flow).#method(#name, #list, #argument) })
        }
        _ => unreachable!("options are checked"),
    }
}

/// `aggregate_list!(flow, &list, count, name = "...")`, or with `sum =
/// member` (also `average`, `minimum`, `maximum`), or with `sum_of =
/// expression` for an expression over each `$currentObject`.
fn aggregate_list(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let list = first(arguments, "the list")?;
    let functions = [
        "sum", "average", "minimum", "maximum", "all", "any", "reduce",
    ];
    let mut pairs: Vec<String> = functions.iter().map(|f| f.to_string()).collect();
    pairs.extend(functions.iter().map(|f| format!("{f}_of")));
    pairs.push("name".to_string());
    let pairs: Vec<&str> = pairs.iter().map(String::as_str).collect();
    let options = Options::parse(&arguments[1..], &["count"], &pairs)?;
    let name = options.get("name").ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "name the aggregate's result with `name = \"...\"`",
        )
    })?;
    if options.has("count") {
        return Ok(quote! {
            (#flow).aggregate(#name, #list, ::mxrs::AggregateFunction::Count, |_| {})
        });
    }
    let (key, argument) = options
        .pairs
        .iter()
        .find(|(key, _)| key != "name")
        .ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                "state what to aggregate: `count`, `sum = member`, ...",
            )
        })?;
    let key = key.to_string();
    let (function, over) = match key.strip_suffix("_of") {
        Some(function) => {
            let expression = value(argument);
            (
                function.to_string(),
                quote! { __mxrs_aggregate.expression(#expression); },
            )
        }
        None => (
            key.clone(),
            quote! { __mxrs_aggregate.attribute(#argument); },
        ),
    };
    let mut variant = function.clone();
    variant[..1].make_ascii_uppercase();
    let variant = syn::Ident::new(&variant, argument.span());
    Ok(quote! {
        (#flow).aggregate(#name, #list, ::mxrs::AggregateFunction::#variant, |__mxrs_aggregate| {
            #over
        })
    })
}

/// `create_variable!(flow, DataType::Long, value, name = "...")`.
fn create_variable(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let data_type = first(arguments, "the variable's type")?;
    let initial = arguments.get(1).ok_or_else(|| {
        syn::Error::new_spanned(data_type, "expected the variable's value after its type")
    })?;
    let options = Options::parse(&arguments[2..], &[], &["name"])?;
    let name = options.get("name").ok_or_else(|| {
        syn::Error::new(Span::call_site(), "name the variable with `name = \"...\"`")
    })?;
    let initial = value(initial);
    Ok(quote! { (#flow).create_variable(#name, #data_type, #initial) })
}

/// `change_variable!(flow, &count, value)`.
fn change_variable(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let variable = first(arguments, "the variable it changes")?;
    let new = arguments
        .get(1)
        .ok_or_else(|| syn::Error::new_spanned(variable, "expected the variable's new value"))?;
    Options::parse(&arguments[2..], &[], &[])?;
    let new = value(new);
    Ok(quote! { (#flow).change_variable(#variable, #new) })
}

/// `call_microflow!(flow, ACT_Order_Ship { Order: order }, name = "...")`:
/// the flow, by the type its declaration generates, and its arguments by
/// the names of its parameters. With `name`, the result is kept.
fn call_microflow(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let call = first(arguments, "the microflow it calls")?;
    let (target, mappings) = match call {
        Expr::Struct(literal) => {
            if let Some(rest) = &literal.rest {
                return Err(syn::Error::new_spanned(
                    rest,
                    "a call passes only the arguments it names",
                ));
            }
            let mappings = literal
                .fields
                .iter()
                .map(|field| {
                    let syn::Member::Named(parameter) = &field.member else {
                        return Err(syn::Error::new_spanned(
                            &field.member,
                            "expected a parameter's name",
                        ));
                    };
                    let parameter = parameter.to_string();
                    let parameter = parameter
                        .strip_prefix("r#")
                        .unwrap_or(&parameter)
                        .to_string();
                    let value = value(&field.expr);
                    Ok(quote! { __mxrs_call.argument(#parameter, #value); })
                })
                .collect::<syn::Result<Vec<_>>>()?;
            (literal.path.clone(), mappings)
        }
        Expr::Path(path) => (path.path.clone(), Vec::new()),
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "expected the microflow and its arguments: `ACT_Order_Ship { Order: order }`",
            ));
        }
    };
    let options = Options::parse(&arguments[1..], &[], &["name"])?;
    Ok(match options.get("name") {
        Some(name) => quote! {
            (#flow).call_into(#name, ::mxrs::MicroflowRef::<#target>::new(), |__mxrs_call| {
                #(#mappings)*
            })
        },
        None => quote! {
            (#flow).call(::mxrs::MicroflowRef::<#target>::new(), |__mxrs_call| {
                #(#mappings)*
            })
        },
    })
}

/// `log!(flow, Info, "Node", "{1} shipped", parameters = [order_number],
/// stack_trace)`.
fn log(flow: &Expr, arguments: &[Expr]) -> syn::Result<TokenStream> {
    let level = first(arguments, "the log level")?;
    let level = match level {
        Expr::Path(path) if path.path.get_ident().is_some() => {
            let level = path.path.get_ident().expect("checked");
            quote! { ::mxrs::LogSeverity::#level }
        }
        other => other.to_token_stream(),
    };
    let node = arguments.get(1).ok_or_else(|| {
        syn::Error::new(Span::call_site(), "expected the log node after the level")
    })?;
    let message = arguments
        .get(2)
        .ok_or_else(|| syn::Error::new(Span::call_site(), "expected the message after the node"))?;
    let options = Options::parse(&arguments[3..], &["stack_trace"], &["parameters"])?;
    let node = value(node);
    let parameters = match options.get("parameters") {
        Some(Expr::Array(array)) => array
            .elems
            .iter()
            .map(|parameter| {
                let parameter = value(parameter);
                quote! { __mxrs_log.parameter(#parameter); }
            })
            .collect(),
        Some(other) => {
            return Err(syn::Error::new_spanned(
                other,
                "expected `parameters = [value, ...]`",
            ));
        }
        None => Vec::new(),
    };
    let trace = options
        .has("stack_trace")
        .then(|| quote! { __mxrs_log.include_stack_trace(true); });
    Ok(quote! {
        (#flow).log(#level, #node, #message, |__mxrs_log| {
            #(#parameters)*
            #trace
        })
    })
}
