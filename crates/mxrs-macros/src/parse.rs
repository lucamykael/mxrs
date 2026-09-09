//! Parses `project! { ... }`'s custom grammar. Deliberately narrower than
//! `mxrs-dsl`'s full builder surface for this first pass: entity
//! documentation/`persistable`, module-level microflows, and cross-module
//! association targets beyond a single `Module.Entity` dotted path are not
//! part of the grammar yet — widen incrementally, the same way every other
//! "first slice" in this codebase has (see `mxrs-writer`'s and
//! `mxrs-typegen`'s crate docs for the same pattern).
//!
//! Grammar (informally):
//! ```text
//! project!   := <string-lit> "," <module>*
//! module     := "module" <ident> "{" <entity>* "}"
//! entity     := "entity" <ident> "{" (<attribute> | <association>)* "}"
//! attribute  := <attr-kind> <ident> ("=" <expr>)? ";"
//! attr-kind  := "string" | "integer" | "long" | "decimal" | "boolean" | "datetime" | "autonumber"
//! association:= "association" <ident> "->" <path> "as" <ident> ";"
//! path       := <ident> ("." <ident>)?
//! ```

use syn::parse::{Parse, ParseStream};
use syn::{braced, Ident, LitStr, Result, Token};

pub struct ProjectInput {
    pub version: LitStr,
    pub modules: Vec<ModuleInput>,
}

pub struct ModuleInput {
    pub name: Ident,
    pub entities: Vec<EntityInput>,
}

pub struct EntityInput {
    pub name: Ident,
    pub attributes: Vec<AttributeInput>,
    pub associations: Vec<AssociationInput>,
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
    /// `"Entity"` or `"Module.Entity"` — already joined into the same
    /// string shape `mxrs-dsl`'s `association(name, target, ...)` expects.
    pub target: String,
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
        while !content.is_empty() {
            entities.push(content.parse()?);
        }
        Ok(ModuleInput { name, entities })
    }
}

impl Parse for EntityInput {
    fn parse(input: ParseStream) -> Result<Self> {
        expect_keyword(input, "entity")?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut attributes = Vec::new();
        let mut associations = Vec::new();
        while !content.is_empty() {
            let peeked: Ident = content.fork().parse()?;
            if peeked == "association" {
                associations.push(content.parse()?);
            } else {
                attributes.push(content.parse()?);
            }
        }
        Ok(EntityInput { name, attributes, associations })
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
        Ok(AttributeInput { kind, name, default })
    }
}

impl Parse for AssociationInput {
    fn parse(input: ParseStream) -> Result<Self> {
        expect_keyword(input, "association")?;
        let name: Ident = input.parse()?;
        input.parse::<Token![->]>()?;
        let first: Ident = input.parse()?;
        let target = if input.peek(Token![.]) {
            input.parse::<Token![.]>()?;
            let second: Ident = input.parse()?;
            format!("{first}.{second}")
        } else {
            first.to_string()
        };
        input.parse::<Token![as]>()?;
        let association_type: Ident = input.parse()?;
        if association_type != "Reference" && association_type != "ReferenceSet" {
            return Err(syn::Error::new(
                association_type.span(),
                format!("unknown association type `{association_type}` (expected `Reference` or `ReferenceSet`)"),
            ));
        }
        input.parse::<Token![;]>()?;
        Ok(AssociationInput { name, target, association_type })
    }
}

fn expect_keyword(input: ParseStream, keyword: &str) -> Result<()> {
    let ident: Ident = input.parse()?;
    if ident == keyword {
        Ok(())
    } else {
        Err(syn::Error::new(ident.span(), format!("expected `{keyword}`, found `{ident}`")))
    }
}
