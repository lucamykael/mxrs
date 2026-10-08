//! Nanoflows written for the frontend.
//!
//! A nanoflow is a method of a TypeScript service in
//! `frontend/src/services/<module>/<subject>Service.ts`: TypeScript's own
//! control flow, and an `await` per activity (`frontend/src/mxrs/flows.ts`
//! is the vocabulary). The importer writes it by restating the Rust body it
//! would otherwise write, statement by statement, and keeps it only when the
//! frontend's reader gives back the declaration that body declares.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use mxrs_ir::flow::MicroflowDecl;
use syn::punctuated::Punctuated;
use syn::{Expr, Stmt, Token};

use mxrs_frontend::naming::{camel, camel_from_snake, pascal};

use crate::names;

/// A nanoflow a service method declares, as a call names it.
#[derive(Debug, Clone)]
pub(crate) struct Callee {
    /// `MeasurementUnitService`.
    pub(crate) service: String,
    /// `editValidation`.
    pub(crate) method: String,
    /// Its parameters' Mendix names, in order.
    pub(crate) parameters: Vec<String>,
}

/// One nanoflow as a service method.
#[derive(Debug)]
pub(crate) struct Method {
    /// The method, its documentation comment included, indented for the
    /// service's object literal.
    pub(crate) text: String,
    /// What it uses of `@/mxrs/flows`.
    pub(crate) vocabulary: BTreeSet<String>,
    /// The other services it calls.
    pub(crate) services: BTreeSet<String>,
}

type Outcome<T> = Result<T, String>;

/// What a nanoflow's comment says beside its name: who may run it — none
/// said when `None` — and the folder of its module it lives in.
#[derive(Clone, Copy, Default)]
pub(crate) struct Said<'a> {
    pub(crate) roles: Option<&'a [String]>,
    pub(crate) folder: Option<&'a str>,
}

/// Restates `body`, the Rust body of the nanoflow `declaration`, as the
/// service method `method`. `said` is what its comment says besides — the
/// module roles allowed to run it, qualified, and the folder it lives in;
/// `callees` the nanoflows a service method declares.
pub(crate) fn translate(
    module: &str,
    service: &str,
    method: &str,
    declaration: &MicroflowDecl,
    said: Said<'_>,
    body: &[String],
    callees: &HashMap<String, Callee>,
) -> Outcome<Method> {
    if declaration.documentation.contains("*/") {
        return Err("its documentation closes a comment".to_string());
    }
    let source = names::references_as_identifiers(&body.join("\n"));
    let block: syn::Block = syn::parse_str(&format!("{{\n{source}\n}}"))
        .map_err(|error| format!("its Rust body does not parse: {error}"))?;
    let mut translator = Translator {
        service,
        callees,
        variables: HashMap::new(),
        taken: HashSet::new(),
        vocabulary: BTreeSet::from(["nanoflowService".to_string()]),
        services: BTreeSet::new(),
        loops: Vec::new(),
        switches: 0,
        pending: None,
        returns: false,
    };
    let mut parameters = Vec::new();
    let mut returns = None;
    let mut statements = block.stmts.iter().peekable();
    // The signature first: parameters, then what it returns.
    while let Some(statement) = statements.peek() {
        if let Some(parameter) = translator.parameter(statement)? {
            parameters.push(parameter);
            statements.next();
            continue;
        }
        if let Some(ty) = returns_type(statement) {
            returns = Some(translator.data_type(ty)?.0);
            translator.returns = true;
            statements.next();
            continue;
        }
        break;
    }
    let mut lines = Vec::new();
    for statement in statements {
        translator.statement(statement, &mut lines)?;
    }
    if translator.pending.is_some() {
        return Err("an error handler with no activity after it".to_string());
    }

    let mut text = String::from("  /**\n");
    for line in declaration.documentation.lines() {
        if line.is_empty() {
            text.push_str("   *\n");
        } else {
            text.push_str(&format!("   * {line}\n"));
        }
    }
    if !declaration.documentation.is_empty() {
        text.push_str("   *\n");
    }
    text.push_str(&format!("   * @nanoflow {}\n", declaration.name));
    if let Some(roles) = said.roles {
        let roles: Vec<String> = roles
            .iter()
            .map(|role| match role.split_once('.') {
                Some((role_module, name)) if role_module == module => name.to_string(),
                _ => role.clone(),
            })
            .collect();
        if roles.is_empty() {
            text.push_str("   * @roles\n");
        } else {
            text.push_str(&format!("   * @roles {}\n", roles.join(" ")));
        }
    }
    if let Some(folder) = said.folder {
        text.push_str(&format!("   * @folder {folder}\n"));
    }
    for parameter in &parameters {
        if !parameter.documentation.is_empty() {
            if parameter.documentation.contains('\n') {
                return Err("a parameter's documentation spans lines".to_string());
            }
            text.push_str(&format!(
                "   * @param {} {}\n",
                parameter.ident, parameter.documentation
            ));
        }
        if pascal(&parameter.ident) != parameter.mendix {
            text.push_str(&format!(
                "   * @mendixName {} {}\n",
                parameter.ident, parameter.mendix
            ));
        }
        if let Some(default) = &parameter.default {
            text.push_str(&format!(
                "   * @defaultValue {} {default}\n",
                parameter.ident
            ));
        }
    }
    text.push_str("   */\n");
    // An optional parameter is marked `?` while every one after it is too;
    // before a required one it says `| undefined` instead.
    let signature: Vec<String> = parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            if parameter.required {
                format!("{}: {}", parameter.ident, parameter.ty)
            } else if parameters[index..].iter().all(|later| !later.required) {
                format!("{}?: {}", parameter.ident, parameter.ty)
            } else {
                format!("{}: {} | undefined", parameter.ident, parameter.ty)
            }
        })
        .collect();
    let returns = returns.as_deref().unwrap_or("void");
    let opening = if lines.is_empty() { "{}," } else { "{" };
    let one_line = format!(
        "  async {method}({}): Promise<{returns}> {opening}\n",
        signature.join(", ")
    );
    if one_line.len() <= WIDTH + 1 {
        text.push_str(&one_line);
    } else {
        text.push_str(&format!("  async {method}(\n"));
        for parameter in &signature {
            text.push_str(&format!("    {parameter},\n"));
        }
        text.push_str(&format!("  ): Promise<{returns}> {opening}\n"));
    }
    for line in lines {
        if line.is_empty() {
            text.push('\n');
            continue;
        }
        let nested = line.len() - line.trim_start().len();
        for wrapped in wrap(line.trim_start(), 4 + nested) {
            text.push_str(&wrapped);
            text.push('\n');
        }
    }
    if opening == "{" {
        text.push_str("  },\n");
    }
    Ok(Method {
        text,
        vocabulary: translator.vocabulary,
        services: translator.services,
    })
}

struct Parameter {
    ident: String,
    mendix: String,
    ty: String,
    required: bool,
    documentation: String,
    default: Option<String>,
}

/// A variable of the flow as the TypeScript names it.
#[derive(Clone)]
struct Variable {
    ident: String,
    /// The entity of the object or list it holds, when known.
    entity: Option<String>,
}

/// An error handling the next activity takes.
enum Pending {
    OnError {
        kind: &'static str,
        handler: Vec<String>,
    },
    Continue,
    Disabled,
}

struct Translator<'a> {
    service: &'a str,
    callees: &'a HashMap<String, Callee>,
    /// Each Rust binding's variable.
    variables: HashMap<String, Variable>,
    /// The identifiers in use.
    taken: HashSet<String>,
    vocabulary: BTreeSet<String>,
    services: BTreeSet<String>,
    /// For each loop being written: whether a `break` inside a `switch`
    /// needs it labelled.
    loops: Vec<bool>,
    /// How many `switch`es the current statement is inside, counted from
    /// the innermost loop.
    switches: usize,
    pending: Option<Pending>,
    /// Whether the nanoflow returns a value.
    returns: bool,
}

/// A macro's arguments: the flow first, then the rest.
fn macro_arguments(mac: &syn::Macro) -> Outcome<Vec<Expr>> {
    let arguments = mac
        .parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
        .map_err(|error| format!("a macro's arguments do not parse: {error}"))?;
    Ok(arguments.into_iter().skip(1).collect())
}

fn macro_name(mac: &syn::Macro) -> String {
    mac.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_default()
}

/// The reference an identifier stands for.
fn reference(expr: &Expr) -> Option<(char, String)> {
    match expr {
        Expr::Path(path) if path.qself.is_none() => path_reference(&path.path),
        Expr::Reference(reference) => self::reference(&reference.expr),
        Expr::Paren(paren) => self::reference(&paren.expr),
        _ => None,
    }
}

fn path_reference(path: &syn::Path) -> Option<(char, String)> {
    let [segment] = path.segments.iter().collect::<Vec<_>>()[..] else {
        return None;
    };
    names::identifier_reference(&segment.ident.to_string())
}

/// The reference in `Ref::<X>::new()`, `MicroflowRef::<X>::new()` and
/// `NanoflowRef::<X>::new()`, or a string literal naming it.
fn marker_target(expr: &Expr) -> Option<String> {
    if let Some(text) = string_literal(expr) {
        return Some(text);
    }
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let segments: Vec<_> = path.path.segments.iter().collect();
    let [owner, new] = segments[..] else {
        return None;
    };
    if new.ident != "new" || !call.args.is_empty() {
        return None;
    }
    let syn::PathArguments::AngleBracketed(generics) = &owner.arguments else {
        return None;
    };
    let [syn::GenericArgument::Type(syn::Type::Path(ty))] =
        generics.args.iter().collect::<Vec<_>>()[..]
    else {
        return None;
    };
    path_reference(&ty.path).map(|(_, target)| target)
}

fn string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(text),
            ..
        }) => Some(text.value()),
        Expr::Paren(paren) => string_literal(&paren.expr),
        _ => None,
    }
}

/// `MethodCall` on the flow builder: its method and arguments.
fn flow_call(expr: &Expr) -> Option<(&syn::ExprMethodCall, String)> {
    let Expr::MethodCall(call) = expr else {
        return None;
    };
    Some((call, call.method.to_string()))
}

fn is_flow(expr: &Expr) -> bool {
    matches!(expr, Expr::Path(path) if path.path.is_ident("flow") || path.path.is_ident("_flow"))
}

/// `flow.returns(ty);`.
fn returns_type(statement: &Stmt) -> Option<&Expr> {
    let Stmt::Expr(Expr::MethodCall(call), _) = statement else {
        return None;
    };
    (call.method == "returns" && is_flow(&call.receiver) && call.args.len() == 1)
        .then(|| &call.args[0])
}

/// A closure's parameters and statements.
fn closure(expr: &Expr) -> Outcome<(Vec<String>, &[Stmt])> {
    let Expr::Closure(closure) = expr else {
        return Err("expected a closure".to_string());
    };
    let parameters = closure
        .inputs
        .iter()
        .map(|input| match input {
            syn::Pat::Ident(ident) => Ok(ident.ident.to_string()),
            syn::Pat::Wild(_) => Ok("_".to_string()),
            _ => Err("an unexpected closure parameter".to_string()),
        })
        .collect::<Outcome<Vec<_>>>()?;
    match &*closure.body {
        Expr::Block(block) => Ok((parameters, &block.block.stmts)),
        _ => Err("a closure without a block".to_string()),
    }
}

/// The calls a builder closure makes on its receiver: `(method, args)`.
fn options_calls(expr: &Expr) -> Outcome<Vec<(String, Vec<Expr>)>> {
    let (_, statements) = closure(expr)?;
    statements
        .iter()
        .map(|statement| {
            let Stmt::Expr(Expr::MethodCall(call), _) = statement else {
                return Err("an unexpected statement among options".to_string());
            };
            Ok((call.method.to_string(), call.args.iter().cloned().collect()))
        })
        .collect()
}

