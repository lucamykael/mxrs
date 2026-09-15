//! Parses `project! { ... }`'s custom grammar.
//!
//! Grammar (informally):
//! ```text
//! project!   := <string-lit> "," <module-item>*
//! module     := "module" <ident> "{" <module-item>* "}"
//! module-item:= <entity> | <microflow> | <nanoflow> | <task-queue>
//! task-queue := "task_queue" <ident> "{" <queue-option>* "}"
//! queue-option := "parallelism" <int-lit> ";" | "parallelism_expression" <string-lit> ";"
//!               | "scope" ("PerNode" | "ClusterWide") ";"
//!               | "documentation" <string-lit> ";" | "excluded" <bool-lit> ";"
//!               | "export_level" ("Hidden" | "Published") ";"
//! entity     := "entity" <ident> "{" <entity-item>* "}"
//! entity-item:= <attribute> | <association> | <inheritance> | <index>
//!             | <lifecycle> | "clear_indexes" ";" | "clear_lifecycle" ";"
//!             | "documentation" <string-lit> ";"
//!             | "persistable" <bool-lit> ";"
//! inheritance:= "generalizes" <rust-path> ";"
//!             | "system_members" "{" <system-member-option>* "}"
//! system-member-option := ("owner" | "created_date" | "changed_date" | "changed_by") <bool-lit> ";"
//! index      := "index" "{" <index-item>* "}"
//! index-item := ("attribute" <ident> | "system" <system-member>) ("ascending" <bool-lit>)? ";"
//!             | "include_offline" <bool-lit> ";"
//! lifecycle  := ("before_commit" | "after_commit" | "before_delete" | "after_delete")
//!               <rust-path> (";" | "{" <lifecycle-option>* "}" ";"?)
//! lifecycle-option := "pass_event_object" <bool-lit> ";"
//!                   | "raise_error_on_false" <bool-lit> ";"
//! attribute  := <attr-kind> <ident> ("(" <expr> ")")? ("=" <expr>)?
//!               (";" | "{" <attribute-option>* "}" ";"?)
//! attribute-option := "documentation" <string-lit> ";" | "length" <int-lit> ";"
//!               | "localize_date" <bool-lit> ";" | "required" <bool-lit> ";"
//!               | "unique" <bool-lit> ";"
//! attr-kind  := "string" | "integer" | "long" | "float" | "decimal" | "boolean"
//!             | "datetime" | "autonumber" | "hash_string" | "binary" | "enumeration"
//! association:= "association" <ident> "->" <rust-path> "as" <ident>
//!               (";" | "{" <association-option>* "}" ";"?)
//! association-option := "owner" ("Default" | "Both") ";"
//!               | "storage" ("Column" | "Table") ";"
//!               | "documentation" <string-lit> ";"
//! microflow  := "microflow" <ident> "{" <flow-item>* <rescue>? ("return" <expr> ";")? "}"
//! nanoflow   := "nanoflow" <ident> "{" <flow-item>* <rescue>? ("return" <expr> ";")? "}"
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
use syn::{Expr, Ident, LitStr, Result, Token, braced, bracketed, parenthesized};

pub struct ProjectInput {
    pub version: LitStr,
    pub modules: Vec<ModuleInput>,
}

pub struct ModuleInput {
    pub name: Ident,
    pub entities: Vec<EntityInput>,
    pub roles: Vec<ModuleRoleInput>,
    pub oql_view_sources: Vec<OqlViewSourceInput>,
    pub task_queues: Vec<TaskQueueInput>,
    pub microflows: Vec<MicroflowInput>,
    pub nanoflows: Vec<MicroflowInput>,
}

pub struct TaskQueueInput {
    pub name: Ident,
    pub parallelism: Option<syn::LitInt>,
    pub parallelism_expression: Option<LitStr>,
    pub scope: Option<Ident>,
    pub documentation: Option<LitStr>,
    pub excluded: Option<syn::LitBool>,
    pub export_level: Option<Ident>,
}

