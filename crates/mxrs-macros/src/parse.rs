//! Parses `project! { ... }`'s custom grammar. Deliberately narrower than
//! `mxrs-dsl`'s full builder surface for this first pass: a module-level
//! microflow's body only covers a `return` statement (no
//! create/change-object, decisions, or microflow calls — the full activity
//! DSL is a much larger custom grammar than a first slice needs), and
//! there's no escape hatch to the native/pluggable widget surface yet.
//! Widen incrementally, the same way every other "first slice" in this
//! codebase has (see `mxrs-writer`'s and `mxrs-typegen`'s crate docs for the
//! same pattern).
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
//! microflow  := "microflow" <ident> "{" "return" <expr> ";" "}"
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

use syn::parse::{Parse, ParseStream};
use syn::{braced, Ident, LitStr, Result, Token};

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
    pub return_expression: syn::Expr,
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
                ))
            }
        })
    }
}

pub struct AttributeInput {
    pub kind: AttrKind,
    pub name: Ident,
    pub default: Option<syn::Expr>,
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
        content.parse::<Token![return]>()?;
        let return_expression: syn::Expr = content.parse()?;
        content.parse::<Token![;]>()?;
        Ok(MicroflowInput {
            name,
            return_expression,
        })
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
                format!("unknown association type `{association_type}` (expected `Reference` or `ReferenceSet`)"),
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