/// The short name of `Module.Entity`.
fn short(qualified: &str) -> &str {
    qualified
        .rsplit_once('.')
        .map_or(qualified, |(_, name)| name)
}

fn json(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}

/// Text written over several lines as it reads: a template literal, its
/// lines its own. One-line text, or text with a carriage return a template
/// would not keep, is a string literal.
fn text_literal(text: &str) -> String {
    if !text.contains('\n') || text.contains('\r') {
        return json(text);
    }
    let escaped = text
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${");
    format!("`{escaped}`")
}

/// A key of an object literal: bare when it is an identifier.
fn key(name: &str) -> String {
    let identifier = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if identifier {
        name.to_string()
    } else {
        json(name)
    }
}

/// `{ a: 1, b: 2 }`.
fn object(entries: &[(String, String)]) -> String {
    if entries.is_empty() {
        return "{}".to_string();
    }
    format!(
        "{{ {} }}",
        entries
            .iter()
            .map(|(name, value)| format!("{}: {value}", key(name)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Words a TypeScript identifier cannot be, or should not hide.
const RESERVED: &[&str] = &[
    "arguments",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "NaN",
    "Infinity",
    "loop",
    // The vocabulary.
    "aggregateList",
    "callJavaScriptAction",
    "callMicroflow",
    "callNanoflow",
    "changeList",
    "changeObject",
    "changeVariable",
    "closePage",
    "commitObject",
    "createList",
    "createObject",
    "createVariable",
    "deleteObject",
    "disabled",
    "iterate",
    "jump",
    "label",
    "log",
    "mx",
    "nanoflowService",
    "onError",
    "raiseError",
    "retrieve",
    "rollbackObject",
    "showMessage",
    "showPage",
    "variable",
];

impl Translator<'_> {
    fn uses(&mut self, word: &str) {
        self.vocabulary.insert(word.to_string());
    }

    /// Declares the variable `mendix` for the Rust `binding` and returns
    /// its identifier.
    fn declare(&mut self, binding: Option<&str>, mendix: &str, entity: Option<String>) -> String {
        let mut ident = camel(mendix);
        if ident.is_empty()
            || !ident
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || RESERVED.contains(&ident.as_str())
            || ident.ends_with("Service")
        {
            ident = format!(
                "{}Value",
                camel(&mendix.replace(|c: char| !c.is_ascii_alphanumeric(), "_"))
            );
        }
        if !self.taken.insert(ident.clone()) {
            ident = (2..)
                .map(|suffix| format!("{ident}{suffix}"))
                .find(|candidate| self.taken.insert(candidate.clone()))
                .expect("an unbounded suffix always finds a free name");
        }
        if let Some(binding) = binding {
            self.variables.insert(
                binding.to_string(),
                Variable {
                    ident: ident.clone(),
                    entity,
                },
            );
        }
        ident
    }

    fn variable(&self, expr: &Expr) -> Outcome<&Variable> {
        match expr {
            Expr::Reference(reference) => self.variable(&reference.expr),
            Expr::Paren(paren) => self.variable(&paren.expr),
            Expr::Path(path) => {
                let name = path
                    .path
                    .get_ident()
                    .map(ToString::to_string)
                    .ok_or("expected a variable")?;
                self.variables
                    .get(&name)
                    .ok_or_else(|| format!("variable {name} is not in scope"))
            }
            _ => Err("expected a variable".to_string()),
        }
    }

    /// A value as an activity macro reads it.
    fn macro_value(&mut self, expr: &Expr) -> Outcome<String> {
        match expr {
            Expr::Lit(literal) => match &literal.lit {
                syn::Lit::Str(text) => Ok(json(&text.value())),
                syn::Lit::Int(number) => Ok(number.base10_digits().to_string()),
                syn::Lit::Float(number) => Ok(number.base10_digits().to_string()),
                syn::Lit::Bool(flag) => Ok(flag.value.to_string()),
                _ => Err("an unexpected literal".to_string()),
            },
            Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }) => match &**expr {
                Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(number),
                    ..
                }) => Ok(format!("-{}", number.base10_digits())),
                Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Float(number),
                    ..
                }) => Ok(format!("-{}", number.base10_digits())),
                _ => Err("an unexpected negation".to_string()),
            },
            _ => self.builder_value(expr),
        }
    }

    /// A value as a builder's `impl Into<Mx>` reads it.
    fn builder_value(&mut self, expr: &Expr) -> Outcome<String> {
        match expr {
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text),
                ..
            }) => {
                self.uses("mx");
                Ok(format!("mx({})", text_literal(&text.value())))
            }
            Expr::Paren(paren) => self.builder_value(&paren.expr),
            Expr::Reference(reference) => self.builder_value(&reference.expr),
            Expr::Path(_) => {
                if let Some((kind, target)) = reference(expr) {
                    if kind == 'V' {
                        self.uses("mx");
                        return Ok(format!("mx({})", json(&target)));
                    }
                    return Err(format!("a reference of kind {kind} as a value"));
                }
                Ok(self.variable(expr)?.ident.clone())
            }
            Expr::Call(call) => {
                let Expr::Path(function) = &*call.func else {
                    return Err("an unexpected call as a value".to_string());
                };
                let function = function
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default();
                let argument = (call.args.len() == 1)
                    .then(|| string_literal(&call.args[0]))
                    .flatten();
                match (function.as_str(), argument) {
                    ("mx", Some(text)) => {
                        self.uses("mx");
                        Ok(format!("mx({})", text_literal(&text)))
                    }
                    ("string", Some(text)) => Ok(json(&text)),
                    ("var", Some(name)) => {
                        self.uses("variable");
                        Ok(format!("variable({})", json(&name)))
                    }
                    _ => Err(format!("`{function}(...)` as a value")),
                }
            }
            _ => Err("an unexpected value".to_string()),
        }
    }

    /// `(TypeScript type, entity, list)` of a `DataType`.
    fn data_type(&mut self, expr: &Expr) -> Outcome<(String, Option<String>)> {
        let object = |this: &mut Self, kind: &str, entity: String| {
            this.uses(kind);
            (format!("{kind}<{}>", json(&entity)), Some(entity))
        };
        match expr {
            Expr::Path(path) => {
                let variant = path
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default();
                let ty = match variant.as_str() {
                    "String" => "string",
                    "Boolean" => "boolean",
                    "Integer" => "MxInteger",
                    "Long" => "MxLong",
                    "Decimal" => "MxDecimal",
                    "Float" => "MxFloat",
                    "DateTime" => "MxDateTime",
                    "Binary" => "MxBinary",
                    other => return Err(format!("data type {other}")),
                };
                if ty.starts_with("Mx") {
                    self.uses(ty);
                }
                Ok((ty.to_string(), None))
            }
            Expr::Call(call) => {
                let Expr::Path(function) = &*call.func else {
                    return Err("an unexpected data type".to_string());
                };
                let segments: Vec<_> = function.path.segments.iter().collect();
                let last = segments.last().ok_or("an empty data type")?;
                let name = last.ident.to_string();
                // `DataType::object::<Order>()`.
                if let syn::PathArguments::AngleBracketed(generics) = &last.arguments
                    && let [syn::GenericArgument::Type(syn::Type::Path(ty))] =
                        generics.args.iter().collect::<Vec<_>>()[..]
                    && let Some((_, entity)) = path_reference(&ty.path)
                {
                    return match name.as_str() {
                        "object" => Ok(object(self, "MxObject", entity)),
                        "list" => Ok(object(self, "MxList", entity)),
                        other => Err(format!("data type {other}")),
                    };
                }
                // `DataType::Object("Module.Entity".to_string())`.
                let argument = call.args.first().and_then(|argument| match argument {
                    Expr::MethodCall(method) => string_literal(&method.receiver),
                    Expr::Call(inner) => inner.args.first().and_then(string_literal),
                    other => string_literal(other),
                });
                let argument = argument.ok_or("a data type without its name")?;
                match name.as_str() {
                    "Object" => Ok(object(self, "MxObject", argument)),
                    "List" => Ok(object(self, "MxList", argument)),
                    "Enumeration" => {
                        self.uses("MxEnum");
                        Ok((format!("MxEnum<{}>", json(&argument)), None))
                    }
                    other => Err(format!("data type {other}")),
                }
            }
            _ => Err("an unexpected data type".to_string()),
        }
    }

    /// `createVariable`'s type argument.
    fn variable_type(&mut self, expr: &Expr) -> Outcome<String> {
        let (ty, entity) = self.data_type(expr)?;
        Ok(match (ty.as_str(), entity) {
            (object, Some(entity)) if object.starts_with("MxObject") => {
                format!("{{ object: {} }}", json(&entity))
            }
            (list, Some(entity)) if list.starts_with("MxList") => {
                format!("{{ list: {} }}", json(&entity))
            }
            (enumeration, None) if enumeration.starts_with("MxEnum<") => format!(
                "{{ enumeration: {} }}",
                &enumeration["MxEnum<".len()..enumeration.len() - 1]
            ),
            ("string", _) => json("String"),
            ("boolean", _) => json("Boolean"),
            (other, _) => json(other.trim_start_matches("Mx")),
        })
    }

    /// `let x = flow.parameter_of("Name", ty, |parameter| { ... });`.
    fn parameter(&mut self, statement: &Stmt) -> Outcome<Option<Parameter>> {
        let (binding, expr) = match statement {
            Stmt::Local(local) => {
                let syn::Pat::Ident(ident) = &local.pat else {
                    return Ok(None);
                };
                let Some(init) = &local.init else {
                    return Ok(None);
                };
                (Some(ident.ident.to_string()), &*init.expr)
            }
            Stmt::Expr(expr, Some(_)) => (None, expr),
            _ => return Ok(None),
        };
        let Some((call, method)) = flow_call(expr) else {
            return Ok(None);
        };
        if method != "parameter_of" || !is_flow(&call.receiver) {
            return Ok(None);
        }
        let args: Vec<&Expr> = call.args.iter().collect();
        let [name, ty, options] = args[..] else {
            return Err("a parameter of an unexpected shape".to_string());
        };
        let mendix = string_literal(name).ok_or("a parameter without a name")?;
        let (ty, entity) = self.data_type(ty)?;
        let mut parameter = Parameter {
            ident: String::new(),
            mendix: mendix.clone(),
            ty,
            required: false,
            documentation: String::new(),
            default: None,
        };
        for (option, values) in options_calls(options)? {
            match (option.as_str(), &values[..]) {
                (
                    "required",
                    [
                        Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Bool(flag),
                            ..
                        }),
                    ],
                ) => parameter.required = flag.value,
                ("documentation", [text]) => {
                    parameter.documentation =
                        string_literal(text).ok_or("a parameter documentation")?;
                }
                ("default_value", [value]) => {
                    let Some(text) = (match value {
                        Expr::Call(call) if call.args.len() == 1 => string_literal(&call.args[0]),
                        other => string_literal(other),
                    }) else {
                        return Err("a parameter default".to_string());
                    };
                    if text.contains('\n') {
                        return Err("a parameter default spans lines".to_string());
                    }
                    parameter.default = Some(text);
                }
                (other, _) => return Err(format!("parameter option {other}")),
            }
        }
        parameter.ident = self.declare(binding.as_deref(), &mendix, entity);
        Ok(Some(parameter))
    }

    fn statements(&mut self, statements: &[Stmt]) -> Outcome<Vec<String>> {
        let mut lines = Vec::new();
        for statement in statements {
            self.statement(statement, &mut lines)?;
        }
        if self.pending.is_some() {
            return Err("an error handler with no activity after it".to_string());
        }
        Ok(lines)
    }

    fn statement(&mut self, statement: &Stmt, lines: &mut Vec<String>) -> Outcome<()> {
        match statement {
            Stmt::Local(local) => {
                let syn::Pat::Ident(ident) = &local.pat else {
                    return Err("an unexpected binding".to_string());
                };
                let init = local.init.as_ref().ok_or("a binding without a value")?;
                let binding = ident.ident.to_string();
                self.expression(&init.expr, Some(&binding), lines)
            }
            Stmt::Expr(expr, _) => self.expression(expr, None, lines),
            Stmt::Macro(mac) => {
                let call = self.macro_call(&mac.mac, None)?;
                self.emit(call, lines)
            }
            Stmt::Item(_) => Err("an item inside a flow".to_string()),
        }
    }

    /// Writes an activity call, with the error handling stated before it.
    fn emit(&mut self, call: Call, lines: &mut Vec<String>) -> Outcome<()> {
        let Call { binding, text } = call;
        let head = match &binding {
            Some(ident) => format!("const {ident} = await "),
            None => "await ".to_string(),
        };
        match self.pending.take() {
            None => lines.push(format!("{head}{text};")),
            Some(Pending::Continue) => {
                self.uses("onError");
                lines.push(format!("{head}onError(\"continue\", () => {text});"));
            }
            Some(Pending::Disabled) => {
                self.uses("disabled");
                lines.push(format!("{head}disabled(() => {text});"));
            }
            Some(Pending::OnError { kind, handler }) => {
                self.uses("onError");
                lines.push(format!("{head}onError("));
                lines.push(format!("  \"{kind}\","));
                lines.push(format!("  () => {text},"));
                lines.push("  async () => {".to_string());
                lines.extend(handler.into_iter().map(|line| {
                    if line.is_empty() {
                        line
                    } else {
                        format!("    {line}")
                    }
                }));
                lines.push("  },".to_string());
                lines.push(");".to_string());
            }
        }
        Ok(())
    }

    fn expression(
        &mut self,
        expr: &Expr,
        binding: Option<&str>,
        lines: &mut Vec<String>,
    ) -> Outcome<()> {
        if let Expr::Macro(mac) = expr {
            let call = self.macro_call(&mac.mac, binding)?;
            return self.emit(call, lines);
        }
        let Some((call, method)) = flow_call(expr) else {
            return Err("an unexpected statement".to_string());
        };
        // `flow.on_error(|flow| { ... }).change(...)`: the handling, then
        // the activity it is for.
        if !is_flow(&call.receiver) {
            let Some((modifier, name)) = flow_call(&call.receiver) else {
                return Err("a call on something other than the flow".to_string());
            };
            if !is_flow(&modifier.receiver) {
                return Err("a call on something other than the flow".to_string());
            }
            self.modifier(&name, modifier)?;
            let mut plain = call.clone();
            plain.receiver = Box::new(syn::parse_quote!(flow));
            return self.expression(&Expr::MethodCall(plain), binding, lines);
        }
        let args: Vec<&Expr> = call.args.iter().collect();
        match method.as_str() {
            "on_error" | "on_error_without_rollback" | "continue_on_error" | "disabled" => {
                if binding.is_some() {
                    return Err("an error handler bound to a variable".to_string());
                }
                self.modifier(&method, call)
            }
            "documentation" => Ok(()),
            "end" => {
                lines.push("return;".to_string());
                Ok(())
            }
            "return_with" => {
                let [value] = args[..] else {
                    return Err("a return of an unexpected shape".to_string());
                };
                let value = self.builder_value(value)?;
                // A model can store a value where its nanoflow returns
                // none: TypeScript returns it as nothing, on purpose.
                let value = match (self.returns, value.strip_prefix("mx(")) {
                    (true, _) => value,
                    (false, Some(rest)) => format!("mx<void>({rest}"),
                    (false, None) => {
                        return Err("a value returned where the nanoflow returns none".to_string());
                    }
                };
                lines.push(format!("return {value};"));
                Ok(())
            }
            "label" | "jump" => {
                let [name] = args[..] else {
                    return Err("a label of an unexpected shape".to_string());
                };
                let name = string_literal(name).ok_or("a label without a name")?;
                self.uses(&method);
                lines.push(format!("{method}({});", json(&name)));
                Ok(())
            }
            "break_loop" => {
                if self.loops.is_empty() {
                    return Err("a break outside a loop".to_string());
                }
                if self.switches > 0 {
                    let depth = self.loops.len();
                    *self.loops.last_mut().expect("checked") = true;
                    lines.push(format!("break {};", loop_label(depth)));
                } else {
                    lines.push("break;".to_string());
                }
                Ok(())
            }
            "continue_loop" => {
                lines.push("continue;".to_string());
                Ok(())
            }
            "raise_error" => {
                self.uses("raiseError");
                lines.push("raiseError();".to_string());
                Ok(())
            }
            "decision" => self.decision(&args, lines),
            "switch" => self.switch(&args, lines),
            "for_each" => self.for_each(&args, lines),
            "while_loop" => self.while_loop(&args, lines),
            _ => {
                let call = self.builder_call(&method, &args, binding)?;
                self.emit(call, lines)
            }
        }
    }

    fn modifier(&mut self, name: &str, call: &syn::ExprMethodCall) -> Outcome<()> {
        if self.pending.is_some() {
            return Err("two error handlers for one activity".to_string());
        }
        self.pending = Some(match name {
            "on_error" | "on_error_without_rollback" => {
                let [handler] = call.args.iter().collect::<Vec<_>>()[..] else {
                    return Err("an error handler of an unexpected shape".to_string());
                };
                let (_, statements) = closure(handler)?;
                let handler = self.nested(statements)?;
                Pending::OnError {
                    kind: if name == "on_error" {
                        "custom"
                    } else {
                        "customWithoutRollback"
                    },
                    handler,
                }
            }
            "continue_on_error" => Pending::Continue,
            "disabled" => Pending::Disabled,
            other => return Err(format!("modifier {other}")),
        });
        Ok(())
    }

    /// A nested block: its own scope for the variables it declares.
    fn nested(&mut self, statements: &[Stmt]) -> Outcome<Vec<String>> {
        let variables = self.variables.clone();
        let pending = self.pending.take();
        let result = self.statements(statements);
        self.variables = variables;
        self.pending = pending;
        result
    }

    fn block(lines: &mut Vec<String>, body: Vec<String>) {
        lines.extend(body.into_iter().map(|line| {
            if line.is_empty() {
                line
            } else {
                format!("  {line}")
            }
        }));
    }

    fn decision(&mut self, args: &[&Expr], lines: &mut Vec<String>) -> Outcome<()> {
        let [condition, then, otherwise] = args[..] else {
            return Err("a decision of an unexpected shape".to_string());
        };
        let condition = self.builder_value(condition)?;
        let (_, then) = closure(then)?;
        let (_, otherwise) = closure(otherwise)?;
        let then = self.nested(then)?;
        let otherwise = self.nested(otherwise)?;
        lines.push(format!("if ({condition}) {{"));
        Self::block(lines, then);
        if otherwise.is_empty() {
            lines.push("}".to_string());
        } else {
            lines.push("} else {".to_string());
            Self::block(lines, otherwise);
            lines.push("}".to_string());
        }
        Ok(())
    }

    fn switch(&mut self, args: &[&Expr], lines: &mut Vec<String>) -> Outcome<()> {
        let [on, cases] = args[..] else {
            return Err("a switch of an unexpected shape".to_string());
        };
        let on = self.builder_value(on)?;
        // What the cases compare to is text, or nothing.
        let on = match on.strip_prefix("mx(") {
            Some(rest) => format!("mx<string | null>({rest}"),
            None => on,
        };
        lines.push(format!("switch ({on}) {{"));
        let (_, statements) = closure(cases)?;
        self.switches += 1;
        for statement in statements {
            let Stmt::Expr(Expr::MethodCall(case), _) = statement else {
                self.switches -= 1;
                return Err("an unexpected statement among cases".to_string());
            };
            let args: Vec<&Expr> = case.args.iter().collect();
            let (labels, body) = match (case.method.to_string().as_str(), &args[..]) {
                ("case", [value, body]) => {
                    let value = string_literal(value).ok_or("a case without a value")?;
                    (vec![json(&value)], *body)
                }
                ("cases", [Expr::Array(values), body]) => (
                    values
                        .elems
                        .iter()
                        .map(|value| string_literal(value).map(|value| json(&value)))
                        .collect::<Option<Vec<_>>>()
                        .ok_or("cases without values")?,
                    *body,
                ),
                ("empty", [body]) => (vec!["null".to_string()], *body),
                (other, _) => {
                    self.switches -= 1;
                    return Err(format!("switch case {other}"));
                }
            };
            let (_, statements) = closure(body)?;
            let mut body = self.nested(statements)?;
            if !body.last().is_some_and(|line| terminates(line)) {
                body.push("break;".to_string());
            }
            for label in &labels[..labels.len() - 1] {
                lines.push(format!("  case {label}:"));
            }
            lines.push(format!("  case {}: {{", labels[labels.len() - 1]));
            lines.extend(body.into_iter().map(|line| {
                if line.is_empty() {
                    line
                } else {
                    format!("    {line}")
                }
            }));
            lines.push("  }".to_string());
        }
        self.switches -= 1;
        lines.push("}".to_string());
        Ok(())
    }

    fn for_each(&mut self, args: &[&Expr], lines: &mut Vec<String>) -> Outcome<()> {
        let [list, iterator, body] = args[..] else {
            return Err("a loop of an unexpected shape".to_string());
        };
        let list_variable = self.variable(list)?.clone();
        let iterator = string_literal(iterator).ok_or("a loop without an iterator name")?;
        let (parameters, statements) = closure(body)?;
        let [_, item] = &parameters[..] else {
            return Err("a loop body of an unexpected shape".to_string());
        };
        let variables = self.variables.clone();
        let ident = self.declare(Some(item), &iterator, list_variable.entity.clone());
        let source = if pascal(&ident) == iterator {
            list_variable.ident.clone()
        } else {
            self.uses("iterate");
            format!("iterate({}, {})", list_variable.ident, json(&iterator))
        };
        let switches = std::mem::replace(&mut self.switches, 0);
        self.loops.push(false);
        let body = self.nested(statements);
        let labelled = self.loops.pop().unwrap_or(false);
        let depth = self.loops.len() + 1;
        self.switches = switches;
        self.variables = variables;
        let body = body?;
        let label = if labelled {
            format!("{}: ", loop_label(depth))
        } else {
            String::new()
        };
        lines.push(format!("{label}for (const {ident} of {source}) {{"));
        Self::block(lines, body);
        lines.push("}".to_string());
        Ok(())
    }

    fn while_loop(&mut self, args: &[&Expr], lines: &mut Vec<String>) -> Outcome<()> {
        let [condition, body] = args[..] else {
            return Err("a while loop of an unexpected shape".to_string());
        };
        let condition = self.builder_value(condition)?;
        let (_, statements) = closure(body)?;
        let switches = std::mem::replace(&mut self.switches, 0);
        self.loops.push(false);
        let body = self.nested(statements);
        let labelled = self.loops.pop().unwrap_or(false);
        let depth = self.loops.len() + 1;
        self.switches = switches;
        let body = body?;
        let label = if labelled {
            format!("{}: ", loop_label(depth))
        } else {
            String::new()
        };
        lines.push(format!("{label}while ({condition}) {{"));
        Self::block(lines, body);
        lines.push("}".to_string());
        Ok(())
    }
}

