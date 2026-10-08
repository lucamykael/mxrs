//! The model's published REST services, on `mxrs run`'s runtime.
//!
//! Every operation of every service the model publishes becomes an action
//! of the runtime — the microflow it calls, its parameters bound from the
//! request, its export mapping applied — and a route of the runtime's
//! server, signing its caller in as the service asks. What the generated
//! axum boundary serves, the runtime serves too, from the model alone.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use mxrs_ir::{PublishedRestServiceDecl, RestAuthentication, RestParameterSource};
use mxrs_runtime::{RuntimeError, SecurityContext, SecurityPolicy, Store};
use mxrs_runtime_flows::{
    ExportMapping, FlowEngine, FlowError, FlowValue, HttpObjects, RequestValue, Variables,
};
use mxrs_runtime_http::{RestAccounts, RestRoute};
use serde_json::{Value, json};

/// The operations the model at `mpr` publishes: the actions that run them,
/// by name, and the routes that reach those actions.
pub(crate) fn operations(
    mpr: &Path,
    modules: &[mxrs_model::Module],
    engine: &Arc<FlowEngine>,
) -> Result<Vec<(RestRoute, RestAction)>, String> {
    let project = mxrs_model::Project::open(mpr, true).map_err(|error| error.to_string())?;
    let units = project.all_units().map_err(|error| error.to_string())?;
    let containers: HashMap<&str, &str> = units
        .iter()
        .map(|unit| (unit.unit_id.as_str(), unit.container_id.as_str()))
        .collect();
    let module_names: HashMap<&str, &str> = modules
        .iter()
        .filter_map(|module| Some((module.id.as_str(), module.name.as_deref()?)))
        .collect();
    let owner = |unit: &str| {
        let mut current = unit;
        for _ in 0..64 {
            let container = containers.get(current)?;
            if let Some(name) = module_names.get(container) {
                return Some(name.to_string());
            }
            current = container;
        }
        None
    };
    let mut services = Vec::new();
    let mut mappings = HashMap::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(|error| error.to_string())?;
        let ty = document.get_str("$Type").unwrap_or_default();
        if ty != "Rest$PublishedRestService" && ty != "ExportMappings$ExportMapping" {
            continue;
        }
        let Some(module) = owner(&unit.unit_id) else {
            continue;
        };
        let Ok(stated) = mxrs_writer::stated_document(&document) else {
            continue;
        };
        if ty == "Rest$PublishedRestService" {
            if let Some(service) = PublishedRestServiceDecl::read(&stated)
                && !service.excluded
            {
                services.push((module, service));
            }
        } else if let (Some(name), Some(mapping)) = (
            stated.text("Name").map(str::to_string),
            ExportMapping::from_document(&stated),
        ) {
            mappings.insert(format!("{module}.{name}"), mapping);
        }
    }
    let http = http_parameters(modules);
    let mut operations = Vec::new();
    for (module, service) in &services {
        let authentication = authentication(service);
        for resource in &service.resources {
            for operation in &resource.operations {
                let Ok(method) = operation.method.as_http().parse() else {
                    continue;
                };
                let path = service.route(resource, operation);
                let action = format!(
                    "{module}.{} {} {path}",
                    service.name,
                    operation.method.as_http()
                );
                let (request, response) = http
                    .get(operation.microflow.as_str())
                    .cloned()
                    .unwrap_or_default();
                operations.push((
                    RestRoute {
                        method,
                        path,
                        action,
                        authentication: authentication.clone(),
                    },
                    RestAction {
                        engine: engine.clone(),
                        microflow: operation.microflow.clone(),
                        parameters: operation
                            .parameters
                            .iter()
                            .map(|parameter| {
                                (
                                    parameter.name.clone(),
                                    parameter.source,
                                    parameter.microflow_parameter.clone(),
                                    request_value(&parameter.ty),
                                )
                            })
                            .collect(),
                        mapping: mappings.get(&operation.export_mapping).cloned(),
                        request,
                        response,
                    },
                ));
            }
        }
    }
    Ok(operations)
}

/// What a path or query parameter's text is read as.
fn request_value(ty: &mxrs_ir::RestParameterType) -> RequestValue {
    match ty {
        mxrs_ir::RestParameterType::Integer | mxrs_ir::RestParameterType::Long => {
            RequestValue::Integer
        }
        mxrs_ir::RestParameterType::Decimal => RequestValue::Decimal,
        mxrs_ir::RestParameterType::Boolean => RequestValue::Boolean,
        mxrs_ir::RestParameterType::DateTime => RequestValue::DateTime,
        _ => RequestValue::Text,
    }
}

