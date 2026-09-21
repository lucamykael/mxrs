//! The flow interpreter: graph walking and data activities.
//!
//! Ports `Native::Interpreter` from mxrb over [`mxrs_runtime::Store`]. All
//! store access goes through this engine so entity lifecycle callbacks
//! (which mxrb hangs on its store) fire identically, and every unsupported
//! activity or configuration is a named error.

use std::collections::BTreeMap;

use mxrs_bson::{Bson, Document};
use mxrs_model::Module;
use mxrs_model::association::AssociationType;
use mxrs_runtime::{EntityAction, ObjectValue, SecurityContext, SecurityPolicy, Store};
use serde_json::{Value, json};

use crate::FlowError;
use crate::expression::{Expression, MemberSource};
use crate::value::{FlowValue, ObjectRef, Variables};

/// One prepared flow document.
#[derive(Debug, Clone)]
pub struct Flow {
    pub name: String,
    pub apply_entity_access: bool,
    pub parameter_names: Vec<String>,
    pub objects: Vec<Document>,
    pub edges: Vec<Document>,
}

#[derive(Debug, Clone)]
struct LifecycleHook {
    event: String,
    handler: String,
    pass_event_object: bool,
    raise_error_on_false: bool,
}

#[derive(Debug, Clone)]
struct AssociationInfo {
    reference: bool,
    from_entity: Option<String>,
}

/// A side effect emitted by a client-facing activity (messages, page
/// navigation, downloads). JSON so HTTP boundaries can pass it through.
pub type Effect = Value;

/// Per-root-call execution state: mxrb's `@log`/`@effects` plus the security
/// context and the per-flow entity-access flag.
#[derive(Debug, Default)]
pub struct Execution {
    pub log: Vec<String>,
    pub effects: Vec<Effect>,
    security: Option<SecurityContext>,
    apply_entity_access: bool,
    depth: usize,
}

/// External activity adapters (web services, mappings, document generation,
/// client actions). mxrb takes Ruby lambdas; the port takes one trait per
/// registry.
pub trait Adapter: Send + Sync {
    fn call(
        &self,
        name: &str,
        action: &Document,
        variables: &Variables,
    ) -> Result<FlowValue, FlowError>;
}

/// A registered Java custom action implementation.
pub trait JavaAction: Send + Sync {
    fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub code: u16,
    pub body: String,
}

/// Outbound HTTP seam for `RestCallAction`. The engine itself never opens a
/// socket; the CLI injects a real client.
pub trait HttpCall: Send + Sync {
    fn call(
        &self,
        method: &str,
        location: &str,
        headers: &[(String, String)],
        body: Option<&str>,
        timeout: Option<f64>,
    ) -> Result<HttpResponse, FlowError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdapterKind {
    Javascript,
    Nanoflow,
    AppService,
    WebService,
    ImportXml,
    Document,
    ImportMapping,
    ExportMapping,
}

impl AdapterKind {
    fn label(self) -> &'static str {
        match self {
            AdapterKind::Javascript => "javascript",
            AdapterKind::Nanoflow => "nanoflow",
            AdapterKind::AppService => "app_service",
            AdapterKind::WebService => "web_service",
            AdapterKind::ImportXml => "import_xml",
            AdapterKind::Document => "document",
            AdapterKind::ImportMapping => "import_mapping",
            AdapterKind::ExportMapping => "export_mapping",
        }
    }
}

#[derive(Default)]
pub struct FlowEngine {
    flows: BTreeMap<String, Flow>,
    associations: BTreeMap<String, AssociationInfo>,
    lifecycle: BTreeMap<String, Vec<LifecycleHook>>,
    policy: Option<SecurityPolicy>,
    adapters: BTreeMap<AdapterKind, Box<dyn Adapter>>,
    java_actions: BTreeMap<String, Box<dyn JavaAction>>,
    http: Option<Box<dyn HttpCall>>,
    expression: Expression,
}

enum Completion {
    Return(FlowValue),
    Break,
    Continue,
    Complete,
}

impl FlowEngine {
    pub fn from_modules(modules: &[Module]) -> Self {
        let mut engine = FlowEngine::default();
        for module in modules {
            let Some(module_name) = module.name.as_deref() else {
                continue;
            };
            for flow in module
                .microflows
                .iter()
                .chain(&module.nanoflows)
                .chain(&module.rules)
            {
                let Some(name) = flow.name.as_deref() else {
                    continue;
                };
                engine.flows.insert(
                    format!("{module_name}.{name}"),
                    Flow {
                        name: name.to_string(),
                        apply_entity_access: flow.apply_entity_access,
                        parameter_names: flow
                            .parameters
                            .iter()
                            .filter_map(|parameter| {
                                parameter.get_str("Name").ok().map(str::to_string)
                            })
                            .collect(),
                        objects: flow.objects.clone(),
                        edges: flow.flows.clone(),
                    },
                );
            }
            let entity_names: BTreeMap<String, String> = module
                .entities()
                .iter()
                .filter_map(|entity| {
                    Some((entity.id.clone()?, qualified_entity(module_name, entity)))
                })
                .collect();
            for association in module.associations() {
                let Some(name) = association.name.as_deref() else {
                    continue;
                };
                engine.associations.insert(
                    format!("{module_name}.{name}"),
                    AssociationInfo {
                        reference: association.association_type == AssociationType::Reference,
                        from_entity: association
                            .from_entity_id
                            .as_deref()
                            .and_then(|id| entity_names.get(id).cloned()),
                    },
                );
            }
            for entity in module.entities() {
                if entity.lifecycle.is_empty() {
                    continue;
                }
                let qualified = qualified_entity(module_name, entity);
                let hooks = entity
                    .lifecycle
                    .iter()
                    .map(|callback| LifecycleHook {
                        event: callback.event.clone(),
                        handler: if callback.handler.contains('.') {
                            callback.handler.clone()
                        } else {
                            format!("{module_name}.{}", callback.handler)
                        },
                        pass_event_object: callback.pass_event_object,
                        raise_error_on_false: callback.raise_error_on_false,
                    })
                    .collect();
                engine.lifecycle.insert(qualified, hooks);
            }
        }
        engine
    }