/// The widest a line is written.
const WIDTH: usize = 100;

/// `line`, indented `indent` spaces, broken the way prettier would where it
/// is wider than [`WIDTH`]: the last call's arguments, or an object's or
/// array's entries, one per line.
fn wrap(line: &str, indent: usize) -> Vec<String> {
    let pad = " ".repeat(indent);
    if indent + line.len() <= WIDTH {
        return vec![format!("{pad}{line}")];
    }
    // The bracket group to open: the outermost one that closes last.
    let Some((open, close)) = outer_group(line) else {
        return vec![format!("{pad}{line}")];
    };
    let head = &line[..=open];
    let inner = &line[open + 1..close];
    let tail = &line[close..];
    let parts = split_top_level(inner);
    if parts.len() == 1 && outer_group(parts[0].trim()).is_none() {
        return vec![format!("{pad}{line}")];
    }
    let inside = indent + 2;
    let mut out = vec![format!("{pad}{}", head.trim_end())];
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.extend(wrap(&format!("{part},"), inside));
    }
    out.push(format!("{pad}{tail}"));
    out
}

/// Where the bracket opened last at the top level of `text` opens and closes,
/// when it closes at the end of `text` but for a few closing characters.
fn outer_group(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string: Option<u8> = None;
    let mut escaped = false;
    let mut last = None;
    let mut open_at = None;
    for (index, &byte) in bytes.iter().enumerate() {
        if let Some(quote) = in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, closing) if closing == quote => in_string = None,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' | b'`' => in_string = Some(byte),
            b'(' | b'{' | b'[' => {
                if depth == 0 {
                    open_at = Some(index);
                }
                depth += 1;
            }
            b')' | b'}' | b']' => {
                depth -= 1;
                if depth == 0
                    && let Some(open) = open_at
                {
                    last = Some((open, index));
                }
            }
            _ => {}
        }
    }
    // Only a group that ends the line (but for `;`, `)` or `,`) is opened.
    let (open, close) = last?;
    text[close + 1..]
        .chars()
        .all(|c| matches!(c, ';' | ')' | ',' | ' '))
        .then_some((open, close))
        .filter(|(open, close)| close > &(open + 1))
}

/// The comma-separated parts of `text` at its top level.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_string: Option<u8> = None;
    let mut escaped = false;
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if let Some(quote) = in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, closing) if closing == quote => in_string = None,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' | b'`' => in_string = Some(byte),
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// The label of the loop `depth` deep.
fn loop_label(depth: usize) -> String {
    if depth == 1 {
        "loop".to_string()
    } else {
        format!("loop{depth}")
    }
}