/// How a service signs its callers in, as the model states it.
fn authentication(service: &PublishedRestServiceDecl) -> mxrs_runtime_http::RestAuthentication {
    use mxrs_runtime_http::RestAuthentication as Served;
    match &service.authentication {
        RestAuthentication::None => Served::Public,
        RestAuthentication::Required {
            types,
            allowed_roles,
            microflow,
        } => match types.as_slice() {
            [only] if only == "basic" && microflow.is_empty() => Served::Basic {
                realm: service.name.clone(),
                allowed_roles: allowed_roles.clone(),
            },
            [] if microflow.is_empty() => Served::Unsupported(format!(
                "the roles {} without an authentication type to identify a caller by",
                allowed_roles.join(", ")
            )),
            _ if !microflow.is_empty() => {
                Served::Unsupported(format!("the custom authentication microflow {microflow}"))
            }
            types => {
                Served::Unsupported(format!("the authentication type(s) {}", types.join(", ")))
            }
        },
    }
}

/// The variable names each microflow takes its `System.HttpRequest` and
/// `System.HttpResponse` objects under, by its qualified name.
fn http_parameters(
    modules: &[mxrs_model::Module],
) -> HashMap<String, (Option<String>, Option<String>)> {
    let mut found = HashMap::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        for flow in &module.microflows {
            let Some(flow_name) = flow.name.as_deref() else {
                continue;
            };
            let mut objects: (Option<String>, Option<String>) = (None, None);
            for parameter in &flow.parameters {
                let Ok(name) = parameter.get_str("Name") else {
                    continue;
                };
                let entity = parameter
                    .get_document("VariableType")
                    .ok()
                    .and_then(|kind| kind.get_str("Entity").ok());
                match entity {
                    Some("System.HttpRequest") => objects.0 = Some(name.to_string()),
                    Some("System.HttpResponse") => objects.1 = Some(name.to_string()),
                    _ => {}
                }
            }
            if objects.0.is_some() || objects.1.is_some() {
                found.insert(format!("{module_name}.{flow_name}"), objects);
            }
        }
    }
    found
}

/// One operation, as a runtime action: it binds what the request carries
/// to the microflow's parameters, calls it, and answers the operation's
/// document under the status the flow chose.
pub(crate) struct RestAction {
    engine: Arc<FlowEngine>,
    microflow: String,
    /// `(name the request carries it under, where, microflow parameter,
    /// what its text is read as)`.
    parameters: Vec<(String, RestParameterSource, String, RequestValue)>,
    mapping: Option<ExportMapping>,
    request: Option<String>,
    response: Option<String>,
}

impl mxrs_runtime::Action for RestAction {
    fn execute(
        &self,
        store: &mut Store,
        arguments: &Value,
        context: &SecurityContext,
    ) -> mxrs_runtime::Result<Value> {
        let text = |part: &str, name: &str| {
            arguments
                .get(part)
                .and_then(|values| values.get(name))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let mut variables = Variables::new();
        for (name, source, bound, kind) in &self.parameters {
            let read = |part: &str| {
                FlowValue::from_request(text(part, name).as_deref(), *kind)
                    .map_err(|why| format!("parameter {name}: {why}"))
            };
            let value = match source {
                RestParameterSource::Path => match read("path") {
                    Ok(FlowValue::Empty) if *kind == RequestValue::Text => {
                        FlowValue::String(String::new())
                    }
                    Ok(value) => value,
                    Err(why) => return Ok(refused(&why)),
                },
                RestParameterSource::Query => match read("query") {
                    Ok(value) => value,
                    Err(why) => return Ok(refused(&why)),
                },
                RestParameterSource::Body => match arguments.get("body") {
                    Some(Value::Null) | None => FlowValue::Empty,
                    Some(body) => FlowValue::Json(body.clone()),
                },
                // Headers are not carried to the runtime yet.
                RestParameterSource::Header => FlowValue::Empty,
            };
            variables.insert(bound.clone(), value);
        }
        let mut objects = HttpObjects::none();
        if let Some(parameter) = &self.request {
            let uri = arguments
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or_default();
            objects = objects.with_request(parameter.clone(), uri);
        }
        if let Some(parameter) = &self.response {
            objects = objects.with_response(parameter.clone());
        }
        let answer = self
            .engine
            .answer_operation(
                store,
                &self.microflow,
                variables,
                self.mapping.as_ref(),
                context,
                &objects,
            )
            .map_err(|error| match error {
                FlowError::Runtime(error) => error,
                FlowError::Native(message) => RuntimeError::Transaction(message),
            })?;
        Ok(json!({
            "status": answer.status,
            "content": answer.content,
            "document": answer.document,
        }))
    }
}

/// The answer to a request whose text is no value of its parameter's
/// type: the caller's mistake, `400`.
fn refused(why: &str) -> Value {
    json!({ "status": 400, "content": why, "document": Value::Null })
}

/// The model's accounts and roles, as the REST boundary signs callers in.
pub(crate) struct Accounts {
    pub(crate) accounts: mxrs_runtime_boot::LocalAccounts,
    pub(crate) policy: SecurityPolicy,
}

impl RestAccounts for Accounts {
    fn sign_in(&self, user: &str, password: &str) -> Option<SecurityContext> {
        self.accounts.sign_in(user, password)
    }

    fn anonymous(&self) -> SecurityContext {
        self.accounts.anonymous()
    }

    fn allows(&self, caller: &SecurityContext, roles: &[&str]) -> bool {
        self.policy.holds_any_module_role(caller, roles)
    }
}
