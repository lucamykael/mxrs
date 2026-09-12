//! Parses `project! { ... }`'s custom grammar.
//!
//! Grammar (informally):
//! ```text
//! project!   := <string-lit> "," <module-item>*
//! module     := "module" <ident> "{" <module-item>* "}"
//! module-item:= <entity> | <microflow>
//! entity     := "entity" <ident> "{" <entity-item>* "}"
//! entity-item:= <attribute> | <association>
//!             | "documentation" <string-lit> ";"
//!             | "persistable" <bool-lit> ";"
//! attribute  := <attr-kind> <ident> ("(" <expr> ")")? ("=" <expr>)? ";"
//! attr-kind  := "string" | "integer" | "long" | "float" | "decimal" | "boolean"
//!             | "datetime" | "autonumber" | "hash_string" | "binary" | "enumeration"
//! association:= "association" <ident> "->" <rust-path> "as" <ident> ";"
//! microflow  := "microflow" <ident> "{" <flow-item>* <rescue>? ("return" <expr> ";")? "}"
//! flow-item  := <create> | <create-list> | <change> | <delete> | <commit> | <call> | <if>
//!             | <for> | <while> | "break" ";" | "continue" ";"
//! create     := "create" <ident> "=" <rust-path> "{" <member>* "}" "commit"? ";"
//! change     := "change" <ident> "{" <member>* "}" "commit"? ";"
//! create-list:= "create_list" <ident> "=" <rust-path> ";"
//! delete     := "delete" <ident> ";"
//! commit     := "commit" <ident> ";"
//! call       := "call" <rust-path> ("(" <mapping> ("," <mapping>)* ","? ")")? ("->" <ident>)? ";"
//! if         := "if" <expr> "{" <flow-item>* "}" "else" "{" <flow-item>* "}"
//! for        := "for" <ident> "in" <ident> "{" <flow-item>* "}"
//! while      := "while" <expr> "{" <flow-item>* "}"
//! rescue     := "rescue" "{" <flow-item>* "}"
//! member     := "assoc"? <ident> "=" <expr> ";"
//! mapping    := <ident> ":" <expr>
//! ```
//!
//! Entity and member markers are generated from the declarations in this
//! same invocation. A create therefore names `Sales::Order`, binds a typed
//! `Var<Sales::Order>`, and later expressions can use `order.total()`.
//! Attribute assignments and Boolean conditions are ordinary Rust
//! expressions checked against `mxrs-expr`; raw Mendix expression strings
//! do not cross this authoring boundary.
//!
//! A `call`'s target is a `syn::Path` (e.g. `Sales::ACT_Notify`), marker-
//! checked the same way `create`/`create_list`'s entity path is — not a
//! general `Expr` like the others. This is deliberately narrower, because
//! the obvious grammar (`<expr> (...)`) is genuinely ambiguous against
//! Rust's own call-expression syntax: `Expr::parse` happily parses
//! `<expr> (<args>)` as a single call expression, and then chokes on
//! `OrderNumber: <value>` since that's not valid call-argument syntax.
//! Parsing a plain `syn::Path` (which never itself consumes a trailing
//! `(...)`) sidesteps the ambiguity entirely, at the cost of one narrower
//! rule for this one field — the same reason `create`'s entity path already
//! had to be `syn::Path`, not `Expr`. A `mapping`'s value is still an
//! `Expr`, since it's never immediately followed by `(`. Expanded via
//! `::mxrs_ir::MicroflowRef::<#microflow>::new()`, mirroring `create`'s
//! `::mxrs_ir::Ref::<#entity>::new()` — a nonexistent/renamed microflow
//! marker now fails `cargo build`, same guarantee Phase 4 already gives
//! entity/association references.
//!
//! `call`'s `-> <ident>` is deliberately narrower than
//! `FlowBuilder::call_microflow`'s full signature: presence of `-> var`
//! sets both `result_variable = Some(var)` *and* `use_return = true`
//! together (the common case — assign the call's return value and use it),
//! absence sets both `None`/`false` together. The decoupled case (assign a
//! result variable but mark it unused, or vice versa) isn't reachable from
//! this grammar — narrow now, widen later, same pattern as everywhere else
//! in this codebase.
//!
//! The grammar intentionally has no native BSON or raw-expression escape
//! hatch; unsupported authoring concepts fail closed until typed support is
//! added.

