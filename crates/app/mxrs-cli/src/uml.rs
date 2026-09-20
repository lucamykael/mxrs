//! UML text exports — ports `bin/mxrb`'s `uml --export` path:
//! `lib/mxrb/uml/{class,activity,sequence}_diagram.rb` plus `support.rb`,
//! reproducing their Mermaid and PlantUML text byte-for-byte so the command
//! oracle can compare both implementations directly. The interactive viewer
//! (`uml/server.rb` + `web_ui/`) is a browser subsystem and is not ported;
//! the CLI refuses viewer mode explicitly instead of pretending to serve.

use mxrs_bson::{Bson, Document};
use mxrs_model::association::AssociationType;
use mxrs_model::{Association, AttributeType, Entity, Microflow, Module};
use mxrs_semantic::documents::{Artifact, DocumentIndex};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// `Mxrb::Uml::Support.identifier`: deterministic diagram-safe identifier.
fn identifier(raw: &str, prefix: &str) -> String {
    let mut body: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if body.is_empty() {
        body = prefix.to_string();
    }
    if !body
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        body = format!("{prefix}_{body}");
    }
    if body != raw {
        let digest = format!("{:x}", Sha256::digest(raw.as_bytes()));
        body = format!("{body}_{}", &digest[..8]);
    }
    body
}

/// `Mxrb::Uml::Support.mermaid_text`.
fn mermaid_text(value: &str) -> String {
    collapse_newlines(
        &value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;"),
    )
}

