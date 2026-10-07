//! `frontend/src/services/**/*.ts`: the nanoflows the frontend declares.
//!
//! A service is `export const XService = nanoflowService("Module", { ... })`
//! and each of its methods is a nanoflow: its documentation comment names it
//! (`@nanoflow ACT_Order_Open`) and says who may run it (`@roles`), its
//! parameters and return type are its signature, and its body is
//! TypeScript's own control flow with an `await` per activity, in the
//! vocabulary of `src/mxrs/flows.ts`. The body is read, not run: each
//! statement is the flow builder call it stands for.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mxrs_dsl::{
    AggregateFunction, Commit, FlowBuilder, ListChange, LogSeverity, MemberName, MessageKind,
    NanoflowModuleBuilder, SortOrder, var,
};
use mxrs_expr::{Mx, mx};
use mxrs_ir::flow::{DataType, MicroflowDecl};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BindingPattern, Expression, ForStatementLeft,
    ObjectPropertyKind, PropertyKey, Statement, TSType, TSTypeName,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

use crate::FrontendError;
use crate::naming::pascal;

/// A nanoflow a service declares: its module and its declaration.
pub(crate) type Declared = (String, MicroflowDecl);

/// One option a retrieve states, applied when the builder asks.
type RetrieveCall = Box<dyn Fn(&mut mxrs_dsl::flow_actions::RetrieveOptions<'_>)>;

/// What a call to a service method needs: the nanoflow and its parameters.
#[derive(Clone)]
struct Signature {
    qualified: String,
    parameters: Vec<String>,
}

/// Every service's methods, by service and method name.
type Registry = HashMap<String, HashMap<String, Signature>>;

/// The `.ts` files under `services`, in a stable order.
fn service_files(services: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut folders = vec![services.to_path_buf()];
    // A link back to a folder already read would be read forever.
    let mut seen = std::collections::BTreeSet::new();
    while let Some(folder) = folders.pop() {
        if !seen.insert(std::fs::canonicalize(&folder).unwrap_or_else(|_| folder.clone())) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|extension| extension == "ts")
                && !path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    [".d.ts", ".test.ts", ".spec.ts"]
                        .iter()
                        .any(|suffix| name.ends_with(suffix))
                })
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The nanoflows the services under `services` declare.
#[cfg(test)]
pub(crate) fn read(services: &Path) -> Result<Vec<Declared>, FrontendError> {
    Ok(read_with_origins(services)?.0)
}

/// Where a declaration is: its file and line.
fn origin(path: &Path, source: &str, span: Span) -> String {
    let start = (span.start as usize).min(source.len());
    format!(
        "{}:{}",
        path.display(),
        source[..start].matches('\n').count() + 1
    )
}

/// The nanoflows the services under `services` declare, and where each is
/// declared, by its qualified name.
pub(crate) fn read_with_origins(
    services: &Path,
) -> Result<(Vec<Declared>, HashMap<String, String>), FrontendError> {
    let mut sources = Vec::new();
    for path in service_files(services) {
        let source = std::fs::read_to_string(&path).map_err(|source| FrontendError::Io {
            path: path.display().to_string(),
            source,
        })?;
        sources.push((path, source));
    }
    // What each service declares first, so a call reads whichever file its
    // service is in.
    let mut registry = Registry::new();
    // Where each service and each nanoflow is declared: a second one of the
    // same name is refused, saying where the first is.
    let mut service_origins: HashMap<String, String> = HashMap::new();
    let mut origins: HashMap<String, String> = HashMap::new();
    for (path, source) in &sources {
        let allocator = Allocator::default();
        let program = parse(&allocator, path, source)?;
        for service in services_in(&program, path, source)? {
            // A module's services are in the module's folder: a module
            // named otherwise is a module of its own, made by a typo.
            let folder = path
                .strip_prefix(services)
                .ok()
                .and_then(|relative| relative.components().next())
                .map(|folder| folder.as_os_str().to_string_lossy().to_string())
                .filter(|_| path.parent() != Some(services));
            if let Some(folder) = folder
                && !crate::naming::is_module_folder(&folder, &service.module)
            {
                return Err(at(
                    path,
                    source,
                    service.span,
                    &format!(
                        "a service of the module whose folder `{folder}` is: {:?} has its own",
                        service.module
                    ),
                ));
            }
            if let Some(first) =
                service_origins.insert(service.name.clone(), origin(path, source, service.span))
            {
                return Err(at(
                    path,
                    source,
                    service.span,
                    &format!(
                        "a service of its own name: {first} declares {}",
                        service.name
                    ),
                ));
            }
            let mut methods = HashMap::new();
            for method in &service.methods {
                let qualified = format!("{}.{}", service.module, method.nanoflow);
                if let Some(first) =
                    origins.insert(qualified.clone(), origin(path, source, method.span))
                {
                    return Err(at(
                        path,
                        source,
                        method.span,
                        &format!("a nanoflow of its own name: {first} declares {qualified}"),
                    ));
                }
                if methods
                    .insert(
                        method.name.clone(),
                        Signature {
                            qualified,
                            parameters: method
                                .parameters
                                .iter()
                                .map(|p| p.mendix.clone())
                                .collect(),
                        },
                    )
                    .is_some()
                {
                    return Err(at(path, source, method.span, "each method once"));
                }
            }
            registry.insert(service.name.clone(), methods);
        }
    }
    let mut declared = Vec::new();
    for (path, source) in &sources {
        let allocator = Allocator::default();
        let program = parse(&allocator, path, source)?;
        for service in services_in(&program, path, source)? {
            for method in &service.methods {
                let reader = Reader {
                    path,
                    source,
                    module: &service.module,
                    registry: &registry,
                    state: RefCell::new(State::default()),
                };
                let declaration = reader.method(method)?;
                declared.push((service.module.clone(), declaration));
            }
        }
    }
    Ok((declared, origins))
}

fn parse<'a>(
    allocator: &'a Allocator,
    path: &Path,
    source: &'a str,
) -> Result<oxc_ast::ast::Program<'a>, FrontendError> {
    let parsed = Parser::new(allocator, source, SourceType::ts()).parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(crate::syntax_error(
            &path.display().to_string(),
            source,
            error,
        ));
    }
    Ok(parsed.program)
}

struct Service<'p, 'a> {
    name: String,
    module: String,
    span: Span,
    methods: Vec<Method<'p, 'a>>,
}

struct Parameter {
    ident: String,
    mendix: String,
    ty: DataType,
    required: bool,
    documentation: String,
    default: Option<String>,
}

struct Method<'p, 'a> {
    name: String,
    nanoflow: String,
    documentation: String,
    /// Who may run it: no one in particular when its comment names no role.
    roles: Vec<String>,
    parameters: Vec<Parameter>,
    returns: Option<DataType>,
    span: Span,
    body: &'p [Statement<'a>],
}

fn at(path: &Path, source: &str, span: Span, expected: &str) -> FrontendError {
    let start = (span.start as usize).min(source.len());
    let end = (span.end as usize).clamp(start, source.len());
    FrontendError::Unsupported {
        path: path.display().to_string(),
        line: source[..start].matches('\n').count() + 1,
        expected: expected.to_string(),
        found: source[start..end].chars().take(60).collect(),
    }
}

/// The `nanoflowService(...)` exports of a file.
fn services_in<'p, 'a>(
    program: &'p oxc_ast::ast::Program<'a>,
    path: &Path,
    source: &str,
) -> Result<Vec<Service<'p, 'a>>, FrontendError> {
    let refuse = |span: Span, expected: &str| at(path, source, span, expected);
    let mut services = Vec::new();
    // A service file declares services, and a service is declared one way:
    // anything else would be a nanoflow the build does not see.
    let declares = "`export const XService = nanoflowService(\"Module\", { ... })`";
    for statement in &program.body {
        let export = match statement {
            Statement::ImportDeclaration(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_) => continue,
            Statement::ExportDeclaration(export) => export,
            other => return Err(refuse(other.span(), declares)),
        };
        let declaration = match &export.declaration {
            oxc_ast::ast::Declaration::VariableDeclaration(declaration) => declaration,
            oxc_ast::ast::Declaration::TSTypeAliasDeclaration(_)
            | oxc_ast::ast::Declaration::TSInterfaceDeclaration(_) => continue,
            _ => return Err(refuse(export.span, declares)),
        };
        if declaration.kind != oxc_ast::ast::VariableDeclarationKind::Const {
            return Err(refuse(declaration.span, declares));
        }
        for declarator in &declaration.declarations {
            let Some(Expression::CallExpression(call)) = &declarator.init else {
                return Err(refuse(declarator.span, declares));
            };
            let Expression::Identifier(callee) = &call.callee else {
                return Err(refuse(declarator.span, declares));
            };
            if callee.name != "nanoflowService" {
                return Err(refuse(declarator.span, declares));
            }
            let BindingPattern::BindingIdentifier(name) = &declarator.id else {
                return Err(refuse(declarator.span, "a service's name"));
            };
            let [module, flows] = call.arguments.as_slice() else {
                return Err(refuse(call.span, "nanoflowService(\"Module\", { ... })"));
            };
            let Some(Expression::StringLiteral(module)) = module.as_expression() else {
                return Err(refuse(module.span(), "the module's name"));
            };
            let Some(Expression::ObjectExpression(flows)) = flows.as_expression() else {
                return Err(refuse(flows.span(), "the service's methods"));
            };
            let mut methods = Vec::new();
            for property in &flows.properties {
                let ObjectPropertyKind::ObjectProperty(property) = property else {
                    return Err(refuse(property.span(), "a method"));
                };
                let Expression::FunctionExpression(function) = &property.value else {
                    return Err(refuse(property.span, "an async method"));
                };
                if !property.method
                    || !function.r#async
                    || function.generator
                    || function.type_parameters.is_some()
                    || function.this_param.is_some()
                {
                    return Err(refuse(property.span, "an async method"));
                }
                let Some(name) = property.key.static_name() else {
                    return Err(refuse(property.key.span(), "a method name"));
                };
                let (doc, doc_line) = documentation(program, source, property.span.start)
                    .ok_or_else(|| {
                        refuse(
                            property.span,
                            "a documentation comment naming the nanoflow (`@nanoflow ...`)",
                        )
                    })?;
                let tags = Tags::parse(&doc).map_err(|(line, detail)| FrontendError::Shape {
                    path: path.display().to_string(),
                    line: doc_line + line,
                    detail,
                })?;
                let nanoflow = tags
                    .nanoflow
                    .clone()
                    .ok_or_else(|| refuse(property.span, "`@nanoflow <Name>` in its comment"))?;
                if let Some(rest) = &function.params.rest {
                    return Err(refuse(rest.span, "named parameters, without a rest"));
                }
                let mut parameters = Vec::new();
                for parameter in &function.params.items {
                    if let Some(initializer) = &parameter.initializer {
                        return Err(refuse(
                            initializer.span(),
                            "no default here: `@defaultValue <parameter> <expression>` in the comment",
                        ));
                    }
                    let BindingPattern::BindingIdentifier(ident) = &parameter.pattern else {
                        return Err(refuse(parameter.span, "a named parameter"));
                    };
                    let ident = ident.name.to_string();
                    let annotation = parameter
                        .type_annotation
                        .as_ref()
                        .ok_or_else(|| refuse(parameter.span, "a parameter's type"))?;
                    let (ty, undefined) =
                        data_type(&annotation.type_annotation).ok_or_else(|| {
                            refuse(
                                annotation.span,
                                "a Mendix type: MxObject<\"Module.Entity\">, string, ...",
                            )
                        })?;
                    parameters.push(Parameter {
                        mendix: tags
                            .names
                            .get(&ident)
                            .cloned()
                            .unwrap_or_else(|| pascal(&ident)),
                        required: !(parameter.optional || undefined),
                        documentation: tags.parameters.get(&ident).cloned().unwrap_or_default(),
                        default: tags.defaults.get(&ident).cloned(),
                        ident,
                        ty,
                    });
                }
                for ident in tags
                    .parameters
                    .keys()
                    .chain(tags.names.keys())
                    .chain(tags.defaults.keys())
                {
                    if !parameters
                        .iter()
                        .any(|parameter: &Parameter| &parameter.ident == ident)
                    {
                        return Err(refuse(
                            property.span,
                            &format!("a parameter `{ident}`, which its comment names"),
                        ));
                    }
                }
                let returns = match &function.return_type {
                    None => {
                        return Err(refuse(
                            function.params.span,
                            "its return type after the parameters: `Promise<void>` or `Promise<a Mendix type>`",
                        ));
                    }
                    Some(annotation) => promised(&annotation.type_annotation)
                        .ok_or_else(|| refuse(annotation.span, "Promise<a Mendix type>"))?,
                };
                let body = function
                    .body
                    .as_ref()
                    .ok_or_else(|| refuse(function.span, "a body"))?;
                methods.push(Method {
                    name: name.to_string(),
                    nanoflow,
                    documentation: tags.documentation,
                    roles: tags
                        .roles
                        .unwrap_or_default()
                        .into_iter()
                        .map(|role| {
                            if role.contains('.') {
                                role
                            } else {
                                format!("{}.{role}", module.value)
                            }
                        })
                        .collect(),
                    parameters,
                    returns,
                    span: property.key.span(),
                    body: &body.statements,
                });
            }
            services.push(Service {
                name: name.name.to_string(),
                module: module.value.to_string(),
                span: declarator.span,
                methods,
            });
        }
    }
    Ok(services)
}