impl Parse for TaskQueueInput {
    fn parse(input: ParseStream) -> Result<Self> {
        expect_keyword(input, "task_queue")?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut queue = Self {
            name,
            parallelism: None,
            parallelism_expression: None,
            scope: None,
            documentation: None,
            excluded: None,
            export_level: None,
        };
        while !content.is_empty() {
            let option: Ident = content.parse()?;
            match option.to_string().as_str() {
                "parallelism" => {
                    let value: syn::LitInt = content.parse()?;
                    let number = value.base10_parse::<u32>()?;
                    if number == 0 || number > i32::MAX as u32 {
                        return Err(syn::Error::new(
                            value.span(),
                            "parallelism must be between 1 and 2147483647",
                        ));
                    }
                    set_once(&mut queue.parallelism, value, &option)?;
                }
                "parallelism_expression" => {
                    let value: LitStr = content.parse()?;
                    if value.value().trim().is_empty() {
                        return Err(syn::Error::new(
                            value.span(),
                            "parallelism expression must not be empty",
                        ));
                    }
                    set_once(&mut queue.parallelism_expression, value, &option)?;
                }
                "scope" => {
                    let value: Ident = content.parse()?;
                    if !matches!(value.to_string().as_str(), "PerNode" | "ClusterWide") {
                        return Err(syn::Error::new(
                            value.span(),
                            "unknown task queue scope (expected PerNode or ClusterWide)",
                        ));
                    }
                    set_once(&mut queue.scope, value, &option)?;
                }
                "documentation" => set_once(&mut queue.documentation, content.parse()?, &option)?,
                "excluded" => set_once(&mut queue.excluded, content.parse()?, &option)?,
                "export_level" => {
                    let value: Ident = content.parse()?;
                    if !matches!(value.to_string().as_str(), "Hidden" | "Published") {
                        return Err(syn::Error::new(
                            value.span(),
                            "unknown export level (expected Hidden or Published)",
                        ));
                    }
                    set_once(&mut queue.export_level, value, &option)?;
                }
                _ => return Err(syn::Error::new(option.span(), "unknown task queue option")),
            }
            content.parse::<Token![;]>()?;
        }
        if queue.parallelism.is_some() == queue.parallelism_expression.is_some() {
            return Err(syn::Error::new(
                queue.name.span(),
                "task_queue requires exactly one of parallelism or parallelism_expression",
            ));
        }
        if queue.parallelism.is_some() && queue.scope.is_some() {
            return Err(syn::Error::new(
                queue.name.span(),
                "scope requires parallelism_expression",
            ));
        }
        Ok(queue)
    }
}

pub struct EntityInput {
    pub name: Ident,
    pub documentation: Option<LitStr>,
    pub persistable: Option<syn::LitBool>,
    pub image: Option<EntityImageInput>,
    pub source: Option<EntitySourceInput>,
    pub attributes: Vec<AttributeInput>,
    pub associations: Vec<AssociationInput>,
    pub inheritance: Option<InheritanceInput>,
    pub indexes: Option<Vec<IndexInput>>,
    pub lifecycle: Option<Vec<LifecycleInput>>,
    pub access_rules: Option<Vec<AccessRuleInput>>,
}

pub struct ModuleRoleInput {
    pub name: Ident,
    pub description: LitStr,
}

pub struct OqlViewSourceInput {
    pub name: Ident,
    pub query: LitStr,
    pub documentation: Option<LitStr>,
    pub excluded: Option<syn::LitBool>,
    pub export_level: Option<Ident>,
}

pub enum EntityImageInput {
    None,
    Reference(LitStr),
}

pub enum EntitySourceInput {
    Stored,
    OqlView(syn::Path),
}

pub struct AccessRuleInput {
    pub roles: Vec<LitStr>,
    pub documentation: Option<LitStr>,
    pub allow_create: Option<syn::LitBool>,
    pub allow_delete: Option<syn::LitBool>,
    pub default_rights: Option<Ident>,
    pub xpath: Option<LitStr>,
    pub xpath_caption: Option<LitStr>,
    pub members: Vec<AccessMemberInput>,
}

pub enum AccessMemberInput {
    Attribute { name: Ident, rights: Ident },
    Association { name: Ident, rights: Ident },
}

pub enum InheritanceInput {
    Root {
        owner: Option<syn::LitBool>,
        created_date: Option<syn::LitBool>,
        changed_date: Option<syn::LitBool>,
        changed_by: Option<syn::LitBool>,
    },
    Generalizes(syn::Path),
}

pub struct IndexInput {
    pub members: Vec<IndexMemberInput>,
    pub include_offline: Option<syn::LitBool>,
}

