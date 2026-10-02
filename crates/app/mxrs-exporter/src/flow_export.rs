//! Reconstructs attested structured bodies as typed builder calls. Unsupported
//! expressions, action options and graph shapes stay in the imported model.
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use mxrs_bson::{Bson, Document};
use mxrs_ir::Member;
use mxrs_ir::flow::FlowReturnType as Ty;
use mxrs_ir::{Activity, FlowParameterDecl, MicroflowCallMapping, MicroflowDecl};
use mxrs_model::attribute::AttributeType;
use mxrs_model::{Microflow, Module, Project};
use mxrs_writer::flow_graph::{Node, documents, structured_nodes};

use crate::names::{self, ModelNames};
use crate::{Result, derive_pascal_case, inner_file_stem, rust_keyword, rust_string};

pub(crate) struct ConvertedFlow {
    pub module: String,
    pub native_type: String,
    pub declaration: MicroflowDecl,
    source: Vec<String>,
}

impl ConvertedFlow {
    pub(crate) fn is_nanoflow(&self) -> bool {
        self.native_type == "Microflows$Nanoflow"
    }
}

pub(crate) struct RenderedFlowSource {
    pub module: String,
    pub file_name: String,
    pub source: String,
}

struct AttributeInfo {
    marker: String,
    value_type: Ty,
    writable: bool,
}

type Attributes = HashMap<(String, String), AttributeInfo>;

fn attributes(modules: &[Module]) -> Attributes {
    let mut result = HashMap::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        for entity in module.entities() {
            let Some(entity_name) = entity.name.as_deref() else {
                continue;
            };
            let qualified = format!("{module_name}.{entity_name}");
            if marker(&qualified).is_none() {
                continue;
            }
            for attribute in &entity.attributes {
                let Some(name) = attribute.name.as_deref() else {
                    continue;
                };
                if !mxrs_typegen::is_rust_identifier(name) {
                    continue;
                }
                let Some(native) = attribute
                    .raw_type_doc
                    .as_ref()
                    .and_then(|d| d.get_str("$Type").ok())
                else {
                    continue;
                };
                if AttributeType::from_storage_type(native) != Some(attribute.attribute_type) {
                    continue;
                }
                let value_type = match attribute.attribute_type {
                    AttributeType::String | AttributeType::HashString => Ty::String,
                    AttributeType::Integer => Ty::Integer,
                    AttributeType::Long | AttributeType::AutoNumber => Ty::Long,
                    AttributeType::Float => Ty::Float,
                    AttributeType::Decimal => Ty::Decimal,
                    AttributeType::Boolean => Ty::Boolean,
                    AttributeType::DateTime => Ty::DateTime,
                    AttributeType::Binary => Ty::Binary,
                    AttributeType::Enum => continue,
                };
                result.insert(
                    (qualified.clone(), name.into()),
                    AttributeInfo {
                        marker: names::attribute(&qualified, name),
                        value_type,
                        writable: attribute.attribute_type != AttributeType::AutoNumber
                            && attribute
                                .raw_value_doc
                                .as_ref()
                                .and_then(|d| d.get_str("$Type").ok())
                                == Some("DomainModels$StoredValue"),
                    },
                );
            }
        }
    }
    result
}

pub(crate) fn data_type(doc: &Document) -> Option<Ty> {
    Some(match doc.get_str("$Type").ok()? {
        "DataTypes$StringType" => Ty::String,
        "DataTypes$IntegerType" => Ty::Long,
        "DataTypes$BooleanType" => Ty::Boolean,
        "DataTypes$FloatType" => Ty::Float,
        "DataTypes$DecimalType" => Ty::Decimal,
        "DataTypes$DateTimeType" => Ty::DateTime,
        "DataTypes$BinaryType" => Ty::Binary,
        "DataTypes$ObjectType" => Ty::Object(doc.get_str("Entity").ok()?.into()),
        "DataTypes$ListType" => Ty::List(doc.get_str("Entity").ok()?.into()),
        _ => return None,
    })
}

/// A reference to the struct declaring entity `Module.Entity`.
pub(crate) fn marker(name: &str) -> Option<String> {
    let (module, entity) = name.split_once('.')?;
    if !mxrs_typegen::is_rust_identifier(module) || !mxrs_typegen::is_rust_identifier(entity) {
        return None;
    }
    Some(names::entity(name))
}

/// A reference to the type naming microflow `Module.Flow`.
pub(crate) fn flow_marker(name: &str) -> Option<String> {
    let (module, flow) = name.split_once('.')?;
    if !mxrs_typegen::is_rust_identifier(module) || !mxrs_typegen::is_rust_identifier(flow) {
        return None;
    }
    Some(names::microflow(name))
}

fn tag(ty: &Ty, entities: &HashSet<String>) -> Option<String> {
    Some(match ty {
        Ty::String => "MxString".into(),
        Ty::Integer => "MxInteger".into(),
        Ty::Long => "MxLong".into(),
        Ty::Boolean => "MxBool".into(),
        Ty::Float => "MxFloat".into(),
        Ty::Decimal => "MxDecimal".into(),
        Ty::DateTime => "MxDateTime".into(),
        Ty::Binary => "MxBinary".into(),
        // An enumeration value has no typed expression form here yet.
        Ty::Enumeration(_) => return None,
        Ty::Object(entity) | Ty::List(entity) => {
            if !entities.contains(entity) {
                return None;
            }
            format!(
                "{}<{}>",
                if matches!(ty, Ty::Object(_)) {
                    "MxObject"
                } else {
                    "MxList"
                },
                marker(entity)?
            )
        }
    })
}