use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Expr, Ident, LitStr, Result, Token, braced, parenthesized};

pub struct ProjectInput {
    pub version: LitStr,
    pub modules: Vec<ModuleInput>,
}

pub struct ModuleInput {
    pub name: Ident,
    pub entities: Vec<EntityInput>,
    pub microflows: Vec<MicroflowInput>,
}

pub struct EntityInput {
    pub name: Ident,
    pub documentation: Option<LitStr>,
    pub persistable: Option<syn::LitBool>,
    pub attributes: Vec<AttributeInput>,
    pub associations: Vec<AssociationInput>,
}

pub struct MicroflowInput {
    pub name: Ident,
    pub activities: Vec<FlowItem>,
    pub rescue_activities: Vec<FlowItem>,
    pub return_expression: Option<Expr>,
}

/// One statement in a microflow body. Mirrors `mxrs_ir::flow::Activity`
/// one-to-one, plus `If` (which lowers to `FlowBuilder::decision`, not a
/// direct `Activity` variant — see this module's doc comment).
pub enum FlowItem {
    Create {
        variable: Ident,
        entity: syn::Path,
        members: Vec<MemberInput>,
        commit: bool,
    },
    Change {
        variable: Ident,
        members: Vec<MemberInput>,
        commit: bool,
    },
    CreateList {
        variable: Ident,
        entity: syn::Path,
    },
    Delete {
        variable: Ident,
    },
    Commit {
        variable: Ident,
    },
    Call {
        microflow: syn::Path,
        mappings: Vec<MappingInput>,
        result_variable: Option<Ident>,
    },
    If {
        condition: Expr,
        then_branch: Vec<FlowItem>,
        else_branch: Vec<FlowItem>,
    },
    LoopOver {
        iterator: Ident,
        list: Ident,
        activities: Vec<FlowItem>,
    },
    While {
        condition: Expr,
        activities: Vec<FlowItem>,
    },
    Break,
    Continue,
}

pub struct MemberInput {
    pub is_association: bool,
    pub name: Ident,
    pub value: Expr,
}

pub struct MappingInput {
    pub parameter: Ident,
    pub value: Expr,
}

pub enum AttrKind {
    String,
    Integer,
    Long,
    Float,
    Decimal,
    Boolean,
    DateTime,
    AutoNumber,
    HashString,
    Binary,
    Enumeration,
}

impl AttrKind {
    pub fn builder_method(&self) -> &'static str {
        match self {
            AttrKind::String => "string",
            AttrKind::Integer => "integer",
            AttrKind::Long => "long",
            AttrKind::Float => "float",
            AttrKind::Decimal => "decimal",
            AttrKind::Boolean => "boolean",
            AttrKind::DateTime => "datetime",
            AttrKind::AutoNumber => "autonumber",
            AttrKind::HashString => "hash_string",
            AttrKind::Binary => "binary",
            AttrKind::Enumeration => "enumeration",
        }
    }

    fn from_ident(ident: &Ident) -> Result<Self> {
        Ok(match ident.to_string().as_str() {
            "string" => AttrKind::String,
            "integer" => AttrKind::Integer,
            "long" => AttrKind::Long,
            "float" => AttrKind::Float,
            "decimal" => AttrKind::Decimal,
            "boolean" => AttrKind::Boolean,
            "datetime" => AttrKind::DateTime,
            "autonumber" => AttrKind::AutoNumber,
            "hash_string" => AttrKind::HashString,
            "binary" => AttrKind::Binary,
            "enumeration" => AttrKind::Enumeration,
            other => {
                return Err(syn::Error::new(
                    ident.span(),
                    format!(
                        "unknown attribute kind `{other}` (expected one of: string, integer, long, float, decimal, boolean, datetime, autonumber, hash_string, binary, enumeration)"
                    ),
                ));
            }
        })
    }
}

pub struct AttributeInput {
    pub kind: AttrKind,
    pub name: Ident,
    pub enumeration: Option<Expr>,
    pub default: Option<Expr>,
}