/// The text of the `/** ... */` comment right before `start`, and the line
/// its first line is on.
fn documentation(
    program: &oxc_ast::ast::Program<'_>,
    source: &str,
    start: u32,
) -> Option<(String, usize)> {
    let comment = program
        .comments
        .iter()
        .rfind(|comment| comment.span.end <= start && comment.is_block())?;
    let between = &source[comment.span.end as usize..start as usize];
    if !between.trim().is_empty() {
        return None;
    }
    let text = &source[comment.span.start as usize..comment.span.end as usize];
    let text = text.strip_prefix("/**")?.strip_suffix("*/")?;
    let lines: Vec<&str> = text
        .lines()
        .map(|line| {
            let line = line.trim_start();
            let line = line.strip_prefix('*').unwrap_or(line);
            line.strip_prefix(' ').unwrap_or(line).trim_end()
        })
        .collect();
    let start = lines.iter().position(|line| !line.is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(0, |end| end + 1);
    let first = source[..comment.span.start as usize].matches('\n').count() + 1;
    Some((lines[start..end.max(start)].join("\n"), first + start))
}

/// What a method's comment says.
#[derive(Default)]
struct Tags {
    documentation: String,
    nanoflow: Option<String>,
    roles: Option<Vec<String>>,
    parameters: HashMap<String, String>,
    names: HashMap<String, String>,
    defaults: HashMap<String, String>,
}

/// Whether `name` is one a model gives: letters, digits and `_`, not
/// opening with a digit.
fn is_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Tags {
    /// What a comment says, or the line of it — counted from its first —
    /// that says something no tag does.
    fn parse(comment: &str) -> Result<Self, (usize, String)> {
        let mut tags = Tags::default();
        let mut documentation = Vec::new();
        for (index, line) in comment.lines().enumerate() {
            let Some(tag) = line.strip_prefix('@') else {
                documentation.push(line);
                continue;
            };
            let refuse = |detail: &str| Err((index, format!("`{line}`: expected {detail}")));
            let (name, rest) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
            let rest = rest.trim();
            let (ident, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let value = value.trim();
            let once = |taken: bool| {
                if taken {
                    Err((index, format!("`@{name}` is said twice")))
                } else {
                    Ok(())
                }
            };
            match name {
                "nanoflow" => {
                    once(tags.nanoflow.is_some())?;
                    if !is_name(rest) {
                        return refuse("`@nanoflow <Name>`, the nanoflow's name in the model");
                    }
                    tags.nanoflow = Some(rest.to_string());
                }
                "roles" => {
                    once(tags.roles.is_some())?;
                    let roles: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
                    let named = |role: &String| match role.split_once('.') {
                        Some((module, role)) => is_name(module) && is_name(role),
                        None => is_name(role),
                    };
                    if !roles.iter().all(named) {
                        return refuse(
                            "`@roles` and module roles between spaces: `User` or `Module.User`",
                        );
                    }
                    tags.roles = Some(roles);
                }
                "param" => {
                    if !is_name(ident) {
                        return refuse("`@param <parameter> <what it is>`");
                    }
                    once(
                        tags.parameters
                            .insert(ident.to_string(), value.to_string())
                            .is_some(),
                    )?;
                }
                "mendixName" => {
                    if !is_name(ident) || !is_name(value) {
                        return refuse("`@mendixName <parameter> <Name>`");
                    }
                    once(
                        tags.names
                            .insert(ident.to_string(), value.to_string())
                            .is_some(),
                    )?;
                }
                "defaultValue" => {
                    if !is_name(ident) || value.is_empty() {
                        return refuse("`@defaultValue <parameter> <expression>`");
                    }
                    once(
                        tags.defaults
                            .insert(ident.to_string(), value.to_string())
                            .is_some(),
                    )?;
                }
                _ => {
                    return refuse(
                        "a tag of a nanoflow: @nanoflow, @roles, @param, @mendixName or @defaultValue",
                    );
                }
            }
        }
        while documentation.last().is_some_and(|line| line.is_empty()) {
            documentation.pop();
        }
        tags.documentation = documentation.join("\n");
        Ok(tags)
    }
}

/// A Mendix type, and whether it admits `undefined`.
fn data_type(ty: &TSType<'_>) -> Option<(DataType, bool)> {
    match ty {
        TSType::TSStringKeyword(_) => Some((DataType::String, false)),
        TSType::TSBooleanKeyword(_) => Some((DataType::Boolean, false)),
        TSType::TSUnionType(union) => {
            let [ty, TSType::TSUndefinedKeyword(_)] = union.types.as_slice() else {
                return None;
            };
            Some((data_type(ty)?.0, true))
        }
        TSType::TSTypeReference(reference) => {
            let TSTypeName::IdentifierReference(name) = &reference.type_name else {
                return None;
            };
            let argument = || -> Option<String> {
                let [TSType::TSLiteralType(literal)] =
                    reference.type_arguments.as_ref()?.params.as_slice()
                else {
                    return None;
                };
                match &literal.literal {
                    oxc_ast::ast::TSLiteral::StringLiteral(text) => Some(text.value.to_string()),
                    _ => None,
                }
            };
            Some((
                match name.name.as_str() {
                    "MxObject" => DataType::Object(argument()?),
                    "MxList" => DataType::List(argument()?),
                    "MxEnum" => DataType::Enumeration(argument()?),
                    "MxInteger" => DataType::Integer,
                    "MxLong" => DataType::Long,
                    "MxDecimal" => DataType::Decimal,
                    "MxFloat" => DataType::Float,
                    "MxDateTime" => DataType::DateTime,
                    "MxBinary" => DataType::Binary,
                    _ => return None,
                },
                false,
            ))
        }
        _ => None,
    }
}

/// `Promise<T>`: `T`, or nothing for `void`.
fn promised(ty: &TSType<'_>) -> Option<Option<DataType>> {
    let TSType::TSTypeReference(reference) = ty else {
        return None;
    };
    let TSTypeName::IdentifierReference(name) = &reference.type_name else {
        return None;
    };
    if name.name != "Promise" {
        return None;
    }
    let [inner] = reference.type_arguments.as_ref()?.params.as_slice() else {
        return None;
    };
    match inner {
        TSType::TSVoidKeyword(_) => Some(None),
        other => Some(Some(data_type(other)?.0)),
    }
}

/// A variable in scope: its Mendix name, and the entity it holds when known.
#[derive(Clone)]
struct Variable {
    mendix: String,
    entity: Option<String>,
}

#[derive(Default)]
struct State {
    scopes: Vec<HashMap<String, Variable>>,
    /// The loops and switches being read, the innermost last.
    frames: Vec<Frame>,
    /// Whether the nanoflow returns a value.
    returns: bool,
    error: Option<FrontendError>,
}

/// What a `break` or `continue` can be inside.
enum Frame {
    /// A loop, with its label.
    Loop(Option<String>),
    Switch,
}

struct Reader<'s> {
    path: &'s Path,
    source: &'s str,
    module: &'s str,
    registry: &'s Registry,
    state: RefCell<State>,
}

type Read<T> = Result<T, FrontendError>;

/// What an activity call is bound to.
struct Binding<'b> {
    ident: Option<&'b str>,
}