/// `Mxrb::Uml::Support.plantuml_text`.
fn plantuml_text(value: &str) -> String {
    collapse_newlines(&value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn collapse_newlines(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut in_break = false;
    for character in value.chars() {
        if character == '\r' || character == '\n' {
            if !in_break {
                result.push(' ');
                in_break = true;
            }
        } else {
            result.push(character);
            in_break = false;
        }
    }
    result
}

/// `Mxrb::Uml::Support.words`.
fn words(value: &str) -> String {
    let without_prefix = value.strip_prefix("Microflows$").unwrap_or(value);
    let trimmed = without_prefix
        .strip_suffix("Action")
        .unwrap_or(without_prefix);
    let mut spaced = String::with_capacity(trimmed.len());
    let mut previous: Option<char> = None;
    for character in trimmed.chars() {
        if character.is_ascii_uppercase()
            && previous.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
        {
            spaced.push(' ');
        }
        spaced.push(if character == '_' { ' ' } else { character });
        previous = Some(character);
    }
    spaced.trim().to_string()
}

/// Ports `Mxrb::Uml::ClassDiagram`.
pub struct ClassDiagram<'a> {
    modules: Vec<&'a Module>,
    entities: Vec<&'a Entity>,
    module_by_entity: HashMap<String, String>,
    entity_by_key: HashMap<String, &'a Entity>,
}

impl<'a> ClassDiagram<'a> {
    pub fn new(modules: Vec<&'a Module>) -> Self {
        let entities: Vec<&Entity> = modules
            .iter()
            .flat_map(|module| module.entities().iter())
            .collect();
        let mut module_by_entity = HashMap::new();
        for module in &modules {
            for entity in module.entities() {
                module_by_entity.insert(
                    entity.id.clone().unwrap_or_default(),
                    module.name.clone().unwrap_or_default(),
                );
            }
        }
        let mut entity_by_key = HashMap::new();
        for module in &modules {
            for entity in module.entities() {
                let qualified = qualified_entity_name(entity, &module_by_entity);
                entity_by_key.insert(entity.id.clone().unwrap_or_default(), entity);
                entity_by_key.insert(qualified, entity);
            }
        }
        Self {
            modules,
            entities,
            module_by_entity,
            entity_by_key,
        }
    }

    pub fn to_mermaid(&self) -> String {
        let mut lines = vec!["classDiagram".to_string()];
        for entity in &self.entities {
            self.append_mermaid_class(&mut lines, entity);
        }
        for (association, from, to) in self.associations() {
            lines.push(self.render_association(association, from, to, true));
        }
        format!("{}\n", lines.join("\n"))
    }

    pub fn to_plantuml(&self) -> String {
        let mut lines = vec!["@startuml".to_string(), "hide empty methods".to_string()];
        for entity in &self.entities {
            self.append_plantuml_class(&mut lines, entity);
        }
        for (association, from, to) in self.associations() {
            lines.push(self.render_association(association, from, to, false));
        }
        lines.push("@enduml".to_string());
        format!("{}\n", lines.join("\n"))
    }

    fn append_mermaid_class(&self, lines: &mut Vec<String>, entity: &Entity) {
        let identifier = self.entity_identifier(entity);
        let qualified = qualified_entity_name(entity, &self.module_by_entity);
        lines.push(format!(
            "class {identifier}[\"{}\"]",
            mermaid_text(&qualified)
        ));
        lines.push(format!("class {identifier} {{"));
        for attribute in &entity.attributes {
            lines.push(format!(
                "  +{} {}",
                attribute_type_label(attribute),
                mermaid_text(attribute.name.as_deref().unwrap_or_default())
            ));
        }
        lines.push("}".to_string());
        if let Some(stereotype) = stereotype(entity) {
            lines.push(format!("<<{stereotype}>> {identifier}"));
        }
    }

    fn append_plantuml_class(&self, lines: &mut Vec<String>, entity: &Entity) {
        let identifier = self.entity_identifier(entity);
        let qualified = qualified_entity_name(entity, &self.module_by_entity);
        let suffix = stereotype(entity).map_or(String::new(), |s| format!(" <<{s}>>"));
        lines.push(format!(
            "class \"{}\" as {identifier}{suffix} {{",
            plantuml_text(&qualified)
        ));
        for attribute in &entity.attributes {
            lines.push(format!(
                "  +{} : {}",
                plantuml_text(attribute.name.as_deref().unwrap_or_default()),
                attribute_type_label(attribute)
            ));
        }
        lines.push("}".to_string());
    }

    fn associations(&self) -> Vec<(&Association, &Entity, &Entity)> {
        self.modules
            .iter()
            .flat_map(|module| module.associations())
            .filter_map(|association| {
                let from = self
                    .entity_by_key
                    .get(association.from_entity_id.as_deref().unwrap_or_default())?;
                let to = self
                    .entity_by_key
                    .get(association.to_entity_id.as_deref().unwrap_or_default())?;
                Some((association, *from, *to))
            })
            .collect()
    }

    fn render_association(
        &self,
        association: &Association,
        from: &Entity,
        to: &Entity,
        mermaid: bool,
    ) -> String {
        let (left, right) = if association.association_type == AssociationType::ReferenceSet {
            ("*", "*")
        } else {
            ("1", "N")
        };
        let name = association.name.as_deref().unwrap_or_default();
        let label = if mermaid {
            mermaid_text(name)
        } else {
            plantuml_text(name)
        };
        format!(
            "{} \"{left}\" --> \"{right}\" {} : {label}",
            self.entity_identifier(from),
            self.entity_identifier(to)
        )
    }

    fn entity_identifier(&self, entity: &Entity) -> String {
        identifier(
            &qualified_entity_name(entity, &self.module_by_entity),
            "class",
        )
    }
}

fn qualified_entity_name(entity: &Entity, module_by_entity: &HashMap<String, String>) -> String {
    match entity.qualified_name.as_deref() {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => format!(
            "{}.{}",
            module_by_entity
                .get(entity.id.as_deref().unwrap_or_default())
                .cloned()
                .unwrap_or_default(),
            entity.name.as_deref().unwrap_or_default()
        ),
    }
}

fn attribute_type_label(attribute: &mxrs_model::Attribute) -> String {
    if let Some(enumeration) = attribute
        .enumeration
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return words(enumeration.rsplit('.').next().unwrap_or_default());
    }
    match attribute.attribute_type {
        AttributeType::String => "String",
        AttributeType::Integer => "Integer",
        AttributeType::Long => "Long",
        AttributeType::Float => "Float",
        AttributeType::Decimal => "Decimal",
        AttributeType::Boolean => "Boolean",
        AttributeType::DateTime => "DateTime",
        AttributeType::AutoNumber => "AutoNumber",
        AttributeType::HashString => "HashedString",
        AttributeType::Binary => "Binary",
        AttributeType::Enum => "Enumeration",
    }
    .to_string()
}

