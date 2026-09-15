//! Native document inventory and textual reference queries.
//!
//! MXRB's browse commands scan named documents and expression tokens, including
//! unknown document families. This graph deliberately has no synthetic
//! containment edges. It is separate from the typed, explicit-reference graph
//! used by compiler diagnostics and refactoring: a token match is useful for
//! browsing, but is not sufficient evidence to rewrite a model reference.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

use mxrs_bson::{Bson, Document};
use mxrs_model::{DomainModel, Project};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Result, SemanticError};

#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub id: String,
    pub qualified_name: String,
    pub kind: String,
    pub module_name: Option<String>,
    pub name: Option<String>,
    pub unit_id: Option<String>,
    pub path: Vec<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reference {
    pub from: String,
    pub to: String,
    pub relation: String,
    pub path: Vec<String>,
    pub value: String,
}

#[derive(Debug, Default)]
pub struct DocumentIndex {
    artifacts: Vec<Artifact>,
    references: Vec<Reference>,
    by_id: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
}

impl DocumentIndex {
    pub fn build(project: &Project) -> Result<Self> {
        let units = project.all_units()?;
        let documents = units
            .iter()
            .map(|unit| {
                project
                    .mpr()
                    .parse_contents(unit)
                    .map_err(mxrs_model::ModelError::from)
            })
            .collect::<mxrs_model::Result<Vec<_>>>()?;
        let positions: HashMap<_, _> = units
            .iter()
            .enumerate()
            .map(|(i, unit)| (unit.unit_id.as_str(), i))
            .collect();
        let modules: Vec<_> = units
            .iter()
            .enumerate()
            .filter(|(_, unit)| unit.containment_name == "Modules")
            .collect();
        let module_names: HashMap<_, _> = modules
            .iter()
            .map(|(i, unit)| (unit.unit_id.as_str(), documents[*i].get_str("Name").ok()))
            .collect();
        let mut index = Self::default();
        let mut sources = Vec::new();
        for (i, unit) in modules {
            let name = documents[i].get_str("Name").ok().map(str::to_string);
            let module_name = name.clone();
            let module = name.clone().unwrap_or_default();
            index.add(Artifact {
                id: format!("module:{}", unit.unit_id),
                qualified_name: module.clone(),
                kind: "module".into(),
                module_name: name.clone(),
                name,
                unit_id: Some(unit.unit_id.clone()),
                path: vec![],
                metadata: json!({}),
            });
            let Some((dm_position, _)) = units.iter().enumerate().find(|(_, child)| {
                child.container_id == unit.unit_id && child.containment_name == "DomainModel"
            }) else {
                continue;
            };
            let raw = &documents[dm_position];
            let domain = DomainModel::from_bson(raw, Some(&module));
            for entity in &domain.entities {
                let name = entity.name.clone().unwrap_or_default();
                let qualified = entity
                    .qualified_name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{module}.{name}"));
                let path = vec!["entities".into(), name];
                index.add(Artifact {
                    id: format!("entity:{}", entity.id.as_deref().unwrap_or_default()),
                    qualified_name: qualified.clone(),
                    kind: "entity".into(),
                    module_name: module_name.clone(),
                    name: entity.name.clone(),
                    unit_id: domain.id.clone(),
                    path: path.clone(),
                    metadata: json!({}),
                });
                for attribute in &entity.attributes {
                    let name = attribute.name.clone().unwrap_or_default();
                    let mut path = path.clone();
                    path.extend(["attributes".into(), name.clone()]);
                    index.add(Artifact {
                        id: format!("attribute:{}", attribute.id.as_deref().unwrap_or_default()),
                        qualified_name: format!("{qualified}.{name}"),
                        kind: "attribute".into(),
                        module_name: module_name.clone(),
                        name: attribute.name.clone(),
                        unit_id: domain.id.clone(),
                        path,
                        metadata: json!({}),
                    });
                }
            }
            for association in domain.all_associations() {
                let name = association.name.clone().unwrap_or_default();
                let entity_name = |id: &Option<String>| {
                    domain
                        .entities
                        .iter()
                        .find(|entity| entity.id == *id)
                        .and_then(|entity| entity.qualified_name.clone())
                };
                index.add(Artifact { id: format!("association:{}", association.id.as_deref().unwrap_or_default()), qualified_name: format!("{module}.{name}"), kind: "association".into(), module_name: module_name.clone(), name: association.name.clone(), unit_id: domain.id.clone(), path: vec!["associations".into(), name], metadata: json!({"from": entity_name(&association.from_entity_id), "to": entity_name(&association.to_entity_id)}) });
            }
            for (keys, kind) in [
                (["entities", "Entities"], "entity"),
                (["associations", "Associations"], "association"),
                (["crossAssociations", "CrossAssociations"], "association"),
            ] {
                for value in array_items(field(raw, &keys)) {
                    if let Bson::Document(doc) = value {
                        let id = doc
                            .get("$ID")
                            .and_then(mxrs_bson::extract_id)
                            .unwrap_or_default();
                        if let Some(&source) = index.by_id.get(&format!("{kind}:{id}")) {
                            sources.push((source, doc));
                        }
                    }
                }
            }
        }
        for (i, unit) in units.iter().enumerate() {
            let doc = &documents[i];
            let ty = doc.get_str("$Type").unwrap_or_default();
            if ty.is_empty()
                || matches!(ty, "Projects$Module" | "Projects$ModuleImpl")
                || ty.contains("DomainModel")
            {
                continue;
            }
            let mut name = field(doc, &["Name", "name"])
                .and_then(Bson::as_str)
                .unwrap_or_default()
                .to_string();
            if name.is_empty() && ty == "Navigation$NavigationDocument" {
                name = "Navigation".into();
            }
            if name.is_empty() {
                continue;
            }
            let mut current = unit;
            let mut visited = HashSet::new();
            let module = loop {
                if let Some(name) = module_names.get(current.container_id.as_str()) {
                    break name.map(str::to_string);
                }
                if !visited.insert(&current.container_id) {
                    break None;
                }
                let Some(&parent) = positions.get(current.container_id.as_str()) else {
                    break None;
                };
                current = &units[parent];
            };
            let qualified_name = if ty == "Navigation$NavigationDocument" {
                "Project.Navigation".into()
            } else {
                module
                    .as_ref()
                    .map_or_else(|| name.clone(), |module| format!("{module}.{name}"))
            };
            let roles = array_items(field(doc, &["AllowedModuleRoles", "AllowedRoles"]))
                .iter()
                .map(scalar_string)
                .collect::<Vec<_>>();
            let source = index.add(Artifact { id: format!("unit:{}", unit.unit_id), qualified_name, kind: kind_for(ty), module_name: module, name: Some(name), unit_id: Some(unit.unit_id.clone()), path: vec![], metadata: json!({
                "bson_type": ty, "documentation": field(doc, &["Documentation", "documentation"]).map(scalar_string).unwrap_or_default(), "allowed_roles": roles,
                "excluded": doc.get_bool("Excluded").unwrap_or(false), "mark_as_used": doc.get_bool("MarkAsUsed").unwrap_or(false),
                "exposed": doc.iter().any(|(key, value)| ["Expose", "Exposed", "Published"].iter().any(|prefix| key.starts_with(prefix)) && truthy(value)),
            }) });
            sources.push((source, doc.clone()));
        }
        let mut seen = HashSet::new();
        for (source, document) in sources {
            index.walk(
                source,
                &Bson::Document(document),
                &mut Vec::new(),
                &mut seen,
            );
        }
        Ok(index)
    }