/// The Rust binding for one flow variable.
///
/// A variable named the way Mendix names them — `NewOrder` — is bound as a
/// Rust author would bind it, `new_order`. That mapping is only taken when
/// it can be read back (`new_order` → `NewOrder`), so two variables never
/// share a binding; any other name keeps its exact spelling behind a
/// `value_` prefix, which no converted name starts with.
pub(crate) fn binding(name: &str) -> Option<String> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let snake = crate::snake_ident(name);
    let readable = derive_pascal_case(&snake) == name
        && !snake.starts_with("value_")
        && !rust_keyword(&snake)
        && !matches!(
            snake.as_str(),
            "flow"
                | "parameter"
                | "string"
                | "integer"
                | "long"
                | "float"
                | "decimal"
                | "boolean"
                | "attribute"
                | "association"
                // What the builders' own closures and helpers are called:
                // a variable of the same name would hide them, or be taken
                // for used because of them.
                | "mx"
                | "var"
                | "create"
                | "change"
                | "commit"
                | "delete"
                | "rollback"
                | "retrieve"
                | "aggregate"
                | "log"
                | "call"
                | "message"
                | "page"
                | "sort"
                | "on"
                | "rule"
        );
    Some(if readable {
        snake
    } else {
        format!("value_{name}")
    })
}

fn expression(
    value: &str,
    ty: &Ty,
    variables: &HashMap<String, Ty>,
    attributes: &Attributes,
) -> Option<String> {
    if let Some(name) = value.strip_prefix('$') {
        if let Some((object, member)) = name.split_once('/') {
            let Ty::Object(entity) = variables.get(object)? else {
                return None;
            };
            let attribute = attributes.get(&(entity.clone(), member.into()))?;
            let rendered = format!("{}.get({})", binding(object)?, attribute.marker);
            return if &attribute.value_type == ty {
                Some(rendered)
            } else if attribute.value_type == Ty::Integer && ty == &Ty::Long {
                Some(format!("{rendered}.into_long()"))
            } else {
                None
            };
        }
        if variables.get(name) != Some(ty) {
            return None;
        }
        return Some(format!("&{}", binding(name)?));
    }
    match ty {
        Ty::String => {
            let inner = value.strip_prefix('\'')?.strip_suffix('\'')?;
            let text = inner.replace("''", "'");
            if format!("'{}'", text.replace('\'', "''")) != value {
                return None;
            }
            Some(format!("string({})", rust_string(&text)))
        }
        Ty::Boolean if matches!(value, "true" | "false") => Some(format!("boolean({value})")),
        Ty::Long => {
            let parsed = value.parse::<i64>().ok()?;
            (parsed.to_string() == value).then(|| format!("long({value})"))
        }
        Ty::Integer => {
            let parsed = value.parse::<i32>().ok()?;
            (parsed.to_string() == value).then(|| format!("integer({value})"))
        }
        Ty::Float | Ty::Decimal => {
            let parsed = value.parse::<f64>().ok()?;
            let canonical = if parsed.to_string().contains(['.', 'e', 'E']) {
                parsed.to_string()
            } else {
                format!("{parsed}.0")
            };
            if !parsed.is_finite() || canonical != value {
                return None;
            }
            Some(format!(
                "{}({value})",
                if ty == &Ty::Float { "float" } else { "decimal" }
            ))
        }
        _ => None,
    }
}

fn signature(flow: &Microflow) -> Option<Vec<(String, Ty)>> {
    let mut seen = HashSet::new();
    flow.parameters
        .iter()
        .map(|p| {
            if p.get_str("$Type").ok()? != "Microflows$MicroflowParameter" {
                return None;
            }
            let name = p.get_str("Name").ok()?.to_string();
            binding(&name)?;
            if !seen.insert(name.clone()) {
                return None;
            }
            Some((name, data_type(p.get_document("VariableType").ok()?)?))
        })
        .collect()
}

/// Why each flow that is not declared in Rust stays in the imported model,
/// by module, name and whether it is a nanoflow.
pub(crate) type KeptFlows = HashMap<(String, String, bool), String>;

pub(crate) fn collect(project: &Project, modules: &[Module]) -> Result<Vec<ConvertedFlow>> {
    Ok(collect_all(project, modules)?.0)
}