pub enum IndexMemberInput {
    Attribute {
        name: Ident,
        ascending: Option<syn::LitBool>,
    },
    System {
        member: Ident,
        ascending: Option<syn::LitBool>,
    },
}

pub struct LifecycleInput {
    pub event: Ident,
    pub handler: syn::Path,
    pub pass_event_object: Option<syn::LitBool>,
    pub raise_error_on_false: Option<syn::LitBool>,
}

pub struct MicroflowInput {
    pub name: Ident,
    pub is_nanoflow: bool,
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
    pub documentation: Option<LitStr>,
    pub length: Option<syn::LitInt>,
    pub localize_date: Option<syn::LitBool>,
    pub required: Option<syn::LitBool>,
    pub unique: Option<syn::LitBool>,
}

pub struct AssociationInput {
    pub name: Ident,
    /// A Rust path to a marker type implementing `mxrs_ir::EntityMarker`
    /// (e.g. `Customer` or `markers::CRM::Account`), resolved in the
    /// macro call site's own scope — see this module's doc comment.
    pub target: syn::Path,
    pub association_type: Ident,
    pub owner: Option<Ident>,
    pub storage: Option<Ident>,
    pub documentation: Option<LitStr>,
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
        let mut roles = Vec::new();
        let mut oql_view_sources = Vec::new();
        let mut task_queues = Vec::new();
        let mut microflows = Vec::new();
        let mut nanoflows = Vec::new();
        while !content.is_empty() {
            let peeked: Ident = content.fork().parse()?;
            if peeked == "microflow" {
                microflows.push(content.parse()?);
            } else if peeked == "nanoflow" {
                nanoflows.push(content.parse()?);
            } else if peeked == "role" {
                content.parse::<Ident>()?;
                let role_name = content.parse()?;
                let description = content.parse()?;
                content.parse::<Token![;]>()?;
                roles.push(ModuleRoleInput {
                    name: role_name,
                    description,
                });
            } else if peeked == "task_queue" {
                task_queues.push(content.parse()?);
            } else if peeked == "oql_view_source" {
                content.parse::<Ident>()?;
                let source_name = content.parse()?;
                let query = content.parse()?;
                let mut documentation = None;
                let mut excluded = None;
                let mut export_level = None;
                if content.peek(Token![;]) {
                    content.parse::<Token![;]>()?;
                } else {
                    let options;
                    braced!(options in content);
                    while !options.is_empty() {
                        let option: Ident = options.parse()?;
                        match option.to_string().as_str() {
                            "documentation" => {
                                set_once(&mut documentation, options.parse()?, &option)?
                            }
                            "excluded" => set_once(&mut excluded, options.parse()?, &option)?,
                            "export_level" => {
                                let value: Ident = options.parse()?;
                                if !matches!(value.to_string().as_str(), "Hidden" | "Published") {
                                    return Err(syn::Error::new(
                                        value.span(),
                                        "unknown export level (expected Hidden or Published)",
                                    ));
                                }
                                set_once(&mut export_level, value, &option)?;
                            }
                            _ => {
                                return Err(syn::Error::new(
                                    option.span(),
                                    "unknown OQL view source option",
                                ));
                            }
                        }
                        options.parse::<Token![;]>()?;
                    }
                }
                oql_view_sources.push(OqlViewSourceInput {
                    name: source_name,
                    query,
                    documentation,
                    excluded,
                    export_level,
                });
            } else {
                entities.push(content.parse()?);
            }
        }
        Ok(ModuleInput {
            name,
            entities,
            roles,
            oql_view_sources,
            task_queues,
            microflows,
            nanoflows,
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
        let mut image = None;
        let mut source = None;
        let mut attributes = Vec::new();
        let mut associations = Vec::new();
        let mut inheritance = None;
        let mut indexes = None;
        let mut lifecycle = None;
        let mut access_rules = None;
        while !content.is_empty() {
            let peeked: Ident = content.fork().parse()?;
            if peeked == "association" {
                associations.push(content.parse()?);
            } else if peeked == "access_rule" {
                access_rules
                    .get_or_insert_with(Vec::new)
                    .push(parse_access_rule(&content)?);
            } else if peeked == "clear_access_rules" {
                content.parse::<Ident>()?;
                content.parse::<Token![;]>()?;
                if access_rules
                    .as_ref()
                    .is_some_and(|values: &Vec<AccessRuleInput>| !values.is_empty())
                {
                    return Err(
                        content.error("`clear_access_rules` conflicts with declared access rules")
                    );
                }
                access_rules = Some(vec![]);
            } else if peeked == "image" {
                let keyword: Ident = content.parse()?;
                let reference = content.parse()?;
                content.parse::<Token![;]>()?;
                if image
                    .replace(EntityImageInput::Reference(reference))
                    .is_some()
                {
                    return Err(syn::Error::new(keyword.span(), "duplicate entity image"));
                }
            } else if peeked == "clear_image" {
                let keyword: Ident = content.parse()?;
                content.parse::<Token![;]>()?;
                if image.replace(EntityImageInput::None).is_some() {
                    return Err(syn::Error::new(keyword.span(), "duplicate entity image"));
                }
            } else if peeked == "stored" {
                let keyword: Ident = content.parse()?;
                content.parse::<Token![;]>()?;
                if source.replace(EntitySourceInput::Stored).is_some() {
                    return Err(syn::Error::new(keyword.span(), "duplicate entity source"));
                }
            } else if peeked == "oql_view" {
                let keyword: Ident = content.parse()?;
                let source_document = content.parse()?;
                content.parse::<Token![;]>()?;
                if source
                    .replace(EntitySourceInput::OqlView(source_document))
                    .is_some()
                {
                    return Err(syn::Error::new(keyword.span(), "duplicate entity source"));
                }
            } else if peeked == "generalizes" {
                let keyword: Ident = content.parse()?;
                let target = content.parse()?;
                content.parse::<Token![;]>()?;
                if inheritance.is_some() {
                    return Err(syn::Error::new(
                        keyword.span(),
                        "duplicate entity inheritance",
                    ));
                }
                inheritance = Some(InheritanceInput::Generalizes(target));
            } else if peeked == "system_members" {
                let keyword: Ident = content.parse()?;
                if inheritance.is_some() {
                    return Err(syn::Error::new(
                        keyword.span(),
                        "duplicate entity inheritance",
                    ));
                }
                inheritance = Some(parse_system_members(&content)?);
            } else if peeked == "index" {
                indexes
                    .get_or_insert_with(Vec::new)
                    .push(parse_index(&content)?);
            } else if peeked == "clear_indexes" {
                content.parse::<Ident>()?;
                content.parse::<Token![;]>()?;
                if indexes
                    .as_ref()
                    .is_some_and(|values: &Vec<IndexInput>| !values.is_empty())
                {
                    return Err(content.error("`clear_indexes` conflicts with declared indexes"));
                }
                indexes = Some(vec![]);
            } else if peeked == "clear_lifecycle" {
                content.parse::<Ident>()?;
                content.parse::<Token![;]>()?;
                if lifecycle
                    .as_ref()
                    .is_some_and(|values: &Vec<LifecycleInput>| !values.is_empty())
                {
                    return Err(
                        content.error("`clear_lifecycle` conflicts with declared callbacks")
                    );
                }
                lifecycle = Some(vec![]);
            } else if matches!(
                peeked.to_string().as_str(),
                "before_commit" | "after_commit" | "before_delete" | "after_delete"
            ) {
                lifecycle
                    .get_or_insert_with(Vec::new)
                    .push(parse_lifecycle(&content)?);
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
        if matches!(source.as_ref(), Some(EntitySourceInput::OqlView(_)))
            && persistable.as_ref().is_some_and(|value| value.value)
        {
            return Err(input.error("an OQL view entity cannot be persistable"));
        }
        Ok(EntityInput {
            name,
            documentation,
            persistable,
            image,
            source,
            attributes,
            associations,
            inheritance,
            indexes,
            lifecycle,
            access_rules,
        })
    }
}

fn parse_system_members(input: ParseStream) -> Result<InheritanceInput> {
    let content;
    braced!(content in input);
    let mut owner = None;
    let mut created_date = None;
    let mut changed_date = None;
    let mut changed_by = None;
    while !content.is_empty() {
        let name: Ident = content.parse()?;
        let value: syn::LitBool = content.parse()?;
        content.parse::<Token![;]>()?;
        let slot = match name.to_string().as_str() {
            "owner" => &mut owner,
            "created_date" => &mut created_date,
            "changed_date" => &mut changed_date,
            "changed_by" => &mut changed_by,
            _ => return Err(syn::Error::new(name.span(), "unknown system member")),
        };
        if slot.replace(value).is_some() {
            return Err(syn::Error::new(name.span(), "duplicate system member"));
        }
    }
    Ok(InheritanceInput::Root {
        owner,
        created_date,
        changed_date,
        changed_by,
    })
}

fn parse_access_rule(input: ParseStream) -> Result<AccessRuleInput> {
    expect_keyword(input, "access_rule")?;
    let roles_content;
    bracketed!(roles_content in input);
    let mut roles = Vec::new();
    while !roles_content.is_empty() {
        roles.push(roles_content.parse()?);
        if roles_content.peek(Token![,]) {
            roles_content.parse::<Token![,]>()?;
        }
    }
    if roles.is_empty() {
        return Err(input.error("an access rule requires at least one role"));
    }
    let content;
    braced!(content in input);
    let mut documentation = None;
    let mut allow_create = None;
    let mut allow_delete = None;
    let mut default_rights = None;
    let mut xpath = None;
    let mut xpath_caption = None;
    let mut members = Vec::new();
    while !content.is_empty() {
        let keyword: Ident = content.parse()?;
        match keyword.to_string().as_str() {
            "documentation" => set_once(&mut documentation, content.parse()?, &keyword)?,
            "allow_create" => set_once(&mut allow_create, content.parse()?, &keyword)?,
            "allow_delete" => set_once(&mut allow_delete, content.parse()?, &keyword)?,
            "default_rights" => {
                let rights = parse_member_rights(&content)?;
                set_once(&mut default_rights, rights, &keyword)?;
            }
            "xpath" => set_once(&mut xpath, content.parse()?, &keyword)?,
            "xpath_caption" => set_once(&mut xpath_caption, content.parse()?, &keyword)?,
            "attribute" | "association" => {
                let name = content.parse()?;
                let rights = parse_member_rights(&content)?;
                members.push(if keyword == "attribute" {
                    AccessMemberInput::Attribute { name, rights }
                } else {
                    AccessMemberInput::Association { name, rights }
                });
            }
            _ => {
                return Err(syn::Error::new(
                    keyword.span(),
                    "unknown access-rule option",
                ));
            }
        }
        content.parse::<Token![;]>()?;
    }
    Ok(AccessRuleInput {
        roles,
        documentation,
        allow_create,
        allow_delete,
        default_rights,
        xpath,
        xpath_caption,
        members,
    })
}

fn parse_member_rights(input: ParseStream) -> Result<Ident> {
    let rights: Ident = input.parse()?;
    if !matches!(
        rights.to_string().as_str(),
        "None" | "ReadOnly" | "ReadWrite"
    ) {
        return Err(syn::Error::new(
            rights.span(),
            "unknown member rights (expected None, ReadOnly, or ReadWrite)",
        ));
    }
    Ok(rights)
}

fn set_once<T>(slot: &mut Option<T>, value: T, keyword: &Ident) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(syn::Error::new(
            keyword.span(),
            format!("duplicate `{keyword}`"),
        ));
    }
    Ok(())
}

fn parse_index(input: ParseStream) -> Result<IndexInput> {
    expect_keyword(input, "index")?;
    let content;
    braced!(content in input);
    let mut members = Vec::new();
    let mut include_offline = None;
    while !content.is_empty() {
        let keyword: Ident = content.parse()?;
        match keyword.to_string().as_str() {
            "attribute" | "system" => {
                let name = content.parse()?;
                let ascending = if peek_keyword(&content, "ascending") {
                    content.parse::<Ident>()?;
                    Some(content.parse()?)
                } else {
                    None
                };
                content.parse::<Token![;]>()?;
                if keyword == "attribute" {
                    members.push(IndexMemberInput::Attribute { name, ascending });
                } else {
                    if !matches!(
                        name.to_string().as_str(),
                        "CreatedDate" | "ChangedDate" | "Owner" | "ChangedBy"
                    ) {
                        return Err(syn::Error::new(
                            name.span(),
                            "unknown indexed system member",
                        ));
                    }
                    members.push(IndexMemberInput::System {
                        member: name,
                        ascending,
                    });
                }
            }
            "include_offline" => {
                let value = content.parse()?;
                content.parse::<Token![;]>()?;
                if include_offline.replace(value).is_some() {
                    return Err(syn::Error::new(
                        keyword.span(),
                        "duplicate `include_offline`",
                    ));
                }
            }
            _ => return Err(syn::Error::new(keyword.span(), "unknown index option")),
        }
    }
    if members.is_empty() {
        return Err(input.error("an index requires at least one member"));
    }
    Ok(IndexInput {
        members,
        include_offline,
    })
}

fn parse_lifecycle(input: ParseStream) -> Result<LifecycleInput> {
    let event: Ident = input.parse()?;
    let handler = input.parse()?;
    let mut pass_event_object = None;
    let mut raise_error_on_false = None;
    if input.peek(Token![;]) {
        input.parse::<Token![;]>()?;
    } else {
        let content;
        braced!(content in input);
        while !content.is_empty() {
            let option: Ident = content.parse()?;
            let value: syn::LitBool = content.parse()?;
            content.parse::<Token![;]>()?;
            let slot = match option.to_string().as_str() {
                "pass_event_object" => &mut pass_event_object,
                "raise_error_on_false" => &mut raise_error_on_false,
                _ => return Err(syn::Error::new(option.span(), "unknown lifecycle option")),
            };
            if slot.replace(value).is_some() {
                return Err(syn::Error::new(option.span(), "duplicate lifecycle option"));
            }
        }
        if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
        }
    }
    Ok(LifecycleInput {
        event,
        handler,
        pass_event_object,
        raise_error_on_false,
    })
}

