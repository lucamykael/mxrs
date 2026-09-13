//! Semantic artifact and reference graph derived from an MPR.
//!
//! The index is deterministic and read-only. Exact references use native IDs
//! when available and canonical names otherwise; unsupported opaque values are
//! never guessed into dependencies.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mxrs_bson::{Bson, Document};
use mxrs_model::navigation::NavigationItem;
use mxrs_model::page::Widget;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum SemanticError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error("semantic index is ambiguous for {0}")]
    Ambiguous(String),
}

pub type Result<T> = std::result::Result<T, SemanticError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Module,
    Entity,
    Attribute,
    Association,
    Microflow,
    Nanoflow,
    Rule,
    Page,
    ModuleRole,
    NavigationProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    pub key: String,
    pub id: Option<String>,
    pub name: String,
    pub qualified_name: String,
    pub module: Option<String>,
    pub kind: ArtifactKind,
    pub documentation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub from: String,
    pub to: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchHit {
    pub artifact: Artifact,
    pub score: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticIndex {
    artifacts: BTreeMap<String, Artifact>,
    references: BTreeSet<Reference>,
    aliases: BTreeMap<String, BTreeSet<String>>,
    diagnostics: Vec<Diagnostic>,
    fingerprint: String,
}

impl SemanticIndex {
    pub fn build(project: &mxrs_model::Project) -> Result<Self> {
        let modules = project.modules()?;
        let mut builder = IndexBuilder::default();
        for module in &modules {
            let module_name = module.name.as_deref().unwrap_or("Unnamed");
            let module_key = builder.add(Artifact {
                key: format!("module:{module_name}"),
                id: Some(module.id.clone()),
                name: module_name.to_string(),
                qualified_name: module_name.to_string(),
                module: None,
                kind: ArtifactKind::Module,
                documentation: String::new(),
            });
            for entity in module.entities() {
                let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
                let qualified = entity
                    .qualified_name
                    .clone()
                    .unwrap_or_else(|| format!("{module_name}.{entity_name}"));
                let entity_key = builder.add(Artifact {
                    key: format!("entity:{qualified}"),
                    id: entity.id.clone(),
                    name: entity_name.to_string(),
                    qualified_name: qualified.clone(),
                    module: Some(module_name.to_string()),
                    kind: ArtifactKind::Entity,
                    documentation: entity.documentation.clone(),
                });
                builder.reference_keys(&module_key, &entity_key, "contains");
                for attribute in &entity.attributes {
                    let Some(name) = attribute.name.as_deref() else {
                        continue;
                    };
                    let key = builder.add(Artifact {
                        key: format!("attribute:{qualified}/{name}"),
                        id: attribute.id.clone(),
                        name: name.to_string(),
                        qualified_name: format!("{qualified}/{name}"),
                        module: Some(module_name.to_string()),
                        kind: ArtifactKind::Attribute,
                        documentation: attribute.documentation.clone(),
                    });
                    builder.reference_keys(&entity_key, &key, "contains");
                    if let Some(enumeration) = &attribute.enumeration {
                        builder.pending(key, enumeration.clone(), "enumeration");
                    }
                }
                for callback in &entity.lifecycle {
                    builder.pending(entity_key.clone(), callback.handler.clone(), "lifecycle");
                }
            }
            for association in module.associations() {
                let name = association.name.as_deref().unwrap_or("Unnamed");
                let qualified = if name.contains('.') {
                    name.to_string()
                } else {
                    format!("{module_name}.{name}")
                };
                let key = builder.add(Artifact {
                    key: format!("association:{qualified}"),
                    id: association.id.clone(),
                    name: name.to_string(),
                    qualified_name: qualified,
                    module: Some(module_name.to_string()),
                    kind: ArtifactKind::Association,
                    documentation: association.documentation.clone(),
                });
                builder.reference_keys(&module_key, &key, "contains");
                if let Some(from) = &association.from_entity_id {
                    builder.pending(key.clone(), from.clone(), "from_entity");
                }
                if let Some(to) = &association.to_entity_id {
                    builder.pending(key, to.clone(), "to_entity");
                }
            }
            add_flows(
                &mut builder,
                &module.microflows,
                module_name,
                ArtifactKind::Microflow,
                &module_key,
            );
            add_flows(
                &mut builder,
                &module.nanoflows,
                module_name,
                ArtifactKind::Nanoflow,
                &module_key,
            );
            add_flows(
                &mut builder,
                &module.rules,
                module_name,
                ArtifactKind::Rule,
                &module_key,
            );
            for page in &module.pages {
                let Some(name) = page.name.as_deref() else {
                    continue;
                };
                let qualified = format!("{module_name}.{name}");
                let key = builder.add(Artifact {
                    key: format!("page:{qualified}"),
                    id: page.id.clone(),
                    name: name.to_string(),
                    qualified_name: qualified,
                    module: Some(module_name.to_string()),
                    kind: ArtifactKind::Page,
                    documentation: page.documentation.clone(),
                });
                builder.reference_keys(&module_key, &key, "contains");
                if let Some(layout) = &page.layout_id {
                    builder.pending(key.clone(), layout.clone(), "layout");
                }
                if let Some(source) = &page.data_source {
                    builder.document_references(&key, source, "data_source");
                }
                for widget in &page.widgets {
                    builder.widget_references(&key, widget);
                }
            }
            for role in &module.module_roles {
                let Some(name) = role.name.as_deref() else {
                    continue;
                };
                let qualified = format!("{module_name}.{name}");
                let key = builder.add(Artifact {
                    key: format!("module_role:{qualified}"),
                    id: role.id.clone(),
                    name: name.to_string(),
                    qualified_name: qualified,
                    module: Some(module_name.to_string()),
                    kind: ArtifactKind::ModuleRole,
                    documentation: role.description.clone(),
                });
                builder.reference_keys(&module_key, &key, "contains");
            }
        }
        let navigation = project.navigation()?;
        for profile in &navigation.profiles {
            builder.add(Artifact {
                key: format!("navigation:{}", profile.name),
                id: None,
                name: profile.name.clone(),
                qualified_name: profile.name.clone(),
                module: None,
                kind: ArtifactKind::NavigationProfile,
                documentation: String::new(),
            });
            for target in [
                profile
                    .home_page
                    .as_ref()
                    .map(|value| (value, "navigation_home")),
                profile
                    .home_microflow
                    .as_ref()
                    .map(|value| (value, "navigation_home")),
                profile
                    .sign_in_page
                    .as_ref()
                    .map(|value| (value, "sign_in")),
            ]
            .into_iter()
            .flatten()
            {
                builder.navigation_reference(&profile.name, target.0, target.1);
            }
            for item in &profile.menu_items {
                builder.navigation_item(&profile.name, item);
            }
        }
        builder.finish()
    }

    pub fn artifacts(&self) -> impl Iterator<Item = &Artifact> {
        self.artifacts.values()
    }

    pub fn references(&self) -> impl Iterator<Item = &Reference> {
        self.references.iter()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn resolve(&self, name_or_id: &str) -> Result<Option<&Artifact>> {
        if let Some(artifact) = self.artifacts.get(name_or_id) {
            return Ok(Some(artifact));
        }
        let Some(keys) = self.aliases.get(&name_or_id.to_ascii_lowercase()) else {
            return Ok(None);
        };
        if keys.len() > 1 {
            return Err(SemanticError::Ambiguous(name_or_id.to_string()));
        }
        Ok(keys.iter().next().and_then(|key| self.artifacts.get(key)))
    }

    pub fn incoming(&self, name_or_id: &str) -> Result<Vec<&Reference>> {
        let Some(artifact) = self.resolve(name_or_id)? else {
            return Ok(vec![]);
        };
        Ok(self
            .references
            .iter()
            .filter(|reference| reference.to == artifact.key)
            .collect())
    }

    pub fn outgoing(&self, name_or_id: &str) -> Result<Vec<&Reference>> {
        let Some(artifact) = self.resolve(name_or_id)? else {
            return Ok(vec![]);
        };
        Ok(self
            .references
            .iter()
            .filter(|reference| reference.from == artifact.key)
            .collect())
    }

    pub fn impact(&self, name_or_id: &str) -> Result<Vec<&Artifact>> {
        let Some(root) = self.resolve(name_or_id)? else {
            return Ok(vec![]);
        };
        let mut visited = BTreeSet::from([root.key.clone()]);
        let mut queue = VecDeque::from([root.key.clone()]);
        while let Some(target) = queue.pop_front() {
            for reference in self
                .references
                .iter()
                .filter(|reference| reference.to == target)
            {
                if visited.insert(reference.from.clone()) {
                    queue.push_back(reference.from.clone());
                }
            }
        }
        visited.remove(&root.key);
        Ok(visited
            .iter()
            .filter_map(|key| self.artifacts.get(key))
            .collect())
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let terms = query
            .split(|character: char| !character.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let mut hits = self
            .artifacts
            .values()
            .filter_map(|artifact| {
                let qualified = artifact.qualified_name.to_ascii_lowercase();
                let documentation = artifact.documentation.to_ascii_lowercase();
                let score = terms
                    .iter()
                    .map(|term| {
                        if qualified == *term {
                            100
                        } else if qualified.contains(term) {
                            20
                        } else if documentation.contains(term) {
                            5
                        } else {
                            0
                        }
                    })
                    .sum();
                (score > 0).then(|| SearchHit {
                    artifact: artifact.clone(),
                    score,
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.artifact.key.cmp(&right.artifact.key))
        });
        hits.truncate(limit);
        hits
    }
}

#[derive(Default)]
struct IndexBuilder {
    artifacts: BTreeMap<String, Artifact>,
    aliases: BTreeMap<String, BTreeSet<String>>,
    references: BTreeSet<Reference>,
    pending: Vec<(String, String, String)>,
    diagnostics: Vec<Diagnostic>,
}

impl IndexBuilder {
    fn add(&mut self, artifact: Artifact) -> String {
        let key = artifact.key.clone();
        if self.artifacts.contains_key(&key) {
            self.diagnostics.push(Diagnostic {
                severity: "error".to_string(),
                code: "duplicate_artifact".to_string(),
                message: format!("duplicate semantic artifact {key}"),
            });
            return key;
        }
        for alias in [
            Some(artifact.key.as_str()),
            Some(artifact.name.as_str()),
            Some(artifact.qualified_name.as_str()),
            artifact.id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            self.aliases
                .entry(alias.to_ascii_lowercase())
                .or_default()
                .insert(key.clone());
        }
        self.artifacts.insert(key.clone(), artifact);
        key
    }

    fn pending(&mut self, from: String, target: String, relation: &str) {
        if !target.is_empty() {
            self.pending.push((from, target, relation.to_string()));
        }
    }

    fn reference_keys(&mut self, from: &str, to: &str, relation: &str) {
        self.references.insert(Reference {
            from: from.to_string(),
            to: to.to_string(),
            relation: relation.to_string(),
        });
    }

    fn document_references(&mut self, from: &str, document: &Document, relation: &str) {
        for value in string_values(&Bson::Document(document.clone())) {
            self.pending(from.to_string(), value, relation);
        }
    }

    fn widget_references(&mut self, from: &str, widget: &Widget) {
        self.document_references(from, &widget.options, "widget");
        for event in &widget.events {
            self.document_references(from, event, "widget_event");
        }
        for child in &widget.children {
            self.widget_references(from, child);
        }
    }

    fn navigation_reference(&mut self, profile: &str, target: &str, relation: &str) {
        self.pending(
            format!("navigation:{profile}"),
            target.to_string(),
            relation,
        );
    }

    fn navigation_item(&mut self, profile: &str, item: &NavigationItem) {
        if let Some(page) = &item.page {
            self.navigation_reference(profile, page, "navigation");
        }
        if let Some(flow) = &item.microflow {
            self.navigation_reference(profile, flow, "navigation");
        }
        for child in &item.items {
            self.navigation_item(profile, child);
        }
    }

    fn finish(mut self) -> Result<SemanticIndex> {
        for (from, target, relation) in self.pending {
            let Some(keys) = self.aliases.get(&target.to_ascii_lowercase()) else {
                continue;
            };
            if keys.len() == 1 {
                let to = keys.iter().next().expect("one key").clone();
                self.references.insert(Reference { from, to, relation });
            }
        }
        let mut digest = Sha256::new();
        digest.update(serde_json::to_vec(&self.artifacts).expect("artifact graph is serializable"));
        digest.update([0]);
        digest
            .update(serde_json::to_vec(&self.references).expect("reference graph is serializable"));
        let fingerprint = format!("{:x}", digest.finalize());
        Ok(SemanticIndex {
            artifacts: self.artifacts,
            references: self.references,
            aliases: self.aliases,
            diagnostics: self.diagnostics,
            fingerprint,
        })
    }
}

fn add_flows(
    builder: &mut IndexBuilder,
    flows: &[mxrs_model::Microflow],
    module: &str,
    kind: ArtifactKind,
    module_key: &str,
) {
    for flow in flows {
        let Some(name) = flow.name.as_deref() else {
            continue;
        };
        let qualified = format!("{module}.{name}");
        let key = builder.add(Artifact {
            key: format!(
                "{}:{qualified}",
                match kind {
                    ArtifactKind::Microflow => "microflow",
                    ArtifactKind::Nanoflow => "nanoflow",
                    ArtifactKind::Rule => "rule",
                    _ => "flow",
                }
            ),
            id: flow.id.clone(),
            name: name.to_string(),
            qualified_name: qualified,
            module: Some(module.to_string()),
            kind,
            documentation: flow.documentation.clone(),
        });
        builder.reference_keys(module_key, &key, "contains");
        for document in flow
            .objects
            .iter()
            .chain(&flow.flows)
            .chain(&flow.parameters)
        {
            builder.document_references(&key, document, "flow");
        }
    }
}

fn string_values(value: &Bson) -> Vec<String> {
    let mut result = Vec::new();
    collect_strings(value, &mut result);
    result
}

fn collect_strings(value: &Bson, result: &mut Vec<String>) {
    match value {
        Bson::String(value) if !value.is_empty() => result.push(value.clone()),
        Bson::Array(values) => values
            .iter()
            .for_each(|value| collect_strings(value, result)),
        Bson::Document(document) => document
            .values()
            .for_each(|value| collect_strings(value, result)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, mxrs_model::Project) {
        let directory = tempfile::tempdir().unwrap();
        let mpr = directory.path().join("Semantic.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
            module.microflow("ACT_Save", |_flow| {});
            module.page("Home", |page| {
                page.layout("Sales.Home", "Main");
                page.text("Orders");
            });
        });
        mxrs_writer::write_project(&mpr, &builder.build()).unwrap();
        let project = mxrs_model::Project::open(&mpr, true).unwrap();
        (directory, project)
    }

    #[test]
    fn indexes_resolves_and_searches_typed_artifacts() {
        let (_directory, project) = fixture();
        let index = SemanticIndex::build(&project).unwrap();
        assert_eq!(
            index.resolve("Sales.Order").unwrap().unwrap().kind,
            ArtifactKind::Entity
        );
        assert_eq!(
            index.resolve("Number").unwrap().unwrap().kind,
            ArtifactKind::Attribute
        );
        assert_eq!(index.search("home", 10)[0].qualified_name(), "Sales.Home");
        assert_eq!(index.fingerprint().len(), 64);
    }

    #[test]
    fn containment_edges_power_transitive_impact() {
        let (_directory, project) = fixture();
        let index = SemanticIndex::build(&project).unwrap();
        let impact = index.impact("Sales.Order/Number").unwrap();
        assert!(
            impact
                .iter()
                .any(|artifact| artifact.qualified_name == "Sales.Order")
        );
        assert!(
            impact
                .iter()
                .any(|artifact| artifact.qualified_name == "Sales")
        );
    }

    #[test]
    fn fingerprints_are_stable_for_the_same_model() {
        let (_directory, project) = fixture();
        let left = SemanticIndex::build(&project).unwrap();
        let right = SemanticIndex::build(&project).unwrap();
        assert_eq!(left.fingerprint(), right.fingerprint());
        assert_eq!(left.references().count(), right.references().count());
    }

    trait QualifiedName {
        fn qualified_name(&self) -> &str;
    }

    impl QualifiedName for SearchHit {
        fn qualified_name(&self) -> &str {
            &self.artifact.qualified_name
        }
    }
}