    pub fn with_policy(mut self, policy: SecurityPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    pub fn with_adapter(mut self, kind: AdapterKind, adapter: impl Adapter + 'static) -> Self {
        self.adapters.insert(kind, Box::new(adapter));
        self
    }

    pub fn with_java_action(
        mut self,
        name: impl Into<String>,
        action: impl JavaAction + 'static,
    ) -> Self {
        self.java_actions.insert(name.into(), Box::new(action));
        self
    }

    pub fn with_http(mut self, http: impl HttpCall + 'static) -> Self {
        self.http = Some(Box::new(http));
        self
    }

    pub fn flow_names(&self) -> impl Iterator<Item = &str> {
        self.flows.keys().map(String::as_str)
    }

    /// A root call as mxrb's `Interpreter#call`: one unit of work — on error
    /// every store change rolls back, on success uncommitted work is
    /// discarded exactly like the end of a transaction.
    pub fn call(
        &self,
        store: &mut Store,
        name: &str,
        arguments: Variables,
        context: Option<SecurityContext>,
    ) -> Result<(FlowValue, Execution), FlowError> {
        let snapshot = store.clone();
        let mut execution = Execution {
            security: context,
            ..Execution::default()
        };
        match self.call_flow(store, &mut execution, name, arguments) {
            Ok(value) => {
                store.end_unit_of_work();
                Ok((value, execution))
            }
            Err(error) => {
                *store = snapshot;
                Err(error)
            }
        }
    }

    /// A call inside an existing unit of work (e.g. under
    /// `Runtime::invoke`'s transaction). No snapshotting here — the caller
    /// owns rollback.
    pub fn call_in_unit(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        name: &str,
        arguments: Variables,
    ) -> Result<FlowValue, FlowError> {
        self.call_flow(store, execution, name, arguments)
    }

    /// mxrb's `Interpreter#count`: stored objects of an entity, optionally
    /// narrowed by a native XPath constraint.
    pub fn count(
        &self,
        store: &Store,
        entity: &str,
        xpath: Option<&str>,
    ) -> Result<usize, FlowError> {
        let values = store.retrieve(entity).map_err(FlowError::Runtime)?;
        let filtered =
            self.filter_by_xpath(store, values, xpath.unwrap_or(""), &Variables::new())?;
        Ok(filtered.len())
    }

    pub fn new_execution(&self, context: Option<SecurityContext>) -> Execution {
        Execution {
            security: context,
            ..Execution::default()
        }
    }

    fn call_flow(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        name: &str,
        arguments: Variables,
    ) -> Result<FlowValue, FlowError> {
        let flow = self
            .flows
            .get(name)
            .ok_or_else(|| FlowError::native(format!("flow {name} not found")))?;
        if execution.depth >= 100 {
            return Err(FlowError::native(format!(
                "flow call depth exceeded at {name}"
            )));
        }
        let previous_apply = execution.apply_entity_access;
        execution.apply_entity_access = flow.apply_entity_access;
        execution.depth += 1;
        let mut variables = self.normalize_arguments(flow, arguments)?;
        let result = self.execute(store, execution, flow, &mut variables);
        execution.apply_entity_access = previous_apply;
        execution.depth -= 1;
        match result? {
            Completion::Return(value) => Ok(value),
            _ => Ok(FlowValue::Empty),
        }
    }

    fn normalize_arguments(
        &self,
        flow: &Flow,
        mut arguments: Variables,
    ) -> Result<Variables, FlowError> {
        let mut variables = Variables::new();
        for name in &flow.parameter_names {
            let value = arguments
                .remove(name)
                .ok_or_else(|| FlowError::native(format!("missing argument {name}")))?;
            variables.insert(name.clone(), value);
        }
        Ok(variables)
    }

    fn execute(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        flow: &Flow,
        variables: &mut Variables,
    ) -> Result<Completion, FlowError> {
        let outgoing = group_edges(&flow.edges);
        let objects: Vec<&Document> = flow.objects.iter().collect();
        self.execute_collection(
            store,
            execution,
            &objects,
            &outgoing,
            variables,
            &format!("microflow {}", flow.name),
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_collection(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        collection: &[&Document],
        outgoing: &BTreeMap<String, Vec<&Document>>,
        variables: &mut Variables,
        label: &str,
        root: bool,
    ) -> Result<Completion, FlowError> {
        let objects: BTreeMap<String, &Document> = collection
            .iter()
            .map(|object| (identifier_of(object), *object))
            .collect();
        let mut current = collection_entry(collection, &objects, outgoing, root)
            .ok_or_else(|| FlowError::native(format!("{label} has no start event")))?;
        for _ in 0..10_000 {
            let kind = current.get_str("$Type").unwrap_or_default();
            match kind {
                "Microflows$EndEvent" => {
                    let value = self.eval(store, current.get("ReturnValue"), variables, None)?;
                    return Ok(Completion::Return(value));
                }
                "Microflows$BreakEvent" => return Ok(Completion::Break),
                "Microflows$ContinueEvent" => return Ok(Completion::Continue),
                "Microflows$ErrorEvent" => {
                    let message =
                        self.eval(store, current.get("ErrorExpression"), variables, None)?;
                    return Err(FlowError::native(message.mendix_string()));
                }
                _ => {}
            }
            let id = identifier_of(current);
            let empty = Vec::new();
            let edges = outgoing.get(&id).unwrap_or(&empty);
            let error_edge = edges
                .iter()
                .copied()
                .find(|edge| edge.get_bool("IsErrorHandler").unwrap_or(false));
            let rollback_on_error = kind == "Microflows$ActionActivity"
                && error_edge.is_some()
                && current
                    .get_document("Action")
                    .ok()
                    .and_then(|action| action.get_str("ErrorHandlingType").ok())
                    == Some("Rollback");
            let action_snapshot = rollback_on_error.then(|| store.clone());
            let normal_edges: Vec<&Document> = edges
                .iter()
                .copied()
                .filter(|edge| !edge.get_bool("IsErrorHandler").unwrap_or(false))
                .collect();
            let step = (|| -> Result<Option<&Document>, FlowError> {
                if kind == "Microflows$ActionActivity" {
                    let action = current
                        .get_document("Action")
                        .map_err(|_| FlowError::native("action activity has no action"))?;
                    self.execute_action(store, execution, action, variables)?;
                }
                if kind == "Microflows$LoopedActivity" {
                    self.execute_loop(store, execution, current, outgoing, variables, label)?;
                }
                self.select_edge(store, current, &normal_edges, variables)
            })();
            let edge = match step {
                Ok(edge) => edge,
                Err(error) => {
                    let Some(handler) = error_edge else {
                        return Err(error);
                    };
                    if let Some(snapshot) = action_snapshot {
                        *store = snapshot;
                    }
                    variables.insert(
                        "latestError".to_string(),
                        FlowValue::String(error.to_string()),
                    );
                    Some(handler)
                }
            };
            let Some(edge) = edge else {
                if root {
                    return Err(FlowError::native(format!("{label} stops at {kind}")));
                }
                return Ok(Completion::Complete);
            };
            let destination = destination_of(edge);
            match objects.get(&destination) {
                Some(next) => current = next,
                None if root => {
                    return Err(FlowError::native(
                        "sequence flow points to a missing object",
                    ));
                }
                None => return Ok(Completion::Complete),
            }
        }
        Err(FlowError::native(format!("{label} exceeded 10000 steps")))
    }

    fn execute_loop(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        activity: &Document,
        outgoing: &BTreeMap<String, Vec<&Document>>,
        variables: &mut Variables,
        label: &str,
    ) -> Result<(), FlowError> {
        let source = activity.get_document("LoopSource").ok();
        let nested: Vec<Document> = activity
            .get_document("ObjectCollection")
            .ok()
            .map(|collection| doc_items(collection, "Objects"))
            .unwrap_or_default();
        let nested: Vec<&Document> = nested.iter().collect();
        let source_type = source
            .and_then(|source| source.get_str("$Type").ok())
            .unwrap_or_default();
        let loop_label = format!("{label} loop");
        match source_type {
            "Microflows$WhileLoopCondition" => {
                let source = source.expect("type implies presence");
                for _ in 0..10_000 {
                    let condition =
                        self.eval(store, source.get("WhileExpression"), variables, None)?;
                    if !condition.truthy() {
                        return Ok(());
                    }
                    let result = self.execute_collection(
                        store,
                        execution,
                        &nested,
                        outgoing,
                        variables,
                        &loop_label,
                        false,
                    )?;
                    if matches!(result, Completion::Break) {
                        return Ok(());
                    }
                }
                Err(FlowError::native(format!(
                    "{label} loop exceeded 10000 iterations"
                )))
            }
            "Microflows$IterableList" => {
                let source = source.expect("type implies presence");
                let list_name = source.get_str("ListVariableName").unwrap_or_default();
                let variable_name = source.get_str("VariableName").unwrap_or_default();
                let values = as_list(fetch(variables, list_name)?);
                for value in values {
                    variables.insert(variable_name.to_string(), value);
                    let result = self.execute_collection(
                        store,
                        execution,
                        &nested,
                        outgoing,
                        variables,
                        &loop_label,
                        false,
                    )?;
                    if matches!(result, Completion::Break) {
                        break;
                    }
                }
                Ok(())
            }
            other => Err(FlowError::native(format!(
                "unsupported loop source {other}"
            ))),
        }
    }

    fn select_edge<'a>(
        &self,
        store: &Store,
        object: &Document,
        edges: &[&'a Document],
        variables: &Variables,
    ) -> Result<Option<&'a Document>, FlowError> {
        let kind = object.get_str("$Type").unwrap_or_default();
        if !matches!(
            kind,
            "Microflows$ExclusiveSplit" | "Microflows$InheritanceSplit"
        ) {
            return Ok(edges.first().copied());
        }
        let value = self.split_value(store, object, variables)?;
        let rendering = value.mendix_string();
        Ok(edges
            .iter()
            .copied()
            .find(|edge| flow_case(edge) == rendering)
            .or_else(|| {
                edges
                    .iter()
                    .copied()
                    .find(|edge| flow_case(edge).is_empty())
            }))
    }

    fn split_value(
        &self,
        store: &Store,
        object: &Document,
        variables: &Variables,
    ) -> Result<FlowValue, FlowError> {
        if object.get_str("$Type").ok() == Some("Microflows$InheritanceSplit") {
            let name = object.get_str("SplitVariableName").unwrap_or_default();
            let value = fetch(variables, name)?;
            return Ok(match value {
                FlowValue::Object(reference) => FlowValue::String(reference.entity),
                other => FlowValue::String(other.kind().to_string()),
            });
        }
        let source = object
            .get_document("SplitCondition")
            .ok()
            .and_then(|condition| condition.get("Expression").cloned());
        self.eval_owned(store, source, variables, None)
    }

    fn execute_action(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let kind = action.get_str("$Type").unwrap_or_default();
        let handler = underscore(
            kind.strip_prefix("Microflows$")
                .unwrap_or(kind)
                .strip_suffix("Action")
                .unwrap_or(kind),
        );
        match handler.as_str() {
            "create_change" | "create_object" => {
                self.action_create(store, execution, action, variables)
            }
            "change" | "change_object" => self.action_change(store, execution, action, variables),
            "create_variable" => {
                let value = self.eval(store, action.get("InitialValue"), variables, None)?;
                variables.insert(
                    action.get_str("VariableName").unwrap_or_default().into(),
                    value,
                );
                Ok(())
            }
            "change_variable" => {
                let value = self.eval(store, action.get("Value"), variables, None)?;
                variables.insert(
                    action
                        .get_str("ChangeVariableName")
                        .unwrap_or_default()
                        .into(),
                    value,
                );
                Ok(())
            }
            "retrieve" => self.action_retrieve(store, execution, action, variables),
            "aggregate" | "aggregate_list" => self.action_aggregate(store, action, variables),
            "microflow_call" => self.action_microflow_call(store, execution, action, variables),
            "commit" | "commit_objects" => self.action_commit(store, execution, action, variables),
            "create_list" => {
                variables.insert(
                    action.get_str("VariableName").unwrap_or_default().into(),
                    FlowValue::List(Vec::new()),
                );
                Ok(())
            }
            "change_list" => self.action_change_list(store, action, variables),
            "delete" => self.action_delete(store, execution, action, variables),
            "log_message" => {
                let message = self.render_template(
                    store,
                    action.get_document("MessageTemplate").ok(),
                    variables,
                )?;
                execution.log.push(message);
                Ok(())
            }
            "cast" | "empty" => Ok(()),
            "rollback" => {
                let name = action.get_str("RollbackVariableName").unwrap_or_default();
                let object = as_object(fetch(variables, name)?)?;
                store
                    .rollback(&object.entity, &object.id)
                    .map_err(FlowError::Runtime)
            }
            "show_message" => {
                let message = self.render_text_template(
                    store,
                    action.get_document("Template").ok(),
                    variables,
                )?;
                execution.effects.push(json!({
                    "type": "show_message",
                    "level": action.get_str("Type").unwrap_or_default().to_lowercase(),
                    "blocking": action.get_bool("Blocking").unwrap_or(false),
                    "message": message,
                }));
                Ok(())
            }
            "validation_feedback" => {
                self.action_validation_feedback(store, execution, action, variables)
            }
            "list_operations" => self.action_list_operations(store, action, variables),
            "java_action_call" | "java_action" | "basic_java" | "basic_code"
            | "entity_type_java" | "microflow_java" => {
                self.action_java_action(store, action, variables)
            }
            "java_script_action_call" => self.client_action(
                execution,
                AdapterKind::Javascript,
                action.get_str("JavaScriptAction").unwrap_or_default(),
                action,
                variables,
                action.get_str("OutputVariableName").unwrap_or_default(),
            ),
            "nanoflow_call" => {
                let call = action.get_document("NanoflowCall").ok();
                let name = call
                    .and_then(|call| call.get_str("Nanoflow").ok())
                    .unwrap_or_default();
                self.client_action(
                    execution,
                    AdapterKind::Nanoflow,
                    name,
                    call.unwrap_or(action),
                    variables,
                    action.get_str("OutputVariableName").unwrap_or_default(),
                )
            }
            "app_service_call" => self.invoke_adapter(
                AdapterKind::AppService,
                action.get_str("AppServiceAction").unwrap_or_default(),
                action,
                variables,
                action.get_str("ResultVariableName").unwrap_or_default(),
            ),
            "call_web_service" => self.invoke_adapter(
                AdapterKind::WebService,
                action
                    .get_str("WebServiceCall")
                    .or_else(|_| action.get_str("Operation"))
                    .unwrap_or_default(),
                action,
                variables,
                action.get_str("ResultVariableName").unwrap_or_default(),
            ),
            "import_xml" => self.invoke_adapter(
                AdapterKind::ImportXml,
                action.get_str("ImportMapping").unwrap_or_default(),
                action,
                variables,
                action.get_str("ResultVariableName").unwrap_or_default(),
            ),
            "generate_document" => self.invoke_adapter(
                AdapterKind::Document,
                action.get_str("DocumentTemplate").unwrap_or_default(),
                action,
                variables,
                action.get_str("ResultVariableName").unwrap_or_default(),
            ),
            "import_mapping_java" => self.invoke_adapter(
                AdapterKind::ImportMapping,
                action.get_str("ImportMapping").unwrap_or_default(),
                action,
                variables,
                action.get_str("ResultVariableName").unwrap_or_default(),
            ),
            "export_mapping_java" => self.invoke_adapter(
                AdapterKind::ExportMapping,
                action.get_str("ExportMapping").unwrap_or_default(),
                action,
                variables,
                action.get_str("ResultVariableName").unwrap_or_default(),
            ),
            "export_xml" => self.action_export_xml(action, variables),
            "rest_call" => self.action_rest_call(store, execution, action, variables),
            "show_form" => self.action_show_form(store, execution, action, variables),
            "close_form" => {
                execution.effects.push(json!({ "type": "close_page" }));
                Ok(())
            }
            "download_file" => {
                let name = action
                    .get_str("FileDocumentVariableName")
                    .unwrap_or_default();
                let value = variables
                    .get(name)
                    .map(FlowValue::to_json_shallow)
                    .unwrap_or(Value::Null);
                execution.effects.push(json!({
                    "type": "download_file",
                    "value": value,
                    "show_in_browser": action.get_bool("ShowFileInBrowser").unwrap_or(false),
                }));
                Ok(())
            }
            "show_home_page" => {
                execution.effects.push(json!({ "type": "show_home_page" }));
                Ok(())
            }
            _ => Err(FlowError::native(format!("unsupported activity {kind}"))),
        }
    }

    fn action_create(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let entity = action.get_str("Entity").unwrap_or_default();
        self.authorize_entity(execution, entity, EntityAction::Create, None)?;
        let created = store.create(entity).map_err(FlowError::Runtime)?;
        let reference = ObjectRef {
            entity: created.entity,
            id: created.id,
        };
        self.apply_changes(store, execution, &reference, action.get("Items"), variables)?;
        variables.insert(
            action.get_str("VariableName").unwrap_or_default().into(),
            FlowValue::Object(reference.clone()),
        );
        self.commit_for_action(store, execution, action, &reference)
    }

    fn action_change(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let name = action.get_str("ChangeVariableName").unwrap_or_default();
        let reference = as_object(fetch(variables, name)?)?;
        self.authorize_entity(execution, &reference.entity, EntityAction::Write, None)?;
        self.apply_changes(store, execution, &reference, action.get("Items"), variables)?;
        self.commit_for_action(store, execution, action, &reference)
    }

    fn apply_changes(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        reference: &ObjectRef,
        items: Option<&Bson>,
        variables: &Variables,
    ) -> Result<(), FlowError> {
        for item in bson_items(items) {
            let Bson::Document(item) = item else { continue };
            let member = member_name(&item);
            self.authorize_entity(
                execution,
                &reference.entity,
                EntityAction::Write,
                Some(&member),
            )?;
            let value = self.eval(store, item.get("Value"), variables, None)?;
            store
                .set_member(&reference.entity, &reference.id, &member, value.to_member())
                .map_err(FlowError::Runtime)?;
        }
        Ok(())
    }

    fn commit_for_action(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        reference: &ObjectRef,
    ) -> Result<(), FlowError> {
        let setting = commit_setting(action);
        if setting.is_empty() || setting.eq_ignore_ascii_case("no") {
            return Ok(());
        }
        let events = !setting.to_ascii_lowercase().contains("withoutevents");
        self.commit_object(store, execution, reference, events)
    }

    fn action_commit(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let name = action.get_str("CommitVariableName").unwrap_or_default();
        if name.is_empty() {
            return Ok(());
        }
        let value = fetch(variables, name)?;
        let events = action.get_bool("WithEvents").unwrap_or(true);
        for reference in object_list(&value)? {
            self.authorize_record(execution, &reference, EntityAction::Write)?;
            self.commit_object(store, execution, &reference, events)?;
        }
        Ok(())
    }

    fn action_delete(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let name = action.get_str("DeleteVariableName").unwrap_or_default();
        let value = fetch(variables, name)?;
        let events = action.get_bool("WithEvents").unwrap_or(true);
        for reference in object_list(&value)? {
            self.authorize_record(execution, &reference, EntityAction::Delete)?;
            if events {
                self.run_hooks(store, execution, &reference, "before_delete")?;
            }
            store
                .delete(&reference.entity, &reference.id)
                .map_err(FlowError::Runtime)?;
            if events {
                self.run_hooks(store, execution, &reference, "after_delete")?;
            }
        }
        Ok(())
    }

    fn commit_object(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        reference: &ObjectRef,
        events: bool,
    ) -> Result<(), FlowError> {
        let creating = !store.is_committed(&reference.entity, &reference.id);
        if events {
            self.run_hooks(
                store,
                execution,
                reference,
                if creating {
                    "before_create"
                } else {
                    "before_update"
                },
            )?;
            self.run_hooks(store, execution, reference, "before_commit")?;
        }
        store
            .commit(&reference.entity, &reference.id)
            .map_err(FlowError::Runtime)?;
        if events {
            self.run_hooks(store, execution, reference, "after_commit")?;
            self.run_hooks(
                store,
                execution,
                reference,
                if creating {
                    "after_create"
                } else {
                    "after_update"
                },
            )?;
        }
        Ok(())
    }

    fn run_hooks(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        reference: &ObjectRef,
        event: &str,
    ) -> Result<(), FlowError> {
        let Some(hooks) = self.lifecycle.get(&reference.entity) else {
            return Ok(());
        };
        for hook in hooks.iter().filter(|hook| hook.event == event) {
            let mut arguments = Variables::new();
            if hook.pass_event_object
                && let Some(flow) = self.flows.get(&hook.handler)
                && let Some(parameter) = flow.parameter_names.first()
            {
                arguments.insert(parameter.clone(), FlowValue::Object(reference.clone()));
            }
            let result = self.call_flow(store, execution, &hook.handler, arguments)?;
            if hook.raise_error_on_false && result == FlowValue::Bool(false) {
                return Err(FlowError::native(format!(
                    "entity lifecycle {} rejected {}",
                    hook.handler, reference.entity
                )));
            }
        }
        Ok(())
    }

    fn action_retrieve(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let source = action.get_document("RetrieveSource").ok();
        let source_type = source
            .and_then(|source| source.get_str("$Type").ok())
            .unwrap_or_default();
        if source_type == "Microflows$AssociationRetrieveSource" {
            let source = source.expect("type implies presence");
            return self.action_association_retrieve(store, execution, action, source, variables);
        }
        if source_type != "Microflows$DatabaseRetrieveSource" {
            return Err(FlowError::native(format!(
                "unsupported retrieve source {source_type}"
            )));
        }
        let source = source.expect("type implies presence");
        let entity = source.get_str("Entity").unwrap_or_default();
        let values = store.retrieve(entity).map_err(FlowError::Runtime)?;
        let xpath = source.get_str("XpathConstraint").unwrap_or_default();
        let mut values = self.filter_by_xpath(store, values, xpath, variables)?;
        if self.enforce_entity_access(execution) {
            values.retain(|value| self.record_readable(execution, &value.entity));
        }
        let sortings = source
            .get_document("NewSortings")
            .ok()
            .map(|sortings| doc_items(sortings, "Sortings"))
            .unwrap_or_default();
        let mut values = sort_values(values, &sortings)?;
        let range = action_range(source);
        if let Some(range) = &range {
            let limit = self.eval(store, range.get("LimitExpression"), variables, None)?;
            if let FlowValue::Int(limit) = limit
                && limit > 0
            {
                values.truncate(limit as usize);
            }
        }
        let single = range
            .as_ref()
            .and_then(|range| range.get_bool("SingleObject").ok())
            .unwrap_or(false);
        let result = if single {
            values
                .first()
                .map(|value| FlowValue::Object(reference_of(value)))
                .unwrap_or(FlowValue::Empty)
        } else {
            FlowValue::List(
                values
                    .iter()
                    .map(|value| FlowValue::Object(reference_of(value)))
                    .collect(),
            )
        };
        variables.insert(
            action
                .get_str("ResultVariableName")
                .unwrap_or_default()
                .into(),
            result,
        );
        Ok(())
    }

    fn action_association_retrieve(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        source: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let variable = source.get_str("StartVariableName").unwrap_or_default();
        let association_id = source.get_str("AssociationId").unwrap_or_default();
        let association = association_id.rsplit('.').next().unwrap_or(association_id);
        if variable.is_empty() || association.is_empty() {
            return Err(FlowError::native(format!(
                "unsupported retrieve source {}",
                source.get_str("$Type").unwrap_or_default()
            )));
        }
        let start = as_object(fetch(variables, variable)?)?;
        let start_value = self.object_value(store, &start)?;
        let mut values = store.retrieve_association(association, &start_value);
        if self.enforce_entity_access(execution) {
            values.retain(|value| self.record_readable(execution, &value.entity));
        }
        let definition = self.associations.get(association_id);
        let collapse = definition.is_some_and(|definition| {
            definition.reference && definition.from_entity.as_deref() == Some(start.entity.as_str())
        });
        let result = if collapse {
            values
                .first()
                .map(|value| FlowValue::Object(reference_of(value)))
                .unwrap_or(FlowValue::Empty)
        } else {
            FlowValue::List(
                values
                    .iter()
                    .map(|value| FlowValue::Object(reference_of(value)))
                    .collect(),
            )
        };
        variables.insert(
            action
                .get_str("ResultVariableName")
                .unwrap_or_default()
                .into(),
            result,
        );
        Ok(())
    }

    fn action_aggregate(
        &self,
        store: &Store,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let list_name = action.get_str("AggregateVariableName").unwrap_or_default();
        let values = as_list(fetch(variables, list_name)?);
        let attribute_path = action.get_str("Attribute").unwrap_or_default();
        let attribute = attribute_path.rsplit('.').next().unwrap_or(attribute_path);
        let members: Vec<FlowValue> = if attribute.is_empty() {
            values
        } else {
            values
                .iter()
                .map(|value| match value {
                    FlowValue::Object(reference) => {
                        StoreMembers(store).object_member(reference, attribute)
                    }
                    other => Ok(other.clone()),
                })
                .collect::<Result<_, _>>()?
        };
        let function = action.get_str("AggregateFunction").unwrap_or_default();
        let result = aggregate(function, &members)?;
        variables.insert(
            action.get_str("VariableName").unwrap_or_default().into(),
            result,
        );
        Ok(())
    }

    fn action_microflow_call(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let call = action.get_document("MicroflowCall").ok();
        let mut arguments = Variables::new();
        if let Some(call) = call {
            for mapping in bson_items(call.get("ParameterMappings")) {
                let Bson::Document(mapping) = mapping else {
                    continue;
                };
                let parameter = mapping.get_str("Parameter").unwrap_or_default();
                let name = parameter.rsplit('.').next().unwrap_or(parameter);
                let value = self.eval(store, mapping.get("Argument"), variables, None)?;
                arguments.insert(name.to_string(), value);
            }
        }
        let target = call
            .and_then(|call| call.get_str("Microflow").ok())
            .unwrap_or_default();
        let result = self.call_flow(store, execution, target, arguments)?;
        if action.get_bool("UseReturnVariable").unwrap_or(false) {
            variables.insert(
                action
                    .get_str("ResultVariableName")
                    .unwrap_or_default()
                    .into(),
                result,
            );
        }
        Ok(())
    }

    fn action_change_list(
        &self,
        store: &mut Store,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let value = self.eval(store, action.get("Value"), variables, None)?;
        let name = action.get_str("ChangeVariableName").unwrap_or_default();
        let FlowValue::List(mut list) = fetch(variables, name)? else {
            return Err(FlowError::native(format!("${name} is not a list")));
        };
        match action
            .get_str("Type")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "add" => {
                if !list.iter().any(|existing| existing.equals(&value)) {
                    list.push(value);
                }
            }
            "remove" => list.retain(|existing| !existing.equals(&value)),
            "clear" => list.clear(),
            other => {
                return Err(FlowError::native(format!(
                    "unsupported list change {other}"
                )));
            }
        }
        variables.insert(name.to_string(), FlowValue::List(list));
        Ok(())
    }

    fn action_validation_feedback(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let name = action.get_str("ValidationVariableName").unwrap_or_default();
        let object = as_object(fetch(variables, name)?)?;
        let attribute = action.get_str("Attribute").unwrap_or_default();
        let member = if attribute.is_empty() {
            action.get_str("Association").unwrap_or_default()
        } else {
            attribute
        };
        let member = member
            .rsplit(['.', '/'])
            .next()
            .unwrap_or(member)
            .to_string();
        let message = self.render_text_template(
            store,
            action.get_document("FeedbackTemplate").ok(),
            variables,
        )?;
        execution.effects.push(json!({
            "type": "validation_feedback",
            "object_id": object.id,
            "member": member,
            "message": message,
        }));
        Ok(())
    }

    fn action_list_operations(
        &self,
        store: &mut Store,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let operation = action
            .get_document("NewOperation")
            .map_err(|_| FlowError::native("unsupported list operation"))?;
        let list_name = operation.get_str("ListName").unwrap_or_default();
        let list = as_list(fetch(variables, list_name)?);
        let second = variables
            .get(
                operation
                    .get_str("SecondListOrObjectName")
                    .unwrap_or_default(),
            )
            .cloned();
        let kind = operation.get_str("$Type").unwrap_or_default();
        let name = kind
            .strip_prefix("Microflows$")
            .unwrap_or(kind)
            .to_ascii_lowercase();
        let expression = operation.get("Expression");
        let result = match name.as_str() {
            "head" => list.first().cloned().unwrap_or(FlowValue::Empty),
            "tail" => FlowValue::List(list.into_iter().skip(1).collect()),
            "find" => {
                let mut found = FlowValue::Empty;
                for value in &list {
                    if self
                        .predicate(store, expression, variables, value)?
                        .truthy()
                    {
                        found = value.clone();
                        break;
                    }
                }
                found
            }
            "filter" => {
                let mut kept = Vec::new();
                for value in list {
                    if self
                        .predicate(store, expression, variables, &value)?
                        .truthy()
                    {
                        kept.push(value);
                    }
                }
                FlowValue::List(kept)
            }
            "sort" => FlowValue::List(list),
            "union" => {
                let mut union = list;
                for value in second_list(second) {
                    if !union.iter().any(|existing| existing.equals(&value)) {
                        union.push(value);
                    }
                }
                FlowValue::List(union)
            }
            "intersect" => {
                let second = second_list(second);
                FlowValue::List(
                    list.into_iter()
                        .filter(|value| second.iter().any(|other| other.equals(value)))
                        .collect(),
                )
            }
            "subtract" => {
                let second = second_list(second);
                FlowValue::List(
                    list.into_iter()
                        .filter(|value| !second.iter().any(|other| other.equals(value)))
                        .collect(),
                )
            }
            "contains" => {
                let second = second.unwrap_or(FlowValue::Empty);
                FlowValue::Bool(list.iter().any(|value| value.equals(&second)))
            }
            _ => {
                return Err(FlowError::native(format!(
                    "unsupported list operation {kind}"
                )));
            }
        };
        variables.insert(
            action
                .get_str("ResultVariableName")
                .unwrap_or_default()
                .into(),
            result,
        );
        Ok(())
    }

    fn predicate(
        &self,
        store: &Store,
        expression: Option<&Bson>,
        variables: &Variables,
        value: &FlowValue,
    ) -> Result<FlowValue, FlowError> {
        let node = match value {
            FlowValue::Object(reference) => Some(self.object_value(store, reference)?),
            _ => None,
        };
        self.eval_owned(store, expression.cloned(), variables, node.as_ref())
    }

    fn action_java_action(
        &self,
        store: &mut Store,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let name = action.get_str("JavaAction").unwrap_or_default();
        let Some(adapter) = self.java_actions.get(name) else {
            let label = if name.is_empty() {
                "(missing name)"
            } else {
                name
            };
            return Err(FlowError::native(format!(
                "Java Custom Action {label} is not registered; register an explicit adapter with FlowEngine::with_java_action"
            )));
        };
        let mut arguments = BTreeMap::new();
        for mapping in bson_items(action.get("ParameterMappings")) {
            let Bson::Document(mapping) = mapping else {
                continue;
            };
            let reference = mapping
                .get("Parameter")
                .map(identifier_value)
                .unwrap_or_default();
            let parameter_name = reference
                .rsplit('.')
                .next()
                .unwrap_or(&reference)
                .to_string();
            if parameter_name.is_empty() {
                return Err(FlowError::native(format!(
                    "Java Custom Action {name} has an unnamed parameter mapping"
                )));
            }
            let value = self.java_action_argument(
                store,
                name,
                &parameter_name,
                mapping.get_document("Value").ok(),
                variables,
            )?;
            arguments.insert(parameter_name, value);
        }
        let result = adapter.call(&arguments)?;
        let result_name = action
            .get_str("ResultVariableName")
            .or_else(|_| action.get_str("OutputVariableName"))
            .unwrap_or_default();
        let use_return = match action.get_bool("UseReturnVariable") {
            Ok(value) => value,
            Err(_) => !result_name.is_empty(),
        };
        if use_return && !result_name.is_empty() {
            variables.insert(result_name.to_string(), result);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn java_action_argument(
        &self,
        store: &Store,
        action_name: &str,
        parameter_name: &str,
        value: Option<&Document>,
        variables: &Variables,
    ) -> Result<FlowValue, FlowError> {
        let kind = value
            .and_then(|value| value.get_str("$Type").ok())
            .unwrap_or_default()
            .strip_prefix("Microflows$")
            .unwrap_or_default()
            .to_string();
        let value_doc = value.cloned().unwrap_or_default();
        match kind.as_str() {
            "BasicJavaActionParameterValue" | "BasicCodeActionParameterValue" => {
                self.eval_owned(store, value_doc.get("Argument").cloned(), variables, None)
            }
            "EntityTypeJavaActionParameterValue" => Ok(FlowValue::String(
                value_doc.get_str("Entity").unwrap_or_default().to_string(),
            )),
            "EntityTypeCodeActionParameterValue" => {
                let entity = value_doc.get_str("Entity").unwrap_or_default();
                if entity.is_empty() {
                    return Err(FlowError::native(format!(
                        "Java Custom Action {action_name} parameter {parameter_name} has an empty entity type"
                    )));
                }
                Ok(FlowValue::String(entity.to_string()))
            }
            "MicroflowJavaActionParameterValue" | "MicroflowParameterValue" => {
                Ok(FlowValue::String(
                    value_doc
                        .get_str("Microflow")
                        .unwrap_or_default()
                        .to_string(),
                ))
            }
            "ImportMappingJavaActionParameterValue" => Ok(FlowValue::String(
                value_doc
                    .get_str("ImportMapping")
                    .unwrap_or_default()
                    .to_string(),
            )),
            "ExportMappingJavaActionParameterValue" => Ok(FlowValue::String(
                value_doc
                    .get_str("ExportMapping")
                    .unwrap_or_default()
                    .to_string(),
            )),
            other => Err(FlowError::native(format!(
                "Java Custom Action {action_name} parameter {parameter_name} uses unsupported mapping \"Microflows${other}\""
            ))),
        }
    }

    fn client_action(
        &self,
        execution: &mut Execution,
        kind: AdapterKind,
        name: &str,
        document: &Document,
        variables: &mut Variables,
        result_name: &str,
    ) -> Result<(), FlowError> {
        if let Some(adapter) = self.adapters.get(&kind) {
            let result = adapter.call(name, document, variables)?;
            if !result_name.is_empty() {
                variables.insert(result_name.to_string(), result);
            }
            return Ok(());
        }
        execution.effects.push(json!({
            "type": kind.label(),
            "name": name,
        }));
        if !result_name.is_empty() {
            variables.insert(result_name.to_string(), FlowValue::Empty);
        }
        Ok(())
    }

    fn invoke_adapter(
        &self,
        kind: AdapterKind,
        name: &str,
        document: &Document,
        variables: &mut Variables,
        result_name: &str,
    ) -> Result<(), FlowError> {
        let result = self.adapter_result(kind, name, document, variables)?;
        if !result_name.is_empty() {
            variables.insert(result_name.to_string(), result);
        }
        Ok(())
    }

    fn adapter_result(
        &self,
        kind: AdapterKind,
        name: &str,
        document: &Document,
        variables: &Variables,
    ) -> Result<FlowValue, FlowError> {
        let adapter = self.adapters.get(&kind).ok_or_else(|| {
            FlowError::native(format!("{} adapter is not configured", kind.label()))
        })?;
        adapter.call(name, document, variables)
    }

    fn action_export_xml(
        &self,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let output = action.get_document("OutputMethod").ok();
        let handling = action.get_document("ResultHandling").ok();
        let output_type = output
            .and_then(|output| output.get_str("$Type").ok())
            .unwrap_or_default();
        let handling_type = handling
            .and_then(|handling| handling.get_str("$Type").ok())
            .unwrap_or_default();
        let content_type = handling
            .and_then(|handling| handling.get_str("ContentType").ok())
            .unwrap_or_default();
        if output_type != "ExportXmlAction$StringExport"
            || handling_type != "Microflows$MappingRequestHandling"
            || !matches!(content_type, "Xml" | "Json")
            || action.get_bool("IsValidationRequired").is_err()
        {
            return Err(FlowError::native("unsupported export XML configuration"));
        }
        let output = output.expect("validated above");
        let handling = handling.expect("validated above");
        let output_name = output.get_str("OutputVariableName").unwrap_or_default();
        let mapping = handling.get_str("MappingId").unwrap_or_default();
        let source_name = handling.get_str("MappingVariableName").unwrap_or_default();
        if output_name.is_empty() || mapping.is_empty() || source_name.is_empty() {
            return Err(FlowError::native(
                "export XML requires output, mapping, and source variables",
            ));
        }
        fetch(variables, source_name)?;
        let result = self.adapter_result(AdapterKind::ExportMapping, mapping, action, variables)?;
        let FlowValue::String(_) = &result else {
            return Err(FlowError::native(
                "string export mapping returned a non-string value",
            ));
        };
        variables.insert(output_name.to_string(), result);
        Ok(())
    }

    fn action_rest_call(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let configuration = action.get_document("HttpConfiguration").ok();
        let configuration = configuration.cloned().unwrap_or_default();
        let location = self.render_template(
            store,
            configuration.get_document("CustomLocationTemplate").ok(),
            variables,
        )?;
        let mut headers = Vec::new();
        for entry in bson_items(configuration.get("HttpHeaderEntries")) {
            let Bson::Document(entry) = entry else {
                continue;
            };
            let key = entry.get_str("Key").unwrap_or_default().to_string();
            let value = match self.eval(store, entry.get("Value"), variables, None) {
                Ok(value) => value.mendix_string(),
                // Like mxrb: a header value that is not an expression stays
                // the literal text.
                Err(FlowError::Native(_)) => source_text(entry.get("Value")),
                Err(error) => return Err(error),
            };
            headers.push((key, value));
        }
        let request = action.get_document("RequestHandling").ok();
        let body = request
            .and_then(|request| request.get_str("MappingVariableName").ok())
            .and_then(|name| variables.get(name))
            .map(|value| self.runtime_json(store, value))
            .transpose()?
            .map(|value| value.to_string());
        let timeout = if action.get_bool("UseRequestTimeOut").unwrap_or(false) {
            self.eval(store, action.get("TimeOutExpression"), variables, None)?
                .as_float()
        } else {
            None
        };
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| FlowError::native("http adapter is not configured"))?;
        let method = configuration.get_str("HttpMethod").unwrap_or_default();
        let response = http.call(method, &location, &headers, body.as_deref(), timeout)?;
        if !(200..=299).contains(&response.code) {
            return Err(FlowError::native(format!(
                "REST call returned HTTP {}",
                response.code
            )));
        }
        let handling = action
            .get_document("ResultHandling")
            .cloned()
            .unwrap_or_default();
        let handling_type = action
            .get_str("ResultHandlingType")
            .unwrap_or("Mapping")
            .to_string();
        self.bind_rest_result(
            store,
            execution,
            &handling,
            &response.body,
            variables,
            &handling_type,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_rest_result(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        handling: &Document,
        body: &str,
        variables: &mut Variables,
        handling_type: &str,
    ) -> Result<(), FlowError> {
        if !handling.get_bool("Bind").unwrap_or(false) {
            return Ok(());
        }
        let variable_type = handling.get_document("VariableType").ok();
        let result = match handling_type {
            "String" => {
                let is_string_type = variable_type.and_then(|kind| kind.get_str("$Type").ok())
                    == Some("DataTypes$StringType");
                if !is_string_type || handling.get("ImportMappingCall").is_some() {
                    return Err(FlowError::native("invalid string REST result handling"));
                }
                FlowValue::String(body.to_string())
            }
            "" | "Mapping" | "HttpResponse" => {
                let parsed: Value = serde_json::from_str(body)
                    .map_err(|error| FlowError::native(format!("REST call failed: {error}")))?;
                FlowValue::from_member(&parsed)
            }
            other => {
                return Err(FlowError::native(format!(
                    "unsupported REST result handling {other:?}"
                )));
            }
        };
        let entity = variable_type
            .and_then(|kind| kind.get_str("Entity").ok())
            .unwrap_or_default();
        let result = if entity.is_empty() {
            result
        } else {
            let parsed: Value = serde_json::from_str(body)
                .map_err(|error| FlowError::native(format!("REST call failed: {error}")))?;
            self.rest_object(store, execution, entity, &parsed)?
        };
        variables.insert(
            handling
                .get_str("ResultVariableName")
                .unwrap_or_default()
                .to_string(),
            result,
        );
        Ok(())
    }

    fn rest_object(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        entity: &str,
        value: &Value,
    ) -> Result<FlowValue, FlowError> {
        self.authorize_entity(execution, entity, EntityAction::Create, None)?;
        let created = store.create(entity).map_err(FlowError::Runtime)?;
        if let Value::Object(members) = value {
            for (key, child) in members {
                self.authorize_entity(execution, entity, EntityAction::Write, Some(key))?;
                store
                    .set_member(&created.entity, &created.id, key, child.clone())
                    .map_err(FlowError::Runtime)?;
            }
        }
        Ok(FlowValue::Object(ObjectRef {
            entity: created.entity,
            id: created.id,
        }))
    }

    fn action_show_form(
        &self,
        store: &mut Store,
        execution: &mut Execution,
        action: &Document,
        variables: &mut Variables,
    ) -> Result<(), FlowError> {
        let settings = action
            .get_document("FormSettings")
            .cloned()
            .unwrap_or_default();
        let mut arguments = serde_json::Map::new();
        for mapping in bson_items(settings.get("ParameterMappings")) {
            let Bson::Document(mapping) = mapping else {
                continue;
            };
            let parameter = mapping.get_str("Parameter").unwrap_or_default().to_string();
            let value = self.eval(store, mapping.get("Argument"), variables, None)?;
            arguments.insert(parameter, value.to_json_shallow());
        }
        execution.effects.push(json!({
            "type": "open_page",
            "page": settings.get_str("Form").unwrap_or_default(),
            "arguments": arguments,
        }));
        Ok(())
    }

    /// mxrb's `runtime_json`: object references expand to their member maps.
    fn runtime_json(&self, store: &Store, value: &FlowValue) -> Result<Value, FlowError> {
        Ok(match value {
            FlowValue::Object(reference) => {
                let object = self.object_value(store, reference)?;
                Value::Object(object.members.into_iter().collect())
            }
            FlowValue::List(values) => Value::Array(
                values
                    .iter()
                    .map(|value| self.runtime_json(store, value))
                    .collect::<Result<_, _>>()?,
            ),
            other => other.to_json_shallow(),
        })
    }

    fn filter_by_xpath(
        &self,
        store: &Store,
        values: Vec<ObjectValue>,
        xpath: &str,
        variables: &Variables,
    ) -> Result<Vec<ObjectValue>, FlowError> {
        let predicate = xpath_predicate(xpath)?;
        let Some(predicate) = predicate else {
            return Ok(values);
        };
        let mut kept = Vec::new();
        for value in values {
            let outcome = self.expression.evaluate(
                &predicate,
                variables,
                Some(&value),
                &StoreMembers(store),
            )?;
            if outcome.truthy() {
                kept.push(value);
            }
        }
        Ok(kept)
    }

    fn render_template(
        &self,
        store: &Store,
        template: Option<&Document>,
        variables: &Variables,
    ) -> Result<String, FlowError> {
        let mut text = template
            .and_then(|template| template.get_str("Text").ok())
            .unwrap_or_default()
            .to_string();
        for (index, parameter) in template
            .map(|template| bson_items(template.get("Parameters")))
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let Bson::Document(parameter) = parameter else {
                continue;
            };
            let value =
                self.eval_owned(store, parameter.get("Expression").cloned(), variables, None)?;
            text = text.replace(&format!("{{{}}}", index + 1), &value.mendix_string());
        }
        Ok(text)
    }

    fn render_text_template(
        &self,
        store: &Store,
        template: Option<&Document>,
        variables: &Variables,
    ) -> Result<String, FlowError> {
        let text_value = template.and_then(|template| template.get("Text"));
        let mut text = match text_value {
            Some(Bson::Document(text)) => {
                let items = bson_items(text.get("Items"));
                let translation = items
                    .iter()
                    .filter_map(|item| match item {
                        Bson::Document(item) => Some(item),
                        _ => None,
                    })
                    .find(|item| item.get_str("LanguageCode").ok() == Some("en_US"))
                    .or_else(|| {
                        items.iter().find_map(|item| match item {
                            Bson::Document(item) => Some(item),
                            _ => None,
                        })
                    });
                translation
                    .and_then(|item| item.get_str("Text").ok())
                    .unwrap_or_default()
                    .to_string()
            }
            Some(other) => source_text(Some(other)),
            None => String::new(),
        };
        for (index, parameter) in template
            .map(|template| bson_items(template.get("Parameters")))
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let Bson::Document(parameter) = parameter else {
                continue;
            };
            let value =
                self.eval_owned(store, parameter.get("Expression").cloned(), variables, None)?;
            text = text.replace(&format!("{{{}}}", index + 1), &value.mendix_string());
        }
        Ok(text)
    }

    fn eval(
        &self,
        store: &Store,
        source: Option<&Bson>,
        variables: &Variables,
        node: Option<&ObjectValue>,
    ) -> Result<FlowValue, FlowError> {
        let text = source_text(source);
        self.expression
            .evaluate(&text, variables, node, &StoreMembers(store))
    }

    fn eval_owned(
        &self,
        store: &Store,
        source: Option<Bson>,
        variables: &Variables,
        node: Option<&ObjectValue>,
    ) -> Result<FlowValue, FlowError> {
        self.eval(store, source.as_ref(), variables, node)
    }

    fn object_value(&self, store: &Store, reference: &ObjectRef) -> Result<ObjectValue, FlowError> {
        store
            .find(&reference.entity, &reference.id)
            .map_err(FlowError::Runtime)?
            .ok_or_else(|| {
                FlowError::native(format!(
                    "unknown object {}/{}",
                    reference.entity, reference.id
                ))
            })
    }

    fn enforce_entity_access(&self, execution: &Execution) -> bool {
        execution.apply_entity_access && self.policy.is_some() && execution.security.is_some()
    }

    fn record_readable(&self, execution: &Execution, entity: &str) -> bool {
        let (Some(policy), Some(context)) = (&self.policy, &execution.security) else {
            return true;
        };
        policy.entity_allowed(entity, EntityAction::Read, None, context)
    }

    fn authorize_entity(
        &self,
        execution: &Execution,
        entity: &str,
        action: EntityAction,
        member: Option<&str>,
    ) -> Result<(), FlowError> {
        if !self.enforce_entity_access(execution) {
            return Ok(());
        }
        let (Some(policy), Some(context)) = (&self.policy, &execution.security) else {
            return Ok(());
        };
        if policy.entity_allowed(entity, action, member, context) {
            Ok(())
        } else {
            Err(FlowError::Runtime(
                mxrs_runtime::RuntimeError::NotAuthorized {
                    action: format!("{action:?}").to_lowercase(),
                    resource: match member {
                        Some(member) => format!("{entity}/{member}"),
                        None => entity.to_string(),
                    },
                },
            ))
        }
    }

    fn authorize_record(
        &self,
        execution: &Execution,
        reference: &ObjectRef,
        action: EntityAction,
    ) -> Result<(), FlowError> {
        self.authorize_entity(execution, &reference.entity, action, None)
    }
}

/// Reads object members through the store for `$variable/Member` and XPath
/// node references.
pub struct StoreMembers<'a>(pub &'a Store);

impl MemberSource for StoreMembers<'_> {
    fn object_member(&self, reference: &ObjectRef, member: &str) -> Result<FlowValue, FlowError> {
        let object = self
            .0
            .find(&reference.entity, &reference.id)
            .map_err(FlowError::Runtime)?
            .ok_or_else(|| {
                FlowError::native(format!(
                    "unknown object {}/{}",
                    reference.entity, reference.id
                ))
            })?;
        Ok(object
            .members
            .get(member)
            .map(FlowValue::from_member)
            .unwrap_or(FlowValue::Empty))
    }
}