fn stereotype(entity: &Entity) -> Option<&'static str> {
    if oql_view(entity) {
        return Some("OQL View");
    }
    if !entity.persistable {
        return Some("DTO");
    }
    None
}

/// `Mxrb::Model::Entity#oql_view?`.
fn oql_view(entity: &Entity) -> bool {
    let native = entity.native_type.as_deref().unwrap_or_default();
    if native.to_ascii_lowercase().contains("viewentity") {
        return true;
    }
    let source_type = entity
        .source
        .as_ref()
        .and_then(|source| source.get_str("$Type").ok())
        .unwrap_or_default();
    if source_type
        .to_ascii_lowercase()
        .contains("oqlviewentitysource")
    {
        return true;
    }
    entity.oql_query.as_deref().is_some_and(|q| !q.is_empty())
}

/// Ports `Mxrb::Uml::ActivityDiagram`: renders one microflow's native
/// object/sequence-flow graph.
pub struct ActivityDiagram<'a> {
    microflow: &'a Microflow,
    objects_by_id: HashMap<String, &'a Document>,
}

impl<'a> ActivityDiagram<'a> {
    pub fn new(microflow: &'a Microflow) -> Self {
        let objects_by_id = microflow
            .objects
            .iter()
            .map(|object| (raw_object_id(object), object))
            .collect();
        Self {
            microflow,
            objects_by_id,
        }
    }

    pub fn to_mermaid(&self) -> String {
        let mut lines = vec!["flowchart TD".to_string()];
        for object in &self.microflow.objects {
            lines.push(format!("  {}", mermaid_node(object)));
        }
        for flow in &self.microflow.flows {
            if let Some(line) = self.flow_line(flow, true) {
                lines.push(format!("  {line}"));
            }
        }
        format!("{}\n", lines.join("\n"))
    }

    pub fn to_plantuml(&self) -> String {
        let mut lines = vec![
            "@startuml".to_string(),
            format!(
                "title {}",
                plantuml_text(self.microflow.name.as_deref().unwrap_or_default())
            ),
        ];
        for object in &self.microflow.objects {
            lines.push(plantuml_node(object));
        }
        for flow in &self.microflow.flows {
            if let Some(line) = self.flow_line(flow, false) {
                lines.push(line);
            }
        }
        lines.push("@enduml".to_string());
        format!("{}\n", lines.join("\n"))
    }

    fn flow_line(&self, flow: &Document, mermaid: bool) -> Option<String> {
        let origin = pointer(flow, &["OriginPointer", "Origin"]);
        let destination = pointer(flow, &["DestinationPointer", "Destination"]);
        let origin = self.objects_by_id.get(&origin)?;
        let destination = self.objects_by_id.get(&destination)?;
        let label = flow_label(flow);
        Some(if mermaid {
            let middle = if label.is_empty() {
                "-->".to_string()
            } else {
                format!("-->|\"{}\"|", mermaid_text(&label))
            };
            format!("{} {middle} {}", node_id(origin), node_id(destination))
        } else {
            let suffix = if label.is_empty() {
                String::new()
            } else {
                format!(" : {}", plantuml_text(&label))
            };
            format!("{} --> {}{suffix}", node_id(origin), node_id(destination))
        })
    }
}

