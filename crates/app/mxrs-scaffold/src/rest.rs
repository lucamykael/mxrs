//! A published REST operation, scaffolded the way the importer writes one:
//! the operation in its service's declaration, in
//! `src/controllers/<module>/<service>.rs`, and — in a project an axum
//! router serves — the controller function serving it.
//!
//! A project the importer gave an axum HTTP layer gets the route table
//! that binds each operation to its handler; any other project gets the
//! declaration alone, which every build writes into the model all the
//! same. A service the project already declares gains the operation; a new
//! one is published to anyone, as Studio Pro publishes a new service.

use std::path::Path;

use crate::templates::snake_case;
use crate::transaction::Transaction;
use crate::{Result, ScaffoldError};

/// What `mxrs published-rest new` says of the operation besides the
/// handler microflow it calls. Everything left unsaid is derived from the
/// handler's name.
#[derive(Debug, Clone, Default)]
pub struct RestOperationOptions {
    /// The service (`--service`); the module's only one, else
    /// `<Module>Api`.
    pub service: Option<String>,
    /// A new service's path (`--path`); `rest/<service>/v1` by default.
    pub path: Option<String>,
    /// The resource (`--resource`); the subject the handler's name names.
    pub resource: Option<String>,
    /// `get`, `post`, `put`, `patch` or `delete` (`--method`); what the
    /// handler's verb does.
    pub method: Option<String>,
    /// The operation's path under its resource (`--operation-path`):
    /// `{id}` for a verb that addresses one object, else the resource's own.
    pub operation_path: Option<String>,
    /// Query string parameters (`--query`, repeatable).
    pub query: Vec<String>,
}

/// The operation, every choice made.
pub(crate) struct RestOperation {
    module_name: String,
    module_stem: String,
    microflow: String,
    service: String,
    service_stem: String,
    service_path: String,
    resource: String,
    method: &'static str,
    path: String,
    /// `(name, from the path)`: every `{name}` of the path, then each query
    /// parameter. Each binds the microflow parameter of its name.
    parameters: Vec<(String, bool)>,
}

impl RestOperation {
    /// The handler microflow's documentation: the operation calling it.
    pub(crate) fn handler_docs(&self) -> Vec<String> {
        vec![format!(
            "Published REST handler: `{} {}` of `{}` calls it.",
            self.method.to_ascii_uppercase(),
            route(self),
            self.service
        )]
    }

    /// The statements declaring the handler microflow's parameters.
    pub(crate) fn microflow_body(&self) -> Vec<String> {
        self.parameters
            .iter()
            .map(|(name, _)| format!("flow.parameter::<MxString>({name:?}, |_| {{}});"))
            .collect()
    }

    fn addresses_one(&self) -> bool {
        self.path
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.starts_with('{') && segment.ends_with('}'))
    }

    /// `|operation| { ... }`, or `|_| {}` when the operation says nothing
    /// besides its path and microflow.
    fn configure(&self) -> String {
        if self.parameters.is_empty() {
            return "|_| {}".to_string();
        }
        let parameters: String = self
            .parameters
            .iter()
            .map(|(name, from_path)| {
                format!(
                    ".{}_parameter({name:?}, RestParameterType::String)",
                    if *from_path { "path" } else { "query" }
                )
            })
            .collect();
        format!("|operation| {{ operation{parameters}; }}")
    }

    /// The resource's closure parameter: its name, when Rust can spell it.
    fn binding(&self) -> String {
        let binding = stem(&self.resource);
        if binding.is_empty() || binding.starts_with(|c: char| c.is_ascii_digit()) {
            "resource".to_string()
        } else {
            binding
        }
    }
}

