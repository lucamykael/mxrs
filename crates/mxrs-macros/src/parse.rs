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
//! attribute  := <attr-kind> <ident> ("=" <expr>)? ";"
//! attr-kind  := "string" | "integer" | "long" | "decimal" | "boolean" | "datetime" | "autonumber"
//! association:= "association" <ident> "->" <rust-path> "as" <ident> ";"
//! microflow  := "microflow" <ident> "{" <flow-item>* ("return" <expr> ";")? "}"
//! flow-item  := <create> | <change> | <delete> | <commit> | <call> | <if>
//! create     := "create" <ident> "=" <expr> "{" <member>* "}" "commit"? ";"
//! change     := "change" <ident> "=" <expr> "{" <member>* "}" "commit"? ";"
//! delete     := "delete" <ident> ";"
//! commit     := "commit" <ident> ";"
//! call       := "call" <string-lit> ("(" <mapping> ("," <mapping>)* ","? ")")? ("->" <ident>)? ";"
//! if         := "if" <expr> "{" <flow-item>* "}" "else" "{" <flow-item>* "}"
//! member     := "assoc"? <ident> "=" <expr> ";"
//! mapping    := <ident> ":" <expr>
//! ```
//!
//! `<rust-path>` (an association's target) is a real Rust type path — e.g.
//! `Customer` or `markers::CRM::Account` — resolving to a marker type
//! implementing `mxrs_ir::EntityMarker`, exactly what
//! `mxrs_dsl::EntityBuilder::association` requires since `mxrs-dsl` was
//! wired to reject raw strings there. This is deliberately *not* a
//! Mendix-style dotted `Module.Entity` name parsed and re-joined by this
//! crate (an earlier version of this grammar did that): the target now has
//! to be something that resolves in the caller's own scope the same way
//! any other Rust path would (via `use`, a fully-qualified path, ...),
//! because it expands directly into `Ref::<#target>::new()`.
//!
//! A `create`/`change` statement's entity, an `if`'s condition, and a
//! `member`'s value are all a `syn::Expr` rather than a further custom
//! grammar — same trick already used by `microflow`'s `return` statement: a
//! plain Rust string literal (e.g. `"Sales.Order"`, `"$order/Total > 0"`)
//! is itself a valid `Expr` that satisfies the `impl Into<String>` these
//! expand into, so this crate doesn't need its own expression-parsing
//! grammar for Mendix expressions. This intentionally mirrors
//! `mxrs-dsl::FlowBuilder`'s own scope: entity names in `create`/`change`
//! are plain strings, not `EntityMarker`-checked
//! (`FlowBuilder::create_object`/`change_object` take `impl Into<String>`
//! for `entity`, same as this macro emits) — widening that to compile-time
//! checking is a separate, larger `mxrs-dsl` change, not something this
//! macro can do on its own without inventing a capability `mxrs-dsl` itself
//! doesn't already expose (the non-negotiable rule from the plan's DSL/
//! macro track section).
//!
//! A `call`'s microflow name is a `syn::LitStr`, not a general `Expr` like
//! the others — deliberately narrower, because the obvious grammar
//! (`"Sales.ACT_Notify" (...)`) is genuinely ambiguous against Rust's own
//! call-expression syntax: `Expr::parse` happily parses `<expr> (<args>)`
//! as a single call expression (a string literal is a valid callee in
//! Rust's grammar, even though it'd never type-check), and then chokes on
//! `OrderNumber: <value>` since that's not valid call-argument syntax. A
//! plain `LitStr` sidesteps the ambiguity entirely, at the cost of one
//! narrower rule for this one field. A `mapping`'s value is still an
//! `Expr`, since it's never immediately followed by `(`.
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
//! Still not covered by this grammar (no escape hatch to the native/
//! pluggable widget surface, and no loops/rescue blocks in microflow
//! bodies) — widen incrementally, the same way every other "first slice" in
//! this codebase has (see `mxrs-writer`'s and `mxrs-typegen`'s crate docs
//! for the same pattern).

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
    pub return_expression: Option<Expr>,
}

/// One statement in a microflow body. Mirrors `mxrs_ir::flow::Activity`
/// one-to-one, plus `If` (which lowers to `FlowBuilder::decision`, not a
/// direct `Activity` variant — see this module's doc comment).
pub enum FlowItem {
    Create {
        variable: Ident,
        entity: Expr,
        members: Vec<MemberInput>,
        commit: bool,
    },
    Change {
        variable: Ident,
        entity: Expr,
        members: Vec<MemberInput>,
        commit: bool,
    },
    Delete {
        variable: Ident,
    },
    Commit {
        variable: Ident,
    },
    Call {
        name: LitStr,
        mappings: Vec<MappingInput>,
        result_variable: Option<Ident>,
    },
    If {
        condition: Expr,
        then_branch: Vec<FlowItem>,
        else_branch: Vec<FlowItem>,
    },
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
    Decimal,
    Boolean,
    DateTime,
    AutoNumber,
}

impl AttrKind {
    pub fn builder_method(&self) -> &'static str {
        match self {
            AttrKind::String => "string",
            AttrKind::Integer => "integer",
            AttrKind::Long => "long",
            AttrKind::Decimal => "decimal",
            AttrKind::Boolean => "boolean",
            AttrKind::DateTime => "datetime",
            AttrKind::AutoNumber => "autonumber",
        }
    }

    fn from_ident(ident: &Ident) -> Result<Self> {
        Ok(match ident.to_string().as_str() {
            "string" => AttrKind::String,
            "integer" => AttrKind::Integer,
            "long" => AttrKind::Long,
            "decimal" => AttrKind::Decimal,
            "boolean" => AttrKind::Boolean,
            "datetime" => AttrKind::DateTime,
            "autonumber" => AttrKind::AutoNumber,
            other => {
                return Err(syn::Error::new(
                    ident.span(),
                    format!(
                        "unknown attribute kind `{other}` (expected one of: string, integer, long, decimal, boolean, datetime, autonumber)"
                    ),
                ));
            }
        })
    }
}

pub struct AttributeInput {
    pub kind: AttrKind,
    pub name: Ident,
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
            } else {
                activities.push(content.parse()?);
            }
        }
        Ok(MicroflowInput {
            name,
            activities,
            return_expression,
        })
    }
}

impl Parse for FlowItem {
    fn parse(input: ParseStream) -> Result<Self> {
        if input.peek(Token![if]) {
            return parse_if(input);
        }
        let keyword: Ident = input.fork().parse()?;
        match keyword.to_string().as_str() {
            "create" => parse_create_or_change(input, true),
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
                    "unknown microflow statement `{other}` (expected one of: create, change, delete, commit, call, if, return)"
                ),
            )),
        }
    }
}

fn parse_create_or_change(input: ParseStream, is_create: bool) -> Result<FlowItem> {
    input.parse::<Ident>()?; // consumes "create"/"change"
    let variable: Ident = input.parse()?;
    input.parse::<Token![=]>()?;
    let entity: Expr = input.call(Expr::parse_without_eager_brace)?;
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
            entity,
            members,
            commit,
        }
    } else {
        FlowItem::Change {
            variable,
            entity,
            members,
            commit,
        }
    })
}

fn parse_call(input: ParseStream) -> Result<FlowItem> {
    input.parse::<Ident>()?; // consumes "call"
    let name: LitStr = input.parse()?;
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
        name,
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