    pub fn artifacts(&self) -> &[Artifact] {
        &self.artifacts
    }
    pub fn references(&self) -> &[Reference] {
        &self.references
    }
    pub fn artifact(&self, id: &str) -> &Artifact {
        &self.artifacts[self.by_id[id]]
    }

    pub fn require(&self, name: &str) -> Result<&Artifact> {
        match self
            .by_name
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            [] => Err(SemanticError::Unknown(name.into())),
            [index] => Ok(&self.artifacts[*index]),
            _ => Err(SemanticError::Ambiguous(name.into())),
        }
    }

    pub fn incoming(&self, name: &str) -> Result<Vec<&Reference>> {
        let root = self.require(name)?;
        Ok(self
            .references
            .iter()
            .filter(|reference| reference.to == root.id)
            .collect())
    }
    pub fn outgoing(&self, name: &str) -> Result<Vec<&Reference>> {
        let root = self.require(name)?;
        Ok(self
            .references
            .iter()
            .filter(|reference| reference.from == root.id)
            .collect())
    }
    pub fn calls(&self, name: &str, incoming: bool) -> Result<Vec<&Artifact>> {
        let references = if incoming {
            self.incoming(name)?
        } else {
            self.outgoing(name)?
        };
        let mut seen = HashSet::new();
        Ok(references
            .into_iter()
            .filter(|reference| reference.relation == "calls")
            .filter_map(|reference| {
                let id = if incoming {
                    &reference.from
                } else {
                    &reference.to
                };
                seen.insert(id).then(|| self.artifact(id))
            })
            .collect())
    }
    pub fn impact(&self, name: &str) -> Result<Vec<&Artifact>> {
        let root = self.require(name)?;
        let mut seen = HashSet::from([root.id.as_str()]);
        let mut queue = VecDeque::from([root]);
        let mut affected = Vec::new();
        while let Some(current) = queue.pop_front() {
            for reference in self
                .references
                .iter()
                .filter(|reference| reference.to == current.id)
            {
                if seen.insert(&reference.from) {
                    let source = self.artifact(&reference.from);
                    affected.push(source);
                    queue.push_back(source);
                }
            }
        }
        Ok(affected)
    }
    pub fn fingerprint(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&self.artifacts, &self.references))
                    .expect("document graph is serializable")
            )
        )
    }
    fn add(&mut self, artifact: Artifact) -> usize {
        if let Some(&index) = self.by_id.get(&artifact.id) {
            return index;
        }
        let index = self.artifacts.len();
        self.by_id.insert(artifact.id.clone(), index);
        if artifact.kind != "module" || artifact.name.is_some() {
            self.by_name
                .entry(artifact.qualified_name.clone())
                .or_default()
                .push(index);
        }
        self.artifacts.push(artifact);
        index
    }
    fn walk(
        &mut self,
        source: usize,
        value: &Bson,
        path: &mut Vec<String>,
        seen: &mut HashSet<(usize, usize, String, Vec<String>)>,
    ) {
        match value {
            Bson::Document(doc) => {
                for (key, value) in doc {
                    path.push(key.clone());
                    self.walk(source, value, path, seen);
                    path.pop();
                }
            }
            Bson::Array(items) => {
                for (index, value) in items.iter().enumerate() {
                    path.push(index.to_string());
                    self.walk(source, value, path, seen);
                    path.pop();
                }
            }
            Bson::String(value) => {
                let field = path.last().map(String::as_str).unwrap_or_default();
                if matches!(
                    field,
                    "$Type" | "$ID" | "ContentsHash" | "Documentation" | "Caption" | "Text"
                ) {
                    return;
                }
                for candidate in candidates(value, self.artifacts[source].module_name.as_deref()) {
                    let Some(target) = self.resolve_candidate(&candidate, source, field) else {
                        continue;
                    };
                    if source == target
                        && matches!(field, "$QualifiedName" | "QualifiedName" | "Name" | "name")
                    {
                        continue;
                    }
                    let relation = relation_for(field, &self.artifacts[target].kind);
                    if seen.insert((source, target, relation.clone(), path.clone())) {
                        self.references.push(Reference {
                            from: self.artifacts[source].id.clone(),
                            to: self.artifacts[target].id.clone(),
                            relation,
                            path: path.clone(),
                            value: value.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    fn resolve_candidate(&self, candidate: &str, source: usize, field: &str) -> Option<usize> {
        let matches = self.by_name.get(candidate)?;
        if matches.len() == 1 {
            return Some(matches[0]);
        }
        let expected = expected_kinds(field);
        let narrowed = matches
            .iter()
            .copied()
            .filter(|&i| expected.contains(&self.artifacts[i].kind.as_str()))
            .collect::<Vec<_>>();
        if narrowed.len() == 1 {
            return Some(narrowed[0]);
        }
        let local = matches
            .iter()
            .copied()
            .filter(|&i| self.artifacts[i].module_name == self.artifacts[source].module_name)
            .collect::<Vec<_>>();
        (local.len() == 1).then(|| local[0])
    }
}

fn truthy(value: &Bson) -> bool {
    !matches!(value, Bson::Null | Bson::Boolean(false))
}
fn field<'a>(doc: &'a Document, keys: &[&str]) -> Option<&'a Bson> {
    keys.iter()
        .find_map(|key| doc.get(key).filter(|value| truthy(value)))
}
fn array_items(value: Option<&Bson>) -> Vec<Bson> {
    mxrs_bson::parse_array(value.and_then(Bson::as_array).map(Vec::as_slice)).items
}
fn scalar_string(value: &Bson) -> String {
    match value {
        Bson::String(text) => text.clone(),
        Bson::Null => String::new(),
        _ => value.to_string(),
    }
}
fn kind_for(ty: &str) -> String {
    let suffix = ty.split_once('$').map_or(ty, |(_, suffix)| suffix);
    if suffix == "MenuDocument" {
        return "menu".into();
    }
    if suffix == "JavaScriptAction" {
        return "javascript_action".into();
    }
    let mut result = String::new();
    let mut previous = None;
    for ch in suffix.chars() {
        if ch.is_ascii_uppercase()
            && previous.is_some_and(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            result.push('_');
        }
        result.extend(ch.to_lowercase());
        previous = Some(ch);
    }
    result
}
fn candidates(value: &str, module: Option<&str>) -> Vec<String> {
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    static MEMBER: OnceLock<Regex> = OnceLock::new();
    let token = TOKEN.get_or_init(|| {
        Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*){1,2}").unwrap()
    });
    let member = MEMBER.get_or_init(|| {
        Regex::new(r"[A-Za-z_][A-Za-z0-9_]*\.[A-Za-z_][A-Za-z0-9_]*/[A-Za-z_][A-Za-z0-9_]*")
            .unwrap()
    });
    let mut result = vec![value.to_string()];
    if let Some(module) = module
        && !value.contains('.')
    {
        result.push(format!("{module}.{value}"));
    }
    result.extend(
        token
            .find_iter(value)
            .map(|found| found.as_str().to_string()),
    );
    result.extend(
        member
            .find_iter(value)
            .map(|found| found.as_str().replace('/', ".")),
    );
    let mut seen = HashSet::new();
    result.retain(|value| seen.insert(value.clone()));
    result
}
fn normalized_field(field: &str) -> String {
    field
        .chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}
fn expected_kinds(field: &str) -> &'static [&'static str] {
    match normalized_field(field).as_str() {
        "microflow" => &["microflow"],
        "nanoflow" => &["nanoflow"],
        "javaaction" => &["java_action"],
        "javascriptaction" => &["javascript_action"],
        "workflow" => &["workflow"],
        "form" | "page" => &["page", "layout", "snippet"],
        "layout" => &["layout"],
        "snippet" => &["snippet"],
        "entity" => &["entity"],
        "attribute" => &["attribute"],
        "association" => &["association"],
        "enumeration" => &["enumeration"],
        _ => &[],
    }
}
fn relation_for(field: &str, kind: &str) -> String {
    match normalized_field(field).as_str() {
        "microflow" | "nanoflow" | "javaaction" | "javascriptaction" | "workflow" => "calls".into(),
        "form" | "page" => "opens".into(),
        "layout" => "uses_layout".into(),
        "snippet" => "uses_snippet".into(),
        "entity" => "uses_entity".into(),
        "attribute" => "uses_attribute".into(),
        "association" => "uses_association".into(),
        "enumeration" => "uses_enumeration".into(),
        _ if matches!(
            kind,
            "microflow" | "nanoflow" | "java_action" | "javascript_action" | "workflow"
        ) =>
        {
            "references".into()
        }
        _ => format!("uses_{kind}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Project.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Record", |entity| {
                entity.string("Name");
            });
            module.microflow("Target", |_| {});
            module.page("Shared", |_| {});
            module.microflow("Shared", |_| {});
        });
        mxrs_writer::write_project(&path, &builder.build()).unwrap();
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let parent = mpr.units_by_containment("Modules").unwrap()[0]
            .unit_id
            .clone();
        let id = mpr
            .insert_unit(
                &parent,
                "Documents",
                doc! {
                    "$Type": "Future$Document", "Name": "Future",
                    "Documentation": "Sales.Target", "Caption": "Sales.Target",
                    "Refs": [1, {"Microflow": "Sales.Target"}, {"Microflow": "Target"}],
                    "Expression": "$record/Sales.Record/Name + Sales.Target",
                    "Page": "Sales.Shared", "Microflow": "Sales.Shared",
                    "Lowercase": "sales.Target",
                },
                None,
            )
            .unwrap();
        drop(mpr);
        (directory, path, id)
    }

    #[test]
    fn unknown_documents_keep_distinct_occurrences_and_expression_member_paths() {
        let (_directory, path, id) = fixture();
        let before = std::fs::read(&path).unwrap();
        let project = Project::open(&path, true).unwrap();
        let index = DocumentIndex::build(&project).unwrap();
        let source = index.require("Sales.Future").unwrap();
        assert_eq!(source.id, format!("unit:{id}"));
        assert_eq!(source.kind, "document");
        let incoming = index.incoming("Sales.Target").unwrap();
        assert_eq!(incoming.len(), 3);
        assert_eq!(incoming[0].path, ["Refs", "1", "Microflow"]);
        assert_eq!(incoming[1].path, ["Refs", "2", "Microflow"]);
        assert_eq!(incoming[2].path, ["Expression"]);
        assert_eq!(incoming[2].relation, "references");
        assert_eq!(index.calls("Sales.Target", true).unwrap().len(), 1);
        assert_eq!(
            index.incoming("Sales.Record.Name").unwrap()[0].path,
            ["Expression"]
        );
        assert!(index.references().iter().all(|r| r.relation != "contains"));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn queries_are_exact_and_ambiguous_fields_resolve_by_kind_without_guessing() {
        let (_directory, path, id) = fixture();
        let index = DocumentIndex::build(&Project::open(path, true).unwrap()).unwrap();
        for name in ["Target", "sales.Target", &id, &format!("unit:{id}")] {
            assert!(matches!(
                index.require(name),
                Err(SemanticError::Unknown(_))
            ));
        }
        assert!(matches!(
            index.require("Sales.Shared"),
            Err(SemanticError::Ambiguous(_))
        ));
        let outgoing = index.outgoing("Sales.Future").unwrap();
        let page = outgoing.iter().find(|r| r.path == ["Page"]).unwrap();
        let call = outgoing.iter().find(|r| r.path == ["Microflow"]).unwrap();
        assert_eq!(index.artifact(&page.to).kind, "page");
        assert_eq!(index.artifact(&call.to).kind, "microflow");
        assert_ne!(page.to, call.to);
    }

    #[test]
    fn impact_handles_cycles_and_preserves_discovery_order() {
        let (_directory, path, _) = fixture();
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let unit = mpr
            .all_units()
            .unwrap()
            .into_iter()
            .find(|unit| mpr.parse_contents(unit).unwrap().get_str("Name").ok() == Some("Target"))
            .unwrap();
        let mut doc = mpr.parse_contents(&unit).unwrap();
        doc.insert("Microflow", "Sales.Future");
        mpr.update_unit(&unit.unit_id, doc).unwrap();
        drop(mpr);
        let index = DocumentIndex::build(&Project::open(path, true).unwrap()).unwrap();
        assert_eq!(
            index
                .impact("Sales.Target")
                .unwrap()
                .iter()
                .map(|a| a.qualified_name.as_str())
                .collect::<Vec<_>>(),
            ["Sales.Future"]
        );
        assert_eq!(
            index
                .impact("Sales.Record.Name")
                .unwrap()
                .iter()
                .map(|a| a.qualified_name.as_str())
                .collect::<Vec<_>>(),
            ["Sales.Future", "Sales.Target"]
        );
    }
}