/// Every flow that can be declared in Rust, and for each one that cannot,
/// why not.
pub(crate) fn collect_all(
    project: &Project,
    modules: &[Module],
) -> Result<(Vec<ConvertedFlow>, KeptFlows)> {
    let mut kept = KeptFlows::new();
    let attributes = attributes(modules);
    let entities: HashSet<_> = modules
        .iter()
        .flat_map(|m| {
            m.entities()
                .iter()
                .filter_map(|e| Some(format!("{}.{}", m.name.as_deref()?, e.name.as_deref()?)))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut targets: HashMap<_, _> = modules
        .iter()
        .flat_map(|m| {
            m.microflows
                .iter()
                .filter_map(|f| Some((format!("{}.{}", m.name.as_deref()?, f.name.as_deref()?), f)))
                .collect::<Vec<_>>()
        })
        .collect();
    let accessors: HashSet<(String, String)> = modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.clone().unwrap_or_default();
            module.entities().iter().flat_map(move |entity| {
                let qualified = format!(
                    "{module_name}.{}",
                    entity.name.as_deref().unwrap_or_default()
                );
                entity
                    .attributes
                    .iter()
                    .filter_map(|attribute| attribute.name.clone())
                    .filter(|name| !name.is_empty())
                    .map(move |name| (qualified.clone(), name))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let microflows: HashSet<String> = modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.clone().unwrap_or_default();
            module
                .microflows
                .iter()
                .filter_map(move |flow| Some(format!("{module_name}.{}", flow.name.as_deref()?)))
        })
        .collect();
    let model = crate::flow_general::Model {
        entities: &entities,
        attributes: &accessors,
        microflows: &microflows,
    };
    // `MXRS_EXPLAIN_FLOWS=1` says why each flow that stays in the imported
    // model does.
    let explain = std::env::var_os("MXRS_EXPLAIN_FLOWS").is_some();
    let mut raw = Vec::new();
    let mut seen = HashSet::new();
    let mut ambiguous = HashSet::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        for flow in module.microflows.iter().chain(&module.nanoflows) {
            let Some(id) = flow.id.as_deref() else {
                continue;
            };
            let Some(unit) = project
                .mpr()
                .unit(id)
                .map_err(mxrs_model::ModelError::from)?
            else {
                continue;
            };
            let doc = project
                .mpr()
                .parse_contents(&unit)
                .map_err(mxrs_model::ModelError::from)?;
            let key = (
                format!("{module_name}.{}", doc.get_str("Name").unwrap_or_default()),
                doc.get_str("$Type").unwrap_or_default().to_string(),
            );
            if !seen.insert(key.clone()) {
                ambiguous.insert(key);
            }
            // A signature the typed builders cannot state is not one a typed
            // call can be checked against; the flow itself may still be
            // declared in full.
            if (parameters_attested(&doc).is_none()
                || signature(&Microflow::from_bson(&doc)).is_none())
                && let Some(name) = &flow.name
            {
                targets.remove(&format!("{module_name}.{name}"));
            }
            if parameters_attested(&doc).is_some() {
                raw.push((module_name, doc));
            } else {
                keep(
                    &mut kept,
                    explain,
                    module_name,
                    &doc,
                    "its parameters are stored in a form nothing here reads".to_string(),
                );
            }
        }
    }
    for (name, kind) in &ambiguous {
        if kind == "Microflows$Microflow" {
            targets.remove(name);
        }
    }
    let mut result = Vec::new();
    for (module, doc) in &raw {
        if ambiguous.contains(&(
            format!("{module}.{}", doc.get_str("Name").unwrap_or_default()),
            doc.get_str("$Type").unwrap_or_default().to_string(),
        )) {
            keep(
                &mut kept,
                explain,
                module,
                doc,
                "its module has two flows of that name".to_string(),
            );
            continue;
        }
        let converted = {
            // The typed conversion first: it checks more. Whatever it cannot
            // express is stated in full instead.
            convert(module, doc, &targets, &entities, &attributes).or_else(|| {
                match crate::flow_general::convert(module, doc, &model) {
                    Ok((declaration, source)) => Some(ConvertedFlow {
                        module: (*module).to_string(),
                        native_type: doc.get_str("$Type").ok()?.to_string(),
                        declaration,
                        source: crate::flow_general::polish(source),
                    }),
                    Err(reason) => {
                        keep(&mut kept, explain, module, doc, reason);
                        None
                    }
                }
            })
        };
        result.extend(converted);
    }
    result.sort_by(|a, b| (&a.module, &a.declaration.name).cmp(&(&b.module, &b.declaration.name)));
    Ok((result, kept))
}

/// Records why a flow stays in the imported model; `MXRS_EXPLAIN_FLOWS=1`
/// also says so while importing.
fn keep(kept: &mut KeptFlows, explain: bool, module: &str, doc: &Document, reason: String) {
    let name = doc.get_str("Name").unwrap_or_default();
    if explain {
        eprintln!("[mxrs] {module}.{name} stays imported: {reason}");
    }
    kept.insert(
        (
            module.to_string(),
            name.to_string(),
            doc.get_str("$Type").ok() == Some("Microflows$Nanoflow"),
        ),
        reason,
    );
}

fn parameters_attested(doc: &Document) -> Option<()> {
    let objects = documents(doc.get_document("ObjectCollection").ok()?.get("Objects")?)?;
    let legacy: Vec<_> = objects
        .into_iter()
        .filter(|o| o.get_str("$Type").ok() == Some("Microflows$MicroflowParameter"))
        .collect();
    let canonical = doc
        .get("MicroflowParameterCollection")
        .or_else(|| doc.get("Parameters"));
    if doc.contains_key("MicroflowParameterCollection") && doc.contains_key("Parameters") {
        return None;
    }
    if let Some(value) = canonical {
        let parameters = documents(value.as_document()?.get("Parameters")?)?;
        if !legacy.is_empty()
            || parameters
                .iter()
                .any(|p| p.get_str("$Type").ok() != Some("Microflows$MicroflowParameter"))
        {
            return None;
        }
    }
    Some(())
}

fn convert(
    module: &str,
    doc: &Document,
    targets: &HashMap<String, &Microflow>,
    entities: &HashSet<String>,
    attributes: &Attributes,
) -> Option<ConvertedFlow> {
    let nodes = structured_nodes(doc)?;
    let model = Microflow::from_bson(doc);
    let mut declaration = MicroflowDecl::new(model.name.as_ref()?);
    declaration.documentation = model.documentation.clone();
    let mut source = Vec::new();
    let mut variables = HashMap::new();
    for ((name, ty), native) in signature(&model)?.into_iter().zip(&model.parameters) {
        let rust_name = binding(&name)?;
        let mut parameter = FlowParameterDecl::new(&name, ty.clone());
        parameter.documentation = native.get_str("Documentation").unwrap_or_default().into();
        parameter.required = native.get_bool("IsRequired").unwrap_or(false);
        let mut options = Vec::new();
        if !parameter.documentation.is_empty() {
            options.push(format!(
                "parameter.documentation({});",
                rust_string(&parameter.documentation)
            ));
        }
        if parameter.required {
            options.push("parameter.required(true);".into());
        }
        if let Ok(default) = native.get_str("DefaultValue")
            && !default.is_empty()
        {
            let rendered = expression(default, &ty, &HashMap::new(), attributes)?;
            parameter.default_value = Some(default.into());
            options.push(format!("parameter.default_value({rendered});"));
        }
        let options = if options.is_empty() {
            "|_| {}".into()
        } else {
            format!("|parameter| {{ {} }}", options.join(" "))
        };
        let call = match &ty {
            Ty::Object(entity) | Ty::List(entity) => {
                tag(&ty, entities)?;
                format!(
                    "flow.{}_parameter({}, Ref::<{}>::new(), {options})",
                    if matches!(&ty, Ty::Object(_)) {
                        "object"
                    } else {
                        "list"
                    },
                    rust_string(&name),
                    marker(entity)?
                )
            }
            _ => format!(
                "flow.parameter::<{}>({}, {options})",
                tag(&ty, entities)?,
                rust_string(&name)
            ),
        };
        source.push(format!("let {rust_name} = {call};"));
        variables.insert(name, ty);
        declaration.parameters.push(parameter);
    }
    let converter = Converter {
        targets,
        entities,
        attributes,
        nanoflow: doc.get_str("$Type").ok() == Some("Microflows$Nanoflow"),
    };
    declaration.activities = converter.block(
        &nodes[1..nodes.len().checked_sub(1)?],
        &mut variables,
        &mut source,
        false,
    )?;
    let Node::Simple(end) = nodes.last()? else {
        return None;
    };
    let returned = end.get_str("ReturnValue").ok()?;
    let native_return = model.return_type_document.as_ref()?;
    if native_return.get_str("$Type").ok()? == "DataTypes$VoidType" {
        if !returned.is_empty() {
            return None;
        }
    } else {
        let ty = data_type(native_return)?;
        tag(&ty, entities)?;
        source.push(format!(
            "flow.return_value({});",
            expression(returned, &ty, &variables, attributes)?
        ));
        declaration.return_type = Some(ty);
        declaration.return_expression = Some(returned.into());
    }
    if !mxrs_writer::flow_graph::preserves_body(doc, &declaration) {
        return None;
    }
    let source = polish_source(source);
    // The flow's file declares a unit type under the flow's own name, and a
    // `let` cannot bind a name a unit type already has. A flow named in
    // lower case whose body binds that very name stays in the imported
    // model instead of becoming source that does not compile.
    let marker = names::flow_marker(&declaration.name);
    if source
        .iter()
        .any(|line| binding_name(line).as_deref() == Some(marker.as_str()))
    {
        return None;
    }
    Some(ConvertedFlow {
        module: module.into(),
        native_type: doc.get_str("$Type").ok()?.into(),
        declaration,
        source,
    })
}

/// Removes the generator's used-suppression scars from a reconstructed
/// body. Every `let _ = &name;` line goes; a binding or loop parameter that
/// is genuinely never read again gets Rust's own `_` prefix instead — the
/// idiom a hand author would have written.
fn polish_source(source: Vec<String>) -> Vec<String> {
    let mut lines: Vec<String> = source
        .into_iter()
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.starts_with("let _ = &") && trimmed.ends_with(';'))
        })
        .collect();
    for index in 0..lines.len() {
        let Some(name) = binding_name(&lines[index]) else {
            continue;
        };
        let used_later = lines[index + 1..]
            .iter()
            .any(|line| find_identifier(line, &name).is_some());
        if !used_later {
            let position =
                find_identifier(&lines[index], &name).expect("binding_name came from this line");
            lines[index].insert(position, '_');
        }
    }
    lines
}