pub struct AssociationInput {
    pub name: Ident,
    /// A Rust path to a marker type implementing `mxrs_ir::EntityMarker`
    /// (e.g. `Customer` or `markers::CRM::Account`), resolved in the
    /// macro call site's own scope — see this module's doc comment.
    pub target: syn::Path,
    pub association_type: Ident,
}

impl Parse for ProjectInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let version: LitStr = input.parse()?;
        input.parse::<Token![,]>()?;
        let mut modules = Vec::new();
        while !input.is_empty() {
            modules.push(input.parse()?);
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(ProjectInput { version, modules })
    }
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream) -> Result<Self> {
        expect_keyword(input, "module")?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut entities = Vec::new();
        let mut microflows = Vec::new();
        while !content.is_empty() {
            let peeked: Ident = content.fork().parse()?;
            if peeked == "microflow" {
                microflows.push(content.parse()?);
            } else {
                entities.push(content.parse()?);
            }
        }
        Ok(ModuleInput {
            name,
            entities,
            microflows,
        })
    }
}

impl Parse for EntityInput {
    fn parse(input: ParseStream) -> Result<Self> {
        expect_keyword(input, "entity")?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut documentation: Option<LitStr> = None;
        let mut persistable: Option<syn::LitBool> = None;
        let mut attributes = Vec::new();
        let mut associations = Vec::new();
        while !content.is_empty() {
            let peeked: Ident = content.fork().parse()?;
            if peeked == "association" {
                associations.push(content.parse()?);
            } else if peeked == "documentation" {
                content.parse::<Ident>()?;
                let lit: LitStr = content.parse()?;
                content.parse::<Token![;]>()?;
                if documentation.is_some() {
                    return Err(syn::Error::new(lit.span(), "duplicate `documentation`"));
                }
                documentation = Some(lit);
            } else if peeked == "persistable" {
                content.parse::<Ident>()?;
                let lit: syn::LitBool = content.parse()?;
                content.parse::<Token![;]>()?;
                if persistable.is_some() {
                    return Err(syn::Error::new(lit.span(), "duplicate `persistable`"));
                }
                persistable = Some(lit);
            } else {
                attributes.push(content.parse()?);
            }
        }
        Ok(EntityInput {
            name,
            documentation,
            persistable,
            attributes,
            associations,
        })
    }
}

impl Parse for MicroflowInput {
    fn parse(input: ParseStream) -> Result<Self> {
        expect_keyword(input, "microflow")?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut activities = Vec::new();
        let mut rescue_activities = Vec::new();
        let mut return_expression: Option<Expr> = None;
        while !content.is_empty() {
            if content.peek(Token![return]) {
                content.parse::<Token![return]>()?;
                let expr: Expr = content.parse()?;
                content.parse::<Token![;]>()?;
                if return_expression.is_some() {
                    return Err(syn::Error::new(expr.span(), "duplicate `return`"));
                }
                return_expression = Some(expr);
                if !content.is_empty() {
                    return Err(
                        content.error("`return` must be the last statement in a microflow body")
                    );
                }
            } else if peek_keyword(&content, "rescue") {
                let rescue_keyword: Ident = content.parse()?;
                if !rescue_activities.is_empty() {
                    return Err(syn::Error::new(rescue_keyword.span(), "duplicate `rescue`"));
                }
                let rescue_content;
                braced!(rescue_content in content);
                while !rescue_content.is_empty() {
                    rescue_activities.push(rescue_content.parse()?);
                }
                if !content.is_empty() && !content.peek(Token![return]) {
                    return Err(content.error("`rescue` must be the last activity block"));
                }
            } else {
                activities.push(content.parse()?);
            }
        }
        validate_loop_control(&activities, false)?;
        validate_loop_control(&rescue_activities, false)?;
        Ok(MicroflowInput {
            name,
            activities,
            rescue_activities,
            return_expression,
        })
    }
}

