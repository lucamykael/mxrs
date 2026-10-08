//! Preflight for authored flow signatures and calls, before persistence starts.
use std::collections::{HashMap, HashSet};

use mxrs_bson::Document;
use mxrs_ir::flow::FlowReturnType;
use mxrs_ir::{Activity, MicroflowDecl, ProjectDecl};
use mxrs_model::{Microflow, Module};
use mxrs_mpr::MprFile;

use crate::{Result, WriterError};

type Signature = Vec<(String, FlowReturnType)>;

enum Target<'a> {
    Authored(&'a MicroflowDecl),
    Native(&'a Microflow),
}

pub(crate) fn validate_project(project: &ProjectDecl, existing: &[Module]) -> Result<()> {
    let mut entities = HashSet::new();
    for module in existing {
        let name = module.name.as_deref().unwrap_or_default();
        // A module's declaration supersedes what the model holds of its
        // entities — unless the project only installed the module, and
        // declares nothing of it but what the frontend states.
        let restated = project
            .modules
            .iter()
            .any(|decl| decl.name == name && !decl.installed);
        if !restated {
            add_entities(&mut entities, module);
        }
    }
    for module in &project.modules {
        entities.extend(
            module
                .entities
                .iter()
                .map(|entity| format!("{}.{}", module.name, entity.name)),
        );
    }
    let flows = project
        .modules
        .iter()
        .flat_map(|module| {
            module
                .microflows
                .iter()
                .map(|flow| (module.name.as_str(), flow, false))
                .chain(
                    module
                        .nanoflows
                        .iter()
                        .map(|flow| (module.name.as_str(), flow, true)),
                )
        })
        .collect::<Vec<_>>();
    validate_roles(project, existing, &flows)?;
    validate(&flows, &entities, existing)?;
    validate_rest_operations(project, existing)
}

/// Checks every operation a published REST service declares against the
/// microflow it calls, as Studio Pro would before the model deploys: the
/// microflow is one the project has, each parameter binds one of its
/// parameters, and the path's `{name}`s are its path parameters.
fn validate_rest_operations(project: &ProjectDecl, existing: &[Module]) -> Result<()> {
    let parameters_of = |qualified: &str| -> Option<Vec<String>> {
        let (module, name) = qualified.split_once('.')?;
        let declared = project
            .modules
            .iter()
            .filter(|declared| declared.name == module)
            .flat_map(|declared| &declared.microflows)
            .find(|flow| flow.name == name);
        if let Some(flow) = declared {
            return Some(flow.parameters.iter().map(|p| p.name.clone()).collect());
        }
        existing
            .iter()
            .filter(|native| native.name.as_deref() == Some(module))
            .flat_map(|native| &native.microflows)
            .find(|flow| flow.name.as_deref() == Some(name))
            .map(|flow| {
                flow.parameters
                    .iter()
                    .filter_map(|parameter| parameter.get_str("Name").ok().map(str::to_string))
                    .collect()
            })
    };
    for module in &project.modules {
        for service in &module.published_rest_services {
            for resource in &service.resources {
                for operation in &resource.operations {
                    let error = |reason: String| WriterError::InvalidRestOperation {
                        service: format!("{}.{}", module.name, service.name),
                        operation: format!(
                            "{} {}",
                            operation.method.as_http(),
                            service.route(resource, operation)
                        ),
                        reason,
                    };
                    let Some(microflow_parameters) = parameters_of(&operation.microflow) else {
                        return Err(error(format!(
                            "calls {:?}, which is no microflow of the project",
                            operation.microflow
                        )));
                    };
                    for parameter in &operation.parameters {
                        if !microflow_parameters.contains(&parameter.microflow_parameter) {
                            return Err(error(format!(
                                "binds its parameter {:?} to {:?}, which {} does not take",
                                parameter.name, parameter.microflow_parameter, operation.microflow
                            )));
                        }
                    }
                    let in_path: Vec<&str> = operation
                        .path
                        .split('/')
                        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
                        .collect();
                    let path_parameters: Vec<&str> = operation
                        .parameters
                        .iter()
                        .filter(|parameter| parameter.source == mxrs_ir::RestParameterSource::Path)
                        .map(|parameter| parameter.name.as_str())
                        .collect();
                    if let Some(missing) =
                        in_path.iter().find(|name| !path_parameters.contains(name))
                    {
                        return Err(error(format!(
                            "has {{{missing}}} in its path and no path parameter of that name"
                        )));
                    }
                    if let Some(unused) =
                        path_parameters.iter().find(|name| !in_path.contains(name))
                    {
                        return Err(error(format!(
                            "has a path parameter {unused:?} its path does not hold"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Checks that every role a flow is stated to allow is one its module
/// declares — or, when the declaration leaves the module's roles to the
/// model, one the model has. A module whose roles neither says is not
/// checked.
fn validate_roles(
    project: &ProjectDecl,
    existing: &[Module],
    flows: &[(&str, &MicroflowDecl, bool)],
) -> Result<()> {
    let mut known: HashMap<&str, HashSet<&str>> = HashMap::new();
    for module in existing {
        if let Some(name) = module.name.as_deref() {
            known.insert(
                name,
                module
                    .module_roles
                    .iter()
                    .filter_map(|role| role.name.as_deref())
                    .collect(),
            );
        }
    }
    for module in &project.modules {
        if let Some(roles) = &module.roles {
            known.insert(
                &module.name,
                roles.iter().map(|role| role.name.as_str()).collect(),
            );
        }
    }
    for &(module, flow, _) in flows {
        for role in flow.allowed_roles.iter().flatten() {
            let is_module = |owner: &str| {
                project.modules.iter().any(|module| module.name == owner)
                    || existing
                        .iter()
                        .any(|module| module.name.as_deref() == Some(owner))
            };
            let valid = role.split_once('.').is_some_and(|(owner, name)| {
                known
                    .get(owner)
                    .map_or_else(|| is_module(owner), |roles| roles.contains(name))
            });
            if !valid {
                return Err(WriterError::InvalidFlowActivity {
                    flow: format!("{module}.{}", flow.name),
                    reason: format!("roles(...) names {role:?}, which is not a module role"),
                });
            }
        }
    }
    Ok(())
}

/// Checks one flow declaration the way a build would before writing it:
/// its parameters, the variables its activities declare, the entities its
/// signature names. An importer asks this before it offers a flow as Rust,
/// so what it generates is something the writer accepts.
pub fn validate_flow_declaration(
    module: &str,
    flow: &MicroflowDecl,
    nanoflow: bool,
    entities: &HashSet<String>,
) -> Result<()> {
    validate(&[(module, flow, nanoflow)], entities, &[])
}

pub(crate) fn validate_documents(
    mpr: &MprFile,
    module: &str,
    flows: &[MicroflowDecl],
    nanoflow: bool,
) -> Result<()> {
    let modules = mpr
        .units_by_containment("Modules")?
        .iter()
        .map(|unit| Module::load(mpr, unit))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut entities = HashSet::new();
    for module in &modules {
        add_entities(&mut entities, module);
    }
    let flows = flows
        .iter()
        .map(|flow| (module, flow, nanoflow))
        .collect::<Vec<_>>();
    validate(&flows, &entities, &modules)
}

fn add_entities(entities: &mut HashSet<String>, module: &Module) {
    let module_name = module.name.as_deref().unwrap_or_default();
    entities.extend(
        module
            .entities()
            .iter()
            .filter_map(|entity| Some(format!("{module_name}.{}", entity.name.as_deref()?))),
    );
}

fn validate(
    flows: &[(&str, &MicroflowDecl, bool)],
    entities: &HashSet<String>,
    existing: &[Module],
) -> Result<()> {
    let mut targets = HashMap::new();
    for module in existing {
        for flow in &module.microflows {
            if let (Some(module), Some(name)) = (&module.name, &flow.name) {
                targets.insert(format!("{module}.{name}"), Target::Native(flow));
            }
        }
    }
    let mut names = HashSet::new();
    for &(module, flow, nanoflow) in flows {
        let name = format!("{module}.{}", flow.name);
        if !names.insert((name.clone(), nanoflow)) {
            return Err(WriterError::InvalidFlowParameter {
                flow: name,
                parameter: String::new(),
                reason: "duplicate flow declaration".into(),
            });
        }
        for previous_module in existing
            .iter()
            .filter(|previous| previous.name.as_deref() == Some(module))
        {
            let previous_flows = if nanoflow {
                &previous_module.nanoflows
            } else {
                &previous_module.microflows
            };
            for previous in previous_flows
                .iter()
                .filter(|previous| previous.name.as_deref() == Some(flow.name.as_str()))
            {
                native_signature(previous).map_err(|reason| WriterError::InvalidFlowParameter {
                    flow: name.clone(),
                    parameter: String::new(),
                    reason,
                })?;
            }
        }
        let mut parameters = HashSet::new();
        for parameter in &flow.parameters {
            let error = |reason: String| WriterError::InvalidFlowParameter {
                flow: name.clone(),
                parameter: parameter.name.clone(),
                reason,
            };
            if !valid_identifier(&parameter.name) {
                return Err(error(
                    "expected a nonempty identifier with letters, digits or underscores".into(),
                ));
            }
            if !parameters.insert(&parameter.name) {
                return Err(error("duplicate parameter".into()));
            }
            validate_entity(&parameter.value_type, entities).map_err(error)?;
        }
        if !nanoflow {
            targets.insert(name, Target::Authored(flow));
        }
    }
    for &(module, flow, _) in flows {
        let name = format!("{module}.{}", flow.name);
        let mut variables = flow
            .parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        validate_activities(&name, &flow.activities, &targets, entities, &mut variables)?;
        validate_structure(&name, &flow.activities, &flow.rescue_activities)?;
        validate_activities(
            &name,
            &flow.rescue_activities,
            &targets,
            entities,
            &mut variables,
        )?;
    }
    Ok(())
}

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn validate_entity(
    value_type: &FlowReturnType,
    entities: &HashSet<String>,
) -> std::result::Result<(), String> {
    // The System module's entities are the runtime's: no model stores
    // them, and every model may name them.
    if let FlowReturnType::Object(entity) | FlowReturnType::List(entity) = value_type
        && !entities.contains(entity)
        && !entity.starts_with("System.")
    {
        return Err(format!("unknown flow value entity {entity:?}"));
    }
    Ok(())
}

fn native_signature(flow: &Microflow) -> std::result::Result<Signature, String> {
    let mut names = HashSet::new();
    flow.parameters
        .iter()
        .map(|parameter| {
            if parameter.get_str("$Type").ok() != Some("Microflows$MicroflowParameter") {
                return Err("unsupported native parameter document".into());
            }
            let name = parameter
                .get_str("Name")
                .map_err(|_| "native parameter has no name")?;
            if !valid_identifier(name) || !names.insert(name) {
                return Err(format!("invalid or duplicate native parameter {name:?}"));
            }
            let value_type = parameter
                .get_document("VariableType")
                .map_err(|_| format!("parameter {name:?} has no variable type"))?;
            Ok((name.to_string(), native_type(value_type)?))
        })
        .collect()
}

fn native_type(doc: &Document) -> std::result::Result<FlowReturnType, String> {
    Ok(match doc.get_str("$Type").unwrap_or_default() {
        "DataTypes$StringType" => FlowReturnType::String,
        "DataTypes$IntegerType" => FlowReturnType::Integer,
        "DataTypes$FloatType" => FlowReturnType::Float,
        "DataTypes$DecimalType" => FlowReturnType::Decimal,
        "DataTypes$BooleanType" => FlowReturnType::Boolean,
        "DataTypes$DateTimeType" => FlowReturnType::DateTime,
        "DataTypes$BinaryType" => FlowReturnType::Binary,
        kind @ ("DataTypes$ObjectType" | "DataTypes$ListType") => {
            let entity = doc
                .get_str("Entity")
                .map_err(|_| "native object/list type has no entity")?
                .to_string();
            if kind == "DataTypes$ObjectType" {
                FlowReturnType::Object(entity)
            } else {
                FlowReturnType::List(entity)
            }
        }
        "DataTypes$EnumerationType" => FlowReturnType::Enumeration(
            doc.get_str("Enumeration")
                .map_err(|_| "native enumeration type has no enumeration")?
                .to_string(),
        ),
        kind => return Err(format!("unsupported native data type {kind:?}")),
    })
}

fn validate_activities(
    flow: &str,
    activities: &[Activity],
    targets: &HashMap<String, Target<'_>>,
    entities: &HashSet<String>,
    variables: &mut HashSet<String>,
) -> Result<()> {
    for activity in activities {
        match activity {
            Activity::CallMicroflow {
                name,
                mappings,
                result_variable,
                result_type,
                use_return,
            } => {
                let error = |reason: String| WriterError::InvalidFlowCall {
                    flow: flow.into(),
                    target: name.clone(),
                    reason,
                };
                let target = targets
                    .get(name)
                    .ok_or_else(|| error("unknown microflow target".into()))?;
                if *use_return {
                    let variable = result_variable
                        .as_deref()
                        .ok_or_else(|| error("result capture requires a variable name".into()))?;
                    declare_variable(flow, variable, variables)?;
                    let expected = result_type.as_ref().ok_or_else(|| {
                        error(
                            "result capture requires a checked type; use call_microflow_result"
                                .into(),
                        )
                    })?;
                    validate_entity(expected, entities).map_err(&error)?;
                    let actual = match target {
                        Target::Authored(flow) => flow.return_type.clone(),
                        Target::Native(flow) => match flow.return_type_document.as_ref() {
                            None => None,
                            Some(doc)
                                if doc.get_str("$Type").ok() == Some("DataTypes$VoidType") =>
                            {
                                None
                            }
                            Some(doc) => Some(native_type(doc).map_err(|reason| {
                                error(format!("unsupported native return signature: {reason}"))
                            })?),
                        },
                    }
                    .ok_or_else(|| error("cannot capture a result from a void flow".into()))?;
                    let native_integer_alias = matches!(target, Target::Native(_))
                        && actual == FlowReturnType::Integer
                        && expected == &FlowReturnType::Long;
                    if &actual != expected && !native_integer_alias {
                        return Err(error(format!(
                            "result expects {expected:?}, but target returns {actual:?}"
                        )));
                    }
                } else if result_variable.is_some() || result_type.is_some() {
                    return Err(error(
                        "discarded call cannot declare a result variable or type".into(),
                    ));
                }
                let signature = match target {
                    Target::Authored(flow) => flow
                        .parameters
                        .iter()
                        .map(|p| (p.name.clone(), p.value_type.clone()))
                        .collect(),
                    Target::Native(flow) => native_signature(flow).map_err(&error)?,
                };
                for (_, value_type) in &signature {
                    validate_entity(value_type, entities).map_err(&error)?;
                }
                let mut seen = HashSet::new();
                for mapping in mappings {
                    let parameter = mapping
                        .parameter
                        .strip_prefix(&format!("{name}."))
                        .unwrap_or(&mapping.parameter);
                    if !seen.insert(parameter) {
                        return Err(error(format!("duplicate argument {parameter:?}")));
                    }
                    let expected = signature
                        .iter()
                        .find(|(name, _)| name == parameter)
                        .ok_or_else(|| error(format!("unknown argument {parameter:?}")))?;
                    let actual = mapping.value_type.as_ref().ok_or_else(|| {
                        error(format!(
                            "argument {parameter:?} has no checked expression type"
                        ))
                    })?;
                    // Native signatures cannot distinguish the authoring
                    // Integer/Long tags: both persist as IntegerType.
                    let native_integer_alias = matches!(target, Target::Native(_))
                        && expected.1 == FlowReturnType::Integer
                        && actual == &FlowReturnType::Long;
                    if actual != &expected.1 && !native_integer_alias {
                        return Err(error(format!(
                            "argument {parameter:?} expects {:?}, got {actual:?}",
                            expected.1
                        )));
                    }
                }
                for (parameter, _) in &signature {
                    if !seen.contains(parameter.as_str()) {
                        return Err(error(format!("missing argument {parameter:?}")));
                    }
                }
            }
            Activity::Decision {
                true_branch,
                false_branch,
                ..
            }
            | Activity::RuleDecision {
                true_branch,
                false_branch,
                ..
            } => {
                validate_activities(flow, true_branch, targets, entities, &mut variables.clone())?;
                validate_activities(
                    flow,
                    false_branch,
                    targets,
                    entities,
                    &mut variables.clone(),
                )?;
            }
            Activity::LoopOver {
                activities,
                iterator,
                ..
            } => {
                let mut local = variables.clone();
                declare_variable(flow, iterator, &mut local)?;
                validate_activities(flow, activities, targets, entities, &mut local)?;
            }
            Activity::WhileLoop { activities, .. } => {
                validate_activities(flow, activities, targets, entities, &mut variables.clone())?;
            }
            Activity::CreateObject { variable, .. }
            | Activity::CreateList { variable, .. }
            | Activity::RetrieveObjects { variable, .. } => {
                declare_variable(flow, variable, variables)?
            }
            Activity::AggregateCount {
                output_variable, ..
            } => declare_variable(flow, output_variable, variables)?,
            Activity::Disabled(inner) => validate_activities(
                flow,
                std::slice::from_ref(inner.as_ref()),
                targets,
                entities,
                variables,
            )?,
            Activity::OnError {
                handling,
                activity,
                handler,
            } => {
                let invalid = |reason: &str| WriterError::InvalidFlowActivity {
                    flow: flow.into(),
                    reason: reason.into(),
                };
                let mut handled = activity.as_ref();
                while let Activity::Disabled(inner) = handled {
                    handled = inner;
                }
                if matches!(
                    handled,
                    Activity::Decision { .. }
                        | Activity::RuleDecision { .. }
                        | Activity::RuleSwitch { .. }
                        | Activity::Label(_)
                        | Activity::Jump(_)
                        | Activity::Switch { .. }
                        | Activity::TypeSwitch { .. }
                        | Activity::ReturnValue { .. }
                        | Activity::BreakLoop
                        | Activity::ContinueLoop
                        | Activity::RaiseError
                        | Activity::OnError { .. }
                ) {
                    return Err(invalid(
                        "only an action or a loop can have an error handler",
                    ));
                }
                if *handling == mxrs_ir::ErrorHandling::Continue && !handler.is_empty() {
                    return Err(invalid(
                        "an activity that continues on error has no handler to run",
                    ));
                }
                // What the handler declares is its own: the activity it
                // answers for did not finish.
                let mut handler_variables = variables.clone();
                validate_activities(
                    flow,
                    std::slice::from_ref(activity.as_ref()),
                    targets,
                    entities,
                    variables,
                )?;
                validate_activities(flow, handler, targets, entities, &mut handler_variables)?;
            }
            Activity::Switch { cases, .. }
            | Activity::RuleSwitch { cases, .. }
            | Activity::TypeSwitch { cases, .. } => {
                let mut values = HashSet::new();
                for case in cases {
                    if case.values.is_empty()
                        || !case
                            .values
                            .iter()
                            .all(|value| values.insert(value.as_str()))
                    {
                        return Err(WriterError::InvalidFlowActivity {
                            flow: flow.into(),
                            reason: "each case of a switch is selected by its own values".into(),
                        });
                    }
                    validate_activities(
                        flow,
                        &case.activities,
                        targets,
                        entities,
                        &mut variables.clone(),
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Checks the shape of a flow's bodies — its own (with the handler that
/// rescues it, which the model draws in the same container) and each loop's:
/// every jump has a label of its name in that same body, no two labels of
/// the flow share a name, no body opens with a jump, no loop body opens with
/// a label (a loop is entered at the activity nothing leads to), no switch
/// is without cases, and only an action can be disabled.
fn validate_structure(flow: &str, activities: &[Activity], rescue: &[Activity]) -> Result<()> {
    let mut names = HashSet::new();
    validate_body(flow, &[activities, rescue], false, &mut names)
}

fn validate_body<'a>(
    flow: &str,
    parts: &[&'a [Activity]],
    in_loop: bool,
    names: &mut HashSet<&'a str>,
) -> Result<()> {
    fn collect<'a>(
        activities: &'a [Activity],
        labels: &mut Vec<&'a str>,
        jumps: &mut Vec<&'a str>,
        bodies: &mut Vec<&'a [Activity]>,
    ) -> std::result::Result<(), String> {
        for activity in activities {
            match activity {
                Activity::Label(name) => labels.push(name),
                Activity::Jump(name) => jumps.push(name),
                Activity::Decision {
                    true_branch,
                    false_branch,
                    ..
                }
                | Activity::RuleDecision {
                    true_branch,
                    false_branch,
                    ..
                } => {
                    collect(true_branch, labels, jumps, bodies)?;
                    collect(false_branch, labels, jumps, bodies)?;
                }
                Activity::Switch { cases, .. }
                | Activity::RuleSwitch { cases, .. }
                | Activity::TypeSwitch { cases, .. } => {
                    if cases.is_empty() {
                        return Err("a switch needs at least one case".to_string());
                    }
                    for case in cases {
                        collect(&case.activities, labels, jumps, bodies)?;
                    }
                }
                Activity::OnError {
                    activity, handler, ..
                } => {
                    collect(
                        std::slice::from_ref(activity.as_ref()),
                        labels,
                        jumps,
                        bodies,
                    )?;
                    collect(handler, labels, jumps, bodies)?;
                }
                Activity::Disabled(inner) => {
                    let action = match inner.as_ref() {
                        Activity::OnError { activity, .. } => activity.as_ref(),
                        other => other,
                    };
                    if matches!(
                        action,
                        Activity::Label(_)
                            | Activity::Jump(_)
                            | Activity::Decision { .. }
                            | Activity::RuleDecision { .. }
                            | Activity::Switch { .. }
                            | Activity::RuleSwitch { .. }
                            | Activity::TypeSwitch { .. }
                            | Activity::LoopOver { .. }
                            | Activity::WhileLoop { .. }
                            | Activity::BreakLoop
                            | Activity::ContinueLoop
                            | Activity::RaiseError
                            | Activity::ReturnValue { .. }
                            | Activity::Disabled(_)
                    ) {
                        return Err("only an action can be disabled".to_string());
                    }
                    collect(std::slice::from_ref(inner.as_ref()), labels, jumps, bodies)?
                }
                Activity::LoopOver { activities, .. } | Activity::WhileLoop { activities, .. } => {
                    bodies.push(activities)
                }
                _ => {}
            }
        }
        Ok(())
    }
    let invalid = |reason: String| WriterError::InvalidFlowActivity {
        flow: flow.into(),
        reason,
    };
    let (mut labels, mut jumps, mut bodies) = (Vec::new(), Vec::new(), Vec::new());
    for part in parts {
        collect(part, &mut labels, &mut jumps, &mut bodies).map_err(invalid)?;
        if matches!(part.first(), Some(Activity::Jump(_))) {
            return Err(invalid("a body cannot open with a jump".to_string()));
        }
    }
    if in_loop
        && matches!(
            parts.first().and_then(|body| body.first()),
            Some(Activity::Label(_))
        )
    {
        return Err(invalid(
            "a loop body cannot open with a label: nothing would mark where the loop starts"
                .to_string(),
        ));
    }
    let known: HashSet<&str> = labels.iter().copied().collect();
    for label in labels {
        if !names.insert(label) {
            return Err(invalid(format!("two labels are named {label:?}")));
        }
    }
    for jump in jumps {
        if !known.contains(jump) {
            return Err(invalid(format!(
                "a jump to {jump:?} has no label of that name in the same flow or loop body"
            )));
        }
    }
    for body in bodies {
        validate_body(flow, &[body], true, names)?;
    }
    Ok(())
}

fn declare_variable(flow: &str, variable: &str, variables: &mut HashSet<String>) -> Result<()> {
    let reason = if !valid_identifier(variable) {
        Some("invalid variable identifier")
    } else if !variables.insert(variable.to_string()) {
        Some("duplicate flow variable")
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(WriterError::InvalidFlowVariable {
            flow: flow.into(),
            variable: variable.into(),
            reason: reason.into(),
        });
    }
    Ok(())
}
