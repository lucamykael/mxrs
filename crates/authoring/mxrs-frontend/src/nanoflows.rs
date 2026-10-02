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
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|extension| extension == "ts") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The nanoflows the services under `services` declare.
pub(crate) fn read(services: &Path) -> Result<Vec<Declared>, FrontendError> {
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
    for (path, source) in &sources {
        let allocator = Allocator::default();
        let program = parse(&allocator, path, source)?;
        for service in services_in(&program, path, source)? {
            let mut methods = HashMap::new();
            for method in &service.methods {
                methods.insert(
                    method.name.clone(),
                    Signature {
                        qualified: format!("{}.{}", service.module, method.nanoflow),
                        parameters: method.parameters.iter().map(|p| p.mendix.clone()).collect(),
                    },
                );
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
    Ok(declared)
}

fn parse<'a>(
    allocator: &'a Allocator,
    path: &Path,
    source: &'a str,
) -> Result<oxc_ast::ast::Program<'a>, FrontendError> {
    let parsed = Parser::new(allocator, source, SourceType::ts()).parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(FrontendError::Syntax {
            path: path.display().to_string(),
            detail: error.to_string(),
        });
    }
    Ok(parsed.program)
}

struct Service<'p, 'a> {
    name: String,
    module: String,
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
    roles: Option<Vec<String>>,
    parameters: Vec<Parameter>,
    returns: Option<DataType>,
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
    for statement in &program.body {
        let Statement::ExportDeclaration(export) = statement else {
            continue;
        };
        let oxc_ast::ast::Declaration::VariableDeclaration(declaration) = &export.declaration
        else {
            continue;
        };
        for declarator in &declaration.declarations {
            let Some(Expression::CallExpression(call)) = &declarator.init else {
                continue;
            };
            let Expression::Identifier(callee) = &call.callee else {
                continue;
            };
            if callee.name != "nanoflowService" {
                continue;
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
                if !property.method || !function.r#async {
                    return Err(refuse(property.span, "an async method"));
                }
                let Some(name) = property.key.static_name() else {
                    return Err(refuse(property.key.span(), "a method name"));
                };
                let doc = documentation(program, source, property.span.start).ok_or_else(|| {
                    refuse(
                        property.span,
                        "a documentation comment naming the nanoflow (`@nanoflow ...`)",
                    )
                })?;
                let tags = Tags::parse(&doc);
                let nanoflow = tags
                    .nanoflow
                    .clone()
                    .ok_or_else(|| refuse(property.span, "`@nanoflow <Name>` in its comment"))?;
                let mut parameters = Vec::new();
                for parameter in &function.params.items {
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
                let returns = match &function.return_type {
                    None => None,
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
                    roles: tags.roles.map(|roles| {
                        roles
                            .into_iter()
                            .map(|role| {
                                if role.contains('.') {
                                    role
                                } else {
                                    format!("{}.{role}", module.value)
                                }
                            })
                            .collect()
                    }),
                    parameters,
                    returns,
                    body: &body.statements,
                });
            }
            services.push(Service {
                name: name.name.to_string(),
                module: module.value.to_string(),
                methods,
            });
        }
    }
    Ok(services)
}