fn mermaid_node(object: &Document) -> String {
    let id = node_id(object);
    let label = mermaid_text(&object_label(object));
    match object_type(object).as_str() {
        "StartEvent" => format!("{id}([\"Start\"])"),
        "EndEvent" | "ErrorEvent" | "ContinueEvent" => format!("{id}([\"{label}\"])"),
        "DecisionMergeActivity" | "ExclusiveMerge" => format!("{id}{{\"{label}\"}}"),
        _ => format!("{id}[\"{label}\"]"),
    }
}

fn plantuml_node(object: &Document) -> String {
    let id = node_id(object);
    let label = plantuml_text(&object_label(object));
    let shape = if matches!(
        object_type(object).as_str(),
        "DecisionMergeActivity" | "ExclusiveMerge"
    ) {
        "choice"
    } else {
        "state"
    };
    format!("{shape} \"{label}\" as {id}")
}

fn object_label(object: &Document) -> String {
    let type_name = object_type(object);
    if type_name == "StartEvent" {
        return "Start".to_string();
    }
    if type_name != "ActionActivity" {
        return words(&type_name);
    }
    let caption = string_field(object, &["Caption", "caption"]);
    let action = document_field(object, &["Action", "action"]);
    let generated =
        object.get_bool("AutoGenerateCaption").unwrap_or(false) || caption == "Activity";
    if !caption.is_empty() && !generated {
        return caption;
    }
    let call = action
        .as_ref()
        .and_then(|action| document_field(action, &["MicroflowCall", "microflowCall"]));
    let target = call
        .as_ref()
        .map(|call| string_field(call, &["Microflow", "microflow"]))
        .unwrap_or_default();
    if !target.is_empty() {
        return format!("Call {target}");
    }
    let action_type = action
        .as_ref()
        .map(|action| string_field(action, &["$Type", "Type"]))
        .unwrap_or_default();
    words(if action_type.is_empty() {
        "Action"
    } else {
        &action_type
    })
}

fn flow_label(flow: &Document) -> String {
    let direct = string_field(flow, &["Caption", "Label", "Condition"]);
    if !direct.is_empty() {
        return direct;
    }
    let value = flow
        .get("CaseValue")
        .or_else(|| flow.get("caseValue"))
        .cloned()
        .or_else(|| {
            flow.get_array("CaseValues")
                .ok()
                .and_then(|raw| mxrs_bson::parse_array(Some(raw)).items.first().cloned())
        });
    let Some(value) = value else {
        return String::new();
    };
    match value {
        Bson::Document(document) => {
            let type_name = string_field(&document, &["$Type", "Type"]);
            if type_name.ends_with("$NoCase") {
                return String::new();
            }
            let representation = string_field(&document, &["StringRepresentation", "Value"]);
            if !representation.is_empty() {
                representation
            } else {
                words(&type_name)
            }
        }
        other => bson_to_display(&other),
    }
}

fn bson_to_display(value: &Bson) -> String {
    match value {
        Bson::String(text) => text.clone(),
        Bson::Boolean(flag) => flag.to_string(),
        Bson::Int32(number) => number.to_string(),
        Bson::Int64(number) => number.to_string(),
        Bson::Double(number) => number.to_string(),
        Bson::Null => String::new(),
        other => other.to_string(),
    }
}

fn pointer(flow: &Document, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| flow.get(*key))
        .and_then(mxrs_bson::extract_id)
        .unwrap_or_default()
}

fn string_field(document: &Document, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| document.get_str(key).ok())
        .unwrap_or_default()
        .to_string()
}

fn document_field(document: &Document, keys: &[&str]) -> Option<Document> {
    keys.iter()
        .find_map(|key| document.get_document(key).ok())
        .cloned()
}