fn qualified_entity(module_name: &str, entity: &mxrs_model::Entity) -> String {
    entity.qualified_name.clone().unwrap_or_else(|| {
        format!(
            "{module_name}.{}",
            entity.name.as_deref().unwrap_or_default()
        )
    })
}

fn fetch(variables: &Variables, name: &str) -> Result<FlowValue, FlowError> {
    variables
        .get(name)
        .cloned()
        .ok_or_else(|| FlowError::native(format!("unknown variable {name}")))
}

fn as_object(value: FlowValue) -> Result<ObjectRef, FlowError> {
    match value {
        FlowValue::Object(reference) => Ok(reference),
        other => Err(FlowError::native(format!(
            "expected an object, found {}",
            other.kind()
        ))),
    }
}

fn object_list(value: &FlowValue) -> Result<Vec<ObjectRef>, FlowError> {
    match value {
        FlowValue::Object(reference) => Ok(vec![reference.clone()]),
        FlowValue::List(values) => values
            .iter()
            .map(|value| as_object(value.clone()))
            .collect(),
        FlowValue::Empty => Ok(Vec::new()),
        other => Err(FlowError::native(format!(
            "expected objects, found {}",
            other.kind()
        ))),
    }
}

fn as_list(value: FlowValue) -> Vec<FlowValue> {
    match value {
        FlowValue::List(values) => values,
        FlowValue::Empty => Vec::new(),
        single => vec![single],
    }
}