/// Decides the operation `module_name.handler` publishes.
pub(crate) fn plan(
    transaction: &Transaction,
    root: &Path,
    module_name: &str,
    handler: &str,
    options: &RestOperationOptions,
) -> Result<RestOperation> {
    let module_stem = snake_case(module_name);
    let (_, core) = crate::service::split_prefix(handler);
    let words: Vec<&str> = core.split('_').filter(|word| !word.is_empty()).collect();
    let verb = words.last().copied().unwrap_or_default();
    let method = match options.method.as_deref() {
        Some(method @ ("get" | "post" | "put" | "patch" | "delete")) => {
            ["get", "post", "put", "patch", "delete"]
                .into_iter()
                .find(|known| *known == method)
                .expect("a method just matched")
        }
        Some(other) => {
            return Err(invalid(
                "HTTP method (expected get, post, put, patch or delete)",
                other,
            ));
        }
        None => match verb {
            "Create" | "Add" | "New" | "Post" | "Submit" => "post",
            "Update" | "Edit" | "Save" | "Change" | "Put" => "put",
            "Patch" => "patch",
            "Delete" | "Remove" | "Destroy" => "delete",
            _ => "get",
        },
    };
    let path = match &options.operation_path {
        Some(path) => path.trim_matches('/').to_string(),
        None if matches!(
            verb,
            "Show"
                | "Get"
                | "Read"
                | "View"
                | "Retrieve"
                | "Update"
                | "Edit"
                | "Save"
                | "Change"
                | "Put"
                | "Patch"
                | "Delete"
                | "Remove"
                | "Destroy"
        ) =>
        {
            "{id}".to_string()
        }
        None => String::new(),
    };
    let resource = match &options.resource {
        Some(resource) => resource.trim_matches('/').to_string(),
        None => {
            let subject = if words.len() > 1 {
                &words[..words.len() - 1]
            } else {
                &words[..]
            };
            snake_case(&subject.join("_"))
        }
    };
    if resource.is_empty() {
        return Err(invalid("resource", &resource));
    }
    let mut parameters = Vec::new();
    for segment in path.split('/') {
        if let Some(name) = segment
            .strip_prefix('{')
            .and_then(|segment| segment.strip_suffix('}'))
        {
            parameters.push((parameter_name(name)?, true));
        }
    }
    for name in &options.query {
        parameters.push((parameter_name(name)?, false));
    }
    let mut seen = std::collections::HashSet::new();
    if let Some((name, _)) = parameters.iter().find(|(name, _)| !seen.insert(name)) {
        return Err(invalid("parameter (named twice)", name));
    }
    let folder = root.join("src/controllers").join(&module_stem);
    let service = match &options.service {
        Some(service) => crate::artifact::identifier(service, "service")?.to_string(),
        None => match declared_services(transaction, &folder)?.as_slice() {
            [only] => only.clone(),
            _ => format!("{module_name}Api"),
        },
    };
    let service_stem = stem(&service);
    let service_path = options
        .path
        .clone()
        .unwrap_or_else(|| format!("rest/{}/v1", service.to_ascii_lowercase()))
        .trim_matches('/')
        .to_string();
    Ok(RestOperation {
        module_name: module_name.to_string(),
        module_stem,
        microflow: format!("{module_name}.{handler}"),
        service,
        service_stem,
        service_path,
        resource,
        method,
        path,
        parameters,
    })
}

/// A file or identifier stem for a name: snake case, a keyword escaped.
fn stem(name: &str) -> String {
    let mut stem = snake_case(name);
    if crate::is_rust_keyword(&stem) {
        stem.push('_');
    }
    stem
}

fn parameter_name(name: &str) -> Result<String> {
    let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(name.to_string())
    } else {
        Err(invalid("parameter", name))
    }
}

fn invalid(label: &'static str, value: &str) -> ScaffoldError {
    ScaffoldError::InvalidIdentifier {
        label,
        value: value.to_string(),
    }
}

/// The services a module's controllers folder declares, by their names.
fn declared_services(transaction: &Transaction, folder: &Path) -> Result<Vec<String>> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Ok(Vec::new());
    };
    let mut services = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let Some(source) = transaction.content(&path)? else {
            continue;
        };
        if let Some(name) = declared_service_name(&source) {
            services.push(name);
        }
    }
    services.sort();
    Ok(services)
}

/// The name a service file declares its service by.
fn declared_service_name(source: &str) -> Option<String> {
    ["Service::new(", "PublishedRestServiceBuilder::new("]
        .into_iter()
        .find_map(|opening| {
            let rest = &source[source.find(opening)? + opening.len()..];
            let rest = rest.trim_start().strip_prefix('"')?;
            Some(rest[..rest.find('"')?].to_string())
        })
}

/// Whether the project serves its services with the axum router the
/// importer writes: its controllers own an `AppState` and a `router`.
fn served_by_axum(transaction: &Transaction, root: &Path) -> Result<bool> {
    let controllers = root.join("src/controllers");
    Ok(transaction
        .content(&controllers.join("state.rs"))?
        .is_some()
        && transaction
            .content(&controllers.join("mod.rs"))?
            .is_some_and(|source| source.contains("pub fn router(state: AppState)")))
}