fn raw_object_id(object: &Document) -> String {
    ["$ID", "ID"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(mxrs_bson::extract_id)
        .unwrap_or_default()
}

fn object_type(object: &Document) -> String {
    let type_name = string_field(object, &["$Type", "Type"]);
    type_name
        .strip_prefix("Microflows$")
        .map_or(type_name.clone(), str::to_string)
}

fn node_id(object: &Document) -> String {
    identifier(&raw_object_id(object), "activity")
}

const SEQUENCE_MAX_DEPTH: i64 = 100;

/// Ports `Mxrb::Uml::SequenceDiagram`: call-chain diagrams over the same
/// document index the verified `callees`/`callers` commands use, so edge
/// order matches MXRB's reference insertion order.
pub struct SequenceDiagram<'a> {
    edges: Vec<(&'a Artifact, &'a Artifact)>,
    participants: Vec<&'a Artifact>,
}

impl<'a> SequenceDiagram<'a> {
    pub fn new(
        index: &'a DocumentIndex,
        root: Option<&str>,
        module_name: Option<&str>,
        depth: i64,
    ) -> Result<Self, String> {
        if root.is_some() && module_name.is_some() {
            return Err("choose either root or module_name".to_string());
        }
        if root.is_none() && module_name.is_none() {
            return Err("root or module_name is required".to_string());
        }
        if depth < 0 {
            return Err("depth must be zero or greater".to_string());
        }
        if depth > SEQUENCE_MAX_DEPTH {
            return Err(format!("depth cannot exceed {SEQUENCE_MAX_DEPTH}"));
        }
        let (edges, selected) = match root {
            Some(root) => root_graph(index, root, depth)?,
            None => module_graph(index, module_name.unwrap_or_default())?,
        };
        let mut participants: Vec<&Artifact> = Vec::new();
        let mut seen = HashSet::new();
        for artifact in selected
            .iter()
            .chain(edges.iter().flat_map(|(from, to)| [from, to]))
        {
            if seen.insert(artifact.id.as_str()) {
                participants.push(artifact);
            }
        }
        participants.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(Self {
            edges,
            participants,
        })
    }

    pub fn to_mermaid(&self) -> String {
        let mut lines = vec!["sequenceDiagram".to_string()];
        for artifact in &self.participants {
            lines.push(format!(
                "  participant {} as {}",
                participant_id(artifact),
                mermaid_text(&artifact.qualified_name)
            ));
        }
        for (source, target) in &self.edges {
            lines.push(format!(
                "  {}->>{}: call",
                participant_id(source),
                participant_id(target)
            ));
        }
        if self.edges.is_empty() {
            lines.push(match self.participants.first() {
                Some(participant) => format!(
                    "  Note over {}: No call relationships found",
                    participant_id(participant)
                ),
                None => "  Note over MXRB: No call relationships found".to_string(),
            });
        }
        format!("{}\n", lines.join("\n"))
    }

    pub fn to_plantuml(&self) -> String {
        let mut lines = vec!["@startuml".to_string()];
        for artifact in &self.participants {
            lines.push(format!(
                "participant \"{}\" as {}",
                plantuml_text(&artifact.qualified_name),
                participant_id(artifact)
            ));
        }
        for (source, target) in &self.edges {
            lines.push(format!(
                "{} -> {} : call",
                participant_id(source),
                participant_id(target)
            ));
        }
        if self.edges.is_empty() {
            lines.push("note \"No call relationships found\" as MXRB".to_string());
        }
        lines.push("@enduml".to_string());
        format!("{}\n", lines.join("\n"))
    }
}

fn callable(artifact: &Artifact) -> bool {
    artifact.kind == "microflow" || artifact.kind == "nanoflow"
}

fn callees<'a>(index: &'a DocumentIndex, source: &Artifact) -> Vec<&'a Artifact> {
    let mut seen = HashSet::new();
    index
        .references()
        .iter()
        .filter(|reference| reference.relation == "calls" && reference.from == source.id)
        .map(|reference| index.artifact(&reference.to))
        .filter(|target| callable(target) && seen.insert(target.id.as_str()))
        .collect()
}