fn second_list(value: Option<FlowValue>) -> Vec<FlowValue> {
    match value {
        Some(FlowValue::List(values)) => values,
        Some(FlowValue::Empty) | None => Vec::new(),
        Some(single) => vec![single],
    }
}

fn reference_of(value: &ObjectValue) -> ObjectRef {
    ObjectRef {
        entity: value.entity.clone(),
        id: value.id.clone(),
    }
}

fn commit_setting(action: &Document) -> String {
    match action.get("Commit") {
        Some(Bson::Document(setting)) => setting.get_str("Value").unwrap_or_default().to_string(),
        Some(Bson::String(setting)) => setting.clone(),
        _ => String::new(),
    }
}

fn action_range(source: &Document) -> Option<Document> {
    source.get_document("Range").ok().cloned()
}

fn member_name(item: &Document) -> String {
    let attribute = item.get_str("Attribute").unwrap_or_default();
    let member = if attribute.is_empty() {
        item.get_str("Association").unwrap_or_default()
    } else {
        attribute
    };
    member
        .rsplit(['.', '/'])
        .next()
        .unwrap_or(member)
        .to_string()
}

/// mxrb's `xpath_predicate`: `[group][group]` → `(group) and (group)`.
/// `None` means no constraint; anything not made purely of groups is an
/// error, never silently ignored.
fn xpath_predicate(xpath: &str) -> Result<Option<String>, FlowError> {
    let text = xpath.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let mut groups = Vec::new();
    let mut remainder = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        remainder.push_str(&rest[..open]);
        let Some(close) = rest[open..].find(']') else {
            remainder.push_str(&rest[open..]);
            break;
        };
        let group = rest[open + 1..open + close].trim();
        if group.contains('[') {
            return Err(FlowError::native(format!(
                "unsupported native XPath constraint: {xpath:?}"
            )));
        }
        if !group.is_empty() {
            groups.push(format!("({group})"));
        }
        rest = &rest[open + close + 1..];
    }
    remainder.push_str(rest);
    if groups.is_empty() || !remainder.trim().is_empty() {
        return Err(FlowError::native(format!(
            "unsupported native XPath constraint: {xpath:?}"
        )));
    }
    Ok(Some(groups.join(" and ")))
}