/// Publishes `operation`: its service's declaration, created or gaining
/// it, and — when an axum router serves the project — its handler.
pub(crate) fn publish(
    transaction: &mut Transaction,
    root: &Path,
    operation: &RestOperation,
) -> Result<()> {
    let axum = served_by_axum(transaction, root)?;
    let controllers = root.join("src/controllers");
    let folder = controllers.join(&operation.module_stem);
    let service_file = folder.join(format!("{}.rs", operation.service_stem));
    let existing = transaction.content(&service_file)?;
    if let Some(source) = &existing {
        if declared_service_name(source).as_deref() != Some(operation.service.as_str()) {
            return Err(ScaffoldError::InvalidProjectSource {
                path: service_file.display().to_string(),
                reason: format!(
                    "it does not declare the service {}, so the operation has nowhere to go",
                    operation.service
                ),
            });
        }
        if operation_declared(source, operation) {
            return Err(ScaffoldError::FileExists(format!(
                "{} already declares {} {}",
                service_file.display(),
                operation.method.to_ascii_uppercase(),
                route(operation)
            )));
        }
    }
    if axum {
        publish_served(transaction, root, operation, existing)?;
    } else {
        publish_declared(transaction, operation, &service_file, existing)?;
    }
    connect(transaction, root, operation)
}

/// The route `operation` answers at.
fn route(operation: &RestOperation) -> String {
    let joined = [
        operation.service_path.as_str(),
        operation.resource.as_str(),
        operation.path.as_str(),
    ]
    .into_iter()
    .filter(|segment| !segment.is_empty())
    .collect::<Vec<_>>()
    .join("/");
    format!("/{joined}")
}

/// Whether a service file already declares the operation's method at its
/// resource and path.
fn operation_declared(source: &str, operation: &RestOperation) -> bool {
    let resource = format!(".resource({:?}", operation.resource);
    source.contains(&resource)
        && source.contains(&format!(".{}({:?},", operation.method, operation.path))
}

/// A project no axum router serves: the declaration alone.
fn publish_declared(
    transaction: &mut Transaction,
    operation: &RestOperation,
    service_file: &Path,
    existing: Option<String>,
) -> Result<()> {
    let binding = operation.binding();
    let call = format!(
        ".{}({:?}, {:?}, {})",
        operation.method,
        operation.path,
        operation.microflow,
        operation.configure(),
    );
    let resource = format!(
        "service.resource({:?}, |{binding}| {{ {binding}{call}; }});",
        operation.resource,
    );
    let Some(source) = existing else {
        return transaction.create(
            service_file,
            format!(
                "{}//!\n//! This file declares the service, and every build writes it into the\n//! model: the runtime serving the model serves it. No router of this\n//! project's routes it.\n\nuse mxrs::prelude::*;\n\n#[declaration(module = {:?})]\npub fn {}(module: &mut ModuleBuilder) {{\n    let mut service = PublishedRestServiceBuilder::new({:?}, {:?});\n    {resource}\n    module.published_rest_service(service);\n}}\n",
                header(operation),
                operation.module_name,
                declaration_function(operation),
                operation.service,
                operation.service_path,
            ),
        );
    };
    // An operation of a resource the service has joins that resource.
    if let Some(edited) = extend_resource(&source, &operation.resource, &binding, &call) {
        return edit(transaction, service_file, &source, edited);
    }
    let closing = "    module.published_rest_service(service);";
    let Some(at) = source.find(closing) else {
        return Err(ScaffoldError::InvalidProjectSource {
            path: service_file.display().to_string(),
            reason: "its declaration does not end in `module.published_rest_service(service);`, where the operation would join it".to_string(),
        });
    };
    let edited = format!("{}    {resource}\n{}", &source[..at], &source[at..]);
    edit(transaction, service_file, &source, edited)
}

