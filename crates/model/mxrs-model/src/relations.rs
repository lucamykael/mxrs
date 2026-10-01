//! What a model says about each of its flows: the flows it calls, the
//! entities it works with, the roles that may run it and the documents that
//! refer to it.
//!
//! None of this is stored as such. A call is an action in the calling flow,
//! an entity a type or a member somewhere in the body, and a use is another
//! document naming the flow — a button, a route, a scheduled event. All of
//! them name what they refer to by qualified name, so the relations are read
//! by finding those names where the model keeps them.

use std::collections::{BTreeSet, HashMap, HashSet};

use mxrs_bson::{Bson, Document};

use crate::Project;
use crate::error::Result;

/// The relations of one flow, each a set of qualified names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowRelations {
    /// The flows this one calls.
    pub calls: BTreeSet<String>,
    /// The entities it names: as a type, in an action, or through one of
    /// their attributes.
    pub uses: BTreeSet<String>,
    /// What refers to it: a flow, page or other document as
    /// `Module.Document`, an entity as `Module.Entity` for an event handler
    /// or a calculated attribute, and `Navigation` or `Settings` for the
    /// project's own.
    pub used_by: BTreeSet<String>,
    /// The module roles allowed to run it, as `Module.Role`.
    pub roles: BTreeSet<String>,
}

const FLOW_TYPES: [&str; 3] = [
    "Microflows$Microflow",
    "Microflows$Nanoflow",
    "Microflows$Rule",
];

/// Reads the relations of every flow in the project, by the flow's
/// qualified name.
pub fn flow_relations(project: &Project) -> Result<HashMap<String, FlowRelations>> {
    struct Unit {
        container: String,
        document: Document,
    }
    let mut units = HashMap::new();
    for raw in project.all_units()? {
        let document = project.mpr().parse_contents(&raw)?;
        units.insert(
            raw.unit_id.clone(),
            Unit {
                container: raw.container_id.clone(),
                document,
            },
        );
    }
    // The module a unit belongs to: the nearest module above it.
    let module_of = |unit: &Unit| -> Option<String> {
        let mut container = unit.container.as_str();
        for _ in 0..units.len() {
            let parent = units.get(container)?;
            if matches!(
                parent.document.get_str("$Type").ok(),
                Some("Projects$Module" | "Projects$ModuleImpl")
            ) {
                return parent.document.get_str("Name").ok().map(str::to_string);
            }
            container = parent.container.as_str();
        }
        None
    };

    let mut flows: HashSet<String> = HashSet::new();
    let mut entities: HashSet<String> = HashSet::new();
    let mut owners: HashMap<&str, (Option<String>, String)> = HashMap::new();
    for (id, unit) in &units {
        let module = module_of(unit);
        let kind = unit.document.get_str("$Type").unwrap_or_default();
        let name = unit.document.get_str("Name").ok();
        if let (Some(module), Some(name)) = (&module, name)
            && FLOW_TYPES.contains(&kind)
        {
            flows.insert(format!("{module}.{name}"));
        }
        if let Some(module) = &module
            && kind == "DomainModels$DomainModel"
        {
            // A domain model spells its keys either way, by the version
            // of the model that wrote it.
            let listed = unit
                .document
                .get("Entities")
                .or_else(|| unit.document.get("entities"));
            for entity in documents(listed) {
                if let Some(name) = name_of(entity) {
                    entities.insert(format!("{module}.{name}"));
                }
            }
        }
        let label = match (&module, name) {
            (Some(module), Some(name)) if !name.is_empty() => format!("{module}.{name}"),
            (Some(module), _) if kind == "DomainModels$DomainModel" => {
                format!("{module} domain model")
            }
            _ => match kind {
                "Navigation$NavigationDocument" => "Navigation".to_string(),
                "Settings$ProjectSettings" => "Settings".to_string(),
                other => other.rsplit('$').next().unwrap_or(other).to_string(),
            },
        };
        owners.insert(id.as_str(), (module, label));
    }

    let mut relations: HashMap<String, FlowRelations> = flows
        .iter()
        .map(|flow| (flow.clone(), FlowRelations::default()))
        .collect();
    for (id, unit) in &units {
        let (module, label) = &owners[id.as_str()];
        let kind = unit.document.get_str("$Type").unwrap_or_default();
        let this = match (module, unit.document.get_str("Name").ok()) {
            (Some(module), Some(name)) if FLOW_TYPES.contains(&kind) => {
                Some(format!("{module}.{name}"))
            }
            _ => None,
        };
        let mut found = Found::default();
        collect(
            &Bson::Document(unit.document.clone()),
            label,
            module.as_deref(),
            &flows,
            &entities,
            &mut found,
        );
        for (flow, user) in found.flows {
            // A flow that calls itself is not thereby used by anything.
            if this.as_deref() == Some(flow.as_str()) && user == *label {
                if let Some(own) = relations.get_mut(&flow) {
                    own.calls.insert(flow.clone());
                }
                continue;
            }
            if let Some(this) = &this
                && let Some(own) = relations.get_mut(this)
            {
                own.calls.insert(flow.clone());
            }
            if let Some(target) = relations.get_mut(&flow) {
                target.used_by.insert(user);
            }
        }
        if let Some(this) = &this
            && let Some(own) = relations.get_mut(this)
        {
            own.uses.extend(found.entities);
            own.roles.extend(
                strings(unit.document.get("AllowedModuleRoles"))
                    .into_iter()
                    .map(str::to_string),
            );
        }
    }
    Ok(relations)
}

#[derive(Default)]
struct Found {
    /// A flow named somewhere, and what names it.
    flows: BTreeSet<(String, String)>,
    entities: BTreeSet<String>,
}

fn collect(
    value: &Bson,
    owner: &str,
    module: Option<&str>,
    flows: &HashSet<String>,
    entities: &HashSet<String>,
    found: &mut Found,
) {
    match value {
        Bson::Document(document) => {
            // Inside a domain model, what refers to a flow is the entity
            // whose event handler or calculated attribute it is.
            let entity = match (module, document.get_str("$Type").ok()) {
                (Some(module), Some("DomainModels$EntityImpl" | "DomainModels$Entity")) => {
                    name_of(document).map(|name| format!("{module}.{name}"))
                }
                _ => None,
            };
            let owner = entity.as_deref().unwrap_or(owner);
            for (key, value) in document {
                if matches!(key.as_str(), "$ID" | "$Type") {
                    continue;
                }
                collect(value, owner, module, flows, entities, found);
            }
        }
        Bson::Array(values) => {
            for value in values {
                collect(value, owner, module, flows, entities, found);
            }
        }
        Bson::String(text) => {
            if flows.contains(text) {
                found.flows.insert((text.clone(), owner.to_string()));
            } else if entities.contains(text) {
                found.entities.insert(text.clone());
            } else if let Some((entity, _)) = text.rsplit_once('.')
                && entities.contains(entity)
            {
                // `Module.Entity.Attribute`: the entity that has it.
                found.entities.insert(entity.to_string());
            }
        }
        _ => {}
    }
}

fn name_of(document: &Document) -> Option<&str> {
    document
        .get_str("Name")
        .or_else(|_| document.get_str("name"))
        .ok()
}

fn documents(value: Option<&Bson>) -> Vec<&Document> {
    match value {
        Some(Bson::Array(items)) => items.iter().filter_map(Bson::as_document).collect(),
        _ => Vec::new(),
    }
}

fn strings(value: Option<&Bson>) -> Vec<&str> {
    match value {
        Some(Bson::Array(items)) => items.iter().filter_map(Bson::as_str).collect(),
        _ => Vec::new(),
    }
}