impl Parse for FlowItem {
    fn parse(input: ParseStream) -> Result<Self> {
        if input.peek(Token![if]) {
            return parse_if(input);
        }
        if input.peek(Token![for]) {
            return parse_loop_over(input);
        }
        if input.peek(Token![while]) {
            return parse_while(input);
        }
        if input.peek(Token![break]) {
            input.parse::<Token![break]>()?;
            input.parse::<Token![;]>()?;
            return Ok(FlowItem::Break);
        }
        if input.peek(Token![continue]) {
            input.parse::<Token![continue]>()?;
            input.parse::<Token![;]>()?;
            return Ok(FlowItem::Continue);
        }
        let keyword: Ident = input.fork().parse()?;
        match keyword.to_string().as_str() {
            "create" => parse_create_or_change(input, true),
            "create_list" => parse_create_list(input),
            "change" => parse_create_or_change(input, false),
            "delete" => {
                input.parse::<Ident>()?;
                let variable: Ident = input.parse()?;
                input.parse::<Token![;]>()?;
                Ok(FlowItem::Delete { variable })
            }
            "commit" => {
                input.parse::<Ident>()?;
                let variable: Ident = input.parse()?;
                input.parse::<Token![;]>()?;
                Ok(FlowItem::Commit { variable })
            }
            "call" => parse_call(input),
            other => Err(syn::Error::new(
                keyword.span(),
                format!(
                    "unknown microflow statement `{other}` (expected one of: create, create_list, change, delete, commit, call, if, for, while, break, continue, return)"
                ),
            )),
        }
    }
}

fn parse_create_list(input: ParseStream) -> Result<FlowItem> {
    input.parse::<Ident>()?;
    let variable = input.parse()?;
    input.parse::<Token![=]>()?;
    let entity = input.parse()?;
    input.parse::<Token![;]>()?;
    Ok(FlowItem::CreateList { variable, entity })
}

fn parse_create_or_change(input: ParseStream, is_create: bool) -> Result<FlowItem> {
    input.parse::<Ident>()?; // consumes "create"/"change"
    let variable: Ident = input.parse()?;
    let entity = if is_create {
        input.parse::<Token![=]>()?;
        Some(input.parse::<syn::Path>()?)
    } else {
        None
    };
    let content;
    braced!(content in input);
    let mut members = Vec::new();
    while !content.is_empty() {
        members.push(content.parse()?);
    }
    let commit = if peek_keyword(input, "commit") {
        input.parse::<Ident>()?;
        true
    } else {
        false
    };
    input.parse::<Token![;]>()?;
    Ok(if is_create {
        FlowItem::Create {
            variable,
            entity: entity.expect("create parsed an entity"),
            members,
            commit,
        }
    } else {
        FlowItem::Change {
            variable,
            members,
            commit,
        }
    })
}

fn parse_loop_over(input: ParseStream) -> Result<FlowItem> {
    input.parse::<Token![for]>()?;
    let iterator: Ident = input.parse()?;
    input.parse::<Token![in]>()?;
    let list: Ident = input.parse()?;
    let content;
    braced!(content in input);
    let mut activities = Vec::new();
    while !content.is_empty() {
        activities.push(content.parse()?);
    }
    Ok(FlowItem::LoopOver {
        iterator,
        list,
        activities,
    })
}

fn parse_while(input: ParseStream) -> Result<FlowItem> {
    input.parse::<Token![while]>()?;
    let condition = input.call(Expr::parse_without_eager_brace)?;
    let content;
    braced!(content in input);
    let mut activities = Vec::new();
    while !content.is_empty() {
        activities.push(content.parse()?);
    }
    Ok(FlowItem::While {
        condition,
        activities,
    })
}