/// A project an axum router serves: the route table binding the operation
/// to its handler, and the handler.
fn publish_served(
    transaction: &mut Transaction,
    root: &Path,
    operation: &RestOperation,
    existing: Option<String>,
) -> Result<()> {
    let folder = root.join("src/controllers").join(&operation.module_stem);
    let controller = controller_stem(transaction, &folder, operation)?;
    let controller_file = folder.join(format!("{controller}.rs"));
    let controller_source = transaction.content(&controller_file)?;
    let authentication = match &existing {
        Some(source) if source.contains("const ALLOWED_ROLES") => Authentication::Basic,
        Some(source) if source.contains("const AUTHENTICATION") => Authentication::Refused,
        _ => Authentication::Public,
    };
    let handler = handler_name(controller_source.as_deref(), operation);
    let binding = operation.binding();
    let call = format!(
        ".{}({:?}, {:?}, {controller}::{handler}, {})",
        operation.method,
        operation.path,
        operation.microflow,
        operation.configure(),
    );
    let link = format!(
        ".resource({:?}, |{binding}| {{ {binding}{call}; }})",
        operation.resource,
    );
    let service_file = folder.join(format!("{}.rs", operation.service_stem));
    match existing {
        None => transaction.create(
            &service_file,
            format!(
                "{}//!\n//! This file declares the service — every build writes it into the model —\n//! and binds each operation to the controller function serving it, beside\n//! this file. Its router serves exactly what it declares.\n\nuse axum::Router;\nuse mxrs::prelude::*;\nuse mxrs::rest::Service;\n\nuse super::{controller};\nuse crate::controllers::AppState;\n\n/// What the service publishes, each operation with the function serving it.\npub fn service() -> Service<AppState> {{\n    Service::new({:?}, {:?}){link}\n}}\n\n#[declaration(module = {:?})]\npub fn {}(module: &mut ModuleBuilder) {{\n    module.published_rest_service(service().declaration());\n}}\n\npub fn router() -> Router<AppState> {{\n    service().router()\n}}\n\n#[cfg(test)]\nmod tests {{\n    /// Every operation the service declares is served, and no two answer\n    /// one method at one route: building the router would panic.\n    #[test]\n    fn the_router_serves_the_service() {{\n        let _router = super::router();\n    }}\n}}\n",
                header(operation),
                operation.service,
                operation.service_path,
                operation.module_name,
                declaration_function(operation),
            ),
        )?,
        Some(source) => {
            let opening = "pub fn service() -> Service<AppState> {";
            let end = source.find(opening).and_then(|start| {
                source[start..]
                    .find("\n}\n")
                    .map(|offset| start + offset)
            });
            let Some(end) = end else {
                return Err(ScaffoldError::InvalidProjectSource {
                    path: service_file.display().to_string(),
                    reason: format!(
                        "it has no `{opening} ... }}` whose statement the operation would join"
                    ),
                });
            };
            // An operation of a resource the service has joins that resource.
            let edited = extend_resource(&source, &operation.resource, &binding, &call)
                .unwrap_or_else(|| {
                    format!("{}\n        {link}{}", &source[..end], &source[end..])
                });
            let edited = import_controller(&edited, &controller);
            edit(transaction, &service_file, &source, edited)?;
        }
    }
    let function = handler_function(operation, &handler, authentication);
    match controller_source {
        None => transaction.create(
            &controller_file,
            format!(
                "//! The `{}` resource of `{}`, served under\n//! `/{}`.\n//!\n//! One function per operation, each calling the microflow the model bound\n//! to it. `{}` declares each operation and routes it here.\n\n{}\n{function}",
                operation.resource,
                operation.service,
                [operation.service_path.as_str(), operation.resource.as_str()]
                    .into_iter()
                    .filter(|segment| !segment.is_empty())
                    .collect::<Vec<_>>()
                    .join("/"),
                operation.service_stem,
                imports(operation, authentication).join("\n"),
            ),
        ),
        Some(source) => {
            let present = imported(&source);
            let missing: Vec<String> = needed(operation, authentication)
                .into_iter()
                .filter(|path| !present.contains(path))
                .map(|path| format!("use {path};"))
                .collect();
            let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
            let last_use = lines
                .iter()
                .rposition(|line| line.starts_with("use "))
                .map_or(0, |position| position + 1);
            lines.splice(last_use..last_use, missing);
            let mut edited = lines.join("\n");
            edited.push_str("\n\n");
            edited.push_str(&function);
            edit(transaction, &controller_file, &source, edited)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Authentication {
    Public,
    Basic,
    /// The model asks for authentication the HTTP layer does not offer:
    /// the handler refuses.
    Refused,
}

/// The function declaring a service file's service: the file's own name,
/// unless that is a function the route table has already.
fn declaration_function(operation: &RestOperation) -> String {
    match operation.service_stem.as_str() {
        stem @ ("service" | "router") => format!("declare_{stem}"),
        stem => stem.to_string(),
    }
}

/// A route table's opening paragraph: the service, and who may call it.
fn header(operation: &RestOperation) -> String {
    format!(
        "//! `{}` — the REST service the {} module publishes at `/{}`.\n//!\n//! The model publishes this service to anyone: it declares no\n//! authentication type and no allowed role. Its operations therefore run as\n//! the project's guest user role, or as a caller holding no role at all when\n//! the project declares none — which entity access then denies.\n",
        operation.service, operation.module_name, operation.service_path,
    )
}

/// The controller of the operation's resource: `<resource>_controller`,
/// unless another service of the module has that one.
fn controller_stem(
    transaction: &Transaction,
    folder: &Path,
    operation: &RestOperation,
) -> Result<String> {
    let preferred = format!("{}_controller", stem(&operation.resource));
    let owned_elsewhere = transaction
        .content(&folder.join(format!("{preferred}.rs")))?
        .is_some_and(|source| !source.contains(&format!("of `{}`", operation.service)));
    Ok(if owned_elsewhere {
        format!("{}_{preferred}", operation.service_stem)
    } else {
        preferred
    })
}

/// The handler's name: what the operation does to the resource, unless
/// the controller has a function of that name already, else after the
/// microflow it calls.
fn handler_name(controller: Option<&str>, operation: &RestOperation) -> String {
    let taken = |name: &str| {
        controller.is_some_and(|source| source.contains(&format!("pub async fn {name}(")))
    };
    let conventional = match operation.method {
        "get" if operation.addresses_one() => "show",
        "get" => "index",
        "post" => "create",
        "put" | "patch" => "update",
        _ => "destroy",
    };
    if !taken(conventional) {
        return conventional.to_string();
    }
    let handler = operation.microflow.rsplit('.').next().unwrap_or_default();
    let base = crate::service::method_name(crate::service::split_prefix(handler).1, handler);
    let mut name = base.clone();
    let mut suffix = 2;
    while taken(&name) {
        name = format!("{base}_{suffix}");
        suffix += 1;
    }
    name
}

/// The handler: it signs the caller in as the service asks, binds the
/// operation's parameters and calls the microflow.
fn handler_function(
    operation: &RestOperation,
    handler: &str,
    authentication: Authentication,
) -> String {
    if authentication == Authentication::Refused {
        return format!(
            "/// Would call `{}`; refuses instead, because the model's\n/// authentication is not available here.\npub async fn {handler}() -> ApiError {{\n    ApiError::unsupported_authentication(AUTHENTICATION)\n}}\n",
            operation.microflow
        );
    }
    let path = operation.parameters.iter().any(|(_, from_path)| *from_path);
    let query = operation.parameters.iter().any(|(_, from_path)| !from_path);
    let mut extractors = String::from("\n    State(state): State<AppState>,");
    if authentication == Authentication::Basic {
        extractors.push_str("\n    headers: HeaderMap,");
    }
    if path {
        extractors.push_str("\n    Path(path): Path<HashMap<String, String>>,");
    }
    if query {
        extractors.push_str("\n    Query(query): Query<HashMap<String, String>>,");
    }
    let caller = match authentication {
        Authentication::Basic => "state.basic_caller(&headers, REALM, ALLOWED_ROLES)?",
        _ => "state.anonymous()",
    };
    let binding = if operation.parameters.is_empty() {
        "let arguments"
    } else {
        "let mut arguments"
    };
    let mut body = format!("    let caller = {caller};\n    {binding} = Variables::new();\n");
    for (name, from_path) in &operation.parameters {
        let value = if *from_path {
            format!("FlowValue::String(path.get({name:?}).cloned().unwrap_or_default())")
        } else {
            format!(
                "query.get({name:?}).map_or(FlowValue::Empty, |value| FlowValue::String(value.clone()))"
            )
        };
        body.push_str(&format!(
            "    arguments.insert({name:?}.to_string(), {value});\n"
        ));
    }
    format!(
        "/// Calls `{microflow}`.\npub async fn {handler}({extractors}\n) -> Result<Json<Value>, ApiError> {{\n{body}    Ok(Json(state.call({microflow:?}, arguments, &caller)?))\n}}\n",
        microflow = operation.microflow,
    )
}

/// The full paths a handler names.
fn needed(operation: &RestOperation, authentication: Authentication) -> Vec<String> {
    let service = &operation.service_stem;
    if authentication == Authentication::Refused {
        return vec![
            format!("super::{service}::AUTHENTICATION"),
            "crate::controllers::ApiError".to_string(),
        ];
    }
    let path = operation.parameters.iter().any(|(_, from_path)| *from_path);
    let query = operation.parameters.iter().any(|(_, from_path)| !from_path);
    let mut paths = Vec::new();
    if path || query {
        paths.push("std::collections::HashMap".to_string());
    }
    paths.push("axum::Json".to_string());
    if path {
        paths.push("axum::extract::Path".to_string());
    }
    if query {
        paths.push("axum::extract::Query".to_string());
    }
    paths.push("axum::extract::State".to_string());
    if authentication == Authentication::Basic {
        paths.push("axum::http::HeaderMap".to_string());
    }
    if !operation.parameters.is_empty() {
        paths.push("mxrs::ports::FlowValue".to_string());
    }
    paths.push("mxrs::ports::Variables".to_string());
    paths.push("serde_json::Value".to_string());
    if authentication == Authentication::Basic {
        paths.push(format!("super::{service}::ALLOWED_ROLES"));
        paths.push(format!("super::{service}::REALM"));
    }
    paths.push("crate::controllers::ApiError".to_string());
    paths.push("crate::controllers::AppState".to_string());
    paths
}

/// A new controller's `use` lines, grouped and merged as the importer
/// writes them.
fn imports(operation: &RestOperation, authentication: Authentication) -> Vec<String> {
    let mut groups: Vec<Vec<String>> = vec![Vec::new(), Vec::new(), Vec::new()];
    let mut merged: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for path in needed(operation, authentication) {
        let (parent, name) = path.rsplit_once("::").expect("a path has a parent");
        merged
            .entry(parent.to_string())
            .or_default()
            .push(name.to_string());
    }
    for (parent, mut names) in merged {
        names.sort();
        let line = match names.as_slice() {
            [name] => format!("use {parent}::{name};"),
            names => format!("use {parent}::{{{}}};", names.join(", ")),
        };
        let group = if parent.starts_with("std") {
            0
        } else if parent.starts_with("super") || parent.starts_with("crate") {
            2
        } else {
            1
        };
        groups[group].push(line);
    }
    groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .map(|group| format!("{}\n", group.join("\n")))
        .collect()
}

/// Every path a source's `use` items bring in.
fn imported(source: &str) -> std::collections::HashSet<String> {
    fn walk(tree: &syn::UseTree, prefix: &str, found: &mut std::collections::HashSet<String>) {
        let join = |name: &str| {
            if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}::{name}")
            }
        };
        match tree {
            syn::UseTree::Path(path) => walk(&path.tree, &join(&path.ident.to_string()), found),
            syn::UseTree::Name(name) => {
                found.insert(join(&name.ident.to_string()));
            }
            syn::UseTree::Rename(rename) => {
                found.insert(join(&rename.ident.to_string()));
            }
            syn::UseTree::Glob(_) => {
                found.insert(join("*"));
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    walk(tree, prefix, found);
                }
            }
        }
    }
    let mut found = std::collections::HashSet::new();
    if let Ok(file) = syn::parse_file(source) {
        for item in file.items {
            if let syn::Item::Use(item) = item {
                walk(&item.tree, "", &mut found);
            }
        }
    }
    found
}