/// The identifier a line binds: `let name = ...` or a `|flow, name|` loop
/// closure parameter. `None` for anything else, including bindings already
/// underscore-prefixed.
pub(crate) fn binding_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let closure_parameter = ["|flow, ", "|_flow, "]
        .iter()
        .find_map(|prefix| trimmed.find(prefix).map(|start| start + prefix.len()));
    let candidate = if let Some(rest) = trimmed.strip_prefix("let ") {
        rest.split_once(" = ").map(|(name, _)| name.trim())?
    } else if let Some(start) = closure_parameter {
        trimmed[start..]
            .split_once('|')
            .map(|(name, _)| name.trim())?
    } else {
        return None;
    };
    let valid = !candidate.is_empty()
        && !candidate.starts_with('_')
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then(|| candidate.to_string())
}

/// The byte position of `name` used as a variable: a whole word, outside
/// string literals and model references, and neither a path segment
/// (`Order::name()`) nor a method (`.name(...)`) — so a variable `name` is
/// not kept alive by an attribute or a parameter that happens to share it.
pub(crate) fn find_identifier(line: &str, name: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                index += 1;
            }
            1 => {
                index += 1;
                while index < bytes.len() && bytes[index] != 2 {
                    index += 1;
                }
                index += 1;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                let qualified = line[..start].trim_end().ends_with([':', '.']);
                if &line[start..index] == name && !qualified {
                    return Some(start);
                }
            }
            _ => index += 1,
        }
    }
    None
}

struct Converter<'a> {
    targets: &'a HashMap<String, &'a Microflow>,
    entities: &'a HashSet<String>,
    attributes: &'a Attributes,
    /// A nanoflow's activities abort where a microflow's roll back.
    nanoflow: bool,
}