/// Whether a line ends its path.
fn terminates(line: &str) -> bool {
    line.starts_with("return")
        || line.starts_with("jump(")
        || line.starts_with("raiseError(")
        || line == "continue;"
        || line.starts_with("break")
}

/// An activity call, and the identifier it binds.
struct Call {
    binding: Option<String>,
    text: String,
}

/// An activity's options: `{ a: 1, ... }`, or nothing when empty.
struct Options(Vec<(String, String)>);

impl Options {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn push(&mut self, name: &str, value: impl Into<String>) {
        self.0.push((name.to_string(), value.into()));
    }

    /// `, { ... }` after the other arguments, or nothing.
    fn trailing(&self) -> String {
        if self.0.is_empty() {
            String::new()
        } else {
            format!(", {}", object(&self.0))
        }
    }
}

/// The parts of a macro's arguments after the main ones: bare words and
/// `key = value` pairs.
struct MacroOptions {
    words: Vec<String>,
    pairs: Vec<(String, Expr)>,
}

impl MacroOptions {
    fn parse(arguments: &[Expr]) -> Outcome<Self> {
        let mut options = MacroOptions {
            words: Vec::new(),
            pairs: Vec::new(),
        };
        for argument in arguments {
            match argument {
                Expr::Path(path) if path.path.get_ident().is_some() => {
                    options
                        .words
                        .push(path.path.get_ident().expect("checked").to_string());
                }
                Expr::Assign(assign) => {
                    let Expr::Path(path) = &*assign.left else {
                        return Err("an unexpected option".to_string());
                    };
                    let key = path.path.get_ident().ok_or("an unexpected option")?;
                    options
                        .pairs
                        .push((key.to_string(), (*assign.right).clone()));
                }
                _ => return Err("an unexpected option".to_string()),
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

    fn name(&self) -> Option<String> {
        self.get("name").and_then(string_literal)
    }
}

/// How an activity's variable is named: what the activity names it when it
/// says nothing, and the name it has.
struct Naming {
    default: Option<String>,
    mendix: Option<String>,
    entity: Option<String>,
}

impl Translator<'_> {
    /// Declares an activity's result and says whether `name` has to be
    /// stated: when the identifier — or, unbound, the activity's default —
    /// does not spell it.
    fn bind(
        &mut self,
        binding: Option<&str>,
        naming: Naming,
        options: &mut Options,
    ) -> Outcome<Option<String>> {
        let Some(mendix) = naming.mendix else {
            if binding.is_some() {
                return Err("a binding for an activity without a result".to_string());
            }
            return Ok(None);
        };
        match binding {
            Some(binding) => {
                let ident = self.declare(Some(binding), &mendix, naming.entity);
                if pascal(&ident) != mendix {
                    options.0.insert(0, ("name".to_string(), json(&mendix)));
                }
                Ok(Some(ident))
            }
            None => {
                if naming.default.as_deref() != Some(mendix.as_str()) {
                    options.0.insert(0, ("name".to_string(), json(&mendix)));
                }
                Ok(None)
            }
        }
    }

    /// The key a member is written with, relative to `entity`.
    fn member_key(&self, member: &str, entity: Option<&str>) -> String {
        // An attribute is `Module.Entity.Attribute`, an association
        // `Module.Association`.
        match (member.rsplit_once('.'), entity) {
            (Some((owner, name)), Some(entity)) if owner == entity && owner.contains('.') => {
                name.to_string()
            }
            _ => member.to_string(),
        }
    }

    /// A member as a builder or a struct literal names it, qualified.
    fn member(&self, expr: &Expr) -> Outcome<String> {
        if let Some((kind, target)) = reference(expr) {
            let (entity, name) = target
                .split_once('/')
                .ok_or_else(|| format!("member reference {target}"))?;
            return Ok(match kind {
                'A' | 'F' if name.contains('.') => name.to_string(),
                'A' | 'F' => format!("{entity}.{name}"),
                other => return Err(format!("a reference of kind {other} as a member")),
            });
        }
        if let Some(text) = string_literal(expr) {
            return Ok(text);
        }
        // `MemberName::attribute("...")`, `MemberName::association("...")`.
        if let Expr::Call(call) = expr
            && let [argument] = call.args.iter().collect::<Vec<_>>()[..]
            && let Some(text) = string_literal(argument)
        {
            return Ok(text);
        }
        Err("an unexpected member".to_string())
    }

    fn struct_members(
        &mut self,
        literal: &syn::ExprStruct,
        entity: Option<&str>,
    ) -> Outcome<String> {
        let mut entries = Vec::new();
        for field in &literal.fields {
            let syn::Member::Named(name) = &field.member else {
                return Err("an unnamed member".to_string());
            };
            let (_, target) = names::identifier_reference(&name.to_string())
                .ok_or("a member that is not a reference")?;
            let (owner, member) = target.split_once('/').ok_or("a member reference")?;
            let qualified = if member.contains('.') {
                member.to_string()
            } else {
                format!("{owner}.{member}")
            };
            let value = self.macro_value(&field.expr)?;
            entries.push((self.member_key(&qualified, entity), value));
        }
        Ok(object(&entries))
    }

    fn macro_call(&mut self, mac: &syn::Macro, binding: Option<&str>) -> Outcome<Call> {
        let name = macro_name(mac);
        let arguments = macro_arguments(mac)?;
        // The one activity whose macro may say nothing but the flow.
        if name == "close_page" {
            self.no_result(binding)?;
            self.uses("closePage");
            return Ok(Call {
                binding: None,
                text: match arguments.first() {
                    None => "closePage()".to_string(),
                    Some(count) => format!("closePage({})", self.macro_value(count)?),
                },
            });
        }
        let first = arguments
            .first()
            .ok_or_else(|| format!("{name}! without arguments"))?;
        match name.as_str() {
            "create_object" => {
                let Expr::Struct(literal) = first else {
                    return Err("create_object! without an entity".to_string());
                };
                let (_, entity) =
                    path_reference(&literal.path).ok_or("create_object! of an unknown entity")?;
                let members = self.struct_members(literal, Some(&entity))?;
                let options = MacroOptions::parse(&arguments[1..])?;
                let mut call_options = Options::new();
                if options.has("commit_without_events") {
                    call_options.push("commit", json("withoutEvents"));
                } else if options.has("commit") {
                    call_options.push("commit", "true");
                }
                if options.has("refresh") {
                    call_options.push("refresh", "true");
                }
                let default = format!("New{}", short(&entity));
                let naming = Naming {
                    mendix: Some(options.name().unwrap_or_else(|| default.clone())),
                    default: Some(default),
                    entity: Some(entity.clone()),
                };
                let binding = self.bind(binding, naming, &mut call_options)?;
                self.uses("createObject");
                let members = if members == "{}" && call_options.0.is_empty() {
                    String::new()
                } else {
                    format!(", {members}")
                };
                Ok(Call {
                    binding,
                    text: format!(
                        "createObject({}{members}{})",
                        json(&entity),
                        call_options.trailing()
                    ),
                })
            }
            "change_object" => {
                let variable = self.variable(first)?.clone();
                let Some(Expr::Struct(literal)) = arguments.get(1) else {
                    return Err("change_object! without members".to_string());
                };
                let members = self.struct_members(literal, variable.entity.as_deref())?;
                let options = MacroOptions::parse(&arguments[2..])?;
                let mut call_options = Options::new();
                if options.has("commit_without_events") {
                    call_options.push("commit", json("withoutEvents"));
                } else if options.has("commit") {
                    call_options.push("commit", "true");
                }
                if options.has("refresh") {
                    call_options.push("refresh", "true");
                }
                self.no_result(binding)?;
                self.uses("changeObject");
                Ok(Call {
                    binding: None,
                    text: format!(
                        "changeObject({}, {members}{})",
                        variable.ident,
                        call_options.trailing()
                    ),
                })
            }
            "commit_object" | "delete_object" | "rollback_object" => {
                let variable = self.variable(first)?.ident.clone();
                let options = MacroOptions::parse(&arguments[1..])?;
                let mut call_options = Options::new();
                if options.has("without_events") {
                    call_options.push("withEvents", "false");
                }
                if options.has("refresh") {
                    call_options.push("refresh", "true");
                }
                self.no_result(binding)?;
                let function = match name.as_str() {
                    "commit_object" => "commitObject",
                    "delete_object" => "deleteObject",
                    _ => "rollbackObject",
                };
                self.uses(function);
                Ok(Call {
                    binding: None,
                    text: format!("{function}({variable}{})", call_options.trailing()),
                })
            }
            "retrieve" => self.retrieve_macro(&arguments, binding),
            "create_list" => {
                let (_, entity) = reference(first).ok_or("create_list! of an unknown entity")?;
                let options = MacroOptions::parse(&arguments[1..])?;
                let default = format!("{}List", short(&entity));
                let mut call_options = Options::new();
                let naming = Naming {
                    mendix: Some(options.name().unwrap_or_else(|| default.clone())),
                    default: Some(default),
                    entity: Some(entity.clone()),
                };
                let binding = self.bind(binding, naming, &mut call_options)?;
                self.uses("createList");
                Ok(Call {
                    binding,
                    text: format!("createList({}{})", json(&entity), call_options.trailing()),
                })
            }
            "change_list" => self.change_list_macro(&arguments, binding),
            "aggregate_list" => self.aggregate_macro(&arguments, binding),
            "create_variable" => {
                let ty = self.variable_type(first)?;
                let value =
                    self.macro_value(arguments.get(1).ok_or("create_variable! without a value")?)?;
                let options = MacroOptions::parse(&arguments[2..])?;
                let mut call_options = Options::new();
                let naming = Naming {
                    mendix: Some(options.name().ok_or("create_variable! without a name")?),
                    default: None,
                    entity: None,
                };
                let binding = self.bind(binding, naming, &mut call_options)?;
                self.uses("createVariable");
                Ok(Call {
                    binding,
                    text: format!("createVariable({ty}, {value}{})", call_options.trailing()),
                })
            }
            "change_variable" => {
                let variable = self.variable(first)?.ident.clone();
                let value =
                    self.macro_value(arguments.get(1).ok_or("change_variable! without a value")?)?;
                self.no_result(binding)?;
                self.uses("changeVariable");
                Ok(Call {
                    binding: None,
                    text: format!("changeVariable({variable}, {value})"),
                })
            }
            "call_microflow" | "call_nanoflow" => {
                let (target, entries) = match first {
                    Expr::Struct(literal) => {
                        let (_, target) = path_reference(&literal.path).ok_or("a call target")?;
                        let mut entries = Vec::new();
                        for field in &literal.fields {
                            let syn::Member::Named(parameter) = &field.member else {
                                return Err("an unnamed argument".to_string());
                            };
                            let parameter = parameter.to_string();
                            let parameter = parameter
                                .strip_prefix("r#")
                                .unwrap_or(&parameter)
                                .to_string();
                            entries.push((parameter, self.macro_value(&field.expr)?));
                        }
                        (target, entries)
                    }
                    other => (reference(other).ok_or("a call target")?.1, Vec::new()),
                };
                let options = MacroOptions::parse(&arguments[1..])?;
                let result = options.name();
                if name == "call_microflow" {
                    self.microflow_call(&target, entries, result, None, binding)
                } else {
                    self.nanoflow_call(&target, entries, result, None, binding)
                }
            }
            "call_javascript_action" => {
                let action = string_literal(first).ok_or("a JavaScript action's name")?;
                let (result, rest) = match arguments.get(1).and_then(string_literal) {
                    Some(result) => (Some(result), &arguments[2..]),
                    None => (None, &arguments[1..]),
                };
                let mut entries = Vec::new();
                for argument in rest {
                    let Expr::Assign(assign) = argument else {
                        return Err("an unexpected JavaScript argument".to_string());
                    };
                    let Expr::Path(parameter) = &*assign.left else {
                        return Err("an unexpected JavaScript argument".to_string());
                    };
                    let parameter = parameter
                        .path
                        .get_ident()
                        .ok_or("a JavaScript parameter's name")?
                        .to_string();
                    let parameter = parameter
                        .strip_prefix("r#")
                        .unwrap_or(&parameter)
                        .to_string();
                    let value = match &*assign.right {
                        Expr::Struct(literal) if literal.fields.is_empty() => {
                            let (_, entity) =
                                path_reference(&literal.path).ok_or("an entity argument")?;
                            format!("{{ entity: {} }}", json(&entity))
                        }
                        other => match reference(other) {
                            Some(('E', entity)) => format!("{{ entity: {} }}", json(&entity)),
                            _ => self.macro_value(other)?,
                        },
                    };
                    entries.push((parameter, value));
                }
                self.javascript_call(&action, entries, result, None, binding)
            }
            "show_page" => {
                let page = string_literal(first).ok_or("a page by name")?;
                let mut entries = Vec::new();
                let mut call_options = Options::new();
                for argument in &arguments[1..] {
                    let Expr::Assign(assign) = argument else {
                        return Err("an unexpected page argument".to_string());
                    };
                    let Expr::Path(parameter) = &*assign.left else {
                        return Err("an unexpected page argument".to_string());
                    };
                    let parameter = parameter
                        .path
                        .get_ident()
                        .ok_or("a page parameter's name")?
                        .to_string();
                    let value = self.macro_value(&assign.right)?;
                    if parameter == "close_pages" {
                        call_options.push("closePages", value);
                    } else {
                        let parameter = parameter
                            .strip_prefix("r#")
                            .unwrap_or(&parameter)
                            .to_string();
                        entries.push((parameter, value));
                    }
                }
                let mut leading = Vec::new();
                if !entries.is_empty() {
                    leading.push(("args".to_string(), object(&entries)));
                }
                leading.extend(call_options.0);
                self.no_result(binding)?;
                self.uses("showPage");
                Ok(Call {
                    binding: None,
                    text: format!("showPage({}{})", json(&page), Options(leading).trailing()),
                })
            }
            "show_message" => {
                let kind = match first {
                    Expr::Path(path) => path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string().to_lowercase())
                        .ok_or("a message kind")?,
                    _ => return Err("a message kind".to_string()),
                };
                let message =
                    string_literal(arguments.get(1).ok_or("show_message! without a text")?)
                        .ok_or("show_message! without a text")?;
                let options = MacroOptions::parse(&arguments[2..])?;
                let mut call_options = Options::new();
                if let Some(Expr::Array(parameters)) = options.get("parameters") {
                    let values = parameters
                        .elems
                        .iter()
                        .map(|value| self.macro_value(value))
                        .collect::<Outcome<Vec<_>>>()?;
                    call_options.push("parameters", format!("[{}]", values.join(", ")));
                }
                if let Some(blocking) = options.get("blocking") {
                    call_options.push("blocking", bool_literal(blocking)?);
                }
                self.no_result(binding)?;
                self.uses("showMessage");
                Ok(Call {
                    binding: None,
                    text: format!(
                        "showMessage({}, {}{})",
                        json(&kind),
                        object(&[("en_US".to_string(), json(&message))]),
                        call_options.trailing()
                    ),
                })
            }
            "log" => {
                let level = match first {
                    Expr::Path(path) => path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string())
                        .ok_or("a log level")?,
                    _ => return Err("a log level".to_string()),
                };
                let node = self.macro_value(arguments.get(1).ok_or("log! without a node")?)?;
                let message = string_literal(arguments.get(2).ok_or("log! without a message")?)
                    .ok_or("log! without a message")?;
                let options = MacroOptions::parse(&arguments[3..])?;
                let mut call_options = Options::new();
                if let Some(Expr::Array(parameters)) = options.get("parameters") {
                    let values = parameters
                        .elems
                        .iter()
                        .map(|value| self.macro_value(value))
                        .collect::<Outcome<Vec<_>>>()?;
                    call_options.push("parameters", format!("[{}]", values.join(", ")));
                }
                if options.has("stack_trace") {
                    call_options.push("stackTrace", "true");
                }
                self.no_result(binding)?;
                self.uses("log");
                Ok(Call {
                    binding: None,
                    text: format!(
                        "log({}, {node}, {}{})",
                        json(&level),
                        json(&message),
                        call_options.trailing()
                    ),
                })
            }
            other => Err(format!("{other}!")),
        }
    }