/// The route table's `use super::...` line, naming `controller` too.
fn import_controller(source: &str, controller: &str) -> String {
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    let Some(position) = lines
        .iter()
        .position(|line| line.starts_with("use super::"))
    else {
        return source.to_string();
    };
    let named = lines[position]
        .trim_start_matches("use super::")
        .trim_end_matches(';')
        .trim_matches(|c| c == '{' || c == '}')
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if named.iter().any(|name| name == controller) {
        return source.to_string();
    }
    let mut names = named;
    names.push(controller.to_string());
    names.sort();
    lines[position] = match names.as_slice() {
        [name] => format!("use super::{name};"),
        names => format!("use super::{{{}}};", names.join(", ")),
    };
    let mut joined = lines.join("\n");
    joined.push('\n');
    joined
}

/// Writes an edited source. A file rustfmt would leave as it is stays so;
/// one its author formats otherwise keeps their formatting, the edit
/// written as the scaffold wrote it.
fn edit(transaction: &mut Transaction, path: &Path, original: &str, edited: String) -> Result<()> {
    let formatted = crate::format_rust(original.to_string()) == original;
    transaction.write(
        path,
        if formatted {
            crate::format_rust(edited)
        } else {
            edited
        },
    )
}

/// Joins the service file to the crate: its module's controllers folder,
/// `src/controllers`, and — in an axum project — the router merging every
/// route table.
fn connect(transaction: &mut Transaction, root: &Path, operation: &RestOperation) -> Result<()> {
    let controllers = root.join("src/controllers");
    let index = controllers.join("mod.rs");
    if transaction.content(&index)?.is_none() {
        transaction.create(&index, crate::templates::empty_concept_index("controllers"))?;
    }
    crate::artifact::declare_child_module(transaction, &root.join("src/lib.rs"), "controllers")?;
    let folder = controllers.join(&operation.module_stem);
    let module_index = folder.join("mod.rs");
    if transaction.content(&module_index)?.is_none() {
        transaction.create(
            &module_index,
            format!(
                "//! The {} module's published REST services: one route table per\n//! service, one controller per resource.\n",
                operation.module_name
            ),
        )?;
    }
    declare_controllers_module(transaction, &index, &operation.module_stem)?;
    crate::artifact::declare_child_module(transaction, &module_index, &operation.service_stem)?;
    if served_by_axum(transaction, root)? {
        for stem in transaction
            .created()
            .iter()
            .filter(|path| path.parent() == Some(folder.as_path()))
            .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()))
            .filter(|stem| *stem != "mod")
            .map(str::to_string)
            .collect::<Vec<_>>()
        {
            crate::artifact::declare_child_module(transaction, &module_index, &stem)?;
        }
        merge_route_table(transaction, &index, operation)?;
    }
    Ok(())
}