type Edges<'a> = Vec<(&'a Artifact, &'a Artifact)>;

fn root_graph<'a>(
    index: &'a DocumentIndex,
    root: &str,
    depth: i64,
) -> Result<(Edges<'a>, Vec<&'a Artifact>), String> {
    let artifact = index
        .require(root)
        .ok()
        .filter(|artifact| callable(artifact))
        .ok_or_else(|| format!("microflow not found: {root}"))?;
    let mut edges = Vec::new();
    let mut visited = HashSet::new();
    // Recursive preorder DFS matching MXRB's visit order.
    fn visit<'a>(
        index: &'a DocumentIndex,
        source: &'a Artifact,
        level: i64,
        depth: i64,
        visited: &mut HashSet<String>,
        edges: &mut Edges<'a>,
    ) {
        if level >= depth || visited.contains(&source.id) {
            return;
        }
        visited.insert(source.id.clone());
        for target in callees(index, source) {
            edges.push((source, target));
            visit(index, target, level + 1, depth, visited, edges);
        }
    }
    visit(index, artifact, 0, depth, &mut visited, &mut edges);
    let mut seen = HashSet::new();
    edges.retain(|(from, to)| seen.insert((from.id.clone(), to.id.clone())));
    Ok((edges, vec![artifact]))
}

fn module_graph<'a>(
    index: &'a DocumentIndex,
    module_name: &str,
) -> Result<(Edges<'a>, Vec<&'a Artifact>), String> {
    let sources: Vec<&Artifact> = index
        .artifacts()
        .iter()
        .filter(|artifact| {
            artifact.module_name.as_deref() == Some(module_name) && callable(artifact)
        })
        .collect();
    if sources.is_empty() {
        return Err(format!(
            "module not found or has no microflows: {module_name}"
        ));
    }
    let mut edges: Edges = sources
        .iter()
        .flat_map(|source| {
            callees(index, source)
                .into_iter()
                .filter(|target| target.module_name.as_deref() == Some(module_name))
                .map(move |target| (*source, target))
        })
        .collect();
    let mut seen = HashSet::new();
    edges.retain(|(from, to)| seen.insert((from.id.clone(), to.id.clone())));
    Ok((edges, sources))
}

fn participant_id(artifact: &Artifact) -> String {
    identifier(&artifact.qualified_name, "participant")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_matches_mxrb_support() {
        let digest = format!("{:x}", Sha256::digest("Sales.Order".as_bytes()));
        assert_eq!(
            identifier("Sales.Order", "class"),
            format!("Sales_Order_{}", &digest[..8])
        );
        assert_eq!(identifier("Plain_Name", "class"), "Plain_Name");
        let empty = identifier("", "class");
        assert!(empty.starts_with("class_"), "{empty}");
        let leading_digit = identifier("1abc", "n");
        assert!(leading_digit.starts_with("n_1abc_"), "{leading_digit}");
    }

    #[test]
    fn mermaid_text_escapes_in_mxrb_order() {
        assert_eq!(
            mermaid_text("a & <b> \"c\""),
            "a &amp; &lt;b&gt; &quot;c&quot;"
        );
        assert_eq!(mermaid_text("one\r\n\r\ntwo"), "one two");
    }

    #[test]
    fn plantuml_text_escapes_backslashes_and_quotes() {
        assert_eq!(plantuml_text("a\\b \"c\""), "a\\\\b \\\"c\\\"");
    }

    #[test]
    fn words_splits_camel_case_and_strips_flow_affixes() {
        assert_eq!(words("Microflows$CreateChangeAction"), "Create Change");
        assert_eq!(words("RetrieveByPath"), "Retrieve By Path");
        assert_eq!(words("snake_case"), "snake case");
    }
}