    fn no_result(&self, binding: Option<&str>) -> Outcome<()> {
        match binding {
            Some(_) => Err("a binding for an activity without a result".to_string()),
            None => Ok(()),
        }
    }

    fn sorting(&self, expr: &Expr, entity: Option<&str>, macro_form: bool) -> Outcome<String> {
        let Expr::Array(array) = expr else {
            return Err("an unexpected sorting".to_string());
        };
        let entries = array
            .elems
            .iter()
            .map(|element| {
                let Expr::Tuple(pair) = element else {
                    return Err("an unexpected sorting".to_string());
                };
                let [member, order] = pair.elems.iter().collect::<Vec<_>>()[..] else {
                    return Err("an unexpected sorting".to_string());
                };
                let _ = macro_form;
                let member = self.member(member)?;
                let order = match order {
                    Expr::Path(path) => path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let order = match order.as_str() {
                    "Ascending" => "asc",
                    "Descending" => "desc",
                    _ => return Err("an unexpected sort order".to_string()),
                };
                Ok(format!(
                    "[{}, {}]",
                    json(&self.member_key(&member, entity)),
                    json(order)
                ))
            })
            .collect::<Outcome<Vec<_>>>()?;
        Ok(format!("[{}]", entries.join(", ")))
    }

    fn retrieve_macro(&mut self, arguments: &[Expr], binding: Option<&str>) -> Outcome<Call> {
        let first = &arguments[0];
        let options = MacroOptions::parse(&arguments[1..])?;
        let mut call_options = Options::new();
        if let Some(association) = options.get("by") {
            let variable = self.variable(first)?.ident.clone();
            let association = self.member(association)?;
            let association = association
                .rsplit_once('.')
                .filter(|(owner, _)| owner.contains('.'))
                .map_or(association.clone(), |(_, name)| name.to_string());
            call_options.push("by", json(&association));
            let naming = Naming {
                mendix: Some(
                    options
                        .name()
                        .ok_or("retrieve! by an association without a name")?,
                ),
                default: None,
                entity: None,
            };
            let binding = self.bind(binding, naming, &mut call_options)?;
            self.uses("retrieve");
            return Ok(Call {
                binding,
                text: format!("retrieve({variable}{})", call_options.trailing()),
            });
        }
        let (_, entity) = reference(first).ok_or("retrieve! of an unknown entity")?;
        if let Some(xpath) = options.get("xpath") {
            call_options.push(
                "xpath",
                text_literal(&string_literal(xpath).ok_or("an XPath that is not text")?),
            );
        }
        if let Some(sort) = options.get("sort") {
            let sort = self.sorting(sort, Some(&entity), true)?;
            call_options.push("sort", sort);
        }
        let first_only = options.has("first");
        if first_only {
            call_options.push("first", "true");
        }
        if let Some(Expr::Tuple(range)) = options.get("range") {
            let [limit, offset] = range.elems.iter().collect::<Vec<_>>()[..] else {
                return Err("an unexpected range".to_string());
            };
            let range = format!(
                "[{}, {}]",
                self.macro_value(limit)?,
                self.macro_value(offset)?
            );
            call_options.push("range", range);
        }
        let default = if first_only {
            short(&entity).to_string()
        } else {
            format!("{}List", short(&entity))
        };
        let naming = Naming {
            mendix: Some(options.name().unwrap_or_else(|| default.clone())),
            default: Some(default),
            entity: Some(entity.clone()),
        };
        let binding = self.bind(binding, naming, &mut call_options)?;
        self.uses("retrieve");
        Ok(Call {
            binding,
            text: format!("retrieve({}{})", json(&entity), call_options.trailing()),
        })
    }

    fn change_list_macro(&mut self, arguments: &[Expr], binding: Option<&str>) -> Outcome<Call> {
        let list = self.variable(&arguments[0])?.clone();
        let options = MacroOptions::parse(&arguments[1..])?;
        let mut call_options = Options::new();
        let operation = if let Some(word) = options.words.first() {
            match word.as_str() {
                "clear" | "head" | "tail" => format!("{{ {word}: true }}"),
                other => return Err(format!("change_list! {other}")),
            }
        } else {
            let (key, argument) = options
                .pairs
                .iter()
                .find(|(key, _)| key != "name")
                .ok_or("change_list! without an operation")?;
            let value = match key.as_str() {
                "add" | "remove" | "replace" | "find_by" | "filter_by" => {
                    self.macro_value(argument)?
                }
                "find" | "filter" => {
                    let Expr::Tuple(pair) = argument else {
                        return Err("an unexpected find".to_string());
                    };
                    let [member, value] = pair.elems.iter().collect::<Vec<_>>()[..] else {
                        return Err("an unexpected find".to_string());
                    };
                    let member = self.member(member)?;
                    format!(
                        "[{}, {}]",
                        json(&self.member_key(&member, list.entity.as_deref())),
                        self.macro_value(value)?
                    )
                }
                "sort" => self.sorting(argument, list.entity.as_deref(), true)?,
                "range" => {
                    let Expr::Tuple(pair) = argument else {
                        return Err("an unexpected range".to_string());
                    };
                    let [limit, offset] = pair.elems.iter().collect::<Vec<_>>()[..] else {
                        return Err("an unexpected range".to_string());
                    };
                    format!(
                        "[{}, {}]",
                        self.macro_value(limit)?,
                        self.macro_value(offset)?
                    )
                }
                "union" | "intersect" | "subtract" | "contains" | "equals" => {
                    self.variable(argument)?.ident.clone()
                }
                other => return Err(format!("change_list! {other}")),
            };
            format!("{{ {}: {value} }}", camel_from_snake(key))
        };
        let makes = !matches!(
            operation.split(':').next().unwrap_or_default(),
            "{ add" | "{ remove" | "{ replace" | "{ clear"
        );
        let entity = list.entity.clone();
        let naming = Naming {
            mendix: if makes {
                Some(
                    options
                        .name()
                        .ok_or("change_list! making a list without a name")?,
                )
            } else {
                None
            },
            default: None,
            entity,
        };
        let binding = self.bind(binding, naming, &mut call_options)?;
        self.uses("changeList");
        Ok(Call {
            binding,
            text: format!(
                "changeList({}, {operation}{})",
                list.ident,
                call_options.trailing()
            ),
        })
    }

    fn aggregate_macro(&mut self, arguments: &[Expr], binding: Option<&str>) -> Outcome<Call> {
        let list = self.variable(&arguments[0])?.clone();
        let options = MacroOptions::parse(&arguments[1..])?;
        let mut call_options = Options::new();
        let aggregate = if options.has("count") {
            "{ count: true }".to_string()
        } else {
            let (key, argument) = options
                .pairs
                .iter()
                .find(|(key, _)| key != "name")
                .ok_or("aggregate_list! without a function")?;
            let value = if key.ends_with("_of") {
                self.macro_value(argument)?
            } else {
                let member = self.member(argument)?;
                json(&self.member_key(&member, list.entity.as_deref()))
            };
            format!("{{ {}: {value} }}", camel_from_snake(key))
        };
        let naming = Naming {
            mendix: Some(options.name().ok_or("aggregate_list! without a name")?),
            default: None,
            entity: None,
        };
        let binding = self.bind(binding, naming, &mut call_options)?;
        self.uses("aggregateList");
        Ok(Call {
            binding,
            text: format!(
                "aggregateList({}, {aggregate}{})",
                list.ident,
                call_options.trailing()
            ),
        })
    }

    /// A microflow call: `callMicroflow("Module.Flow", { Parameter: value })`.
    fn microflow_call(
        &mut self,
        target: &str,
        entries: Vec<(String, String)>,
        result: Option<String>,
        discard: Option<String>,
        binding: Option<&str>,
    ) -> Outcome<Call> {
        let mut call_options = Options::new();
        if let Some(discard) = &discard {
            call_options.push("discardResult", json(discard));
        }
        let naming = Naming {
            mendix: result,
            default: None,
            entity: None,
        };
        let binding = self.bind(binding, naming, &mut call_options)?;
        self.uses("callMicroflow");
        let arguments = if entries.is_empty() && call_options.0.is_empty() {
            String::new()
        } else {
            format!(", {}", object(&entries))
        };
        Ok(Call {
            binding,
            text: format!(
                "callMicroflow({}{arguments}{})",
                json(target),
                call_options.trailing()
            ),
        })
    }

    /// A nanoflow call: the service method itself when its arguments are
    /// exactly its parameters in order and nothing needs naming, else
    /// `callNanoflow("Module.Flow", { Parameter: value })`.
    fn nanoflow_call(
        &mut self,
        target: &str,
        entries: Vec<(String, String)>,
        result: Option<String>,
        discard: Option<String>,
        binding: Option<&str>,
    ) -> Outcome<Call> {
        if let Some(callee) = self.callees.get(target)
            && discard.is_none()
            && entries
                .iter()
                .map(|(name, _)| name)
                .eq(callee.parameters.iter())
        {
            // Bound, the identifier names the result; unbound, there is none.
            let positional = match (&result, binding) {
                (Some(result), Some(_)) => pascal(&camel(result)) == *result,
                (None, None) => true,
                _ => false,
            };
            if positional {
                let mut named = Options::new();
                let binding = self.bind(
                    binding,
                    Naming {
                        mendix: result,
                        default: None,
                        entity: None,
                    },
                    &mut named,
                )?;
                if !named.0.is_empty() {
                    // Its identifier came out other than its name: the call
                    // says the name.
                    self.uses("callNanoflow");
                    return Ok(Call {
                        binding,
                        text: format!(
                            "callNanoflow({}, {}{})",
                            json(target),
                            object(&entries),
                            named.trailing()
                        ),
                    });
                }
                if callee.service != self.service {
                    self.services.insert(callee.service.clone());
                }
                let values: Vec<&str> = entries.iter().map(|(_, value)| value.as_str()).collect();
                return Ok(Call {
                    binding,
                    text: format!(
                        "{}.{}({})",
                        callee.service,
                        callee.method,
                        values.join(", ")
                    ),
                });
            }
        }
        let mut call_options = Options::new();
        if let Some(discard) = &discard {
            call_options.push("discardResult", json(discard));
        }
        let naming = Naming {
            mendix: result,
            default: None,
            entity: None,
        };
        let binding = self.bind(binding, naming, &mut call_options)?;
        self.uses("callNanoflow");
        let arguments = if entries.is_empty() && call_options.0.is_empty() {
            String::new()
        } else {
            format!(", {}", object(&entries))
        };
        Ok(Call {
            binding,
            text: format!(
                "callNanoflow({}{arguments}{})",
                json(target),
                call_options.trailing()
            ),
        })
    }

    fn javascript_call(
        &mut self,
        action: &str,
        entries: Vec<(String, String)>,
        result: Option<String>,
        discard: Option<String>,
        binding: Option<&str>,
    ) -> Outcome<Call> {
        let mut call_options = Options::new();
        if let Some(discard) = &discard {
            call_options.push("discardResult", json(discard));
        }
        let naming = Naming {
            mendix: result,
            default: None,
            entity: None,
        };
        let binding = self.bind(binding, naming, &mut call_options)?;
        self.uses("callJavaScriptAction");
        let arguments = if entries.is_empty() && call_options.0.is_empty() {
            String::new()
        } else {
            format!(", {}", object(&entries))
        };
        Ok(Call {
            binding,
            text: format!(
                "callJavaScriptAction({}{arguments}{})",
                json(action),
                call_options.trailing()
            ),
        })
    }

    /// An activity written as its builder call.
    fn builder_call(
        &mut self,
        method: &str,
        args: &[&Expr],
        binding: Option<&str>,
    ) -> Outcome<Call> {
        match (method, args) {
            ("create", [name, entity, options]) => {
                let mendix = string_literal(name).ok_or("a create without a name")?;
                let entity = marker_target(entity).ok_or("a create of an unknown entity")?;
                let (members, mut call_options) = self.change_options(options, Some(&entity))?;
                let default = format!("New{}", short(&entity));
                let binding = self.bind(
                    binding,
                    Naming {
                        mendix: Some(mendix),
                        default: Some(default),
                        entity: Some(entity.clone()),
                    },
                    &mut call_options,
                )?;
                self.uses("createObject");
                let members = if members == "{}" && call_options.0.is_empty() {
                    String::new()
                } else {
                    format!(", {members}")
                };
                Ok(Call {
                    binding,
                    text: format!(
                        "createObject({}{members}{})",
                        json(&entity),
                        call_options.trailing()
                    ),
                })
            }
            ("change", [variable, options]) => {
                let variable = self.variable(variable)?.clone();
                let (members, call_options) =
                    self.change_options(options, variable.entity.as_deref())?;
                self.no_result(binding)?;
                self.uses("changeObject");
                Ok(Call {
                    binding: None,
                    text: format!(
                        "changeObject({}, {members}{})",
                        variable.ident,
                        call_options.trailing()
                    ),
                })
            }
            ("commit" | "delete_object" | "rollback", [variable]) => {
                let variable = self.variable(variable)?.ident.clone();
                self.no_result(binding)?;
                let function = match method {
                    "commit" => "commitObject",
                    "delete_object" => "deleteObject",
                    _ => "rollbackObject",
                };
                self.uses(function);
                Ok(Call {
                    binding: None,
                    text: format!("{function}({variable})"),
                })
            }
            ("commit_with" | "delete_with" | "rollback_with", [variable, options]) => {
                let variable = self.variable(variable)?.ident.clone();
                let mut call_options = Options::new();
                for (option, values) in options_calls(options)? {
                    match (option.as_str(), &values[..]) {
                        ("with_events", [value]) => {
                            call_options.push("withEvents", bool_literal(value)?)
                        }
                        ("refresh_in_client", [value]) => {
                            call_options.push("refresh", bool_literal(value)?)
                        }
                        (other, _) => return Err(format!("{method} option {other}")),
                    }
                }
                self.no_result(binding)?;
                let function = match method {
                    "commit_with" => "commitObject",
                    "delete_with" => "deleteObject",
                    _ => "rollbackObject",
                };
                self.uses(function);
                Ok(Call {
                    binding: None,
                    text: format!("{function}({variable}{})", call_options.trailing()),
                })
            }
            ("retrieve", [name, entity, options]) => {
                let mendix = string_literal(name).ok_or("a retrieve without a name")?;
                let entity = marker_target(entity).ok_or("a retrieve of an unknown entity")?;
                let mut call_options = Options::new();
                let mut first_only = false;
                let mut sort = Vec::new();
                for (option, values) in options_calls(options)? {
                    match (option.as_str(), &values[..]) {
                        ("xpath", [xpath]) => call_options.push(
                            "xpath",
                            text_literal(
                                &string_literal(xpath).ok_or("an XPath that is not text")?,
                            ),
                        ),
                        ("first", []) => {
                            first_only = true;
                            call_options.push("first", "true");
                        }
                        ("range", [limit, offset]) => {
                            let range = format!(
                                "[{}, {}]",
                                self.builder_value(limit)?,
                                self.builder_value(offset)?
                            );
                            call_options.push("range", range);
                        }
                        ("sort_by", [member, order]) => {
                            let member = self.member(member)?;
                            let order = sort_word(order)?;
                            sort.push(format!(
                                "[{}, {}]",
                                json(&self.member_key(&member, Some(&entity))),
                                json(order)
                            ));
                        }
                        (other, _) => return Err(format!("retrieve option {other}")),
                    }
                }
                if !sort.is_empty() {
                    // Sorting reads before the rest, as the macro states it.
                    let position = usize::from(
                        call_options
                            .0
                            .first()
                            .is_some_and(|(key, _)| key == "xpath"),
                    );
                    call_options.0.insert(
                        position,
                        ("sort".to_string(), format!("[{}]", sort.join(", "))),
                    );
                }
                let default = if first_only {
                    short(&entity).to_string()
                } else {
                    format!("{}List", short(&entity))
                };
                let binding = self.bind(
                    binding,
                    Naming {
                        mendix: Some(mendix),
                        default: Some(default),
                        entity: Some(entity.clone()),
                    },
                    &mut call_options,
                )?;
                self.uses("retrieve");
                Ok(Call {
                    binding,
                    text: format!("retrieve({}{})", json(&entity), call_options.trailing()),
                })
            }
            ("retrieve_associated", [name, variable, association]) => {
                let mendix = string_literal(name).ok_or("a retrieve without a name")?;
                let variable = self.variable(variable)?.ident.clone();
                let association = self.member(association)?;
                let association = association
                    .rsplit_once('.')
                    .filter(|(owner, _)| owner.contains('.'))
                    .map_or(association.clone(), |(_, name)| name.to_string());
                let mut call_options = Options::new();
                call_options.push("by", json(&association));
                let binding = self.bind(
                    binding,
                    Naming {
                        mendix: Some(mendix),
                        default: None,
                        entity: None,
                    },
                    &mut call_options,
                )?;
                self.uses("retrieve");
                Ok(Call {
                    binding,
                    text: format!("retrieve({variable}{})", call_options.trailing()),
                })
            }
            ("create_list_of", [name, entity]) => {
                let mendix = string_literal(name).ok_or("a list without a name")?;
                let entity = marker_target(entity).ok_or("a list of an unknown entity")?;
                let mut call_options = Options::new();
                let binding = self.bind(
                    binding,
                    Naming {
                        mendix: Some(mendix),
                        default: Some(format!("{}List", short(&entity))),
                        entity: Some(entity.clone()),
                    },
                    &mut call_options,
                )?;
                self.uses("createList");
                Ok(Call {
                    binding,
                    text: format!("createList({}{})", json(&entity), call_options.trailing()),
                })
            }
            ("change_list", [list, change, value]) => {
                let list = self.variable(list)?.ident.clone();
                let change = match change {
                    Expr::Path(path) => path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let operation = match change.as_str() {
                    "Add" => format!("{{ add: {} }}", self.builder_value(value)?),
                    "Remove" => format!("{{ remove: {} }}", self.builder_value(value)?),
                    "Set" => format!("{{ replace: {} }}", self.builder_value(value)?),
                    "Clear" => {
                        if string_literal(value).is_none()
                            && !matches!(value, Expr::Call(call) if call.args.first().and_then(string_literal).as_deref() == Some(""))
                        {
                            return Err("a clear with a value".to_string());
                        }
                        "{ clear: true }".to_string()
                    }
                    other => return Err(format!("list change {other}")),
                };
                self.no_result(binding)?;
                self.uses("changeList");
                Ok(Call {
                    binding: None,
                    text: format!("changeList({list}, {operation})"),
                })
            }
            (
                "list_head" | "list_tail" | "list_find" | "list_find_by" | "list_filter"
                | "list_filter_by" | "list_sort" | "list_range" | "list_union" | "list_intersect"
                | "list_subtract" | "list_contains" | "list_equals",
                [name, list, rest @ ..],
            ) => {
                let mendix = string_literal(name).ok_or("a list operation without a name")?;
                let list = self.variable(list)?.clone();
                let operation = method.trim_start_matches("list_");
                let value = match (operation, rest) {
                    ("head" | "tail", []) => "true".to_string(),
                    ("find" | "filter", [member, value]) => {
                        let member = self.member(member)?;
                        format!(
                            "[{}, {}]",
                            json(&self.member_key(&member, list.entity.as_deref())),
                            self.builder_value(value)?
                        )
                    }
                    ("find_by" | "filter_by", [value]) => self.builder_value(value)?,
                    ("sort", [options]) => {
                        let mut sort = Vec::new();
                        for (option, values) in options_calls(options)? {
                            let ("by", [member, order]) = (option.as_str(), &values[..]) else {
                                return Err(format!("sort option {option}"));
                            };
                            let member = self.member(member)?;
                            sort.push(format!(
                                "[{}, {}]",
                                json(&self.member_key(&member, list.entity.as_deref())),
                                json(sort_word(order)?)
                            ));
                        }
                        format!("[{}]", sort.join(", "))
                    }
                    ("range", [limit, offset]) => format!(
                        "[{}, {}]",
                        self.builder_value(limit)?,
                        self.builder_value(offset)?
                    ),
                    ("union" | "intersect" | "subtract" | "contains" | "equals", [other]) => {
                        self.variable(other)?.ident.clone()
                    }
                    _ => return Err(format!("{method} of an unexpected shape")),
                };
                let mut call_options = Options::new();
                let binding = self.bind(
                    binding,
                    Naming {
                        mendix: Some(mendix),
                        default: None,
                        entity: list.entity.clone(),
                    },
                    &mut call_options,
                )?;
                self.uses("changeList");
                Ok(Call {
                    binding,
                    text: format!(
                        "changeList({}, {{ {}: {value} }}{})",
                        list.ident,
                        camel_from_snake(operation),
                        call_options.trailing()
                    ),
                })
            }
            ("aggregate", [name, list, function, options]) => {
                let mendix = string_literal(name).ok_or("an aggregate without a name")?;
                let list = self.variable(list)?.clone();
                let function = match function {
                    Expr::Path(path) => path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string().to_lowercase())
                        .unwrap_or_default(),
                    _ => return Err("an aggregate function".to_string()),
                };
                let calls = match options {
                    Expr::Closure(_) => options_calls(options)?,
                    _ => return Err("an aggregate of an unexpected shape".to_string()),
                };
                let aggregate = match (function.as_str(), &calls[..]) {
                    ("count", []) => "{ count: true }".to_string(),
                    (function, [(option, values)])
                        if option == "attribute" && values.len() == 1 =>
                    {
                        let member = self.member(&values[0])?;
                        format!(
                            "{{ {function}: {} }}",
                            json(&self.member_key(&member, list.entity.as_deref()))
                        )
                    }
                    (function, [(option, values)])
                        if option == "expression" && values.len() == 1 =>
                    {
                        format!("{{ {function}Of: {} }}", self.builder_value(&values[0])?)
                    }
                    _ => return Err("an aggregate of an unexpected shape".to_string()),
                };
                let mut call_options = Options::new();
                let binding = self.bind(
                    binding,
                    Naming {
                        mendix: Some(mendix),
                        default: None,
                        entity: None,
                    },
                    &mut call_options,
                )?;
                self.uses("aggregateList");
                Ok(Call {
                    binding,
                    text: format!(
                        "aggregateList({}, {aggregate}{})",
                        list.ident,
                        call_options.trailing()
                    ),
                })
            }
            ("create_variable", [name, ty, value]) => {
                let mendix = string_literal(name).ok_or("a variable without a name")?;
                let ty = self.variable_type(ty)?;
                let value = self.builder_value(value)?;
                let mut call_options = Options::new();
                let binding = self.bind(
                    binding,
                    Naming {
                        mendix: Some(mendix),
                        default: None,
                        entity: None,
                    },
                    &mut call_options,
                )?;
                self.uses("createVariable");
                Ok(Call {
                    binding,
                    text: format!("createVariable({ty}, {value}{})", call_options.trailing()),
                })
            }
            ("change_variable", [variable, value]) => {
                let variable = self.variable(variable)?.ident.clone();
                let value = self.builder_value(value)?;
                self.no_result(binding)?;
                self.uses("changeVariable");
                Ok(Call {
                    binding: None,
                    text: format!("changeVariable({variable}, {value})"),
                })
            }
            ("call" | "call_nanoflow" | "call_javascript", [target, options]) => {
                self.builder_flow_call(method, None, target, options, binding)
            }
            (
                "call_into" | "call_nanoflow_into" | "call_javascript_into",
                [name, target, options],
            ) => {
                let name = string_literal(name).ok_or("a call result without a name")?;
                self.builder_flow_call(
                    method.trim_end_matches("_into"),
                    Some(name),
                    target,
                    options,
                    binding,
                )
            }
            ("log", [level, node, message, options]) => {
                let level = match level {
                    Expr::Path(path) => path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string())
                        .ok_or("a log level")?,
                    _ => return Err("a log level".to_string()),
                };
                let node = self.builder_value(node)?;
                let message = string_literal(message).ok_or("a log message")?;
                let mut call_options = Options::new();
                let mut parameters = Vec::new();
                for (option, values) in options_calls(options)? {
                    match (option.as_str(), &values[..]) {
                        ("parameter", [value]) => parameters.push(self.builder_value(value)?),
                        ("include_stack_trace", [value]) => {
                            call_options.push("stackTrace", bool_literal(value)?)
                        }
                        (other, _) => return Err(format!("log option {other}")),
                    }
                }
                if !parameters.is_empty() {
                    call_options.0.insert(
                        0,
                        (
                            "parameters".to_string(),
                            format!("[{}]", parameters.join(", ")),
                        ),
                    );
                }
                self.no_result(binding)?;
                self.uses("log");
                Ok(Call {
                    binding: None,
                    text: format!(
                        "log({}, {node}, {}{})",
                        json(&level),
                        json(&message),
                        call_options.trailing()
                    ),
                })
            }
            ("show_page", [page, options]) => {
                let page = string_literal(page).ok_or("a page by name")?;
                let mut arguments = Vec::new();
                let mut title = Vec::new();
                let mut title_parameters = Vec::new();
                let mut call_options = Options::new();
                for (option, values) in options_calls(options)? {
                    match (option.as_str(), &values[..]) {
                        ("argument", [parameter, value]) => arguments.push((
                            string_literal(parameter).ok_or("a page parameter")?,
                            self.builder_value(value)?,
                        )),
                        ("title", [language, text]) => title.push((
                            string_literal(language).ok_or("a title language")?,
                            json(&string_literal(text).ok_or("a title")?),
                        )),
                        ("title_parameter", [value]) => {
                            title_parameters.push(self.builder_value(value)?)
                        }
                        ("close_pages", [value]) => {
                            call_options.push("closePages", self.builder_value(value)?)
                        }
                        (other, _) => return Err(format!("page option {other}")),
                    }
                }
                let mut leading = Vec::new();
                if !arguments.is_empty() {
                    leading.push(("args".to_string(), object(&arguments)));
                }
                if !title.is_empty() {
                    leading.push(("title".to_string(), object(&title)));
                }
                if !title_parameters.is_empty() {
                    leading.push((
                        "titleParameters".to_string(),
                        format!("[{}]", title_parameters.join(", ")),
                    ));
                }
                leading.extend(call_options.0);
                self.no_result(binding)?;
                self.uses("showPage");
                Ok(Call {
                    binding: None,
                    text: format!("showPage({}{})", json(&page), Options(leading).trailing()),
                })
            }
            ("close_page", []) => {
                self.no_result(binding)?;
                self.uses("closePage");
                Ok(Call {
                    binding: None,
                    text: "closePage()".to_string(),
                })
            }
            ("close_pages", [count]) => {
                let count = self.builder_value(count)?;
                self.no_result(binding)?;
                self.uses("closePage");
                Ok(Call {
                    binding: None,
                    text: format!("closePage({count})"),
                })
            }
            ("show_message", [kind, options]) => {
                let kind = match kind {
                    Expr::Path(path) => path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string().to_lowercase())
                        .ok_or("a message kind")?,
                    _ => return Err("a message kind".to_string()),
                };
                let mut text = Vec::new();
                let mut parameters = Vec::new();
                let mut call_options = Options::new();
                for (option, values) in options_calls(options)? {
                    match (option.as_str(), &values[..]) {
                        ("text", [language, value]) => text.push((
                            string_literal(language).ok_or("a message language")?,
                            json(&string_literal(value).ok_or("a message text")?),
                        )),
                        ("parameter", [value]) => parameters.push(self.builder_value(value)?),
                        ("blocking", [value]) => {
                            call_options.push("blocking", bool_literal(value)?)
                        }
                        (other, _) => return Err(format!("message option {other}")),
                    }
                }
                if !parameters.is_empty() {
                    call_options.0.insert(
                        0,
                        (
                            "parameters".to_string(),
                            format!("[{}]", parameters.join(", ")),
                        ),
                    );
                }
                self.no_result(binding)?;
                self.uses("showMessage");
                Ok(Call {
                    binding: None,
                    text: format!(
                        "showMessage({}, {}{})",
                        json(&kind),
                        object(&text),
                        call_options.trailing()
                    ),
                })
            }
            (other, _) => Err(format!("flow.{other}")),
        }
    }

    /// `|create| { create.set(..); create.commit(..); .. }`: the members it
    /// sets and its options.
    fn change_options(
        &mut self,
        options: &Expr,
        entity: Option<&str>,
    ) -> Outcome<(String, Options)> {
        let mut members = Vec::new();
        let mut call_options = Options::new();
        for (option, values) in options_calls(options)? {
            match (option.as_str(), &values[..]) {
                ("set", [member, value]) => {
                    let member = self.member(member)?;
                    members.push((self.member_key(&member, entity), self.builder_value(value)?));
                }
                ("commit", [value]) => {
                    let commit = match value {
                        Expr::Path(path) => path
                            .path
                            .segments
                            .last()
                            .map(|segment| segment.ident.to_string())
                            .unwrap_or_default(),
                        _ => String::new(),
                    };
                    let commit = match commit.as_str() {
                        "Yes" => "true".to_string(),
                        "No" => "false".to_string(),
                        "WithoutEvents" => json("withoutEvents"),
                        _ => return Err("an unexpected commit".to_string()),
                    };
                    call_options.push("commit", commit);
                }
                ("refresh_in_client", [value]) => {
                    call_options.push("refresh", bool_literal(value)?)
                }
                (other, _) => return Err(format!("change option {other}")),
            }
        }
        Ok((object(&members), call_options))
    }

    fn builder_flow_call(
        &mut self,
        method: &str,
        result: Option<String>,
        target: &Expr,
        options: &Expr,
        binding: Option<&str>,
    ) -> Outcome<Call> {
        let target = marker_target(target).ok_or("a call target")?;
        let mut entries = Vec::new();
        let mut discard = None;
        for (option, values) in options_calls(options)? {
            match (option.as_str(), &values[..]) {
                ("argument", [parameter, value]) => {
                    let parameter = string_literal(parameter).ok_or("a parameter")?;
                    let parameter = parameter
                        .strip_prefix(&format!("{target}."))
                        .unwrap_or(&parameter)
                        .to_string();
                    entries.push((parameter, self.builder_value(value)?));
                }
                ("entity_argument", [parameter, entity]) if method == "call_javascript" => {
                    let parameter = string_literal(parameter).ok_or("a parameter")?;
                    let entity = marker_target(entity).ok_or("an entity argument")?;
                    entries.push((parameter, format!("{{ entity: {} }}", json(&entity))));
                }
                ("nanoflow_argument", [parameter, nanoflow]) if method == "call_javascript" => {
                    let parameter = string_literal(parameter).ok_or("a parameter")?;
                    let nanoflow = marker_target(nanoflow).ok_or("a nanoflow argument")?;
                    entries.push((parameter, format!("{{ nanoflow: {} }}", json(&nanoflow))));
                }
                ("discard_result", [name]) => {
                    discard = Some(string_literal(name).ok_or("a discarded result's name")?);
                }
                (other, _) => return Err(format!("call option {other}")),
            }
        }
        match method {
            "call" => self.microflow_call(&target, entries, result, discard, binding),
            "call_nanoflow" => self.nanoflow_call(&target, entries, result, discard, binding),
            _ => self.javascript_call(&target, entries, result, discard, binding),
        }
    }
}