/// Declares a module's controllers folder in `src/controllers/mod.rs` —
/// above the server stub a project of another preset has there.
fn declare_controllers_module(
    transaction: &mut Transaction,
    index: &Path,
    module_stem: &str,
) -> Result<()> {
    let source = transaction
        .content(index)?
        .ok_or_else(|| ScaffoldError::AggregatorNotFound(index.display().to_string()))?;
    let Some(stub) = source.find("\npub mod server {") else {
        return crate::artifact::declare_child_module(transaction, index, module_stem);
    };
    let declaration = format!("pub mod {module_stem};");
    if source.lines().any(|line| line == declaration) {
        return Ok(());
    }
    let mut declared: Vec<String> = source[..stub]
        .lines()
        .filter(|line| line.starts_with("pub mod ") && line.ends_with(';'))
        .map(str::to_string)
        .collect();
    let opening = source[..stub]
        .lines()
        .take_while(|line| !(line.starts_with("pub mod ") && line.ends_with(';')))
        .collect::<Vec<_>>()
        .join("\n");
    declared.push(declaration);
    declared.sort();
    let rest = &source[stub + 1..];
    let opening = opening.trim_end();
    transaction.write(
        index,
        format!("{opening}\n\n{}\n\n{rest}", declared.join("\n")),
    )
}