impl Converter<'_> {
    fn default_handling(&self) -> &'static str {
        if self.nanoflow { "Abort" } else { "Rollback" }
    }

    fn block(
        &self,
        nodes: &[Node<'_>],
        variables: &mut HashMap<String, Ty>,
        source: &mut Vec<String>,
        in_loop: bool,
    ) -> Option<Vec<Activity>> {
        nodes
            .iter()
            .map(|node| self.node(node, variables, source, in_loop))
            .collect()
    }

    fn node(
        &self,
        node: &Node<'_>,
        variables: &mut HashMap<String, Ty>,
        source: &mut Vec<String>,
        in_loop: bool,
    ) -> Option<Activity> {
        match node {
            Node::Simple(doc) => match doc.get_str("$Type").ok()? {
                "Microflows$ActionActivity" => self.action(doc, variables, source),
                "Microflows$BreakEvent" | "Microflows$ContinueEvent" if in_loop => {
                    let is_break = doc.get_str("$Type").ok()? == "Microflows$BreakEvent";
                    source.push(format!(
                        "flow.{}();",
                        if is_break {
                            "break_loop"
                        } else {
                            "continue_loop"
                        }
                    ));
                    Some(if is_break {
                        Activity::BreakLoop
                    } else {
                        Activity::ContinueLoop
                    })
                }
                _ => None,
            },
            Node::Decision { split, yes, no, .. } => {
                if split.get_str("ErrorHandlingType").ok()? != self.default_handling() {
                    return None;
                }
                let condition_doc = split.get_document("SplitCondition").ok()?;
                if condition_doc.get_str("$Type").ok()? != "Microflows$ExpressionSplitCondition" {
                    return None;
                }
                let condition = condition_doc.get_str("Expression").ok()?;
                let rendered = expression(condition, &Ty::Boolean, variables, self.attributes)?;
                let mut yes_source = Vec::new();
                let mut no_source = Vec::new();
                let true_branch =
                    self.block(yes, &mut variables.clone(), &mut yes_source, in_loop)?;
                let false_branch =
                    self.block(no, &mut variables.clone(), &mut no_source, in_loop)?;
                let activity = Activity::Decision {
                    condition: condition.into(),
                    true_branch,
                    false_branch,
                };
                attest_control(split, &activity, "SplitCondition", self.nanoflow)?;
                source.push(format!(
                    "flow.decision({rendered}, {}",
                    closure_start(&yes_source)
                ));
                source.extend(yes_source.into_iter().map(|line| format!("    {line}")));
                source.push(format!("}}, {}", closure_start(&no_source)));
                source.extend(no_source.into_iter().map(|line| format!("    {line}")));
                source.push("});".into());
                Some(activity)
            }
            // The typed builders have no error handlers and no switches; a
            // flow with either is declared in full instead.
            Node::Switch { .. } | Node::Handled { .. } | Node::Label(_) | Node::Jump(_) => None,
            Node::Loop { node, body } => {
                if node.get_str("ErrorHandlingType").ok()? != self.default_handling() {
                    return None;
                }
                let native = node.get_document("LoopSource").ok()?;
                let mut inner_variables = variables.clone();
                let mut inner_source = Vec::new();
                let (opening, mut activity) = match native.get_str("$Type").ok()? {
                    "Microflows$IterableList" => {
                        let list = native.get_str("ListVariableName").ok()?;
                        let iterator = native.get_str("VariableName").ok()?;
                        let Ty::List(entity) = variables.get(list)? else {
                            return None;
                        };
                        let variable = binding(iterator)?;
                        if inner_variables
                            .insert(iterator.into(), Ty::Object(entity.clone()))
                            .is_some()
                        {
                            return None;
                        }
                        (
                            format!(
                                "flow.loop_over(&{}, {}, |flow, {variable}| {{",
                                binding(list)?,
                                rust_string(iterator)
                            ),
                            Activity::LoopOver {
                                list_variable: list.into(),
                                iterator: iterator.into(),
                                activities: Vec::new(),
                            },
                        )
                    }
                    "Microflows$WhileLoopCondition" => {
                        let condition = native.get_str("WhileExpression").ok()?;
                        (
                            format!(
                                "flow.while_loop({}, |flow| {{",
                                expression(condition, &Ty::Boolean, variables, self.attributes)?
                            ),
                            Activity::WhileLoop {
                                condition: condition.into(),
                                activities: Vec::new(),
                            },
                        )
                    }
                    _ => return None,
                };
                let activities = self.block(body, &mut inner_variables, &mut inner_source, true)?;
                match &mut activity {
                    Activity::LoopOver {
                        activities: target, ..
                    }
                    | Activity::WhileLoop {
                        activities: target, ..
                    } => *target = activities,
                    _ => unreachable!(),
                }
                attest_control(node, &activity, "LoopSource", self.nanoflow)?;
                source.push(if body.is_empty() {
                    opening.replace("|flow", "|_flow")
                } else {
                    opening
                });
                source.extend(inner_source.into_iter().map(|line| format!("    {line}")));
                source.push("});".into());
                Some(activity)
            }
        }
    }

    fn action(
        &self,
        node: &Document,
        variables: &mut HashMap<String, Ty>,
        source: &mut Vec<String>,
    ) -> Option<Activity> {
        let Self {
            targets,
            entities,
            attributes,
            ..
        } = self;
        let action = node.get_document("Action").ok()?;
        let activity = match action.get_str("$Type").ok()? {
            kind @ ("Microflows$CreateChangeAction" | "Microflows$ChangeAction") => {
                let create = kind == "Microflows$CreateChangeAction";
                let name = action
                    .get_str(if create {
                        "VariableName"
                    } else {
                        "ChangeVariableName"
                    })
                    .ok()?;
                let variable = binding(name)?;
                let entity = if create {
                    action.get_str("Entity").ok()?.to_string()
                } else {
                    let Ty::Object(entity) = variables.get(name)? else {
                        return None;
                    };
                    entity.clone()
                };
                tag(&Ty::Object(entity.clone()), entities)?;
                let commit = match action.get_str("Commit").ok()? {
                    "Yes" => true,
                    "No" => false,
                    _ => return None,
                };
                let mut members = Vec::new();
                let mut rendered = Vec::new();
                let mut seen = HashSet::new();
                for item in documents(action.get("Items")?)? {
                    if !item.get_str("Association").ok()?.is_empty() {
                        return None;
                    }
                    let qualified = item.get_str("Attribute").ok()?;
                    let member = qualified.strip_prefix(&format!("{entity}."))?;
                    if !seen.insert(member) {
                        return None;
                    }
                    let attribute = attributes.get(&(entity.clone(), member.into()))?;
                    if !attribute.writable {
                        return None;
                    }
                    let value = item.get_str("Value").ok()?;
                    rendered.push(format!(
                        "{}.set({})",
                        attribute.marker,
                        expression(value, &attribute.value_type, variables, attributes)?
                    ));
                    members.push(Member::attribute(member, value));
                }
                let members_source = format!("vec![{}]", rendered.join(", "));
                if create {
                    if variables
                        .insert(name.into(), Ty::Object(entity.clone()))
                        .is_some()
                    {
                        return None;
                    }
                    source.push(format!(
                        "let {variable} = flow.create_object({}, Ref::<{}>::new(), {members_source}, {commit});",
                        rust_string(name),
                        marker(&entity)?
                    ));
                    Activity::CreateObject {
                        variable: name.into(),
                        entity,
                        members,
                        commit,
                    }
                } else {
                    source.push(format!(
                        "flow.change_object(&{variable}, {members_source}, {commit});"
                    ));
                    Activity::ChangeObject {
                        variable: name.into(),
                        entity,
                        members,
                        commit,
                    }
                }
            }
            "Microflows$MicroflowCallAction" => {
                let call = action.get_document("MicroflowCall").ok()?;
                let target = call.get_str("Microflow").ok()?;
                if entities.contains(target) {
                    return None;
                }
                let target_marker = flow_marker(target)?;
                let callee = *targets.get(target)?;
                let expected: HashMap<_, _> = signature(callee)?.into_iter().collect();
                let mut mappings = Vec::new();
                let mut rendered = Vec::new();
                let mut seen = HashSet::new();
                for mapping in documents(call.get("ParameterMappings")?)? {
                    let parameter = mapping.get_str("Parameter").ok()?;
                    let name = parameter.strip_prefix(&format!("{target}."))?;
                    let ty = expected.get(name)?;
                    if !seen.insert(name) {
                        return None;
                    }
                    let value = mapping.get_str("Argument").ok()?;
                    rendered.push(format!(
                        "CallArgument::new({}, {})",
                        rust_string(name),
                        expression(value, ty, variables, attributes)?
                    ));
                    mappings.push(MicroflowCallMapping {
                        parameter: parameter.into(),
                        value: value.into(),
                        value_type: Some(ty.clone()),
                    });
                }
                if seen.len() != expected.len() {
                    return None;
                }
                let mappings_source = format!("vec![{}]", rendered.join(", "));
                let use_return = action.get_bool("UseReturnVariable").ok()?;
                let (result_variable, result_type) = if use_return {
                    let name = action.get_str("ResultVariableName").ok()?;
                    let variable = binding(name)?;
                    let ty = data_type(callee.return_type_document.as_ref()?)?;
                    let result_tag = tag(&ty, entities)?;
                    if variables.insert(name.into(), ty.clone()).is_some() {
                        return None;
                    }
                    source.push(format!(
                        "let {variable} = flow.call_microflow_result::<{result_tag}>(MicroflowRef::<{target_marker}>::new(), {}, {mappings_source});",
                        rust_string(name)
                    ));
                    (Some(name.into()), Some(ty))
                } else {
                    if !action.get_str("ResultVariableName").ok()?.is_empty() {
                        return None;
                    }
                    source.push(format!(
                        "flow.call_microflow(MicroflowRef::<{target_marker}>::new(), None, false, {mappings_source});"
                    ));
                    (None, None)
                };
                Activity::CallMicroflow {
                    name: target.into(),
                    result_variable,
                    result_type,
                    use_return,
                    mappings,
                }
            }
            "Microflows$CreateListAction" => {
                let name = action.get_str("VariableName").ok()?;
                let entity = action.get_str("Entity").ok()?;
                tag(&Ty::List(entity.into()), entities)?;
                let variable = binding(name)?;
                if variables
                    .insert(name.into(), Ty::List(entity.into()))
                    .is_some()
                {
                    return None;
                }
                source.push(format!(
                    "let {variable} = flow.create_list({}, Ref::<{}>::new());",
                    rust_string(name),
                    marker(entity)?
                ));
                Activity::CreateList {
                    variable: name.into(),
                    entity: entity.into(),
                }
            }
            kind @ ("Microflows$CommitAction" | "Microflows$DeleteAction") => {
                let commit = kind == "Microflows$CommitAction";
                let name = action
                    .get_str(if commit {
                        "CommitVariableName"
                    } else {
                        "DeleteVariableName"
                    })
                    .ok()?;
                if !matches!(variables.get(name), Some(Ty::Object(_))) {
                    return None;
                }
                source.push(format!(
                    "flow.{}(&{});",
                    if commit { "commit" } else { "delete_object" },
                    binding(name)?
                ));
                if commit {
                    Activity::Commit {
                        variable: name.into(),
                    }
                } else {
                    Activity::DeleteObject {
                        variable: name.into(),
                    }
                }
            }
            _ => return None,
        };
        // Unknown action fields/options must never be silently called editable.
        let (fresh, _) = mxrs_writer::flow_compiler::build_flow_graph(
            std::slice::from_ref(&activity),
            &[],
            None,
            self.nanoflow,
        );
        if !same_semantics(action, fresh.get(1)?.get_document("Action").ok()?) {
            return None;
        }
        Some(activity)
    }
}