/// The text of the `/** ... */` comment right before `start`.
fn documentation(program: &oxc_ast::ast::Program<'_>, source: &str, start: u32) -> Option<String> {
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
    Some(lines[start..end.max(start)].join("\n"))
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

impl Tags {
    fn parse(comment: &str) -> Self {
        let mut tags = Tags::default();
        let mut documentation = Vec::new();
        for line in comment.lines() {
            let Some(tag) = line.strip_prefix('@') else {
                documentation.push(line);
                continue;
            };
            let (name, rest) = tag.split_once(' ').unwrap_or((tag, ""));
            let rest = rest.trim();
            let (ident, value) = rest.split_once(' ').unwrap_or((rest, ""));
            match name {
                "nanoflow" => tags.nanoflow = Some(rest.to_string()),
                "roles" => {
                    tags.roles = Some(rest.split_whitespace().map(str::to_string).collect());
                }
                "param" => {
                    tags.parameters.insert(ident.to_string(), value.to_string());
                }
                "mendixName" => {
                    tags.names.insert(ident.to_string(), value.to_string());
                }
                "defaultValue" => {
                    tags.defaults.insert(ident.to_string(), value.to_string());
                }
                _ => documentation.push(line),
            }
        }
        while documentation.last().is_some_and(|line| line.is_empty()) {
            documentation.pop();
        }
        tags.documentation = documentation.join("\n");
        tags
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
    /// The labels of the loops being read.
    loops: Vec<Option<String>>,
    error: Option<FrontendError>,
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

    fn declare(&self, ident: &str, mendix: &str, entity: Option<String>) {
        self.state
            .borrow_mut()
            .scopes
            .last_mut()
            .expect("a scope is open")
            .insert(
                ident.to_string(),
                Variable {
                    mendix: mendix.to_string(),
                    entity,
                },
            );
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
        for parameter in &method.parameters {
            let entity = match &parameter.ty {
                DataType::Object(entity) | DataType::List(entity) => Some(entity.clone()),
                _ => None,
            };
            self.declare(&parameter.ident, &parameter.mendix, entity);
        }
        let mut module = NanoflowModuleBuilder::new(self.module);
        module.nanoflow(method.nanoflow.as_str(), |flow| {
            flow.documentation(method.documentation.as_str());
            if let Some(roles) = &method.roles {
                flow.allowed_roles(roles.iter().map(String::as_str));
            }
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
        for statement in statements {
            self.statement(flow, statement)?;
            if self.failed() {
                break;
            }
        }
        Ok(())
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
                match &returned.argument {
                    None => {
                        flow.end();
                    }
                    Some(value) => {
                        flow.return_with(self.value(value)?);
                    }
                }
                Ok(())
            }
            Statement::BreakStatement(stop) => {
                let state = self.state.borrow();
                let known = match &stop.label {
                    None => !state.loops.is_empty(),
                    Some(label) => state
                        .loops
                        .iter()
                        .any(|seen| seen.as_deref() == Some(label.name.as_str())),
                };
                drop(state);
                if !known {
                    return Err(self.refuse(stop.span, "a loop to break"));
                }
                flow.break_loop();
                Ok(())
            }
            Statement::ContinueStatement(_) => {
                flow.continue_loop();
                Ok(())
            }
            other => Err(self.refuse(other.span(), "a statement a nanoflow can say")),
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
        for case in &switch.cases {
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
            let body = match body {
                [rest @ .., Statement::BreakStatement(stop)] if stop.label.is_none() => rest,
                other => other,
            };
            branches.push((std::mem::take(&mut values), body));
        }
        if !values.is_empty() {
            return Err(self.refuse(switch.span, "a body for every case"));
        }
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
        self.state.borrow_mut().loops.push(label);
        flow.for_each(&var(list.mendix.as_str()), iterator.as_str(), |flow, _| {
            if self.failed() {
                return;
            }
            self.state.borrow_mut().scopes.push(HashMap::new());
            self.declare(&item.name, &iterator, list.entity.clone());
            if let Err(error) = self.statements(flow, body) {
                self.fail(error);
            }
            self.state.borrow_mut().scopes.pop();
        });
        self.state.borrow_mut().loops.pop();
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
        self.state.borrow_mut().loops.push(label);
        flow.while_loop(condition, |flow| self.nested(flow, body));
        self.state.borrow_mut().loops.pop();
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
            Expression::NumericLiteral(number) => Ok(mx(number
                .raw
                .as_ref()
                .map_or_else(|| number.value.to_string(), ToString::to_string))),
            Expression::UnaryExpression(unary)
                if unary.operator == oxc_ast::ast::UnaryOperator::UnaryNegation =>
            {
                match &unary.argument {
                    Expression::NumericLiteral(number) => Ok(mx(format!(
                        "-{}",
                        number
                            .raw
                            .as_ref()
                            .map_or_else(|| number.value.to_string(), ToString::to_string)
                    ))),
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
            self.declare(ident, mendix, entity);
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
        let empty: Vec<(String, &Expression<'_>, Span)> = Vec::new();
        let options_at = |index: usize| -> Read<Vec<(String, &Expression<'_>, Span)>> {
            match arguments.get(index) {
                Some(options) => self.entries(options),
                None => Ok(Vec::new()),
            }
        };
        match callee.name.as_str() {
            "onError" | "disabled" => {
                let (activity, handler) = match (callee.name.as_str(), arguments.as_slice()) {
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
                self.activity(flow, inner, binding)
            }
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
                if let Some(by) = Self::option(&options, "by") {
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
                let short = entity
                    .rsplit_once('.')
                    .map_or(entity.as_str(), |(_, name)| name);
                let name = self
                    .result_name(&binding, &options, Some(format!("{short}List")), span)?
                    .expect("a default");
                flow.create_list_of(name.as_str(), entity.as_str());
                self.bind(&binding, Some(&name), Some(entity.clone()), span)
            }
            "changeList" => self.change_list(flow, &arguments, &binding, span),
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
                let [ty, value, rest @ ..] = arguments.as_slice() else {
                    return Err(self.refuse(span, "createVariable(type, value)"));
                };
                let options = match rest.first() {
                    Some(options) => self.entries(options)?,
                    None => Vec::new(),
                };
                let (ty, entity) = self.variable_type(ty)?;
                let value = self.value(value)?;
                let name = self
                    .result_name(&binding, &options, None, span)?
                    .ok_or_else(|| self.refuse(span, "a name for the variable"))?;
                flow.create_variable(name.as_str(), ty, value);
                self.bind(&binding, Some(&name), entity, span)
            }
            "changeVariable" => {
                let [variable, value] = arguments.as_slice() else {
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
                let [level, node, message, rest @ ..] = arguments.as_slice() else {
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
                match arguments.as_slice() {
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
                let [kind, text, rest @ ..] = arguments.as_slice() else {
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
            _ => {
                let _ = &empty;
                Err(self.refuse(call.callee.span(), "an activity of src/mxrs/flows.ts"))
            }
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
        let source = var(list.mendix.as_str());
        let changes = match key.as_str() {
            "add" => Some(ListChange::Add),
            "remove" => Some(ListChange::Remove),
            "replace" => Some(ListChange::Set),
            "clear" => Some(ListChange::Clear),
            _ => None,
        };
        if let Some(change) = changes {
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
}