/// Merges a new route table into the project's router.
fn merge_route_table(
    transaction: &mut Transaction,
    index: &Path,
    operation: &RestOperation,
) -> Result<()> {
    let source = transaction
        .content(index)?
        .ok_or_else(|| ScaffoldError::AggregatorNotFound(index.display().to_string()))?;
    let merge = format!(
        "        .merge(crate::controllers::{}::{}::router())",
        operation.module_stem, operation.service_stem
    );
    if source.contains(merge.trim_start()) {
        return Ok(());
    }
    let Some(at) = source.find("        .with_state(state)") else {
        return Err(ScaffoldError::InvalidProjectSource {
            path: index.display().to_string(),
            reason: "its router has no `.with_state(state)` to merge the route table before"
                .to_string(),
        });
    };
    transaction.write(
        index,
        format!("{}{merge}\n{}", &source[..at], &source[at..]),
    )
}

/// `source` with `call` chained onto the statement that declares the
/// resource's operations — `orders.get(...)` gaining `.post(...)` — when
/// the resource is there and its closure ends in such a statement.
fn extend_resource(source: &str, resource: &str, binding: &str, call: &str) -> Option<String> {
    let marker = format!(".resource({resource:?}, |");
    let start = source.find(&marker)? + marker.len();
    let bar = start + source[start..].find('|')?;
    let parameter = source[start..bar].trim();
    let open = bar + 1 + source[bar + 1..].find('{')?;
    if !source[bar + 1..open].trim().is_empty() {
        return None;
    }
    let (close, statements) = closure_body(source, open)?;
    if parameter == "_" {
        // A resource that said nothing: its closure takes the operation.
        if !source[open + 1..close].trim().is_empty() {
            return None;
        }
        return Some(format!(
            "{}{binding}| {{ {binding}{call}; }}{}",
            &source[..start],
            &source[close + 1..]
        ));
    }
    let last = *statements.last()?;
    if !source[last + 1..close].trim().is_empty() {
        return None;
    }
    let opening = statements
        .len()
        .checked_sub(2)
        .map_or(open + 1, |previous| statements[previous] + 1);
    if !source[opening..last].trim_start().starts_with(parameter) {
        return None;
    }
    Some(format!("{}{call}{}", &source[..last], &source[last..]))
}