fn closure_start(source: &[String]) -> &'static str {
    if source.is_empty() {
        "|_flow| {"
    } else {
        "|flow| {"
    }
}

fn attest_control(node: &Document, activity: &Activity, field: &str, nanoflow: bool) -> Option<()> {
    let (fresh, _) = mxrs_writer::flow_compiler::build_flow_graph(
        std::slice::from_ref(activity),
        &[],
        None,
        nanoflow,
    );
    same_semantics(
        node.get_document(field).ok()?,
        fresh.get(1)?.get_document(field).ok()?,
    )
    .then_some(())
}

fn same_semantics(old: &Document, fresh: &Document) -> bool {
    let old_keys: HashSet<_> = old.keys().filter(|k| k.as_str() != "$ID").collect();
    let new_keys: HashSet<_> = fresh.keys().filter(|k| k.as_str() != "$ID").collect();
    old_keys == new_keys
        && old_keys
            .into_iter()
            .all(|key| match (&old[key], &fresh[key]) {
                (Bson::Document(old), Bson::Document(new)) => same_semantics(old, new),
                (Bson::Array(_), Bson::Array(_)) => {
                    match (documents(&old[key]), documents(&fresh[key])) {
                        (Some(old), Some(new)) => {
                            old.len() == new.len()
                                && old.iter().zip(new).all(|(a, b)| same_semantics(a, b))
                        }
                        _ => false,
                    }
                }
                (old, new) => old == new,
            })
}

/// How one converted flow is written: the file it lives in and the function
/// that declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlowFile {
    pub(crate) file_stem: String,
    /// The declaring function's identifier.
    function: String,
    /// The naming-convention prefix the flow states (`ACT`, `SUB`, `DS`, ...).
    prefix: Option<String>,
    /// Whether the Mendix name has to be stated because the prefix and the
    /// function do not spell it.
    explicit_name: bool,
}

/// `ACT_CreateOrder` → (`ACT`, `CreateOrder`): the capitals before the first
/// underscore, when the name follows that convention.
pub(crate) fn split_prefix(name: &str) -> (Option<&str>, &str) {
    match name.split_once('_') {
        Some((prefix, rest))
            if (2..=5).contains(&prefix.len())
                && !rest.is_empty()
                && prefix.starts_with(|c: char| c.is_ascii_uppercase())
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) =>
        {
            (Some(prefix), rest)
        }
        _ => (None, name),
    }
}