fn sort_values(
    values: Vec<ObjectValue>,
    sortings: &[Document],
) -> Result<Vec<ObjectValue>, FlowError> {
    if sortings.is_empty() {
        return Ok(values);
    }
    let mut keys = Vec::new();
    for sorting in sortings {
        let path = sorting
            .get_str("AttributePath")
            .ok()
            .map(str::to_string)
            .or_else(|| {
                sorting
                    .get_document("AttributeRef")
                    .ok()
                    .and_then(|reference| reference.get_str("Attribute").ok())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if path.is_empty() {
            return Err(FlowError::native(
                "native retrieve sorting requires an attribute",
            ));
        }
        let attribute = path
            .rsplit(['.', '/'])
            .next()
            .unwrap_or(path.as_str())
            .to_string();
        let descending = sorting
            .get_str("SortOrder")
            .unwrap_or_default()
            .eq_ignore_ascii_case("descending");
        keys.push((attribute, descending));
    }
    let mut failure = None;
    let mut sorted = values;
    sorted.sort_by(|left, right| {
        for (attribute, descending) in &keys {
            let ordering =
                compare_members(left.members.get(attribute), right.members.get(attribute));
            let ordering = match ordering {
                Ok(ordering) => ordering,
                Err(error) => {
                    failure.get_or_insert(error);
                    return std::cmp::Ordering::Equal;
                }
            };
            let ordering = if *descending {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        std::cmp::Ordering::Equal
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(sorted),
    }
}

/// mxrb's `compare_members`: nils sort last, mismatched kinds are an error.
fn compare_members(
    left: Option<&Value>,
    right: Option<&Value>,
) -> Result<std::cmp::Ordering, FlowError> {
    let left = left.map(FlowValue::from_member).unwrap_or(FlowValue::Empty);
    let right = right
        .map(FlowValue::from_member)
        .unwrap_or(FlowValue::Empty);
    match (&left, &right) {
        (FlowValue::Empty, FlowValue::Empty) => Ok(std::cmp::Ordering::Equal),
        (FlowValue::Empty, _) => Ok(std::cmp::Ordering::Greater),
        (_, FlowValue::Empty) => Ok(std::cmp::Ordering::Less),
        _ => left.compare(&right).map_err(|_| {
            FlowError::native(format!(
                "cannot sort {:?} and {:?}",
                left.mendix_string(),
                right.mendix_string()
            ))
        }),
    }
}

fn aggregate(function: &str, values: &[FlowValue]) -> Result<FlowValue, FlowError> {
    let compact: Vec<&FlowValue> = values
        .iter()
        .filter(|value| !matches!(value, FlowValue::Empty))
        .collect();
    let sum = || -> Result<FlowValue, FlowError> {
        let mut total = FlowValue::Int(0);
        for value in &compact {
            total = total
                .add(value)
                .map_err(|_| FlowError::native("unsupported aggregate values"))?;
        }
        Ok(total)
    };
    Ok(match function.to_ascii_lowercase().as_str() {
        "count" => FlowValue::Int(values.len() as i64),
        "sum" => sum()?,
        "minimum" | "min" => {
            let mut best: Option<&FlowValue> = None;
            for value in &compact {
                best = Some(match best {
                    None => value,
                    Some(current) => {
                        if value
                            .compare(current)
                            .map_err(|_| FlowError::native("unsupported aggregate values"))?
                            .is_lt()
                        {
                            value
                        } else {
                            current
                        }
                    }
                });
            }
            best.cloned().unwrap_or(FlowValue::Empty)
        }
        "maximum" | "max" => {
            let mut best: Option<&FlowValue> = None;
            for value in &compact {
                best = Some(match best {
                    None => value,
                    Some(current) => {
                        if value
                            .compare(current)
                            .map_err(|_| FlowError::native("unsupported aggregate values"))?
                            .is_gt()
                        {
                            value
                        } else {
                            current
                        }
                    }
                });
            }
            best.cloned().unwrap_or(FlowValue::Empty)
        }
        "average" | "avg" => {
            if values.is_empty() {
                FlowValue::Empty
            } else {
                let total = sum()?;
                let total = total.as_float().unwrap_or(0.0);
                FlowValue::Float(total / compact.len() as f64)
            }
        }
        other => return Err(FlowError::native(format!("unsupported aggregate {other}"))),
    })
}

fn bson_items(value: Option<&Bson>) -> Vec<Bson> {
    match value {
        Some(Bson::Array(items)) => mxrs_bson::parse_array(Some(items)).items,
        _ => Vec::new(),
    }
}

fn doc_items(container: &Document, key: &str) -> Vec<Document> {
    bson_items(container.get(key))
        .into_iter()
        .filter_map(|item| match item {
            Bson::Document(document) => Some(document),
            _ => None,
        })
        .collect()
}

fn identifier_of(object: &Document) -> String {
    object.get("$ID").map(identifier_value).unwrap_or_default()
}

fn destination_of(edge: &Document) -> String {
    edge.get("DestinationPointer")
        .map(identifier_value)
        .unwrap_or_default()
}

fn identifier_value(value: &Bson) -> String {
    match value {
        Bson::String(value) => value.clone(),
        other => mxrs_bson::extract_id(other).unwrap_or_default(),
    }
}

fn group_edges(edges: &[Document]) -> BTreeMap<String, Vec<&Document>> {
    let mut outgoing: BTreeMap<String, Vec<&Document>> = BTreeMap::new();
    for edge in edges {
        let origin = edge
            .get("OriginPointer")
            .map(identifier_value)
            .unwrap_or_default();
        outgoing.entry(origin).or_default().push(edge);
    }
    outgoing
}

/// mxrb's `collection_entry`: the start event, or (in nested collections)
/// the first executable object nothing points at.
fn collection_entry<'a>(
    list: &[&'a Document],
    objects: &BTreeMap<String, &'a Document>,
    outgoing: &BTreeMap<String, Vec<&Document>>,
    root: bool,
) -> Option<&'a Document> {
    let start = list
        .iter()
        .copied()
        .find(|object| object.get_str("$Type").ok() == Some("Microflows$StartEvent"));
    if start.is_some() || root {
        return start;
    }
    let mut has_incoming: BTreeMap<&str, bool> =
        objects.keys().map(|key| (key.as_str(), false)).collect();
    for origin in objects.keys() {
        for edge in outgoing.get(origin).into_iter().flatten() {
            let destination = destination_of(edge);
            if let Some(flag) = has_incoming.get_mut(destination.as_str()) {
                *flag = true;
            }
        }
    }
    list.iter().copied().find(|object| {
        let id = identifier_of(object);
        !has_incoming.get(id.as_str()).copied().unwrap_or(false) && executable_object(object)
    })
}

fn executable_object(object: &Document) -> bool {
    !matches!(
        object.get_str("$Type").unwrap_or_default(),
        "Microflows$Annotation" | "Microflows$MicroflowParameter"
    )
}

fn flow_case(edge: &Document) -> String {
    let values = bson_items(edge.get("CaseValues"));
    let value = values
        .iter()
        .find_map(|value| match value {
            Bson::Document(value) => Some(value.clone()),
            _ => None,
        })
        .or_else(|| edge.get_document("NewCaseValue").ok().cloned())
        .unwrap_or_default();
    if value.get_str("$Type").ok() == Some("Microflows$NoCase") {
        return String::new();
    }
    value.get_str("Value").unwrap_or_default().to_string()
}

/// mxrb's `Expression#unwrap`: a source is a raw string or a document with
/// `Value`/`Expression`.
fn source_text(value: Option<&Bson>) -> String {
    match value {
        Some(Bson::String(text)) => text.clone(),
        Some(Bson::Document(document)) => document
            .get_str("Value")
            .or_else(|_| document.get_str("Expression"))
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// mxrb's `underscore`.
fn underscore(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 4);
    let mut previous_lower = false;
    for character in value.chars() {
        if character.is_ascii_uppercase() && previous_lower {
            result.push('_');
        }
        previous_lower = character.is_ascii_lowercase() || character.is_ascii_digit();
        result.push(character.to_ascii_lowercase());
    }
    result
}