fn bool_literal(expr: &Expr) -> Outcome<String> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Bool(flag),
            ..
        }) => Ok(flag.value.to_string()),
        _ => Err("expected true or false".to_string()),
    }
}

fn sort_word(order: &Expr) -> Outcome<&'static str> {
    let word = match order {
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    };
    match word.as_str() {
        "Ascending" => Ok("asc"),
        "Descending" => Ok("desc"),
        _ => Err("an unexpected sort order".to_string()),
    }
}

/// One service file: its nanoflows, as methods of `nanoflowService`.
pub(crate) fn render_service(
    module: &str,
    service: &str,
    subject: Option<&str>,
    methods: &[Method],
    imports: &[(String, String)],
) -> String {
    let mut vocabulary = BTreeSet::new();
    for method in methods {
        vocabulary.extend(method.vocabulary.iter().cloned());
    }
    let words: Vec<String> = vocabulary
        .iter()
        .map(|word| {
            if word.starts_with("Mx") {
                format!("type {word}")
            } else {
                word.clone()
            }
        })
        .collect();
    let mut out = match subject {
        Some(subject) => format!("// The {module} module's nanoflows about {subject}.\n"),
        None => format!("// The {module} module's nanoflows.\n"),
    };
    let import = format!("import {{ {} }} from \"@/mxrs/flows\";\n", words.join(", "));
    if import.len() <= WIDTH + 1 {
        out.push_str(&import);
    } else {
        out.push_str("import {\n");
        for word in &words {
            out.push_str(&format!("  {word},\n"));
        }
        out.push_str("} from \"@/mxrs/flows\";\n");
    }
    for (name, path) in imports {
        out.push_str(&format!("import {{ {name} }} from \"{path}\";\n"));
    }
    out.push_str(&format!(
        "\nexport const {service} = nanoflowService({}, {{\n",
        json(module)
    ));
    for (index, method) in methods.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&method.text);
    }
    out.push_str("});\n");
    out
}