/// The function declaring a flow, from the part of its name `base` covers.
/// Never one of the prelude's expression functions — a flow body calls
/// those — and never the flow's own naming type.
fn flow_function(base: &str, marker: &str) -> String {
    let mut function = inner_file_stem(base);
    if function.is_empty() {
        function = "flow".to_string();
    }
    if matches!(
        function.as_str(),
        "string"
            | "integer"
            | "long"
            | "float"
            | "decimal"
            | "boolean"
            | "attribute"
            | "association"
    ) {
        function.push('_');
    }
    if function == marker {
        function.push_str("_flow");
    }
    function
}

/// Decides every converted flow's file and function, index for index.
///
/// A flow is named by what it does: `ACT_CreateOrder` is `create_order` in
/// `create_order_service.rs`, with `ACT` stated on the attribute. Two flows
/// of one module that would share that name keep their whole Mendix name
/// instead.
/// The file a flow that stays in the imported model is named in: the one
/// its declaration would have, so declaring it later replaces the file's
/// contents and nothing else moves.
pub(crate) fn kept_file_stem(name: &str, nanoflow: bool) -> String {
    let function = flow_function(split_prefix(name).1, &names::flow_marker(name));
    file_stem(&function, nanoflow)
}

/// The file a flow's declaring function lives in.
fn file_stem(function: &str, nanoflow: bool) -> String {
    let stem = function.trim_end_matches('_');
    // A nanoflow's file is named after its function alone, so it must not
    // be a word Rust keeps (`return.rs`) nor the folder's own index.
    if !nanoflow {
        format!("{stem}_service")
    } else if stem.is_empty() || rust_keyword(stem) || stem == "mod" {
        format!("{stem}_nanoflow")
    } else {
        stem.to_string()
    }
}

pub(crate) fn plan_files(flows: &[ConvertedFlow]) -> Vec<FlowFile> {
    let short = |flow: &ConvertedFlow| {
        let name = &flow.declaration.name;
        flow_function(split_prefix(name).1, &names::flow_marker(name))
    };
    let mut uses: HashMap<(&str, bool, String), usize> = HashMap::new();
    for flow in flows {
        *uses
            .entry((flow.module.as_str(), flow.is_nanoflow(), short(flow)))
            .or_default() += 1;
    }
    let mut taken: HashSet<(&str, bool, String)> = HashSet::new();
    let mut files: HashSet<(&str, bool, String)> = HashSet::new();
    flows
        .iter()
        .map(|flow| {
            let name = &flow.declaration.name;
            let nanoflow = flow.is_nanoflow();
            let marker = names::flow_marker(name);
            let (prefix, _) = split_prefix(name);
            let mut function = short(flow);
            if uses[&(flow.module.as_str(), nanoflow, function.clone())] > 1 {
                function = flow_function(name, &marker);
            }
            if !taken.insert((flow.module.as_str(), nanoflow, function.clone())) {
                function = (2..)
                    .map(|suffix| format!("{function}_{suffix}"))
                    .find(|candidate| {
                        taken.insert((flow.module.as_str(), nanoflow, candidate.clone()))
                    })
                    .expect("an unbounded suffix always finds a free name");
            }
            let derived = match prefix {
                Some(prefix) => format!("{prefix}_{}", derive_pascal_case(&function)),
                None => derive_pascal_case(&function),
            };
            let base = file_stem(&function, nanoflow);
            // Distinct functions can still ask for one file (`return` and
            // `return_nanoflow`): the later one takes a numbered name.
            let mut file_stem = base.clone();
            let mut suffix = 2;
            while !files.insert((flow.module.as_str(), nanoflow, file_stem.clone())) {
                file_stem = format!("{base}_{suffix}");
                suffix += 1;
            }
            FlowFile {
                file_stem,
                explicit_name: derived != *name,
                function,
                prefix: prefix.map(str::to_string),
            }
        })
        .collect()
}