impl Reader<'_> {
    fn refuse(&self, span: Span, expected: &str) -> FrontendError {
        at(self.path, self.source, span, expected)
    }

    /// Notes the first error a builder closure met.
    fn fail(&self, error: FrontendError) {
        let mut state = self.state.borrow_mut();
        if state.error.is_none() {
            state.error = Some(error);
        }
    }

    fn failed(&self) -> bool {
        self.state.borrow().error.is_some()
    }

    /// Declares `ident` as the variable `mendix`. A flow names each of its
    /// variables once wherever the other is still in scope.
    fn declare(&self, ident: &str, mendix: &str, entity: Option<String>, span: Span) -> Read<()> {
        let mut state = self.state.borrow_mut();
        let taken = state.scopes.iter().any(|scope| {
            scope
                .iter()
                .any(|(known, variable)| known == ident || variable.mendix == mendix)
        });
        if taken {
            drop(state);
            return Err(self.refuse(
                span,
                &format!("a variable of its own name: `${mendix}` is already in scope"),
            ));
        }
        state.scopes.last_mut().expect("a scope is open").insert(
            ident.to_string(),
            Variable {
                mendix: mendix.to_string(),
                entity,
            },
        );
        Ok(())
    }

    fn lookup(&self, ident: &str) -> Option<Variable> {
        self.state
            .borrow()
            .scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(ident).cloned())
    }

    fn method(&self, method: &Method<'_, '_>) -> Read<MicroflowDecl> {
        self.state.borrow_mut().scopes.push(HashMap::new());
        self.state.borrow_mut().returns = method.returns.is_some();
        for parameter in &method.parameters {
            let entity = match &parameter.ty {
                DataType::Object(entity) | DataType::List(entity) => Some(entity.clone()),
                _ => None,
            };
            self.declare(&parameter.ident, &parameter.mendix, entity, method.span)?;
        }
        let mut module = NanoflowModuleBuilder::new(self.module);
        module.nanoflow(method.nanoflow.as_str(), |flow| {
            flow.documentation(method.documentation.as_str());
            flow.allowed_roles(method.roles.iter().map(String::as_str));
            for parameter in &method.parameters {
                flow.parameter_of(parameter.mendix.as_str(), parameter.ty.clone(), |options| {
                    if !parameter.documentation.is_empty() {
                        options.documentation(parameter.documentation.as_str());
                    }
                    if parameter.required {
                        options.required(true);
                    }
                    if let Some(default) = &parameter.default {
                        options.default_value(mx(default.as_str()));
                    }
                });
            }
            if let Some(ty) = &method.returns {
                flow.returns(ty.clone());
            }
            if let Err(error) = self.statements(flow, method.body) {
                self.fail(error);
            }
        });
        if let Some(error) = self.state.borrow_mut().error.take() {
            return Err(error);
        }
        let mut declared = module.into_decl();
        declared
            .nanoflows
            .pop()
            .ok_or_else(|| FrontendError::Syntax {
                path: self.path.display().to_string(),
                detail: "a nanoflow the builder did not declare".to_string(),
            })
    }

    /// A block with a scope of its own.
    fn nested(&self, flow: &mut FlowBuilder, statements: &[Statement<'_>]) {
        if self.failed() {
            return;
        }
        self.state.borrow_mut().scopes.push(HashMap::new());
        if let Err(error) = self.statements(flow, statements) {
            self.fail(error);
        }
        self.state.borrow_mut().scopes.pop();
    }

    fn statements(&self, flow: &mut FlowBuilder, statements: &[Statement<'_>]) -> Read<()> {
        let mut ended = false;
        for statement in statements {
            let label = Self::helper_call(statement) == Some("label");
            if ended && !label {
                return Err(self.refuse(
                    statement.span(),
                    "nothing after what ends the path, unless a `label(...)` is jumped to",
                ));
            }
            self.statement(flow, statement)?;
            if self.failed() {
                break;
            }
            ended = Self::ends_path(statement);
        }
        Ok(())
    }

    /// The path helper a statement calls: `label`, `jump` or `raiseError`.
    fn helper_call<'p>(statement: &'p Statement<'_>) -> Option<&'p str> {
        let Statement::ExpressionStatement(expression) = statement else {
            return None;
        };
        let Expression::CallExpression(call) = &expression.expression else {
            return None;
        };
        let Expression::Identifier(callee) = &call.callee else {
            return None;
        };
        match callee.name.as_str() {
            name @ ("label" | "jump" | "raiseError") => Some(name),
            _ => None,
        }
    }

    /// Whether nothing runs after `statement` in its block.
    fn ends_path(statement: &Statement<'_>) -> bool {
        matches!(
            statement,
            Statement::ReturnStatement(_)
                | Statement::BreakStatement(_)
                | Statement::ContinueStatement(_)
        ) || matches!(Self::helper_call(statement), Some("jump" | "raiseError"))
    }

    fn block<'p, 'a>(&self, statement: &'p Statement<'a>) -> Read<&'p [Statement<'a>]> {
        match statement {
            Statement::BlockStatement(block) => Ok(&block.body),
            other => Err(self.refuse(other.span(), "a block `{ ... }`")),
        }
    }

    fn statement(&self, flow: &mut FlowBuilder, statement: &Statement<'_>) -> Read<()> {
        match statement {
            Statement::ExpressionStatement(expression) => match &expression.expression {
                Expression::AwaitExpression(awaited) => {
                    self.activity(flow, &awaited.argument, Binding { ident: None })
                }
                Expression::CallExpression(call) => {
                    let Expression::Identifier(callee) = &call.callee else {
                        return Err(self.refuse(call.span, "an activity, awaited"));
                    };
                    match (callee.name.as_str(), call.arguments.as_slice()) {
                        ("label", [name]) => {
                            flow.label(self.text(name)?);
                        }
                        ("jump", [name]) => {
                            flow.jump(self.text(name)?);
                        }
                        ("raiseError", []) => {
                            flow.raise_error();
                        }
                        _ => return Err(self.refuse(call.span, "an activity, awaited")),
                    }
                    Ok(())
                }
                other => Err(self.refuse(other.span(), "an activity")),
            },
            Statement::VariableDeclaration(declaration) => {
                let [declarator] = declaration.declarations.as_slice() else {
                    return Err(self.refuse(declaration.span, "one variable"));
                };
                if declaration.kind != oxc_ast::ast::VariableDeclarationKind::Const {
                    return Err(self.refuse(declaration.span, "`const`"));
                }
                let BindingPattern::BindingIdentifier(ident) = &declarator.id else {
                    return Err(self.refuse(declarator.span, "a variable's name"));
                };
                let Some(Expression::AwaitExpression(awaited)) = &declarator.init else {
                    return Err(self.refuse(declarator.span, "an awaited activity"));
                };
                self.activity(
                    flow,
                    &awaited.argument,
                    Binding {
                        ident: Some(ident.name.as_str()),
                    },
                )
            }
            Statement::IfStatement(decision) => {
                if matches!(
                    decision.test.without_parentheses(),
                    Expression::StringLiteral(_) | Expression::NumericLiteral(_)
                ) {
                    return Err(self.refuse(decision.test.span(), "a condition: true or false"));
                }
                let condition = self.value(&decision.test)?;
                let then = self.block(&decision.consequent)?;
                let otherwise: &[Statement<'_>] = match &decision.alternate {
                    None => &[],
                    Some(alternate) => self.block(alternate)?,
                };
                flow.decision(
                    condition,
                    |flow| self.nested(flow, then),
                    |flow| self.nested(flow, otherwise),
                );
                Ok(())
            }
            Statement::SwitchStatement(switch) => self.switch(flow, switch),
            Statement::ForOfStatement(each) => self.for_each(flow, each, None),
            Statement::WhileStatement(each) => self.while_loop(flow, each, None),
            Statement::LabeledStatement(labelled) => match &labelled.body {
                Statement::ForOfStatement(each) => {
                    self.for_each(flow, each, Some(labelled.label.name.to_string()))
                }
                Statement::WhileStatement(each) => {
                    self.while_loop(flow, each, Some(labelled.label.name.to_string()))
                }
                other => Err(self.refuse(other.span(), "a loop")),
            },
            Statement::ReturnStatement(returned) => {
                let returns = self.state.borrow().returns;
                match &returned.argument {
                    None if returns => {
                        return Err(self.refuse(
                            returned.span,
                            "the value the nanoflow returns: its type is not `Promise<void>`",
                        ));
                    }
                    None => {
                        flow.end();
                    }
                    // A model can store a value where its nanoflow returns
                    // none; `mx<void>(...)` says so on purpose.
                    Some(value) if !returns && !Self::stored_only(value) => {
                        return Err(self.refuse(
                            value.span(),
                            "no value: the nanoflow returns none (`Promise<void>`)",
                        ));
                    }
                    Some(value) => {
                        flow.return_with(self.value(value)?);
                    }
                }
                Ok(())
            }
            Statement::BreakStatement(stop) => {
                self.leaves_loop(
                    stop.span,
                    stop.label.as_ref().map(|label| label.name.as_str()),
                    true,
                )?;
                flow.break_loop();
                Ok(())
            }
            Statement::ContinueStatement(next) => {
                self.leaves_loop(
                    next.span,
                    next.label.as_ref().map(|label| label.name.as_str()),
                    false,
                )?;
                flow.continue_loop();
                Ok(())
            }
            other => Err(self.refuse(other.span(), "a statement a nanoflow can say")),
        }
    }

    /// Whether `value` is `mx<void>("...")`: an expression the model stores
    /// as what a nanoflow returns although it returns nothing.
    fn stored_only(value: &Expression<'_>) -> bool {
        let Expression::CallExpression(call) = value.without_parentheses() else {
            return false;
        };
        matches!(&call.callee, Expression::Identifier(callee) if callee.name == "mx")
            && call.type_arguments.as_ref().is_some_and(|arguments| {
                matches!(arguments.params.as_slice(), [TSType::TSVoidKeyword(_)])
            })
    }

    /// Checks that a `break` or `continue` is about the loop it is in: the
    /// only one a flow can break or continue.
    fn leaves_loop(&self, span: Span, label: Option<&str>, breaks: bool) -> Read<()> {
        let state = self.state.borrow();
        let innermost = state.frames.iter().rev().find_map(|frame| match frame {
            Frame::Loop(label) => Some(label.as_deref()),
            Frame::Switch => None,
        });
        let Some(own) = innermost else {
            return Err(self.refuse(
                span,
                if breaks {
                    "a loop to break"
                } else {
                    "a loop to continue"
                },
            ));
        };
        match label {
            Some(label) if own != Some(label) => Err(self.refuse(
                span,
                "the label of the loop it is in: a flow leaves only that one",
            )),
            None if breaks && matches!(state.frames.last(), Some(Frame::Switch)) => Err(self
                .refuse(
                    span,
                    "the loop's label: inside a `switch`, `break` alone leaves the switch",
                )),
            _ => Ok(()),
        }
    }

    fn switch(
        &self,
        flow: &mut FlowBuilder,
        switch: &oxc_ast::ast::SwitchStatement<'_>,
    ) -> Read<()> {
        let on = self.value(&switch.discriminant)?;
        // Each branch: its values and its statements, without the `break`
        // that ends it.
        let mut branches: Vec<(Vec<Option<String>>, &[Statement<'_>])> = Vec::new();
        let mut values = Vec::new();
        let last = switch.cases.len().saturating_sub(1);
        for (index, case) in switch.cases.iter().enumerate() {
            let value = match &case.test {
                Some(Expression::StringLiteral(text)) => Some(text.value.to_string()),
                Some(Expression::NullLiteral(_)) => None,
                Some(other) => return Err(self.refuse(other.span(), "a case value: text or null")),
                None => return Err(self.refuse(case.span, "a case value: text or null")),
            };
            values.push(value);
            if case.consequent.is_empty() {
                continue;
            }
            let body: &[Statement<'_>] = match case.consequent.as_slice() {
                [Statement::BlockStatement(block)] => &block.body,
                [
                    Statement::BlockStatement(block),
                    Statement::BreakStatement(stop),
                ] if stop.label.is_none() => &block.body,
                other => other,
            };
            let ended_outside = matches!(
                case.consequent.as_slice(),
                [Statement::BlockStatement(_), Statement::BreakStatement(stop)] if stop.label.is_none()
            );
            let body = match body {
                [rest @ .., Statement::BreakStatement(stop)]
                    if stop.label.is_none() && !ended_outside =>
                {
                    rest
                }
                // TypeScript would go on into the next case; a flow's cases
                // are apart.
                other
                    if !ended_outside
                        && index != last
                        && !other.last().is_some_and(Self::ends_path) =>
                {
                    return Err(self.refuse(
                        case.span,
                        "a case that ends — `break`, `return`, `continue`, `jump(...)` or `raiseError()`: cases do not fall through",
                    ));
                }
                other => other,
            };
            branches.push((std::mem::take(&mut values), body));
        }
        if !values.is_empty() {
            return Err(self.refuse(switch.span, "a body for every case"));
        }
        self.state.borrow_mut().frames.push(Frame::Switch);
        flow.switch(on, |cases| {
            for (values, body) in &branches {
                match values.as_slice() {
                    [None] => {
                        cases.empty(|flow| self.nested(flow, body));
                    }
                    [Some(value)] => {
                        cases.case(value.as_str(), |flow| self.nested(flow, body));
                    }
                    several => {
                        if several.iter().any(Option::is_none) {
                            self.fail(self.refuse(switch.span, "`case null` alone"));
                            return;
                        }
                        cases.cases(several.iter().flatten().map(String::as_str), |flow| {
                            self.nested(flow, body)
                        });
                    }
                }
            }
        });
        self.state.borrow_mut().frames.pop();
        Ok(())
    }

    fn for_each(
        &self,
        flow: &mut FlowBuilder,
        each: &oxc_ast::ast::ForOfStatement<'_>,
        label: Option<String>,
    ) -> Read<()> {
        let ForStatementLeft::VariableDeclaration(declaration) = &each.left else {
            return Err(self.refuse(each.span, "`for (const item of list)`"));
        };
        let [declarator] = declaration.declarations.as_slice() else {
            return Err(self.refuse(each.span, "`for (const item of list)`"));
        };
        let BindingPattern::BindingIdentifier(item) = &declarator.id else {
            return Err(self.refuse(each.span, "`for (const item of list)`"));
        };
        // `list`, or `iterate(list, "Name")` for an iterator named otherwise.
        let (list, iterator) = match &each.right {
            Expression::Identifier(list) => (list.name.as_str(), pascal(&item.name)),
            Expression::CallExpression(call) if matches!(&call.callee, Expression::Identifier(callee) if callee.name == "iterate") =>
            {
                let [Argument::Identifier(list), name] = call.arguments.as_slice() else {
                    return Err(self.refuse(call.span, "iterate(list, \"Name\")"));
                };
                (list.name.as_str(), self.text(name)?)
            }
            other => return Err(self.refuse(other.span(), "the list it goes over")),
        };
        let list = self
            .lookup(list)
            .ok_or_else(|| self.refuse(each.right.span(), "a variable in scope"))?;
        let body = self.block(&each.body)?;
        self.state.borrow_mut().frames.push(Frame::Loop(label));
        flow.for_each(&var(list.mendix.as_str()), iterator.as_str(), |flow, _| {
            if self.failed() {
                return;
            }
            self.state.borrow_mut().scopes.push(HashMap::new());
            let read = self
                .declare(&item.name, &iterator, list.entity.clone(), item.span)
                .and_then(|()| self.statements(flow, body));
            if let Err(error) = read {
                self.fail(error);
            }
            self.state.borrow_mut().scopes.pop();
        });
        self.state.borrow_mut().frames.pop();
        Ok(())
    }

    fn while_loop(
        &self,
        flow: &mut FlowBuilder,
        each: &oxc_ast::ast::WhileStatement<'_>,
        label: Option<String>,
    ) -> Read<()> {
        let condition = self.value(&each.test)?;
        let body = self.block(&each.body)?;
        self.state.borrow_mut().frames.push(Frame::Loop(label));
        flow.while_loop(condition, |flow| self.nested(flow, body));
        self.state.borrow_mut().frames.pop();
        Ok(())
    }

    /// A string literal argument.
    fn text(&self, argument: &Argument<'_>) -> Read<String> {
        match argument.as_expression() {
            Some(expression) => self.expression_text(expression),
            None => Err(self.refuse(argument.span(), "text")),
        }
    }

    /// Text: a string literal, or a template literal with nothing
    /// interpolated, its lines its own.
    fn expression_text(&self, expression: &Expression<'_>) -> Read<String> {
        match expression {
            Expression::StringLiteral(text) if !text.lone_surrogates => Ok(text.value.to_string()),
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
                .quasis
                .iter()
                .map(|quasi| {
                    if quasi.lone_surrogates {
                        return None;
                    }
                    quasi.value.cooked.as_ref().map(ToString::to_string)
                })
                .collect::<Option<String>>()
                .ok_or_else(|| self.refuse(template.span, "text")),
            other => Err(self.refuse(other.span(), "text")),
        }
    }

    /// The Mendix expression a value reads as.
    fn value(&self, expression: &Expression<'_>) -> Read<Mx> {
        match expression {
            Expression::StringLiteral(text) => Ok(Mx::from(mxrs_expr::string(text.value.as_str()))),
            Expression::NumericLiteral(number) => Ok(mx(self.number(number)?)),
            Expression::UnaryExpression(unary)
                if unary.operator == oxc_ast::ast::UnaryOperator::UnaryNegation =>
            {
                match &unary.argument {
                    Expression::NumericLiteral(number) => {
                        Ok(mx(format!("-{}", self.number(number)?)))
                    }
                    other => Err(self.refuse(other.span(), "a number")),
                }
            }
            Expression::BooleanLiteral(flag) => Ok(mx(if flag.value { "true" } else { "false" })),
            Expression::Identifier(ident) => {
                let variable = self
                    .lookup(&ident.name)
                    .ok_or_else(|| self.refuse(ident.span, "a variable in scope"))?;
                Ok(Mx::from(&var(variable.mendix.as_str())))
            }
            Expression::CallExpression(call) => {
                let Expression::Identifier(callee) = &call.callee else {
                    return Err(self.refuse(call.span, "a value"));
                };
                match (callee.name.as_str(), call.arguments.as_slice()) {
                    ("mx", [text]) => Ok(mx(self.text(text)?)),
                    ("variable", [name]) => Ok(Mx::from(&var(self.text(name)?.as_str()))),
                    _ => Err(self.refuse(call.span, "a value")),
                }
            }
            Expression::ParenthesizedExpression(inner) => self.value(&inner.expression),
            other => Err(self.refuse(
                other.span(),
                "a value: text, a number, a variable or mx(\"...\")",
            )),
        }
    }

    /// A number as Mendix reads it: its digits, with a decimal point when
    /// it has decimals. TypeScript's other ways to write one — `0x10`,
    /// `1_000`, `1e3`, `.5` — are not Mendix's.
    fn number(&self, number: &oxc_ast::ast::NumericLiteral<'_>) -> Read<String> {
        let source = &self.source[number.span.start as usize..number.span.end as usize];
        let (whole, decimals) = source.split_once('.').unwrap_or((source, "0"));
        let digits = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
        if digits(whole) && digits(decimals) {
            Ok(source.to_string())
        } else {
            Err(self.refuse(number.span, "a number in plain digits: `16`, `0.5`"))
        }
    }

    /// Checks that `options` states nothing but what the activity reads.
    fn only(&self, options: &[(String, &Expression<'_>, Span)], allowed: &[&str]) -> Read<()> {
        let mut seen: Vec<&str> = Vec::new();
        for (key, _, span) in options {
            if !allowed.contains(&key.as_str()) {
                return Err(self.refuse(
                    *span,
                    &if allowed.is_empty() {
                        "no options".to_string()
                    } else {
                        format!("one of its options: {}", allowed.join(", "))
                    },
                ));
            }
            if seen.contains(&key.as_str()) {
                return Err(self.refuse(*span, "each option once"));
            }
            seen.push(key);
        }
        Ok(())
    }

    /// Checks that a call has no more arguments than the activity takes.
    fn at_most(&self, arguments: &[&Expression<'_>], count: usize, shape: &str) -> Read<()> {
        match arguments.get(count) {
            Some(extra) => Err(self.refuse(extra.span(), &format!("no more arguments: {shape}"))),
            None => Ok(()),
        }
    }

    /// The variable an argument names.
    fn variable(&self, expression: &Expression<'_>) -> Read<Variable> {
        match expression {
            Expression::Identifier(ident) => self
                .lookup(&ident.name)
                .ok_or_else(|| self.refuse(ident.span, "a variable in scope")),
            Expression::CallExpression(call) if matches!(&call.callee, Expression::Identifier(callee) if callee.name == "variable") =>
            {
                let [name] = call.arguments.as_slice() else {
                    return Err(self.refuse(call.span, "variable(\"Name\")"));
                };
                Ok(Variable {
                    mendix: self.text(name)?,
                    entity: None,
                })
            }
            other => Err(self.refuse(other.span(), "a variable")),
        }
    }

    /// A member, by its key: an attribute of `entity` by its name, an
    /// association by its qualified name, an attribute of another entity
    /// by its own.
    fn member(&self, key: &str, entity: Option<&str>, span: Span) -> Read<MemberName> {
        match key.matches('.').count() {
            0 => {
                let entity = entity.ok_or_else(|| {
                    self.refuse(span, "the member qualified: its entity is not known here")
                })?;
                Ok(MemberName::attribute(format!("{entity}.{key}")))
            }
            1 => Ok(MemberName::association(key)),
            _ => Ok(MemberName::attribute(key)),
        }
    }

    fn attribute(&self, key: &str, entity: Option<&str>, span: Span) -> Read<String> {
        match self.member(key, entity, span)? {
            MemberName::Attribute(name) | MemberName::Association(name) => Ok(name),
        }
    }

    /// An object literal's entries, in order.
    fn entries<'p, 'a>(
        &self,
        expression: &'p Expression<'a>,
    ) -> Read<Vec<(String, &'p Expression<'a>, Span)>> {
        let Expression::ObjectExpression(object) = expression else {
            return Err(self.refuse(expression.span(), "an object `{ ... }`"));
        };
        object
            .properties
            .iter()
            .map(|property| {
                let ObjectPropertyKind::ObjectProperty(property) = property else {
                    return Err(self.refuse(property.span(), "`key: value`"));
                };
                if property.computed || property.method {
                    return Err(self.refuse(property.span, "`key: value`"));
                }
                let key = match &property.key {
                    PropertyKey::StringLiteral(text) => text.value.to_string(),
                    other => other
                        .static_name()
                        .map(|name| name.to_string())
                        .ok_or_else(|| self.refuse(property.key.span(), "a key"))?,
                };
                Ok((key, &property.value, property.span))
            })
            .collect()
    }

    fn option<'p, 'a>(
        options: &'p [(String, &'p Expression<'a>, Span)],
        key: &str,
    ) -> Option<&'p Expression<'a>> {
        options
            .iter()
            .find(|(seen, _, _)| seen == key)
            .map(|(_, value, _)| *value)
    }

    fn boolean(&self, expression: &Expression<'_>) -> Read<bool> {
        match expression {
            Expression::BooleanLiteral(flag) => Ok(flag.value),
            other => Err(self.refuse(other.span(), "true or false")),
        }
    }

    /// `[a, b]`.
    fn pair<'p, 'a>(
        &self,
        expression: &'p Expression<'a>,
    ) -> Read<(&'p Expression<'a>, &'p Expression<'a>)> {
        let Expression::ArrayExpression(array) = expression else {
            return Err(self.refuse(expression.span(), "`[a, b]`"));
        };
        match array.elements.as_slice() {
            [first, second] => match (first.as_expression(), second.as_expression()) {
                (Some(first), Some(second)) => Ok((first, second)),
                _ => Err(self.refuse(array.span, "`[a, b]`")),
            },
            _ => Err(self.refuse(array.span, "`[a, b]`")),
        }
    }

    fn sorting(
        &self,
        expression: &Expression<'_>,
        entity: Option<&str>,
    ) -> Read<Vec<(String, SortOrder)>> {
        let Expression::ArrayExpression(array) = expression else {
            return Err(self.refuse(expression.span(), "[[member, \"asc\"], ...]"));
        };
        array
            .elements
            .iter()
            .map(|element| {
                let Some(element) = element.as_expression() else {
                    return Err(self.refuse(element.span(), "[member, \"asc\"]"));
                };
                let (member, order) = self.pair(element)?;
                let member =
                    self.attribute(&self.expression_text(member)?, entity, member.span())?;
                let order = match self.expression_text(order)?.as_str() {
                    "asc" => SortOrder::Ascending,
                    "desc" => SortOrder::Descending,
                    _ => return Err(self.refuse(order.span(), "\"asc\" or \"desc\"")),
                };
                Ok((member, order))
            })
            .collect()
    }

    /// The variable an activity's result is named, from what binds it,
    /// what its options say, and what it is named by default.
    fn result_name(
        &self,
        binding: &Binding<'_>,
        options: &[(String, &Expression<'_>, Span)],
        default: Option<String>,
        span: Span,
    ) -> Read<Option<String>> {
        if let Some(name) = Self::option(options, "name") {
            return Ok(Some(self.expression_text(name)?));
        }
        match (binding.ident, default) {
            (Some(ident), _) => Ok(Some(pascal(ident))),
            (None, Some(default)) => Ok(Some(default)),
            (None, None) => {
                let _ = span;
                Ok(None)
            }
        }
    }

    fn bind(
        &self,
        binding: &Binding<'_>,
        mendix: Option<&str>,
        entity: Option<String>,
        span: Span,
    ) -> Read<()> {
        if let Some(ident) = binding.ident {
            let mendix =
                mendix.ok_or_else(|| self.refuse(span, "an activity with a result to bind"))?;
            self.declare(ident, mendix, entity, span)?;
        }
        Ok(())
    }

    fn arguments<'p, 'a>(
        &self,
        call: &'p oxc_ast::ast::CallExpression<'a>,
    ) -> Read<Vec<&'p Expression<'a>>> {
        call.arguments
            .iter()
            .map(|argument| {
                argument
                    .as_expression()
                    .ok_or_else(|| self.refuse(argument.span(), "an argument, not a spread"))
            })
            .collect()
    }

    fn activity(
        &self,
        flow: &mut FlowBuilder,
        expression: &Expression<'_>,
        binding: Binding<'_>,
    ) -> Read<()> {
        let Expression::CallExpression(call) = expression else {
            return Err(self.refuse(expression.span(), "an activity call"));
        };
        let span = call.span;
        let arguments = self.arguments(call)?;
        // `Service.method(arguments)`: a nanoflow, by its service.
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let Expression::Identifier(service) = &member.object else {
                return Err(self.refuse(member.span, "`Service.method(...)`"));
            };
            let signature = self
                .registry
                .get(service.name.as_str())
                .and_then(|methods| methods.get(member.property.name.as_str()))
                .cloned()
                .ok_or_else(|| self.refuse(member.span, "a method of a service"))?;
            if arguments.len() != signature.parameters.len() {
                return Err(self.refuse(span, "an argument for every parameter"));
            }
            let values = arguments
                .iter()
                .map(|argument| self.value(argument))
                .collect::<Read<Vec<_>>>()?;
            let result = binding.ident.map(pascal);
            let configure = |options: &mut mxrs_dsl::flow_actions::NanoflowCallOptions<'_>| {
                for (parameter, value) in signature.parameters.iter().zip(&values) {
                    options.argument(parameter, value.clone());
                }
            };
            match &result {
                Some(name) => {
                    flow.call_nanoflow_into(name.as_str(), signature.qualified.as_str(), configure);
                }
                None => {
                    flow.call_nanoflow(signature.qualified.as_str(), configure);
                }
            }
            return self.bind(&binding, result.as_deref(), None, span);
        }
        let Expression::Identifier(callee) = &call.callee else {
            return Err(self.refuse(call.callee.span(), "an activity"));
        };
        match callee.name.as_str() {
            "onError" | "disabled" => {
                let read = self.modified(flow, callee.name.as_str(), &arguments, binding, span);
                if read.is_err() || self.failed() {
                    // The activity the modifier is about was not read.
                    flow.forget_pending();
                }
                read
            }
            _ => self.plain_activity(flow, callee, call, &arguments, binding, span),
        }
    }

    /// `onError(kind, () => activity, handler?)` and `disabled(() =>
    /// activity)`: the activity, with what is said about it.
    fn modified(
        &self,
        flow: &mut FlowBuilder,
        name: &str,
        arguments: &[&Expression<'_>],
        binding: Binding<'_>,
        span: Span,
    ) -> Read<()> {
        {
            {
                let (activity, handler) = match (name, arguments) {
                    ("onError", [kind, activity]) => {
                        if self.expression_text(kind)? != "continue" {
                            return Err(self.refuse(kind.span(), "\"continue\" without a handler"));
                        }
                        flow.continue_on_error();
                        (*activity, None)
                    }
                    ("onError", [kind, activity, handler]) => {
                        let kind = self.expression_text(kind)?;
                        (*activity, Some((kind, *handler)))
                    }
                    ("disabled", [activity]) => {
                        flow.disabled();
                        (*activity, None)
                    }
                    _ => {
                        return Err(
                            self.refuse(span, "onError(kind, () => activity, async () => { ... })")
                        );
                    }
                };
                if let Some((kind, handler)) = handler {
                    let Expression::ArrowFunctionExpression(handler) = handler else {
                        return Err(self.refuse(handler.span(), "async () => { ... }"));
                    };
                    let oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) = &handler.body else {
                        return Err(self.refuse(handler.span, "async () => { ... }"));
                    };
                    let statements = &body.statements;
                    match kind.as_str() {
                        "custom" => {
                            flow.on_error(|flow| self.nested(flow, statements));
                        }
                        "customWithoutRollback" => {
                            flow.on_error_without_rollback(|flow| self.nested(flow, statements));
                        }
                        _ => {
                            return Err(
                                self.refuse(span, "\"custom\" or \"customWithoutRollback\"")
                            );
                        }
                    }
                    if self.failed() {
                        return Ok(());
                    }
                }
                let Expression::ArrowFunctionExpression(activity) = activity else {
                    return Err(self.refuse(activity.span(), "() => activity"));
                };
                let Some(inner) = activity.body.as_expression() else {
                    return Err(self.refuse(activity.span, "() => activity"));
                };
                if !activity.params.items.is_empty() || activity.params.rest.is_some() {
                    return Err(self.refuse(activity.span, "() => activity"));
                }
                self.activity(flow, inner, binding)
            }
        }
    }

    fn plain_activity(
        &self,
        flow: &mut FlowBuilder,
        callee: &oxc_ast::ast::IdentifierReference<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
        arguments: &[&Expression<'_>],
        binding: Binding<'_>,
        span: Span,
    ) -> Read<()> {
        let options_at = |index: usize| -> Read<Vec<(String, &Expression<'_>, Span)>> {
            match arguments.get(index) {
                Some(options) => self.entries(options),
                None => Ok(Vec::new()),
            }
        };
        match callee.name.as_str() {
            "createObject" => {
                let entity = self.expression_text(
                    arguments
                        .first()
                        .ok_or_else(|| self.refuse(span, "the entity"))?,
                )?;
                let members = match arguments.get(1) {
                    Some(members) => self.entries(members)?,
                    None => Vec::new(),
                };
                let options = options_at(2)?;
                self.at_most(arguments, 3, "createObject(entity, members, options)")?;
                self.only(&options, &["commit", "refresh", "name"])?;
                let short = entity
                    .rsplit_once('.')
                    .map_or(entity.as_str(), |(_, name)| name);
                let name = self
                    .result_name(&binding, &options, Some(format!("New{short}")), span)?
                    .expect("a default");
                let mut sets = Vec::new();
                for (key, value, at) in &members {
                    sets.push((self.member(key, Some(&entity), *at)?, self.value(value)?));
                }
                let commit = self.commit(&options)?;
                let refresh = Self::option(&options, "refresh")
                    .map(|value| self.boolean(value))
                    .transpose()?;
                flow.create(name.as_str(), entity.as_str(), |create| {
                    for (member, value) in &sets {
                        create.set(member.clone(), value.clone());
                    }
                    if let Some(commit) = commit {
                        create.commit(commit);
                    }
                    if let Some(refresh) = refresh {
                        create.refresh_in_client(refresh);
                    }
                });
                self.bind(&binding, Some(&name), Some(entity.clone()), span)
            }
            "changeObject" => {
                let variable = self.variable(
                    arguments
                        .first()
                        .ok_or_else(|| self.refuse(span, "the object"))?,
                )?;
                let members = match arguments.get(1) {
                    Some(members) => self.entries(members)?,
                    None => Vec::new(),
                };
                let options = options_at(2)?;
                self.at_most(arguments, 3, "changeObject(object, members, options)")?;
                self.only(&options, &["commit", "refresh"])?;
                let mut sets = Vec::new();
                for (key, value, at) in &members {
                    sets.push((
                        self.member(key, variable.entity.as_deref(), *at)?,
                        self.value(value)?,
                    ));
                }
                let commit = self.commit(&options)?;
                let refresh = Self::option(&options, "refresh")
                    .map(|value| self.boolean(value))
                    .transpose()?;
                flow.change(&var(variable.mendix.as_str()), |change| {
                    for (member, value) in &sets {
                        change.set(member.clone(), value.clone());
                    }
                    if let Some(commit) = commit {
                        change.commit(commit);
                    }
                    if let Some(refresh) = refresh {
                        change.refresh_in_client(refresh);
                    }
                });
                self.bind(&binding, None, None, span)
            }
            "commitObject" | "deleteObject" | "rollbackObject" => {
                let variable = self.variable(
                    arguments
                        .first()
                        .ok_or_else(|| self.refuse(span, "the object"))?,
                )?;
                let options = options_at(1)?;
                self.at_most(arguments, 2, "the object and its options")?;
                if callee.name == "commitObject" {
                    self.only(&options, &["withEvents", "refresh"])?;
                } else {
                    self.only(&options, &["refresh"])?;
                }
                let refresh = Self::option(&options, "refresh")
                    .map(|value| self.boolean(value))
                    .transpose()?;
                let events = Self::option(&options, "withEvents")
                    .map(|value| self.boolean(value))
                    .transpose()?;
                let variable = var(variable.mendix.as_str());
                match callee.name.as_str() {
                    "commitObject" => {
                        flow.commit_with(&variable, |commit| {
                            if let Some(events) = events {
                                commit.with_events(events);
                            }
                            if let Some(refresh) = refresh {
                                commit.refresh_in_client(refresh);
                            }
                        });
                    }
                    "deleteObject" => {
                        flow.delete_with(&variable, |delete| {
                            if let Some(refresh) = refresh {
                                delete.refresh_in_client(refresh);
                            }
                        });
                    }
                    _ => {
                        flow.rollback_with(&variable, |rollback| {
                            if let Some(refresh) = refresh {
                                rollback.refresh_in_client(refresh);
                            }
                        });
                    }
                }
                self.bind(&binding, None, None, span)
            }
            "retrieve" => {
                let from = arguments
                    .first()
                    .ok_or_else(|| self.refuse(span, "what it retrieves"))?;
                let options = options_at(1)?;
                self.at_most(arguments, 2, "retrieve(from, options)")?;
                if let Some(by) = Self::option(&options, "by") {
                    self.only(&options, &["by", "name"])?;
                    let start = self.variable(from)?;
                    let association = self.expression_text(by)?;
                    let name = self
                        .result_name(&binding, &options, None, span)?
                        .ok_or_else(|| self.refuse(span, "a name for what it retrieves"))?;
                    flow.retrieve_associated(
                        name.as_str(),
                        &var(start.mendix.as_str()),
                        association.as_str(),
                    );
                    return self.bind(&binding, Some(&name), None, span);
                }
                let entity = self.expression_text(from)?;
                let first = Self::option(&options, "first")
                    .map(|value| self.boolean(value))
                    .transpose()?
                    .unwrap_or(false);
                let short = entity
                    .rsplit_once('.')
                    .map_or(entity.as_str(), |(_, name)| name);
                let default = if first {
                    short.to_string()
                } else {
                    format!("{short}List")
                };
                let name = self
                    .result_name(&binding, &options, Some(default), span)?
                    .expect("a default");
                self.only(&options, &["xpath", "sort", "first", "range", "name"])?;
                let mut calls: Vec<RetrieveCall> = Vec::new();
                for (key, value, _) in &options {
                    match key.as_str() {
                        "xpath" => {
                            let xpath = self.expression_text(value)?;
                            calls.push(Box::new(move |retrieve| {
                                retrieve.xpath(xpath.as_str());
                            }));
                        }
                        "sort" => {
                            for (member, order) in self.sorting(value, Some(&entity))? {
                                calls.push(Box::new(move |retrieve| {
                                    retrieve.sort_by(member.as_str(), order);
                                }));
                            }
                        }
                        "first" => {
                            if first {
                                calls.push(Box::new(|retrieve| {
                                    retrieve.first();
                                }));
                            }
                        }
                        "range" => {
                            let (limit, offset) = self.pair(value)?;
                            let (limit, offset) = (self.value(limit)?, self.value(offset)?);
                            calls.push(Box::new(move |retrieve| {
                                retrieve.range(limit.clone(), offset.clone());
                            }));
                        }
                        "name" => {}
                        _ => {
                            return Err(
                                self.refuse(value.span(), "xpath, sort, first, range, by or name")
                            );
                        }
                    }
                }
                flow.retrieve(name.as_str(), entity.as_str(), |retrieve| {
                    for call in &calls {
                        call(retrieve);
                    }
                });
                self.bind(&binding, Some(&name), Some(entity.clone()), span)
            }
            "createList" => {
                let entity = self.expression_text(
                    arguments
                        .first()
                        .ok_or_else(|| self.refuse(span, "the entity"))?,
                )?;
                let options = options_at(1)?;
                self.at_most(arguments, 2, "createList(entity, options)")?;
                self.only(&options, &["name"])?;
                let short = entity
                    .rsplit_once('.')
                    .map_or(entity.as_str(), |(_, name)| name);
                let name = self
                    .result_name(&binding, &options, Some(format!("{short}List")), span)?
                    .expect("a default");
                flow.create_list_of(name.as_str(), entity.as_str());
                self.bind(&binding, Some(&name), Some(entity.clone()), span)
            }
            "changeList" => self.change_list(flow, arguments, &binding, span),
            "aggregateList" => {
                let list = self.variable(
                    arguments
                        .first()
                        .ok_or_else(|| self.refuse(span, "the list"))?,
                )?;
                let aggregate = self.entries(
                    arguments
                        .get(1)
                        .ok_or_else(|| self.refuse(span, "what it computes"))?,
                )?;
                let options = options_at(2)?;
                self.at_most(arguments, 3, "aggregateList(list, aggregate, options)")?;
                self.only(&options, &["name"])?;
                let name = self
                    .result_name(&binding, &options, None, span)?
                    .ok_or_else(|| self.refuse(span, "a name for its result"))?;
                let [(key, value, at)] = aggregate.as_slice() else {
                    return Err(self.refuse(span, "one aggregate"));
                };
                let (function, over) = match key.strip_suffix("Of") {
                    Some(function) => (function.to_string(), Some(self.value(value)?)),
                    None => (key.clone(), None),
                };
                let function = match function.as_str() {
                    "count" => AggregateFunction::Count,
                    "sum" => AggregateFunction::Sum,
                    "average" => AggregateFunction::Average,
                    "minimum" => AggregateFunction::Minimum,
                    "maximum" => AggregateFunction::Maximum,
                    "all" => AggregateFunction::All,
                    "any" => AggregateFunction::Any,
                    _ => return Err(self.refuse(*at, "an aggregate function")),
                };
                let attribute = if function == AggregateFunction::Count || over.is_some() {
                    None
                } else {
                    Some(self.attribute(
                        &self.expression_text(value)?,
                        list.entity.as_deref(),
                        *at,
                    )?)
                };
                flow.aggregate(
                    name.as_str(),
                    &var(list.mendix.as_str()),
                    function,
                    |aggregate| {
                        if let Some(attribute) = &attribute {
                            aggregate.attribute(attribute.as_str());
                        }
                        if let Some(over) = &over {
                            aggregate.expression(over.clone());
                        }
                    },
                );
                self.bind(&binding, Some(&name), None, span)
            }
            "createVariable" => {
                let [ty, value, rest @ ..] = arguments else {
                    return Err(self.refuse(span, "createVariable(type, value)"));
                };
                let options = match rest.first() {
                    Some(options) => self.entries(options)?,
                    None => Vec::new(),
                };
                self.at_most(arguments, 3, "createVariable(type, value, options)")?;
                self.only(&options, &["name"])?;
                let (ty, entity) = self.variable_type(ty)?;
                let value = self.value(value)?;
                let name = self
                    .result_name(&binding, &options, None, span)?
                    .ok_or_else(|| self.refuse(span, "a name for the variable"))?;
                flow.create_variable(name.as_str(), ty, value);
                self.bind(&binding, Some(&name), entity, span)
            }
            "changeVariable" => {
                let [variable, value] = arguments else {
                    return Err(self.refuse(span, "changeVariable(variable, value)"));
                };
                let variable = self.variable(variable)?;
                let value = self.value(value)?;
                flow.change_variable(&var(variable.mendix.as_str()), value);
                self.bind(&binding, None, None, span)
            }
            "callMicroflow" | "callNanoflow" | "callJavaScriptAction" => {
                let target = self.expression_text(
                    arguments
                        .first()
                        .ok_or_else(|| self.refuse(span, "what it calls"))?,
                )?;
                let entries = match arguments.get(1) {
                    Some(entries) => self.entries(entries)?,
                    None => Vec::new(),
                };
                let options = options_at(2)?;
                self.at_most(
                    arguments,
                    3,
                    "the target, its arguments and the call's options",
                )?;
                if callee.name == "callMicroflow" {
                    self.only(&options, &["name", "discardResult", "queue"])?;
                } else {
                    self.only(&options, &["name", "discardResult"])?;
                }
                let result = match Self::option(&options, "name") {
                    Some(name) => Some(self.expression_text(name)?),
                    None => binding.ident.map(pascal),
                };
                let discard = Self::option(&options, "discardResult")
                    .map(|name| self.expression_text(name))
                    .transpose()?;
                let queue = Self::option(&options, "queue")
                    .map(|name| self.expression_text(name))
                    .transpose()?;
                self.call(
                    flow,
                    callee.name.as_str(),
                    &target,
                    &entries,
                    result.as_deref(),
                    discard,
                    queue,
                )?;
                self.bind(&binding, result.as_deref(), None, span)
            }
            "log" => {
                let [level, node, message, rest @ ..] = arguments else {
                    return Err(self.refuse(span, "log(level, node, message)"));
                };
                let level = match self.expression_text(level)?.as_str() {
                    "Trace" => LogSeverity::Trace,
                    "Debug" => LogSeverity::Debug,
                    "Info" => LogSeverity::Info,
                    "Warning" => LogSeverity::Warning,
                    "Error" => LogSeverity::Error,
                    "Critical" => LogSeverity::Critical,
                    _ => return Err(self.refuse(level.span(), "a log level")),
                };
                let node = self.value(node)?;
                let message = self.expression_text(message)?;
                let options = match rest.first() {
                    Some(options) => self.entries(options)?,
                    None => Vec::new(),
                };
                self.at_most(arguments, 4, "log(level, node, message, options)")?;
                self.only(&options, &["parameters", "stackTrace"])?;
                let parameters = match Self::option(&options, "parameters") {
                    Some(parameters) => self.values(parameters)?,
                    None => Vec::new(),
                };
                let trace = Self::option(&options, "stackTrace")
                    .map(|value| self.boolean(value))
                    .transpose()?;
                flow.log(level, node, message, |log| {
                    for parameter in &parameters {
                        log.parameter(parameter.clone());
                    }
                    if let Some(trace) = trace {
                        log.include_stack_trace(trace);
                    }
                });
                self.bind(&binding, None, None, span)
            }
            "showPage" => {
                let page = self.expression_text(
                    arguments
                        .first()
                        .ok_or_else(|| self.refuse(span, "the page"))?,
                )?;
                let options = options_at(1)?;
                self.at_most(arguments, 2, "showPage(page, options)")?;
                self.only(
                    &options,
                    &["args", "title", "titleParameters", "closePages"],
                )?;
                let page_arguments = match Self::option(&options, "args") {
                    Some(entries) => self
                        .entries(entries)?
                        .into_iter()
                        .map(|(key, value, _)| Ok((key, self.value(value)?)))
                        .collect::<Read<Vec<_>>>()?,
                    None => Vec::new(),
                };
                let title = match Self::option(&options, "title") {
                    Some(entries) => self
                        .entries(entries)?
                        .into_iter()
                        .map(|(language, text, _)| Ok((language, self.expression_text(text)?)))
                        .collect::<Read<Vec<_>>>()?,
                    None => Vec::new(),
                };
                let title_parameters = match Self::option(&options, "titleParameters") {
                    Some(values) => self.values(values)?,
                    None => Vec::new(),
                };
                let close = Self::option(&options, "closePages")
                    .map(|value| self.value(value))
                    .transpose()?;
                flow.show_page(page.as_str(), |options| {
                    for (parameter, value) in &page_arguments {
                        options.argument(parameter, value.clone());
                    }
                    for (language, text) in &title {
                        options.title(language, text.as_str());
                    }
                    for parameter in &title_parameters {
                        options.title_parameter(parameter.clone());
                    }
                    if let Some(close) = &close {
                        options.close_pages(close.clone());
                    }
                });
                self.bind(&binding, None, None, span)
            }
            "closePage" => {
                match arguments {
                    [] => {
                        flow.close_page();
                    }
                    [count] => {
                        flow.close_pages(self.value(count)?);
                    }
                    _ => return Err(self.refuse(span, "closePage(count?)")),
                }
                self.bind(&binding, None, None, span)
            }
            "showMessage" => {
                let [kind, text, rest @ ..] = arguments else {
                    return Err(self.refuse(span, "showMessage(kind, text)"));
                };
                let kind = match self.expression_text(kind)?.as_str() {
                    "information" => MessageKind::Information,
                    "warning" => MessageKind::Warning,
                    "error" => MessageKind::Error,
                    _ => return Err(self.refuse(kind.span(), "information, warning or error")),
                };
                let text = self
                    .entries(text)?
                    .into_iter()
                    .map(|(language, text, _)| Ok((language, self.expression_text(text)?)))
                    .collect::<Read<Vec<_>>>()?;
                let options = match rest.first() {
                    Some(options) => self.entries(options)?,
                    None => Vec::new(),
                };
                self.at_most(arguments, 3, "showMessage(kind, text, options)")?;
                self.only(&options, &["parameters", "blocking"])?;
                let parameters = match Self::option(&options, "parameters") {
                    Some(values) => self.values(values)?,
                    None => Vec::new(),
                };
                let blocking = Self::option(&options, "blocking")
                    .map(|value| self.boolean(value))
                    .transpose()?;
                flow.show_message(kind, |message| {
                    for (language, text) in &text {
                        message.text(language, text.as_str());
                    }
                    for parameter in &parameters {
                        message.parameter(parameter.clone());
                    }
                    if let Some(blocking) = blocking {
                        message.blocking(blocking);
                    }
                });
                self.bind(&binding, None, None, span)
            }
            _ => Err(self.refuse(call.callee.span(), "an activity of src/mxrs/flows.ts")),
        }
    }

    fn values(&self, expression: &Expression<'_>) -> Read<Vec<Mx>> {
        let Expression::ArrayExpression(array) = expression else {
            return Err(self.refuse(expression.span(), "[value, ...]"));
        };
        array
            .elements
            .iter()
            .map(|element| match element {
                ArrayExpressionElement::SpreadElement(spread) => {
                    Err(self.refuse(spread.span, "a value"))
                }
                ArrayExpressionElement::Elision(hole) => Err(self.refuse(hole.span, "a value")),
                other => self.value(other.as_expression().expect("neither a spread nor a hole")),
            })
            .collect()
    }

    fn commit(&self, options: &[(String, &Expression<'_>, Span)]) -> Read<Option<Commit>> {
        match Self::option(options, "commit") {
            None => Ok(None),
            Some(Expression::BooleanLiteral(flag)) => {
                Ok(Some(if flag.value { Commit::Yes } else { Commit::No }))
            }
            Some(Expression::StringLiteral(text)) if text.value == "withoutEvents" => {
                Ok(Some(Commit::WithoutEvents))
            }
            Some(other) => Err(self.refuse(other.span(), "true, false or \"withoutEvents\"")),
        }
    }

    /// `createVariable`'s type: `"Long"`, `{ object: "Module.Entity" }`, ...
    fn variable_type(&self, expression: &Expression<'_>) -> Read<(DataType, Option<String>)> {
        match expression {
            Expression::StringLiteral(text) => Ok((
                match text.value.as_str() {
                    "String" => DataType::String,
                    "Integer" => DataType::Integer,
                    "Long" => DataType::Long,
                    "Decimal" => DataType::Decimal,
                    "Float" => DataType::Float,
                    "Boolean" => DataType::Boolean,
                    "DateTime" => DataType::DateTime,
                    "Binary" => DataType::Binary,
                    _ => return Err(self.refuse(text.span, "a type")),
                },
                None,
            )),
            Expression::ObjectExpression(_) => {
                let entries = self.entries(expression)?;
                let [(kind, name, _)] = entries.as_slice() else {
                    return Err(self.refuse(expression.span(), "{ object: \"Module.Entity\" }"));
                };
                let name = self.expression_text(name)?;
                match kind.as_str() {
                    "object" => Ok((DataType::Object(name.clone()), Some(name))),
                    "list" => Ok((DataType::List(name.clone()), Some(name))),
                    "enumeration" => Ok((DataType::Enumeration(name), None)),
                    _ => Err(self.refuse(expression.span(), "object, list or enumeration")),
                }
            }
            other => Err(self.refuse(other.span(), "a type")),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        &self,
        flow: &mut FlowBuilder,
        kind: &str,
        target: &str,
        entries: &[(String, &Expression<'_>, Span)],
        result: Option<&str>,
        discard: Option<String>,
        queue: Option<String>,
    ) -> Read<()> {
        // Each argument as the call states it: a value, or for a
        // JavaScript action an entity or a nanoflow.
        enum Value {
            Plain(Mx),
            Entity(String),
            Nanoflow(String),
        }
        let mut values = Vec::new();
        for (parameter, value, _) in entries {
            let value = match value {
                Expression::ObjectExpression(_) if kind == "callJavaScriptAction" => {
                    let inner = self.entries(value)?;
                    match inner.as_slice() {
                        [(key, name, _)] if key == "entity" => {
                            Value::Entity(self.expression_text(name)?)
                        }
                        [(key, name, _)] if key == "nanoflow" => {
                            Value::Nanoflow(self.expression_text(name)?)
                        }
                        _ => {
                            return Err(
                                self.refuse(value.span(), "{ entity: ... } or { nanoflow: ... }")
                            );
                        }
                    }
                }
                other => Value::Plain(self.value(other)?),
            };
            values.push((parameter.clone(), value));
        }
        match kind {
            "callMicroflow" => {
                let configure = |options: &mut mxrs_dsl::flow_actions::CallOptions<'_>| {
                    for (parameter, value) in &values {
                        if let Value::Plain(value) = value {
                            options.argument(parameter, value.clone());
                        }
                    }
                    if let Some(discard) = &discard {
                        options.discard_result(discard);
                    }
                    if let Some(queue) = &queue {
                        options.queue(queue);
                    }
                };
                match result {
                    Some(name) => {
                        flow.call_into(name, target, configure);
                    }
                    None => {
                        flow.call(target, configure);
                    }
                }
            }
            "callNanoflow" => {
                let configure = |options: &mut mxrs_dsl::flow_actions::NanoflowCallOptions<'_>| {
                    for (parameter, value) in &values {
                        if let Value::Plain(value) = value {
                            options.argument(parameter, value.clone());
                        }
                    }
                    if let Some(discard) = &discard {
                        options.discard_result(discard);
                    }
                };
                match result {
                    Some(name) => {
                        flow.call_nanoflow_into(name, target, configure);
                    }
                    None => {
                        flow.call_nanoflow(target, configure);
                    }
                }
            }
            _ => {
                let configure =
                    |options: &mut mxrs_dsl::flow_actions::JavaScriptCallOptions<'_>| {
                        for (parameter, value) in &values {
                            match value {
                                Value::Plain(value) => {
                                    options.argument(parameter, value.clone());
                                }
                                Value::Entity(entity) => {
                                    options.entity_argument(parameter, entity.as_str());
                                }
                                Value::Nanoflow(nanoflow) => {
                                    options.nanoflow_argument(parameter, nanoflow.as_str());
                                }
                            }
                        }
                        if let Some(discard) = &discard {
                            options.discard_result(discard);
                        }
                    };
                match result {
                    Some(name) => {
                        flow.call_javascript_into(name, target, configure);
                    }
                    None => {
                        flow.call_javascript(target, configure);
                    }
                }
            }
        }
        Ok(())
    }

    fn change_list(
        &self,
        flow: &mut FlowBuilder,
        arguments: &[&Expression<'_>],
        binding: &Binding<'_>,
        span: Span,
    ) -> Read<()> {
        let [list, operation, rest @ ..] = arguments else {
            return Err(self.refuse(span, "changeList(list, { operation })"));
        };
        let list = self.variable(list)?;
        let operation = self.entries(operation)?;
        let [(key, value, at)] = operation.as_slice() else {
            return Err(self.refuse(span, "one operation"));
        };
        let options = match rest.first() {
            Some(options) => self.entries(options)?,
            None => Vec::new(),
        };
        self.at_most(arguments, 3, "changeList(list, { operation }, options)")?;
        self.only(&options, &["name"])?;
        let source = var(list.mendix.as_str());
        let changes = match key.as_str() {
            "add" => Some(ListChange::Add),
            "remove" => Some(ListChange::Remove),
            "replace" => Some(ListChange::Set),
            "clear" => Some(ListChange::Clear),
            _ => None,
        };
        if let Some(change) = changes {
            self.only(&options, &[])?;
            let value = if change == ListChange::Clear {
                mx("")
            } else {
                self.value(value)?
            };
            flow.change_list(&source, change, value);
            return self.bind(binding, None, None, span);
        }
        let name = self
            .result_name(binding, &options, None, span)?
            .ok_or_else(|| self.refuse(span, "a name for the list it makes"))?;
        let mut entity = list.entity.clone();
        match key.as_str() {
            "head" => {
                flow.list_head(name.as_str(), &source);
            }
            "tail" => {
                flow.list_tail(name.as_str(), &source);
            }
            "find" | "filter" => {
                let (member, wanted) = self.pair(value)?;
                let member =
                    self.member(&self.expression_text(member)?, list.entity.as_deref(), *at)?;
                let wanted = self.value(wanted)?;
                if key == "find" {
                    flow.list_find(name.as_str(), &source, member, wanted);
                } else {
                    flow.list_filter(name.as_str(), &source, member, wanted);
                }
            }
            "findBy" => {
                flow.list_find_by(name.as_str(), &source, self.value(value)?);
            }
            "filterBy" => {
                flow.list_filter_by(name.as_str(), &source, self.value(value)?);
            }
            "sort" => {
                let sorting = self.sorting(value, list.entity.as_deref())?;
                flow.list_sort(name.as_str(), &source, |sort| {
                    for (member, order) in &sorting {
                        sort.by(member.as_str(), *order);
                    }
                });
            }
            "range" => {
                let (limit, offset) = self.pair(value)?;
                flow.list_range(
                    name.as_str(),
                    &source,
                    self.value(limit)?,
                    self.value(offset)?,
                );
            }
            "union" | "intersect" | "subtract" | "contains" | "equals" => {
                let other = var(self.variable(value)?.mendix.as_str());
                match key.as_str() {
                    "union" => flow.list_union(name.as_str(), &source, &other),
                    "intersect" => flow.list_intersect(name.as_str(), &source, &other),
                    "subtract" => flow.list_subtract(name.as_str(), &source, &other),
                    "contains" => flow.list_contains(name.as_str(), &source, &other),
                    _ => flow.list_equals(name.as_str(), &source, &other),
                };
                if matches!(key.as_str(), "contains" | "equals") {
                    entity = None;
                }
            }
            _ => return Err(self.refuse(*at, "a list operation")),
        }
        self.bind(binding, Some(&name), entity, span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (name, source) in files {
            let path = directory.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
        directory
    }

    #[cfg(unix)]
    #[test]
    fn a_link_back_to_a_folder_already_read_is_read_once() {
        let directory = services(&[("sales/orderService.ts", "export {};\n")]);
        std::os::unix::fs::symlink(directory.path(), directory.path().join("sales/again")).unwrap();
        assert_eq!(
            service_files(directory.path()),
            [directory.path().join("sales/orderService.ts")]
        );
    }

    #[test]
    fn a_service_method_is_the_nanoflow_its_comment_names() {
        let directory = services(&[
            (
                "sales/orderService.ts",
                r#"import { createObject, mx, nanoflowService, type MxObject } from "@/mxrs/flows";
import { ShipService } from "@/services/sales/shipService";

export const OrderService = nanoflowService("Sales", {
  /**
   * Opens an order.
   *
   * @nanoflow ACT_Order_Open
   * @roles User Admin.Root
   * @param order What to open
   * @mendixName spc SPC
   */
  async open(order: MxObject<"Sales.Order">, spc?: MxObject<"Sales.Order">): Promise<boolean> {
    const newOrder = await createObject("Sales.Order", { Number: "A", "Sales.Order_Customer": order });
    const shipped = await ShipService.ship(newOrder);
    return mx(`$Shipped
and true`);
  },
});
"#,
            ),
            (
                "sales/shipService.ts",
                r#"export const ShipService = nanoflowService("Sales", {
  /** @nanoflow SUB_Ship */
  async ship(order: MxObject<"Sales.Order">): Promise<boolean> {
    return true;
  },
});
"#,
            ),
        ]);
        let mut read = read(directory.path()).unwrap();
        read.sort_by(|left, right| left.1.name.cmp(&right.1.name));
        let [(module, open), (_, ship)] = read.as_slice() else {
            panic!("{read:?}");
        };
        assert_eq!(module, "Sales");
        assert_eq!(open.name, "ACT_Order_Open");
        assert_eq!(open.documentation, "Opens an order.");
        assert_eq!(
            open.allowed_roles.as_deref(),
            Some(&["Sales.User".to_string(), "Admin.Root".to_string()][..])
        );
        assert_eq!(open.parameters[0].documentation, "What to open");
        assert!(open.parameters[0].required);
        assert_eq!(open.parameters[1].name, "SPC");
        assert!(!open.parameters[1].required);
        assert_eq!(
            open.return_expression.as_deref(),
            Some("$Shipped\nand true")
        );
        let body = format!("{:?}", open.activities);
        for expected in [
            "(\"VariableName\", Text(\"NewOrder\"))",
            "\"Sales.Order.Number\"",
            "\"Sales.Order_Customer\"",
            "\"'A'\"",
            "\"Sales.SUB_Ship\"",
            "\"Shipped\"",
        ] {
            assert!(body.contains(expected), "{expected}: {body}");
        }
        assert_eq!(ship.return_expression.as_deref(), Some("true"));
    }

    #[test]
    fn what_a_nanoflow_cannot_say_is_refused_at_its_line() {
        for (source, expected) in [
            (
                "export const S = nanoflowService(\"M\", {\n  async open(): Promise<void> {},\n});\n",
                "@nanoflow",
            ),
            (
                "export const S = nanoflowService(\"M\", {\n  /** @nanoflow F */\n  async f(): Promise<void> {\n    await sendEmail();\n  },\n});\n",
                ":4:",
            ),
            (
                "export const S = nanoflowService(\"M\", {\n  /** @nanoflow F */\n  async f(): Promise<void> {\n    break;\n  },\n});\n",
                "a loop to break",
            ),
            (
                "export const S = nanoflowService(\"M\", {\n  /** @nanoflow F */\n  async f(x: number): Promise<void> {},\n});\n",
                "a Mendix type",
            ),
        ] {
            let directory = services(&[("m/s.ts", source)]);
            let error = read(directory.path()).err().unwrap().to_string();
            assert!(error.contains(expected), "{source}\n{error}");
        }
    }
    /// One method `f` of a service of `M`, with `body` as its body.
    fn method(signature: &str, body: &str) -> String {
        format!(
            "export const S = nanoflowService(\"M\", {{\n  /** @nanoflow F */\n  async f({signature} {{\n{body}\n  }},\n}});\n"
        )
    }

    fn read_one(source: &str) -> Result<MicroflowDecl, String> {
        let directory = services(&[("m/s.ts", source)]);
        read(directory.path())
            .map(|mut declared| declared.remove(0).1)
            .map_err(|error| error.to_string())
    }

    const LISTS: &str = "outer: MxList<\"M.E\">, inner: MxList<\"M.E\">): Promise<void>";

    #[test]
    fn a_break_or_continue_is_about_the_loop_it_is_in() {
        let nested = |statement: &str| {
            method(
                LISTS,
                &format!(
                    "    outer: for (const a of outer) {{\n      inner: for (const b of inner) {{\n        {statement}\n      }}\n    }}"
                ),
            )
        };
        for statement in ["break outer;", "continue outer;"] {
            let error = read_one(&nested(statement)).unwrap_err();
            assert!(
                error.contains(":6:") && error.contains("the label of the loop it is in"),
                "{error}"
            );
        }
        for statement in ["break inner;", "continue inner;", "break;", "continue;"] {
            read_one(&nested(statement)).unwrap();
        }
        let error = read_one(&method("): Promise<void>", "    continue;")).unwrap_err();
        assert!(error.contains("a loop to continue"), "{error}");
    }

    #[test]
    fn the_cases_of_a_switch_are_apart() {
        let switch = |cases: &str| {
            method(
                "s: string, l: MxList<\"M.E\">): Promise<void>",
                &format!(
                    "    loop: for (const a of l) {{\n      switch (mx<string | null>(\"$S\")) {{\n{cases}\n      }}\n    }}"
                ),
            )
        };
        // TypeScript would run the second case after the first.
        let error = read_one(&switch(
            "        case \"A\":\n          await closePage(1);\n        case \"B\":\n          await closePage(2);\n          break;",
        ))
        .unwrap_err();
        assert!(
            error.contains(":6:") && error.contains("do not fall through"),
            "{error}"
        );
        // In TypeScript this `break` leaves the switch, not the loop.
        let error = read_one(&switch(
            "        case \"A\": {\n          if (mx<boolean>(\"true\")) {\n            break;\n          }\n          break;\n        }",
        ))
        .unwrap_err();
        assert!(
            error.contains(":8:") && error.contains("the loop's label"),
            "{error}"
        );
        let read = read_one(&switch(
            "        case \"A\": {\n          if (mx<boolean>(\"true\")) {\n            break loop;\n          }\n          break;\n        }\n        case \"B\":\n        case \"C\":\n          return;\n        case null: {\n          await closePage();\n        }",
        ))
        .unwrap();
        let body = format!("{:?}", read.activities);
        assert!(
            body.contains("BreakLoop") || body.contains("Break"),
            "{body}"
        );
    }

    #[test]
    fn an_error_in_what_a_modifier_wraps_is_reported_not_panicked() {
        for statement in [
            "await onError(\"custom\", () => sendEmail(), async () => {});",
            "await onError(\"continue\", () => sendEmail());",
            "await disabled(() => sendEmail());",
            "await onError(\"custom\", () => closePage(1, 2), async () => { await closePage(); });",
            "await onError(\"custom\", async () => { await closePage(); }, async () => {});",
            "await onError(\"custom\", () => closePage(), async () => { await sendEmail(); });",
        ] {
            let error =
                read_one(&method("): Promise<void>", &format!("    {statement}"))).unwrap_err();
            assert!(error.contains(":4:"), "{statement}: {error}");
        }
    }

    #[test]
    fn what_typescript_accepts_and_a_flow_cannot_say_is_refused() {
        for (signature, body, expected) in [
            ("a: string = \"x\"): Promise<void>", "", "@defaultValue"),
            ("...rest: string[]): Promise<void>", "", "without a rest"),
            (")", "", "its return type"),
            ("): Promise<void>", "    return \"x\";", "no value"),
            (
                "): Promise<boolean>",
                "    return;",
                "the value the nanoflow returns",
            ),
            (
                "): Promise<void>",
                "    return;\n    await closePage();",
                "nothing after what ends the path",
            ),
            (
                "): Promise<void>",
                "    await closePage(0x10);",
                "plain digits",
            ),
            (
                "): Promise<void>",
                "    await closePage(1_000);",
                "plain digits",
            ),
            (
                "): Promise<void>",
                "    await closePage(1e3);",
                "plain digits",
            ),
            (
                "): Promise<void>",
                "    await closePage(.5);",
                "plain digits",
            ),
            (
                "): Promise<void>",
                "    await showPage(\"M.P\", { arg: {} });",
                "one of its options: args",
            ),
            (
                "a: MxObject<\"M.E\">): Promise<void>",
                "    await commitObject(a, { withEvent: false });",
                "one of its options: withEvents",
            ),
            (
                "a: MxObject<\"M.E\">): Promise<void>",
                "    await deleteObject(a, { withEvents: false });",
                "one of its options: refresh",
            ),
            (
                "a: MxObject<\"M.E\">): Promise<void>",
                "    await changeObject(a, {}, { add: {} });",
                "one of its options: commit",
            ),
            (
                "): Promise<void>",
                "    await callMicroflow(\"M.F\", {}, {}, \"extra\");",
                "no more arguments",
            ),
            (
                "a: MxObject<\"M.E\">): Promise<void>",
                "    const a = await createObject(\"M.E\");",
                "already in scope",
            ),
            (
                "): Promise<void>",
                "    const x = await createObject(\"M.E\", {}, { name: \"Same\" });\n    const y = await createObject(\"M.E\", {}, { name: \"Same\" });",
                "already in scope",
            ),
            (
                "): Promise<void>",
                "    if (\"text\") {\n    }",
                "a condition",
            ),
        ] {
            let error = read_one(&method(signature, body)).unwrap_err();
            assert!(error.contains(expected), "{signature} {body}: {error}");
        }
        read_one(&method("): Promise<void>", "    await closePage(16);")).unwrap();
        // A value the model stores where its nanoflow returns none.
        let stored = read_one(&method(
            "): Promise<void>",
            "    return mx<void>(\"$Kept\");",
        ))
        .unwrap();
        assert_eq!(stored.return_expression.as_deref(), Some("$Kept"));
        assert!(stored.return_type.is_none());
        let jumped = read_one(&method(
            "): Promise<void>",
            "    jump(\"again\");\n    label(\"again\");\n    await closePage();",
        ));
        assert!(jumped.is_ok() || !jumped.unwrap_err().contains("nothing after"));
    }

    #[test]
    fn a_comment_says_only_what_its_tags_say() {
        let service = |comment: &str, signature: &str| {
            format!(
                "export const S = nanoflowService(\"M\", {{\n  /**\n{comment}\n   */\n  async f({signature}): Promise<void> {{}},\n}});\n"
            )
        };
        for (comment, signature, expected) in [
            (
                "   * @nanoflow F\n   * @role User",
                "",
                "a tag of a nanoflow",
            ),
            (
                "   * @nanoflow F\n   * @roles Admin, User",
                "",
                "between spaces",
            ),
            ("   * @nanoflow F extra words", "", "the nanoflow's name"),
            (
                "   * @nanoflow F\n   * @mendixName b",
                "b: string",
                "@mendixName <parameter> <Name>",
            ),
            (
                "   * @nanoflow F\n   * @param c What",
                "b: string",
                "a parameter `c`",
            ),
            ("   * @nanoflow F\n   * @nanoflow G", "", "said twice"),
            (
                "   * @nanoflow F\n   * @returns nothing",
                "",
                "a tag of a nanoflow",
            ),
        ] {
            let error = read_one(&service(comment, signature)).unwrap_err();
            assert!(error.contains(expected), "{comment}: {error}");
        }
        // The line is the tag's own.
        let error = read_one(&service("   * @nanoflow F\n   * @role User", "")).unwrap_err();
        assert!(error.contains("s.ts:4:"), "{error}");
        // A tab separates as a space does, and no `@roles` is no role.
        let read = read_one(&service("   * @nanoflow\tF", "")).unwrap();
        assert_eq!(read.name, "F");
        assert_eq!(read.allowed_roles, Some(Vec::new()));
        let read = read_one(&service("   * @nanoflow F\n   * @roles", "")).unwrap();
        assert_eq!(read.allowed_roles, Some(Vec::new()));
    }

    #[test]
    fn a_service_file_declares_services_each_once() {
        let service = |name: &str, module: &str, nanoflow: &str| {
            format!(
                "export const {name} = nanoflowService(\"{module}\", {{\n  /** @nanoflow {nanoflow} */\n  async f(): Promise<void> {{}},\n}});\n"
            )
        };
        let refusal =
            |files: &[(&str, &str)]| read(services(files).path()).err().unwrap().to_string();
        // A service declared any other way would be a nanoflow no build sees.
        let hidden = "const S = nanoflowService(\"M\", {\n  /** @nanoflow F */\n  async f(): Promise<void> {},\n});\nexport default S;\n";
        assert!(refusal(&[("m/s.ts", hidden)]).contains("export const XService"));
        let wrapped = "export const S = wrap(nanoflowService(\"M\", {}));\n";
        assert!(refusal(&[("m/s.ts", wrapped)]).contains("export const XService"));
        let error = refusal(&[
            ("m/a.ts", &service("S", "M", "F")),
            ("m/b.ts", &service("S", "M", "G")),
        ]);
        assert!(
            error.contains("b.ts:1:") && error.contains("a.ts:1 declares S"),
            "{error}"
        );
        let error = refusal(&[
            ("m/a.ts", &service("A", "M", "F")),
            ("m/b.ts", &service("B", "M", "F")),
        ]);
        assert!(
            error.contains("b.ts:3:") && error.contains("a.ts:3 declares M.F"),
            "{error}"
        );
        let twice = "export const S = nanoflowService(\"M\", {\n  /** @nanoflow F */\n  async f(): Promise<void> {},\n  /** @nanoflow G */\n  async f(): Promise<void> {},\n});\n";
        assert!(refusal(&[("m/s.ts", twice)]).contains("each method once"));
        // A module named otherwise than its folder is a typo, not a module.
        let error = refusal(&[("main/s.ts", &service("S", "Mian", "F"))]);
        assert!(error.contains("whose folder `main` is"), "{error}");
        read(services(&[("spc_program/s.ts", &service("S", "SPCProgram", "F"))]).path()).unwrap();
        // What is not a service's source is not read as one.
        read(
            services(&[
                ("m/s.ts", &service("S", "M", "F")),
                ("m/s.test.ts", "this is not a service"),
                ("m/types.d.ts", "declare const x: number;"),
            ])
            .path(),
        )
        .unwrap();
    }

    #[test]
    fn a_file_typescript_cannot_read_is_refused_at_its_line() {
        let error = read_one("export const S = nanoflowService(\"M\", {\n  /** @nanoflow F */\n  async f(b?: string, c: string): Promise<void> {},\n});\n")
            .unwrap_err();
        assert!(error.contains("s.ts:3:"), "{error}");
    }
}