fn validate_loop_control(items: &[FlowItem], inside_loop: bool) -> Result<()> {
    for item in items {
        match item {
            FlowItem::Break | FlowItem::Continue if !inside_loop => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "`break` and `continue` are only valid inside a microflow loop",
                ));
            }
            FlowItem::LoopOver { activities, .. } | FlowItem::While { activities, .. } => {
                validate_loop_control(activities, true)?;
            }
            FlowItem::If {
                then_branch,
                else_branch,
                ..
            } => {
                validate_loop_control(then_branch, inside_loop)?;
                validate_loop_control(else_branch, inside_loop)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_call(input: ParseStream) -> Result<FlowItem> {
    input.parse::<Ident>()?; // consumes "call"
    let microflow: syn::Path = input.parse()?;
    let mut mappings = Vec::new();
    if input.peek(syn::token::Paren) {
        let content;
        parenthesized!(content in input);
        while !content.is_empty() {
            mappings.push(content.parse()?);
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
    }
    let result_variable = if input.peek(Token![->]) {
        input.parse::<Token![->]>()?;
        Some(input.parse()?)
    } else {
        None
    };
    input.parse::<Token![;]>()?;
    Ok(FlowItem::Call {
        microflow,
        mappings,
        result_variable,
    })
}

fn parse_if(input: ParseStream) -> Result<FlowItem> {
    input.parse::<Token![if]>()?;
    let condition: Expr = input.call(Expr::parse_without_eager_brace)?;
    let then_content;
    braced!(then_content in input);
    let mut then_branch = Vec::new();
    while !then_content.is_empty() {
        then_branch.push(then_content.parse()?);
    }
    input.parse::<Token![else]>()?;
    let else_content;
    braced!(else_content in input);
    let mut else_branch = Vec::new();
    while !else_content.is_empty() {
        else_branch.push(else_content.parse()?);
    }
    Ok(FlowItem::If {
        condition,
        then_branch,
        else_branch,
    })
}

impl Parse for MemberInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let is_association = peek_keyword(input, "assoc");
        if is_association {
            input.parse::<Ident>()?;
        }
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let value: Expr = input.parse()?;
        input.parse::<Token![;]>()?;
        Ok(MemberInput {
            is_association,
            name,
            value,
        })
    }
}

impl Parse for MappingInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let parameter: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let value: Expr = input.parse()?;
        Ok(MappingInput { parameter, value })
    }
}

impl Parse for AttributeInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let kind_ident: Ident = input.parse()?;
        let kind = AttrKind::from_ident(&kind_ident)?;
        let name: Ident = input.parse()?;
        let enumeration = if matches!(kind, AttrKind::Enumeration) {
            if !input.peek(syn::token::Paren) {
                return Err(syn::Error::new(
                    name.span(),
                    "enumeration attributes require a qualified enumeration name in parentheses, for example `enumeration State(\"Sales.State\");`",
                ));
            }
            let content;
            parenthesized!(content in input);
            Some(content.parse()?)
        } else {
            None
        };
        let default = if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        input.parse::<Token![;]>()?;
        Ok(AttributeInput {
            kind,
            name,
            enumeration,
            default,
        })
    }
}

impl Parse for AssociationInput {
    fn parse(input: ParseStream) -> Result<Self> {
        expect_keyword(input, "association")?;
        let name: Ident = input.parse()?;
        input.parse::<Token![->]>()?;
        let target: syn::Path = input.call(syn::Path::parse_mod_style)?;
        input.parse::<Token![as]>()?;
        let association_type: Ident = input.parse()?;
        if association_type != "Reference" && association_type != "ReferenceSet" {
            return Err(syn::Error::new(
                association_type.span(),
                format!(
                    "unknown association type `{association_type}` (expected `Reference` or `ReferenceSet`)"
                ),
            ));
        }
        input.parse::<Token![;]>()?;
        Ok(AssociationInput {
            name,
            target,
            association_type,
        })
    }
}

fn expect_keyword(input: ParseStream, keyword: &str) -> Result<()> {
    let ident: Ident = input.parse()?;
    if ident == keyword {
        Ok(())
    } else {
        Err(syn::Error::new(
            ident.span(),
            format!("expected `{keyword}`, found `{ident}`"),
        ))
    }
}

/// Non-consuming: `true` if the next token is the identifier `keyword`,
/// without erroring (and without advancing `input`) when it isn't — used
/// for optional trailing/leading keywords (`commit`, `assoc`) that aren't
/// reserved words, so a plain `Ident::parse` peek can't distinguish "wrong
/// keyword" from "not present at all" the way `expect_keyword` needs to.
fn peek_keyword(input: ParseStream, keyword: &str) -> bool {
    input
        .fork()
        .parse::<Ident>()
        .map(|ident| ident == keyword)
        .unwrap_or(false)
}