/// The `}` closing the block `{` at `open` opens, and the `;` ending each
/// of its statements — string literals and line comments read past.
fn closure_body(source: &str, open: usize) -> Option<(usize, Vec<usize>)> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut statements = Vec::new();
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'{' | b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.checked_sub(1)?,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((index, statements));
                }
            }
            b';' if depth == 1 => statements.push(index),
            _ => {}
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(handler: &str, options: &RestOperationOptions) -> RestOperation {
        let root = tempfile::tempdir().unwrap();
        plan(
            &Transaction::default(),
            root.path(),
            "Sales",
            handler,
            options,
        )
        .unwrap()
    }

    /// What the handler's name says decides what is left unsaid: its verb
    /// the method and whether it addresses one object, its subject the
    /// resource, its module the service.
    #[test]
    fn an_operation_is_derived_from_its_handler() {
        let show = planned("MF_Order_Show", &RestOperationOptions::default());
        assert_eq!(
            (show.method, show.path.as_str(), show.resource.as_str()),
            ("get", "{id}", "order")
        );
        assert_eq!(show.service, "SalesApi");
        assert_eq!(show.service_path, "rest/salesapi/v1");
        assert_eq!(
            show.microflow_body(),
            ["flow.parameter::<MxString>(\"id\", |_| {});"]
        );
        let create = planned(
            "ACT_OrderLine_Create",
            &RestOperationOptions {
                query: vec!["note".into()],
                ..Default::default()
            },
        );
        assert_eq!(
            (
                create.method,
                create.path.as_str(),
                create.resource.as_str()
            ),
            ("post", "", "order_line")
        );
        assert_eq!(
            create.configure(),
            "|operation| { operation.query_parameter(\"note\", RestParameterType::String); }"
        );
        assert_eq!(route(&create), "/rest/salesapi/v1/order_line");
    }

    /// A parameter is a microflow parameter too: it must be a name, said
    /// once.
    #[test]
    fn parameters_are_names_said_once() {
        let root = tempfile::tempdir().unwrap();
        for options in [
            RestOperationOptions {
                operation_path: Some("{id}/{id}".into()),
                ..Default::default()
            },
            RestOperationOptions {
                query: vec!["not-a-name".into()],
                ..Default::default()
            },
            RestOperationOptions {
                method: Some("trace".into()),
                ..Default::default()
            },
        ] {
            assert!(
                plan(
                    &Transaction::default(),
                    root.path(),
                    "Sales",
                    "MF_Order_Show",
                    &options
                )
                .is_err()
            );
        }
    }

    /// In a project no axum router serves, the operation is declared
    /// alone, and its service and folders are joined to the crate; a second
    /// operation of the resource joins the first, and the same operation
    /// again is refused.
    #[test]
    fn a_project_without_a_router_declares_the_operation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("shop");
        crate::generate_project(&crate::ProjectScaffold::new("Shop", "11.12.1", &root)).unwrap();
        let scaffold = |name: &str, rest: RestOperationOptions| {
            crate::scaffold_artifact(
                &crate::ArtifactScaffold::new(crate::ArtifactKind::PublishedRest, name, &root)
                    .rest(rest),
            )
        };
        scaffold("Main.MF_Order_Show", RestOperationOptions::default()).unwrap();
        scaffold(
            "Main.MF_Order_Create",
            RestOperationOptions {
                query: vec!["note".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(scaffold("Main.MF_Order_Read", RestOperationOptions::default()).is_err());
        let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
        let service = read("src/controllers/main/main_api.rs");
        assert!(
            service.contains(
                "    service.resource(\"order\", |order| {\n        order\n            .get(\"{id}\", \"Main.MF_Order_Show\", |operation| {"
            ) && service.contains(".post(\"\", \"Main.MF_Order_Create\", |operation| {"),
            "{service}"
        );
        assert!(read("src/lib.rs").contains("pub mod controllers;"));
        assert!(read("src/controllers/mod.rs").contains("pub mod main;"));
        assert!(read("src/controllers/main/mod.rs").contains("pub mod main_api;"));
        let handlers = read("src/services/main/main_service.rs");
        assert!(
            handlers.contains("flow.parameter::<MxString>(\"note\", |_| {});"),
            "{handlers}"
        );
    }

    /// A new operation of a resource joins the statement declaring its
    /// operations; braces inside a path are text.
    #[test]
    fn an_operation_joins_its_resource() {
        let source = "service.resource(\"order\", |order| {\n    order.get(\"{id}\", \"Sales.MF_Show\", |_| {});\n});\n";
        assert_eq!(
            extend_resource(
                source,
                "order",
                "order",
                ".post(\"\", \"Sales.MF_Create\", |_| {})"
            )
            .unwrap(),
            "service.resource(\"order\", |order| {\n    order.get(\"{id}\", \"Sales.MF_Show\", |_| {}).post(\"\", \"Sales.MF_Create\", |_| {});\n});\n"
        );
        let empty = "Service::new(\"A\", \"a\").resource(\"order\", |_| {})\n";
        assert_eq!(
            extend_resource(
                empty,
                "order",
                "order",
                ".get(\"\", \"Sales.MF_List\", c::index, |_| {})"
            )
            .unwrap(),
            "Service::new(\"A\", \"a\").resource(\"order\", |order| { order.get(\"\", \"Sales.MF_List\", c::index, |_| {}); })\n"
        );
        // A closure that ends in something else is not extended.
        let other = "service.resource(\"order\", |order| {\n    let _ = 1;\n});\n";
        assert!(extend_resource(other, "order", "order", ".get()").is_none());
        assert!(extend_resource(source, "invoice", "invoice", ".get()").is_none());
    }
}
