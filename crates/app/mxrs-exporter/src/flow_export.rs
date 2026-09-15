//! Reconstructs attested linear bodies as typed builder calls. Unsupported
//! expressions, action options and graph shapes stay in the imported model.
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use mxrs_bson::{Bson, Document};
use mxrs_ir::Member;
use mxrs_ir::flow::FlowReturnType as Ty;
use mxrs_ir::{Activity, FlowParameterDecl, MicroflowCallMapping, MicroflowDecl};
use mxrs_model::attribute::AttributeType;
use mxrs_model::{Microflow, Module, Project};
use mxrs_writer::flow_graph::{documents, linear_nodes};

use crate::{Result, rust_string};

pub(crate) struct ConvertedFlow {
    pub module: String,
    pub native_type: String,
    pub declaration: MicroflowDecl,
    source: Vec<String>,
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
                        marker: format!("model::{module_name}::{entity_name}_{name}"),
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

pub(crate) fn typed_attribute_markers(modules: &[Module]) -> String {
    let mut entries: Vec<_> = attributes(modules).into_values().collect();
    entries.sort_by(|a, b| a.marker.cmp(&b.marker));
    let mut source = String::new();
    for entry in entries {
        let marker = entry.marker.strip_prefix("model::").expect("marker prefix");
        writeln!(
            source,
            "impl mxrs::TypedAttributeMarker for {marker} {{ type Value = {}; }}",
            tag(&entry.value_type, &HashSet::new()).expect("scalar tag")
        )
        .unwrap();
    }
    source
}

fn data_type(doc: &Document) -> Option<Ty> {
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

fn marker(name: &str) -> Option<String> {
    let (module, entity) = name.split_once('.')?;
    if !mxrs_typegen::is_rust_identifier(module) || !mxrs_typegen::is_rust_identifier(entity) {
        return None;
    }
    Some(format!("model::{module}::{entity}"))
}

fn tag(ty: &Ty, entities: &HashSet<String>) -> Option<String> {
    Some(match ty {
        Ty::String => "mxrs::MxString".into(),
        Ty::Integer => "mxrs::MxInteger".into(),
        Ty::Long => "mxrs::MxLong".into(),
        Ty::Boolean => "mxrs::MxBool".into(),
        Ty::Float => "mxrs::MxFloat".into(),
        Ty::Decimal => "mxrs::MxDecimal".into(),
        Ty::DateTime => "mxrs::MxDateTime".into(),
        Ty::Binary => "mxrs::MxBinary".into(),
        Ty::Object(entity) | Ty::List(entity) => {
            if !entities.contains(entity) {
                return None;
            }
            format!(
                "mxrs::{}<{}>",
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

fn binding(name: &str) -> Option<String> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(format!("value_{name}"))
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
            let rendered = format!("{}.attribute::<{}>()", binding(object)?, attribute.marker);
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
        return Some(format!("{}.clone()", binding(name)?));
    }
    match ty {
        Ty::String => {
            let inner = value.strip_prefix('\'')?.strip_suffix('\'')?;
            let text = inner.replace("''", "'");
            if format!("'{}'", text.replace('\'', "''")) != value {
                return None;
            }
            Some(format!("mxrs::string({})", rust_string(&text)))
        }
        Ty::Boolean if matches!(value, "true" | "false") => Some(format!("mxrs::boolean({value})")),
        Ty::Long => {
            let parsed = value.parse::<i64>().ok()?;
            (parsed.to_string() == value).then(|| format!("mxrs::long({value})"))
        }
        Ty::Integer => {
            let parsed = value.parse::<i32>().ok()?;
            (parsed.to_string() == value).then(|| format!("mxrs::integer({value})"))
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
                "mxrs::{}({value})",
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

pub(crate) fn collect(project: &Project, modules: &[Module]) -> Result<Vec<ConvertedFlow>> {
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
            if parameters_attested(&doc).is_none() {
                if let Some(name) = &flow.name {
                    targets.remove(&format!("{module_name}.{name}"));
                }
            } else {
                raw.push((module_name, doc));
            }
        }
    }
    for (name, kind) in &ambiguous {
        if kind == "Microflows$Microflow" {
            targets.remove(name);
        }
    }
    let mut result: Vec<_> = raw
        .iter()
        .filter(|(module, doc)| {
            !ambiguous.contains(&(
                format!("{module}.{}", doc.get_str("Name").unwrap_or_default()),
                doc.get_str("$Type").unwrap_or_default().to_string(),
            ))
        })
        .filter_map(|(module, doc)| convert(module, doc, &targets, &entities, &attributes))
        .collect();
    result.sort_by(|a, b| (&a.module, &a.declaration.name).cmp(&(&b.module, &b.declaration.name)));
    Ok(result)
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
    signature(&Microflow::from_bson(doc))?;
    Some(())
}

fn convert(
    module: &str,
    doc: &Document,
    targets: &HashMap<String, &Microflow>,
    entities: &HashSet<String>,
    attributes: &Attributes,
) -> Option<ConvertedFlow> {
    let nodes = linear_nodes(doc)?;
    let model = Microflow::from_bson(doc);
    let mut declaration = MicroflowDecl::new(model.name.as_ref()?);
    declaration.documentation = model.documentation.clone();
    let mut source = Vec::new();
    let mut variables = HashMap::new();
    if !model.documentation.is_empty() {
        source.push(format!(
            "flow.documentation({});",
            rust_string(&model.documentation)
        ));
    }
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
                    "flow.{}_parameter({}, mxrs::Ref::<{}>::new(), {options})",
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
        source.push(format!("let _ = &{rust_name};"));
        variables.insert(name, ty);
        declaration.parameters.push(parameter);
    }
    for node in nodes.iter().skip(1).take(nodes.len().saturating_sub(2)) {
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
                        "mxrs::attribute::<{}>({})",
                        attribute.marker,
                        expression(value, &attribute.value_type, &variables, attributes)?
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
                    source.push(format!("let {variable} = flow.create_object({}, mxrs::Ref::<{}>::new(), {members_source}, {commit});",rust_string(name),marker(&entity)?));
                    source.push(format!("let _ = &{variable};"));
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
                let target_marker = marker(target)?;
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
                        "mxrs::CallArgument::new({}, {})",
                        rust_string(name),
                        expression(value, ty, &variables, attributes)?
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
                    source.push(format!("let {variable} = flow.call_microflow_result::<{result_tag}>(mxrs::MicroflowRef::<{target_marker}>::new(), {}, {mappings_source});",rust_string(name)));
                    source.push(format!("let _ = &{variable};"));
                    (Some(name.into()), Some(ty))
                } else {
                    if !action.get_str("ResultVariableName").ok()?.is_empty() {
                        return None;
                    }
                    source.push(format!("flow.call_microflow(mxrs::MicroflowRef::<{target_marker}>::new(), None, false, {mappings_source});"));
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
                    "let {variable} = flow.create_list({}, mxrs::Ref::<{}>::new());",
                    rust_string(name),
                    marker(entity)?
                ));
                source.push(format!("let _ = &{variable};"));
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
        let (fresh, _) = mxrs_writer::flow_compiler::build_microflow_graph(
            std::slice::from_ref(&activity),
            &[],
            None,
        );
        if !same_semantics(action, fresh.get(1)?.get_document("Action").ok()?) {
            return None;
        }
        declaration.activities.push(activity);
    }
    let returned = nodes.last()?.get_str("ReturnValue").ok()?;
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
    if !mxrs_writer::flow_graph::preserves_linear_body(doc, &declaration) {
        return None;
    }
    Some(ConvertedFlow {
        module: module.into(),
        native_type: doc.get_str("$Type").ok()?.into(),
        declaration,
        source,
    })
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

pub(crate) fn render(flows: &[ConvertedFlow], version: &str, nanoflow: bool) -> String {
    let kind = if nanoflow { "nanoflow" } else { "microflow" };
    let native = if nanoflow {
        "Microflows$Nanoflow"
    } else {
        "Microflows$Microflow"
    };
    let selected: Vec<_> = flows.iter().filter(|f| f.native_type == native).collect();
    let mutable = if selected.is_empty() { "" } else { "mut " };
    let imports = if selected
        .iter()
        .any(|flow| flow.source.iter().any(|line| line.contains("model::")))
    {
        "use crate::infrastructure::markers as model;\n\n"
    } else {
        ""
    };
    let mut output = format!(
        "//! Editable {kind} declarations.\n\n{imports}pub fn apply(project: &mut mxrs::ProjectDecl) {{\n    let {mutable}declarations = mxrs::ProjectBuilder::new({});\n",
        rust_string(version)
    );
    for flow in selected {
        writeln!(
            output,
            "    declarations.{kind}_module({}, |module| {{",
            rust_string(&flow.module)
        )
        .unwrap();
        let parameter = if flow.source.is_empty() {
            "_flow"
        } else {
            "flow"
        };
        writeln!(
            output,
            "        module.{kind}({}, |{parameter}| {{",
            rust_string(&flow.declaration.name)
        )
        .unwrap();
        for line in &flow.source {
            writeln!(output, "            {line}").unwrap();
        }
        output.push_str("        });\n    });\n");
    }
    output.push_str(
        "    for module in declarations.build().modules { project.merge_module(module); }\n}\n",
    );
    output
}