/// One service file being written: its methods, what it is about and
/// where it goes.
struct ServiceFile {
    methods: Vec<Method>,
    subject: Option<String>,
    file: String,
}

/// The nanoflows the importer declares in the frontend.
pub(crate) struct FrontendNanoflows {
    /// Each service file, by its path under `frontend/src/`.
    pub(crate) files: Vec<(String, String)>,
    /// The nanoflows those files declare, by module.
    pub(crate) declared: std::collections::BTreeMap<String, Vec<String>>,
}

/// Declares in the frontend every nanoflow of an authored module whose
/// TypeScript reads back as the declaration its Rust body declares; the
/// rest stay in Rust. `MXRS_EXPLAIN_FLOWS=1` says why each one stays.
pub(crate) fn declare_in_frontend(
    converted: &[crate::flow_export::ConvertedFlow],
    modules: &[mxrs_model::Module],
    authored: impl Fn(&str) -> bool,
    relations: &HashMap<String, mxrs_model::relations::FlowRelations>,
) -> crate::Result<FrontendNanoflows> {
    use std::collections::BTreeMap;

    let explain = std::env::var_os("MXRS_EXPLAIN_FLOWS").is_some();
    let candidates: Vec<&crate::flow_export::ConvertedFlow> = converted
        .iter()
        .filter(|flow| flow.is_nanoflow() && authored(&flow.module))
        .collect();
    // Each nanoflow's service and method, planned like a microflow's.
    let mut slots: HashMap<(String, String), crate::flow_export::ServiceSlot> = HashMap::new();
    let mut by_module: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for flow in &candidates {
        by_module
            .entry(flow.module.as_str())
            .or_default()
            .push(flow.declaration.name.clone());
    }
    for (module, mut names) in by_module {
        names.sort();
        let entities: Vec<String> = modules
            .iter()
            .find(|candidate| candidate.name.as_deref() == Some(module))
            .map(|found| {
                found
                    .entities()
                    .iter()
                    .filter_map(|entity| entity.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        for (flow, slot) in crate::flow_export::plan_services(module, &names, &entities) {
            slots.insert((module.to_string(), flow), slot);
        }
    }
    let method_of = |slot: &crate::flow_export::ServiceSlot| {
        camel_from_snake(slot.function.trim_start_matches("r#"))
    };
    let mut staying: HashSet<(String, String)> = HashSet::new();
    loop {
        let moving: Vec<&&crate::flow_export::ConvertedFlow> = candidates
            .iter()
            .filter(|flow| !staying.contains(&(flow.module.clone(), flow.declaration.name.clone())))
            .collect();
        let callees: HashMap<String, Callee> = moving
            .iter()
            .filter_map(|flow| {
                let slot = slots.get(&(flow.module.clone(), flow.declaration.name.clone()))?;
                Some((
                    format!("{}.{}", flow.module, flow.declaration.name),
                    Callee {
                        service: slot.service.clone(),
                        method: method_of(slot),
                        parameters: flow
                            .declaration
                            .parameters
                            .iter()
                            .map(|parameter| parameter.name.clone())
                            .collect(),
                    },
                ))
            })
            .collect();
        // Each service's methods, its file and what it imports.
        let mut services: BTreeMap<(String, String), ServiceFile> = BTreeMap::new();
        let mut owners: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut dropped = false;
        for flow in &moving {
            let key = (flow.module.clone(), flow.declaration.name.clone());
            let Some(slot) = slots.get(&key) else {
                staying.insert(key);
                dropped = true;
                continue;
            };
            let qualified = format!("{}.{}", flow.module, flow.declaration.name);
            let roles: Option<Vec<String>> = relations
                .get(&qualified)
                .map(|related| related.roles.iter().cloned().collect::<Vec<_>>())
                .filter(|roles| !roles.is_empty());
            match translate(
                &flow.module,
                &slot.service,
                &method_of(slot),
                &flow.declaration,
                Said {
                    roles: roles.as_deref(),
                    folder: flow.folder.as_deref(),
                },
                flow.body(),
                &callees,
            ) {
                Ok(method) => {
                    let file = format!(
                        "services/{}/{}",
                        crate::module_stem(&flow.module),
                        mxrs_frontend::naming::service_file(&slot.service)
                    );
                    owners.entry(file.clone()).or_default().push(key.clone());
                    services
                        .entry((flow.module.clone(), slot.service.clone()))
                        .or_insert_with(|| ServiceFile {
                            methods: Vec::new(),
                            subject: slot
                                .subject
                                .as_ref()
                                .map(|subject| subject.text().to_string()),
                            file,
                        })
                        .methods
                        .push(method);
                }
                Err(reason) => {
                    if explain {
                        eprintln!("[mxrs] nanoflow {qualified} stays in Rust: {reason}");
                    }
                    staying.insert(key);
                    dropped = true;
                }
            }
        }
        if dropped {
            continue;
        }
        let paths: HashMap<String, String> = services
            .iter()
            .map(|((_, service), ServiceFile { file, .. })| {
                (
                    service.clone(),
                    format!("@/{}", file.trim_end_matches(".ts")),
                )
            })
            .collect();
        let mut files = Vec::new();
        for (
            (module, service),
            ServiceFile {
                methods,
                subject,
                file,
            },
        ) in &services
        {
            let mut imports: BTreeSet<String> = BTreeSet::new();
            for method in methods {
                imports.extend(method.services.iter().cloned());
            }
            let imports: Vec<(String, String)> = imports
                .into_iter()
                .filter_map(|name| Some((name.clone(), paths.get(&name)?.clone())))
                .collect();
            files.push((
                file.clone(),
                render_service(module, service, subject.as_deref(), methods, &imports),
            ));
        }
        // What the frontend's reader makes of them is what a build will
        // declare: each has to be the declaration its Rust body declares.
        let checked =
            tempfile::tempdir().map_err(|source| crate::io_error(Path::new("."), source))?;
        for (file, source) in &files {
            let path = checked.path().join("src").join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|source| crate::io_error(parent, source))?;
            }
            std::fs::write(&path, source).map_err(|source| crate::io_error(&path, source))?;
        }
        let read = match mxrs_frontend::read_frontend(checked.path()) {
            Ok(read) => read,
            Err(error) => {
                // The file it names stays in Rust whole.
                let message = error.to_string();
                let culprit = owners
                    .iter()
                    .find(|(file, _)| message.contains(file.as_str()))
                    .map(|(_, flows)| flows.clone())
                    .unwrap_or_else(|| owners.values().flatten().cloned().collect());
                for key in culprit {
                    if explain {
                        eprintln!(
                            "[mxrs] nanoflow {}.{} stays in Rust: {message}",
                            key.0, key.1
                        );
                    }
                    staying.insert(key);
                }
                continue;
            }
        };
        let read: HashMap<(String, String), MicroflowDecl> = read
            .nanoflows
            .into_iter()
            .map(|(module, declaration)| ((module, declaration.name.clone()), declaration))
            .collect();
        for flow in &moving {
            let key = (flow.module.clone(), flow.declaration.name.clone());
            let qualified = format!("{}.{}", key.0, key.1);
            let mut expected = flow.declaration.clone();
            expected.relations = Default::default();
            // A service's method says who may run it: no one in
            // particular when its comment names no role.
            expected.allowed_roles = Some(
                relations
                    .get(&qualified)
                    .map(|related| related.roles.iter().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            );
            let same = read
                .get(&key)
                .is_some_and(|found| format!("{found:?}") == format!("{expected:?}"));
            if !same {
                if explain {
                    eprintln!(
                        "[mxrs] nanoflow {qualified} stays in Rust: its TypeScript reads back differently"
                    );
                }
                staying.insert(key);
                dropped = true;
            }
        }
        if dropped {
            continue;
        }
        let mut declared: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for flow in moving {
            declared
                .entry(flow.module.clone())
                .or_default()
                .push(flow.declaration.name.clone());
        }
        return Ok(FrontendNanoflows { files, declared });
    }
}