impl Parse for MicroflowInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let keyword: Ident = input.parse()?;
        let is_nanoflow = match keyword.to_string().as_str() {
            "microflow" => false,
            "nanoflow" => true,
            _ => {
                return Err(syn::Error::new(
                    keyword.span(),
                    "expected `microflow` or `nanoflow`",
                ));
            }
        };
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
            is_nanoflow,
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
        let mut documentation = None;
        let mut length = None;
        let mut localize_date = None;
        let mut required = None;
        let mut unique = None;
        if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            while !content.is_empty() {
                let option: Ident = content.parse()?;
                match option.to_string().as_str() {
                    "documentation" => documentation = Some(content.parse()?),
                    "length" => length = Some(content.parse()?),
                    "localize_date" => localize_date = Some(content.parse()?),
                    "required" => required = Some(content.parse()?),
                    "unique" => unique = Some(content.parse()?),
                    other => {
                        return Err(syn::Error::new(
                            option.span(),
                            format!("unknown attribute option `{other}`"),
                        ));
                    }
                }
                content.parse::<Token![;]>()?;
            }
            if input.peek(Token![;]) {
                input.parse::<Token![;]>()?;
            }
        } else {
            input.parse::<Token![;]>()?;
        }
        Ok(AttributeInput {
            kind,
            name,
            enumeration,
            default,
            documentation,
            length,
            localize_date,
            required,
            unique,
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
        let mut owner = None;
        let mut storage = None;
        let mut documentation = None;
        if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            while !content.is_empty() {
                let option: Ident = content.parse()?;
                match option.to_string().as_str() {
                    "owner" => {
                        let value: Ident = content.parse()?;
                        if value != "Default" && value != "Both" {
                            return Err(syn::Error::new(
                                value.span(),
                                "association owner must be `Default` or `Both`",
                            ));
                        }
                        owner = Some(value);
                    }
                    "storage" => {
                        let value: Ident = content.parse()?;
                        if value != "Column" && value != "Table" {
                            return Err(syn::Error::new(
                                value.span(),
                                "association storage must be `Column` or `Table`",
                            ));
                        }
                        storage = Some(value);
                    }
                    "documentation" => documentation = Some(content.parse()?),
                    other => {
                        return Err(syn::Error::new(
                            option.span(),
                            format!("unknown association option `{other}`"),
                        ));
                    }
                }
                content.parse::<Token![;]>()?;
            }
            if input.peek(Token![;]) {
                input.parse::<Token![;]>()?;
            }
        } else {
            input.parse::<Token![;]>()?;
        }
        Ok(AssociationInput {
            name,
            target,
            association_type,
            owner,
            storage,
            documentation,
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