/// Renders each recovered flow as the function that declares it, in its own
/// file. `plans` is [`plan_files`]' answer for the same `flows`.
pub(crate) fn render_files(
    flows: &[ConvertedFlow],
    plans: &[FlowFile],
    nanoflow: bool,
    names: &ModelNames<'_>,
    relations: &HashMap<String, mxrs_model::relations::FlowRelations>,
) -> Result<Vec<RenderedFlowSource>> {
    let attribute = if nanoflow { "nanoflow" } else { "microflow" };
    flows
        .iter()
        .zip(plans)
        .filter(|(flow, _)| flow.is_nanoflow() == nanoflow)
        .map(|(flow, plan)| {
            let name = &flow.declaration.name;
            let mut lines = flow.source.clone();
            let mut item = String::new();
            match crate::entity_export::doc_comment(&flow.declaration.documentation, "") {
                Some(comment) => item.push_str(&comment),
                None => lines.insert(
                    0,
                    format!(
                        "flow.documentation({});",
                        rust_string(&flow.declaration.documentation)
                    ),
                ),
            }
            let mut arguments = Vec::new();
            if let Some(prefix) = &plan.prefix {
                arguments.push(prefix.clone());
            }
            arguments.push(format!("module = {}", rust_string(&flow.module)));
            if plan.explicit_name {
                arguments.push(format!("name = {}", rust_string(name)));
            }
            // What the model says the flow is related to, each named by
            // the Rust item that declares it where there is one.
            if let Some(related) = relations.get(&format!("{}.{name}", flow.module)) {
                let list = |name: &str, items: Vec<String>| {
                    (!items.is_empty()).then(|| format!("{name}({})", items.join(", ")))
                };
                let flow_item = |target: &String| {
                    if names.microflows.contains_key(target) {
                        names::microflow(target)
                    } else if names.nanoflows.contains_key(target) {
                        names::nanoflow(target)
                    } else {
                        rust_string(target)
                    }
                };
                arguments.extend(list(
                    "roles",
                    related
                        .roles
                        .iter()
                        .map(|role| {
                            if names.roles.contains_key(role) {
                                names::role(role)
                            } else {
                                rust_string(role)
                            }
                        })
                        .collect(),
                ));
                arguments.extend(list("calls", related.calls.iter().map(flow_item).collect()));
                arguments.extend(list(
                    "uses",
                    related
                        .uses
                        .iter()
                        .map(|entity| {
                            if names.entities.contains_key(entity) {
                                names::entity(entity)
                            } else {
                                rust_string(entity)
                            }
                        })
                        .collect(),
                ));
                arguments.extend(list(
                    "used_by",
                    related.used_by.iter().map(flow_item).collect(),
                ));
            }
            writeln!(item, "#[{attribute}({})]", arguments.join(", ")).unwrap();
            // A variable whose Mendix name does not read back from snake
            // case keeps its exact spelling, capitals included.
            if lines.iter().any(|line| {
                line.match_indices("value_").any(|(index, _)| {
                    line[index + "value_".len()..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .any(|c| c.is_ascii_uppercase())
                })
            }) {
                item.push_str("#[allow(non_snake_case)]\n");
            }
            let parameter = if lines.is_empty() { "_flow" } else { "flow" };
            writeln!(
                item,
                "pub fn {}({parameter}: &mut FlowBuilder) {{",
                plan.function
            )
            .unwrap();
            for line in &lines {
                writeln!(item, "    {line}").unwrap();
            }
            item.push_str("}\n");
            let marker = names::flow_marker(name);
            let resolved = names.resolve(&item, &[&marker])?;
            let mut source = String::from("use mxrs::prelude::*;\n\n");
            for import in &resolved.imports {
                writeln!(source, "{import}").unwrap();
            }
            if !resolved.imports.is_empty() {
                source.push('\n');
            }
            source.push_str(&resolved.source);
            Ok(RenderedFlowSource {
                module: flow.module.clone(),
                file_name: plan.file_stem.clone(),
                source,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variable_is_bound_the_way_a_rust_author_would_bind_it() {
        assert_eq!(binding("NewOrder").as_deref(), Some("new_order"));
        assert_eq!(binding("Name").as_deref(), Some("name"));
        // Names that do not read back from snake case keep their spelling.
        assert_eq!(binding("NewJSON").as_deref(), Some("value_NewJSON"));
        assert_eq!(binding("order").as_deref(), Some("value_order"));
        assert_eq!(binding("Order_Line").as_deref(), Some("value_Order_Line"));
        // Names the body already uses for something else.
        assert_eq!(binding("Flow").as_deref(), Some("value_Flow"));
        assert_eq!(binding("String").as_deref(), Some("value_String"));
        assert_eq!(binding("Type").as_deref(), Some("value_Type"));
        // The receivers of switch and rule closures.
        assert_eq!(binding("On").as_deref(), Some("value_On"));
        assert_eq!(binding("Rule").as_deref(), Some("value_Rule"));
        assert_eq!(binding("ValueTotal").as_deref(), Some("value_ValueTotal"));
        assert_eq!(binding("not valid"), None);
    }

    #[test]
    fn a_variable_use_is_not_a_path_a_method_a_string_or_a_reference() {
        assert_eq!(find_identifier("let name = x;", "name"), Some(4));
        assert_eq!(
            find_identifier("flow.return_value(name.clone());", "name"),
            Some(18)
        );
        assert_eq!(find_identifier("Order::name().set(1)", "name"), None);
        assert_eq!(find_identifier("order.name(1)", "name"), None);
        assert_eq!(
            find_identifier("f(\"name\", \"a \\\" name\")", "name"),
            None
        );
        assert_eq!(
            find_identifier(&names::attribute("Sales.Order", "name"), "name"),
            None
        );
        assert_eq!(find_identifier("rename(names)", "name"), None);
    }

    #[test]
    fn a_flow_is_named_by_what_it_does() {
        assert_eq!(
            split_prefix("ACT_CreateOrder"),
            (Some("ACT"), "CreateOrder")
        );
        assert_eq!(
            split_prefix("DS_Login_Create"),
            (Some("DS"), "Login_Create")
        );
        assert_eq!(split_prefix("PetApi"), (None, "PetApi"));
        assert_eq!(split_prefix("Act_Create"), (None, "Act_Create"));
        assert_eq!(split_prefix("A_Create"), (None, "A_Create"));
        assert_eq!(
            flow_function("CreateOrder", "ACT_CreateOrder"),
            "create_order"
        );
        assert_eq!(flow_function("String", "ACT_String"), "string_");
        assert_eq!(flow_function("cleanup", "cleanup"), "cleanup_flow");
    }

    fn flow(module: &str, name: &str, nanoflow: bool) -> ConvertedFlow {
        ConvertedFlow {
            module: module.into(),
            native_type: if nanoflow {
                "Microflows$Nanoflow"
            } else {
                "Microflows$Microflow"
            }
            .into(),
            declaration: MicroflowDecl::new(name),
            source: Vec::new(),
        }
    }

    #[test]
    fn every_flow_gets_a_file_rust_can_name_and_no_other_flow_has() {
        let flows = [
            flow("Sales", "ACT_Return", true),
            flow("Sales", "ACT_Type", true),
            flow("Sales", "Mod", true),
            flow("Sales", "ModNanoflow", true),
            flow("Sales", "ACT_Refresh", true),
            flow("Sales", "ACT_Return", false),
            flow("Crm", "ACT_Refresh", true),
        ];
        let stems = plan_files(&flows)
            .into_iter()
            .map(|plan| plan.file_stem)
            .collect::<Vec<_>>();
        assert_eq!(
            stems,
            [
                // `return.rs` and `type.rs` are not modules Rust can declare.
                "return_nanoflow",
                "type_nanoflow",
                // `mod.rs` is the folder's own; the flow that is really
                // called `ModNanoflow` then finds its name taken.
                "mod_nanoflow",
                "mod_nanoflow_2",
                "refresh",
                "return_service",
                // Another module's folder is another namespace.
                "refresh",
            ]
        );
    }

    #[test]
    fn a_body_that_binds_the_flows_own_type_name_stays_imported() {
        // `pub struct cleanup;` and `let cleanup = ...` cannot share a file.
        assert_eq!(names::flow_marker("cleanup"), "cleanup");
        assert_eq!(binding("Cleanup").as_deref(), Some("cleanup"));
        assert_eq!(
            binding_name("let cleanup = flow.create_list();").as_deref(),
            Some("cleanup")
        );
    }
}
