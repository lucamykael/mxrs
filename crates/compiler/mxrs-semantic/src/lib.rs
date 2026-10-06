//! Semantic artifact and reference graph derived from an MPR.
//!
//! The index is deterministic and read-only. Exact references use native IDs
//! when available and canonical names otherwise; unsupported opaque values are
//! never guessed into dependencies.
//! Reference extraction covers explicit model reference fields, not names
//! embedded in expressions; lint reports this boundary instead of claiming a
//! full Studio Pro consistency check.
//! Native dotted and canonical slash-separated attribute names resolve to the
//! same artifact. Bare references prefer matching kinds in the source module;
//! context-bound members within that module still require a qualified name.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mxrs_bson::{Bson, Document};
use mxrs_model::navigation::NavigationItem;
use mxrs_model::page::Widget;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod analysis;
pub mod cache;
pub mod documents;
mod pages;
pub use analysis::{Analysis, CallCycle, ModuleDependency};

#[derive(Debug, thiserror::Error)]
pub enum SemanticError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error("semantic index is ambiguous for {0}")]
    Ambiguous(String),
    #[error("unknown Mendix artifact {0:?}")]
    Unknown(String),
    #[error("cannot access semantic cache {path}: {source}")]
    CacheIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid semantic cache {path}: {reason}")]
    InvalidCache { path: String, reason: String },
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
    Layout,
    Snippet,
    Enumeration,
    Constant,
    ScheduledEvent,
    Menu,
    JavaAction,
    JavaScriptAction,
    Workflow,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Entity => "entity",
            Self::Attribute => "attribute",
            Self::Association => "association",
            Self::Microflow => "microflow",
            Self::Nanoflow => "nanoflow",
            Self::Rule => "rule",
            Self::Page => "page",
            Self::ModuleRole => "module_role",
            Self::NavigationProfile => "navigation_profile",
            Self::Layout => "layout",
            Self::Snippet => "snippet",
            Self::Enumeration => "enumeration",
            Self::Constant => "constant",
            Self::ScheduledEvent => "scheduled_event",
            Self::Menu => "menu",
            Self::JavaAction => "java_action",
            Self::JavaScriptAction => "javascript_action",
            Self::Workflow => "workflow",
        }
    }
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

/// The codes of what a page or snippet names that it, or the page it
/// opens, does not declare: warnings, as a model may be imported so.
pub const PAGE_REFERENCE_CODES: &[&str] = &[
    "unknown_page_parameter",
    "unknown_page_variable",
    "unknown_widget",
    "attribute_outside_context",
];

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
    /// Reference targets that matched no artifact at all — the structured
    /// record behind every `unresolved_reference` diagnostic, kept as data
    /// (not message text) so tooling like the marketplace dependency
    /// resolver can read the missing qualified names. `default` keeps
    /// pre-existing caches deserializable; the cache format version was
    /// bumped so they are rebuilt rather than served with this empty.
    #[serde(default)]
    unresolved_targets: BTreeSet<String>,
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
                        builder.pending_typed(
                            key,
                            enumeration.clone(),
                            "enumeration",
                            ArtifactKind::Enumeration,
                        );
                    }
                }
                for callback in &entity.lifecycle {
                    builder.pending_typed(
                        entity_key.clone(),
                        callback.handler.clone(),
                        "lifecycle",
                        ArtifactKind::Microflow,
                    );
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
        add_named_documents(&mut builder, project, &modules)?;
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
                    .map(|value| (value, "navigation_home", ArtifactKind::Page)),
                profile
                    .home_microflow
                    .as_ref()
                    .map(|value| (value, "navigation_home", ArtifactKind::Microflow)),
                profile
                    .sign_in_page
                    .as_ref()
                    .map(|value| (value, "sign_in", ArtifactKind::Page)),
            ]
            .into_iter()
            .flatten()
            {
                builder.navigation_reference(&profile.name, target.0, target.1, target.2);
            }
            for home in &profile.role_homes {
                if let Some(page) = &home.page {
                    builder.navigation_reference(
                        &profile.name,
                        page,
                        "role_home",
                        ArtifactKind::Page,
                    );
                }
                if let Some(flow) = &home.microflow {
                    builder.navigation_reference(
                        &profile.name,
                        flow,
                        "role_home",
                        ArtifactKind::Microflow,
                    );
                }
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

    /// Qualified names that matched no artifact at all, sorted — the data
    /// behind every `unresolved_reference` diagnostic.
    pub fn unresolved_reference_targets(&self) -> &BTreeSet<String> {
        &self.unresolved_targets
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
        let artifact = self.require(name_or_id)?;
        Ok(self
            .references
            .iter()
            .filter(|reference| reference.to == artifact.key)
            .collect())
    }

    pub fn outgoing(&self, name_or_id: &str) -> Result<Vec<&Reference>> {
        let artifact = self.require(name_or_id)?;
        Ok(self
            .references
            .iter()
            .filter(|reference| reference.from == artifact.key)
            .collect())
    }

    pub fn impact(&self, name_or_id: &str) -> Result<Vec<&Artifact>> {
        let root = self.require(name_or_id)?;
        // Build reverse adjacency once; rescanning every edge for each reached
        // artifact makes long dependency chains quadratic.
        let mut incoming: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for reference in &self.references {
            incoming
                .entry(&reference.to)
                .or_default()
                .push(&reference.from);
        }
        let mut visited = BTreeSet::from([root.key.as_str()]);
        let mut queue = VecDeque::from([root.key.as_str()]);
        while let Some(target) = queue.pop_front() {
            for &source in incoming.get(target).into_iter().flatten() {
                if visited.insert(source) {
                    queue.push_back(source);
                }
            }
        }
        visited.remove(root.key.as_str());
        Ok(visited
            .iter()
            .filter_map(|key| self.artifacts.get(*key))
            .collect())
    }

    pub fn require(&self, name_or_id: &str) -> Result<&Artifact> {
        self.resolve(name_or_id)?
            .ok_or_else(|| SemanticError::Unknown(name_or_id.to_string()))
    }

    pub fn callers(&self, name_or_id: &str) -> Result<Vec<&Artifact>> {
        self.call_neighbors(name_or_id, true)
    }

    pub fn callees(&self, name_or_id: &str) -> Result<Vec<&Artifact>> {
        self.call_neighbors(name_or_id, false)
    }

    fn call_neighbors(&self, name_or_id: &str, incoming: bool) -> Result<Vec<&Artifact>> {
        let root = self.require(name_or_id)?;
        let keys: BTreeSet<_> = self
            .references
            .iter()
            .filter_map(|reference| {
                if reference.relation != "calls" {
                    return None;
                }
                if incoming && reference.to == root.key {
                    Some(&reference.from)
                } else if !incoming && reference.from == root.key {
                    Some(&reference.to)
                } else {
                    None
                }
            })
            .collect();
        let mut artifacts: Vec<_> = keys
            .into_iter()
            .filter_map(|key| self.artifacts.get(key))
            .collect();
        artifacts.sort_by(|left, right| {
            left.qualified_name
                .cmp(&right.qualified_name)
                .then(left.key.cmp(&right.key))
        });
        Ok(artifacts)
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
    pending: Vec<(String, String, String, Option<ExpectedKind>)>,
    diagnostics: Vec<Diagnostic>,
    unresolved_targets: BTreeSet<String>,
}

#[derive(Clone, Copy)]
enum ExpectedKind {
    Exact(ArtifactKind),
    Form,
}

impl ExpectedKind {
    fn accepts(self, actual: ArtifactKind) -> bool {
        match self {
            Self::Exact(kind) => actual == kind,
            Self::Form => matches!(
                actual,
                ArtifactKind::Page | ArtifactKind::Layout | ArtifactKind::Snippet
            ),
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Exact(kind) => kind.as_str(),
            Self::Form => "page, layout or snippet",
        }
    }
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
        if artifact.kind == ArtifactKind::Attribute {
            // Mendix writes members as Module.Entity.Attribute; keep the
            // public canonical Module.Entity/Attribute name and accept both.
            self.aliases
                .entry(
                    artifact
                        .qualified_name
                        .replace('/', ".")
                        .to_ascii_lowercase(),
                )
                .or_default()
                .insert(key.clone());
        }
        self.artifacts.insert(key.clone(), artifact);
        key
    }

    fn pending(&mut self, from: String, target: String, relation: &str) {
        if !target.is_empty() {
            self.pending
                .push((from, target, relation.to_string(), None));
        }
    }

    fn pending_typed(
        &mut self,
        from: String,
        target: String,
        relation: &str,
        expected: ArtifactKind,
    ) {
        if !target.is_empty() {
            self.pending.push((
                from,
                target,
                relation.to_string(),
                Some(ExpectedKind::Exact(expected)),
            ));
        }
    }

    fn reference_keys(&mut self, from: &str, to: &str, relation: &str) {
        self.references.insert(Reference {
            from: from.to_string(),
            to: to.to_string(),
            relation: relation.to_string(),
        });
    }

    fn document_references(&mut self, from: &str, document: &Document, _relation: &str) {
        for (field, value) in document {
            if matches!(
                field.as_str(),
                "$Type" | "$ID" | "Documentation" | "Caption" | "Text" | "Name" | "name"
            ) {
                continue;
            }
            self.reference_value(from, field, value);
        }
    }

    fn reference_value(&mut self, from: &str, field: &str, value: &Bson) {
        match value {
            Bson::Document(document) => self.document_references(from, document, "references"),
            Bson::Array(values) => {
                for value in values {
                    self.reference_value(from, field, value);
                }
            }
            Bson::String(target) if !target.is_empty() => {
                if let Some((kind, relation)) = reference_field(field) {
                    self.pending.push((
                        from.to_string(),
                        target.clone(),
                        relation.to_string(),
                        Some(if kind == ArtifactKind::Page {
                            ExpectedKind::Form
                        } else {
                            ExpectedKind::Exact(kind)
                        }),
                    ));
                }
            }
            _ => {}
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

    fn navigation_reference(
        &mut self,
        profile: &str,
        target: &str,
        relation: &str,
        expected: ArtifactKind,
    ) {
        self.pending_typed(
            format!("navigation:{profile}"),
            target.to_string(),
            relation,
            expected,
        );
    }

    fn navigation_item(&mut self, profile: &str, item: &NavigationItem) {
        if let Some(page) = &item.page {
            self.navigation_reference(profile, page, "navigation", ArtifactKind::Page);
        }
        if let Some(flow) = &item.microflow {
            self.navigation_reference(profile, flow, "navigation", ArtifactKind::Microflow);
        }
        for child in &item.items {
            self.navigation_item(profile, child);
        }
    }

    fn finish(mut self) -> Result<SemanticIndex> {
        for (from, target, relation, expected) in self.pending {
            let mut keys: BTreeSet<_> = self
                .aliases
                .get(&target.to_ascii_lowercase())
                .into_iter()
                .flatten()
                .filter(|key| expected.is_none_or(|kind| kind.accepts(self.artifacts[*key].kind)))
                .collect();
            // Native module-local references need the source context, unlike
            // public name lookup. Filter kinds before narrowing to the module
            // so a same-named local page cannot capture a microflow reference.
            if !target.contains('.')
                && let Some(module) = self
                    .artifacts
                    .get(&from)
                    .and_then(|artifact| artifact.module.as_deref())
            {
                let local: BTreeSet<_> = keys
                    .iter()
                    .copied()
                    .filter(|key| self.artifacts[*key].module.as_deref() == Some(module))
                    .collect();
                if !local.is_empty() {
                    keys = local;
                }
            }
            if keys.len() == 1 {
                let to = (*keys.iter().next().expect("one key")).clone();
                self.references.insert(Reference { from, to, relation });
            } else if expected.is_some() && !target.starts_with("System.") {
                if keys.is_empty() {
                    self.unresolved_targets.insert(target.clone());
                }
                self.diagnostics.push(Diagnostic {
                    severity: "error".into(),
                    code: if keys.is_empty() {
                        "unresolved_reference"
                    } else {
                        "ambiguous_reference"
                    }
                    .into(),
                    message: format!(
                        "{from} {relation} {target:?}: expected {}",
                        expected.expect("checked").description()
                    ),
                });
            }
        }
        self.diagnostics
            .sort_by(|left, right| (&left.code, &left.message).cmp(&(&right.code, &right.message)));
        self.diagnostics.dedup();
        let mut digest = Sha256::new();
        digest.update(serde_json::to_vec(&self.artifacts).expect("artifact graph is serializable"));
        digest.update([0]);
        digest
            .update(serde_json::to_vec(&self.references).expect("reference graph is serializable"));
        digest.update(serde_json::to_vec(&self.diagnostics).expect("diagnostics are serializable"));
        let fingerprint = format!("{:x}", digest.finalize());
        Ok(SemanticIndex {
            artifacts: self.artifacts,
            references: self.references,
            aliases: self.aliases,
            diagnostics: self.diagnostics,
            unresolved_targets: self.unresolved_targets,
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

// The explicit field contract follows mxrb semantic/index.rb FIELD_RELATIONS
// and expected_kinds; free text is deliberately not treated as a reference.
fn reference_field(field: &str) -> Option<(ArtifactKind, &'static str)> {
    use ArtifactKind::*;
    Some(match field.to_ascii_lowercase().as_str() {
        "microflow" => (Microflow, "calls"),
        "nanoflow" => (Nanoflow, "calls"),
        "javaaction" => (JavaAction, "calls"),
        "javascriptaction" => (JavaScriptAction, "calls"),
        "workflow" => (Workflow, "calls"),
        "page" | "form" => (Page, "opens"),
        "layout" => (Layout, "uses_layout"),
        "snippet" => (Snippet, "uses_snippet"),
        "entity" => (Entity, "uses_entity"),
        "attribute" => (Attribute, "uses_attribute"),
        "association" => (Association, "uses_association"),
        "enumeration" => (Enumeration, "uses_enumeration"),
        "allowedmoduleroles" | "allowedroles" => (ModuleRole, "allowed_role"),
        _ => return None,
    })
}

fn add_named_documents(
    builder: &mut IndexBuilder,
    project: &mxrs_model::Project,
    modules: &[mxrs_model::Module],
) -> Result<()> {
    let units = project.all_units()?;
    let by_id: BTreeMap<_, _> = units
        .iter()
        .map(|unit| (unit.unit_id.as_str(), unit))
        .collect();
    let module_names: BTreeMap<_, _> = modules
        .iter()
        .filter_map(|module| Some((module.id.as_str(), module.name.as_deref()?)))
        .collect();
    // The pages and snippets, by qualified name, for what they name of
    // themselves and of each other once all are known.
    let mut forms: Vec<(String, &'static str, Document)> = Vec::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        if let Some(keys) = builder.aliases.get(&unit.unit_id.to_ascii_lowercase()) {
            let keys: Vec<_> = keys.iter().cloned().collect();
            for key in keys {
                builder.document_references(&key, &document, "references");
                if document.get_str("$Type").ok() == Some("Forms$Page")
                    && let Some(qualified) = key.strip_prefix("page:")
                {
                    forms.push((qualified.to_string(), "page", document.clone()));
                }
            }
            continue;
        }
        let kind = match document.get_str("$Type").unwrap_or_default() {
            "Forms$Layout" => ArtifactKind::Layout,
            "Forms$Snippet" => ArtifactKind::Snippet,
            "Enumerations$Enumeration" => ArtifactKind::Enumeration,
            "Constants$Constant" => ArtifactKind::Constant,
            "ScheduledEvents$ScheduledEvent" => ArtifactKind::ScheduledEvent,
            "Menus$MenuDocument" => ArtifactKind::Menu,
            "JavaActions$JavaAction" => ArtifactKind::JavaAction,
            "JavaScriptActions$JavaScriptAction" => ArtifactKind::JavaScriptAction,
            "Workflows$Workflow" => ArtifactKind::Workflow,
            _ => continue,
        };
        let Ok(name) = document.get_str("Name") else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let mut ancestor = unit.container_id.as_str();
        let mut visited = BTreeSet::new();
        let module = loop {
            if let Some(name) = module_names.get(ancestor) {
                break Some(*name);
            }
            if !visited.insert(ancestor) {
                break None;
            }
            let Some(parent) = by_id.get(ancestor) else {
                break None;
            };
            ancestor = &parent.container_id;
        };
        let qualified =
            module.map_or_else(|| name.to_string(), |module| format!("{module}.{name}"));
        let key = builder.add(Artifact {
            key: format!("{}:{qualified}", kind.as_str()),
            id: Some(unit.unit_id.clone()),
            name: name.to_string(),
            qualified_name: qualified,
            module: module.map(str::to_string),
            kind,
            documentation: document
                .get_str("Documentation")
                .unwrap_or_default()
                .to_string(),
        });
        if let Some(module) = module {
            builder.reference_keys(&format!("module:{module}"), &key, "contains");
        }
        builder.document_references(&key, &document, "references");
        if kind == ArtifactKind::Snippet
            && let Some(qualified) = key.strip_prefix("snippet:")
        {
            forms.push((qualified.to_string(), "snippet", document));
        }
    }
    let shapes: BTreeMap<String, pages::FormShape> = forms
        .iter()
        .map(|(qualified, kind, document)| {
            (qualified.clone(), pages::FormShape::of(kind, document))
        })
        .collect();
    let generalizations: BTreeMap<String, String> = modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.clone().unwrap_or_default();
            module.entities().iter().filter_map(move |entity| {
                let qualified = entity.qualified_name.clone().unwrap_or_else(|| {
                    format!(
                        "{module_name}.{}",
                        entity.name.as_deref().unwrap_or_default()
                    )
                });
                Some((qualified, entity.generalization.as_ref()?.target.clone()?))
            })
        })
        .collect();
    for (qualified, _, document) in &forms {
        builder
            .diagnostics
            .extend(pages::check(qualified, document, &shapes, &generalizations));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_create_and_change_members_resolve_native_dotted_attribute_names() {
        use mxrs_ir::flow::{Activity, Member};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("MemberReferences.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        for name in ["Sales", "Other"] {
            builder.module(name, |module| {
                module.entity("Order", |entity| {
                    entity.string("Number");
                    entity.string("Description");
                });
                module.microflow("ACT_Create", |_| {});
            });
        }
        let mut declaration = builder.build();
        declaration.modules[0].microflows[0].activities = vec![
            Activity::CreateObject {
                variable: "order".into(),
                entity: "Sales.Order".into(),
                members: vec![Member::attribute("Number", "'A-1'")],
                commit: false,
            },
            Activity::ChangeObject {
                variable: "order".into(),
                entity: "Sales.Order".into(),
                members: vec![Member::attribute("Sales.Order/Description", "'Updated'")],
                commit: false,
            },
        ];
        mxrs_writer::write_project(&path, &declaration).unwrap();
        let project = mxrs_model::Project::open(&path, true).unwrap();
        let modules = project.modules().unwrap();
        let sales = modules
            .iter()
            .find(|module| module.name.as_deref() == Some("Sales"))
            .unwrap();
        let members: Vec<_> = sales.microflows[0]
            .objects
            .iter()
            .filter_map(|object| object.get_document("Action").ok())
            .filter_map(|action| action.get_array("Items").ok())
            .flat_map(|items| items.iter())
            .filter_map(Bson::as_document)
            .map(|item| item.get_str("Attribute").unwrap())
            .collect();
        assert_eq!(members, ["Sales.Order.Number", "Sales.Order.Description"]);
        let index = SemanticIndex::build(&project).unwrap();
        assert!(index.analyze().valid(), "{:?}", index.diagnostics());
        for member in ["Number", "Description"] {
            let canonical = format!("Sales.Order/{member}");
            let native = format!("Sales.Order.{member}");
            let artifact = index.require(&canonical).unwrap();
            assert_eq!(artifact, index.require(&native).unwrap());
            assert_eq!(
                artifact,
                index.require(&native.to_ascii_lowercase()).unwrap()
            );
            assert!(index.references().any(|reference| {
                reference.from == "microflow:Sales.ACT_Create"
                    && reference.relation == "uses_attribute"
                    && reference.to == artifact.key
            }));
        }
        assert!(matches!(
            index.resolve("Number"),
            Err(SemanticError::Ambiguous(_))
        ));
    }

    #[test]
    fn native_bare_calls_prefer_source_module_after_kind_filtering() {
        use mxrs_ir::flow::Activity;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ModuleLocalReferences.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        for name in ["Sales", "Other"] {
            builder.module(name, |module| {
                module.microflow("ACT_Caller", |_| {});
                module.microflow("ACT_Save", |_| {});
                if name == "Sales" {
                    module.layout("ApplicationLayout", |layout| {
                        layout.placeholder("Main");
                    });
                    module.page("RemoteOnly", |page| {
                        page.layout("Sales.ApplicationLayout", "Main");
                    });
                } else {
                    module.microflow("RemoteOnly", |_| {});
                }
            });
        }
        let mut declaration = builder.build();
        mxrs_writer::write_project(&path, &declaration).unwrap();
        // Bare/case-insensitive native references are intentionally constructed
        // below the typed authoring boundary to exercise the native index.
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        for module in &mut declaration.modules {
            let targets: &[&str] = if module.name == "Sales" {
                &["act_save", "Other.ACT_Save", "RemoteOnly"]
            } else {
                &["ACT_Save", "Sales.ACT_Save"]
            };
            module.microflows[0].activities = targets
                .iter()
                .map(|target| Activity::CallMicroflow {
                    name: (*target).into(),
                    result_variable: None,
                    result_type: None,
                    use_return: false,
                    mappings: vec![],
                })
                .collect();
            let module_unit = mpr
                .units_by_containment("Modules")
                .unwrap()
                .into_iter()
                .find(|unit| {
                    mpr.parse_contents(unit).unwrap().get_str("Name").ok()
                        == Some(module.name.as_str())
                })
                .unwrap();
            let unit = mpr
                .children_of(&module_unit.unit_id)
                .unwrap()
                .into_iter()
                .find(|unit| {
                    mpr.parse_contents(unit).unwrap().get_str("Name").ok() == Some("ACT_Caller")
                })
                .unwrap();
            let mut document = mpr.parse_contents(&unit).unwrap();
            let (objects, flows) = mxrs_writer::flow_compiler::build_microflow_graph(
                &module.microflows[0].activities,
                &[],
                None,
            );
            document
                .get_document_mut("ObjectCollection")
                .unwrap()
                .insert(
                    "Objects",
                    mxrs_bson::build_array(
                        objects.into_iter().map(mxrs_bson::Bson::Document).collect(),
                        3,
                    ),
                );
            document.insert(
                "Flows",
                mxrs_bson::build_array(
                    flows.into_iter().map(mxrs_bson::Bson::Document).collect(),
                    3,
                ),
            );
            mpr.update_unit(&unit.unit_id, document).unwrap();
        }
        drop(mpr);
        let project = mxrs_model::Project::open(&path, true).unwrap();
        let index = SemanticIndex::build(&project).unwrap();
        assert!(index.analyze().valid(), "{:?}", index.diagnostics());
        let calls: BTreeSet<_> = index
            .references()
            .filter(|reference| reference.relation == "calls")
            .map(|reference| (reference.from.as_str(), reference.to.as_str()))
            .collect();
        assert_eq!(
            calls,
            BTreeSet::from([
                ("microflow:Sales.ACT_Caller", "microflow:Sales.ACT_Save"),
                ("microflow:Sales.ACT_Caller", "microflow:Other.ACT_Save"),
                ("microflow:Sales.ACT_Caller", "microflow:Other.RemoteOnly"),
                ("microflow:Other.ACT_Caller", "microflow:Other.ACT_Save"),
                ("microflow:Other.ACT_Caller", "microflow:Sales.ACT_Save"),
            ])
        );
        // No source exists for public lookup, so global ambiguity is retained.
        assert!(matches!(
            index.resolve("ACT_Save"),
            Err(SemanticError::Ambiguous(_))
        ));
    }

    fn explicit_reference_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ExplicitReferences.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.enumeration("State", "Sales.State");
            });
            module.enumeration("State", |enumeration| {
                enumeration.value("Open");
            });
            module.microflow("ACT_Home", |_| {});
            module.layout("ApplicationLayout", |layout| {
                layout.placeholder("Main");
            });
            module.page("Home", |page| {
                page.layout("Sales.ApplicationLayout", "Main")
                    .text("Welcome");
            });
        });
        builder.navigation(|navigation| {
            navigation.profile("Responsive", |profile| {
                profile
                    .home_page("Sales.Home")
                    .sign_in_page("Sales.Home")
                    .home_for_page("Administrator", "Sales.Home");
                profile.item("Home", |item| {
                    item.page("Sales.Home");
                    item.item("Action", |child| {
                        child.microflow("Sales.ACT_Home");
                    });
                });
            });
        });
        mxrs_writer::write_project(&path, &builder.build()).unwrap();
        (directory, path)
    }

    fn native_unit(mpr: &mxrs_mpr::MprFile, kind: &str) -> (String, Document) {
        mpr.all_units()
            .unwrap()
            .into_iter()
            .find_map(|unit| {
                let document = mpr.parse_contents(&unit).unwrap();
                (document.get_str("$Type").ok() == Some(kind)).then_some((unit.unit_id, document))
            })
            .unwrap()
    }

    #[test]
    fn missing_and_wrong_kind_navigation_targets_fail_lint_after_native_mpr_edits() {
        let (_directory, path) = explicit_reference_fixture();
        let project = mxrs_model::Project::open(&path, true).unwrap();
        assert!(SemanticIndex::build(&project).unwrap().analyze().valid());
        drop(project);
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let (id, original) = native_unit(&mpr, "Navigation$NavigationDocument");
        for case in 0..7 {
            let mut document = original.clone();
            let profile = document.get_array_mut("Profiles").unwrap()[1]
                .as_document_mut()
                .unwrap();
            let target = match case {
                0 => {
                    profile
                        .get_document_mut("HomePage")
                        .unwrap()
                        .insert("Page", "Sales.MissingHome");
                    "Sales.MissingHome"
                }
                1 => {
                    profile.insert("HomePage", mxrs_bson::doc! { "Microflow": "Sales.Home" });
                    "Sales.Home"
                }
                2 => {
                    profile
                        .get_document_mut("LoginPageSettings")
                        .unwrap()
                        .insert("Form", "Sales.MissingLogin");
                    "Sales.MissingLogin"
                }
                3 => {
                    profile
                        .get_document_mut("Menu")
                        .unwrap()
                        .get_array_mut("Items")
                        .unwrap()[1]
                        .as_document_mut()
                        .unwrap()
                        .get_document_mut("Action")
                        .unwrap()
                        .get_document_mut("FormSettings")
                        .unwrap()
                        .insert("Form", "Sales.MissingMenuPage");
                    "Sales.MissingMenuPage"
                }
                4 => {
                    profile
                        .get_document_mut("Menu")
                        .unwrap()
                        .get_array_mut("Items")
                        .unwrap()[1]
                        .as_document_mut()
                        .unwrap()
                        .get_array_mut("Items")
                        .unwrap()[1]
                        .as_document_mut()
                        .unwrap()
                        .get_document_mut("Action")
                        .unwrap()
                        .get_document_mut("MicroflowSettings")
                        .unwrap()
                        .insert("Microflow", "Sales.MissingMenuFlow");
                    "Sales.MissingMenuFlow"
                }
                5 => {
                    profile.get_array_mut("HomeItems").unwrap()[1]
                        .as_document_mut()
                        .unwrap()
                        .insert("Page", "Sales.MissingRoleHome");
                    "Sales.MissingRoleHome"
                }
                _ => {
                    profile
                        .get_document_mut("HomePage")
                        .unwrap()
                        .insert("Page", "Sales.ApplicationLayout");
                    "Sales.ApplicationLayout"
                }
            };
            mpr.update_unit(&id, document).unwrap();
            let project = mxrs_model::Project::open(&path, true).unwrap();
            let analysis = SemanticIndex::build(&project).unwrap().analyze();
            assert!(
                !analysis.valid(),
                "case {case} accepted invalid target {target}"
            );
            assert!(
                analysis
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "unresolved_reference"
                        && diagnostic.message.contains(target))
            );
        }
        let mut document = original;
        document.get_array_mut("Profiles").unwrap()[1]
            .as_document_mut()
            .unwrap()
            .get_document_mut("HomePage")
            .unwrap()
            .insert("Page", "System.PlatformHome");
        mpr.update_unit(&id, document).unwrap();
        let project = mxrs_model::Project::open(&path, true).unwrap();
        assert!(SemanticIndex::build(&project).unwrap().analyze().valid());
    }

    #[test]
    fn enumeration_and_lifecycle_references_require_their_actual_artifact_kinds() {
        let (_directory, path) = explicit_reference_fixture();
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let (id, mut domain) = native_unit(&mpr, "DomainModels$DomainModel");
        let entity = domain.get_array_mut("entities").unwrap()[1]
            .as_document_mut()
            .unwrap();
        entity.get_array_mut("attributes").unwrap()[1]
            .as_document_mut()
            .unwrap()
            .get_document_mut("type")
            .unwrap()
            .insert("enumeration", "Sales.ACT_Home");
        entity.insert(
            "eventHandlers",
            mxrs_bson::build_array(
                vec![Bson::Document(mxrs_bson::doc! {
                    "Moment": "Before", "Event": "Commit", "Microflow": "Sales.Order",
                })],
                3,
            ),
        );
        mpr.update_unit(&id, domain.clone()).unwrap();
        let project = mxrs_model::Project::open(&path, true).unwrap();
        let analysis = SemanticIndex::build(&project).unwrap().analyze();
        assert!(!analysis.valid());
        for (relation, target) in [
            ("enumeration", "Sales.ACT_Home"),
            ("lifecycle", "Sales.Order"),
        ] {
            assert!(
                analysis
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "unresolved_reference"
                        && diagnostic.message.contains(relation)
                        && diagnostic.message.contains(target))
            );
        }
        drop(project);
        let entity = domain.get_array_mut("entities").unwrap()[1]
            .as_document_mut()
            .unwrap();
        entity.get_array_mut("attributes").unwrap()[1]
            .as_document_mut()
            .unwrap()
            .get_document_mut("type")
            .unwrap()
            .insert("enumeration", "Sales.State");
        entity.get_array_mut("eventHandlers").unwrap()[1]
            .as_document_mut()
            .unwrap()
            .insert("Microflow", "Sales.ACT_Home");
        mpr.update_unit(&id, domain).unwrap();
        let project = mxrs_model::Project::open(&path, true).unwrap();
        let index = SemanticIndex::build(&project).unwrap();
        assert!(index.analyze().valid());
        assert!(
            index
                .references()
                .any(|reference| reference.relation == "enumeration"
                    && reference.to == "enumeration:Sales.State")
        );
        assert!(
            index
                .references()
                .any(|reference| reference.relation == "lifecycle"
                    && reference.to == "microflow:Sales.ACT_Home")
        );
    }

    fn artifact(key: &str, kind: ArtifactKind) -> Artifact {
        Artifact {
            key: key.to_string(),
            id: Some(format!("id-{key}")),
            name: "Shared".into(),
            qualified_name: format!("Sales.{key}"),
            module: Some("Sales".into()),
            kind,
            documentation: "order processing".into(),
        }
    }

    #[test]
    fn reference_fields_resolve_by_kind_and_ignore_metadata_and_arbitrary_text() {
        let mut builder = IndexBuilder::default();
        builder.add(artifact("source", ArtifactKind::Page));
        let cases = [
            ("Microflow", ArtifactKind::Microflow, "calls"),
            ("Nanoflow", ArtifactKind::Nanoflow, "calls"),
            ("JavaAction", ArtifactKind::JavaAction, "calls"),
            ("JavaScriptAction", ArtifactKind::JavaScriptAction, "calls"),
            ("Workflow", ArtifactKind::Workflow, "calls"),
            ("Form", ArtifactKind::Page, "opens"),
            ("Page", ArtifactKind::Page, "opens"),
            ("Layout", ArtifactKind::Layout, "uses_layout"),
            ("Snippet", ArtifactKind::Snippet, "uses_snippet"),
            ("Entity", ArtifactKind::Entity, "uses_entity"),
            ("Attribute", ArtifactKind::Attribute, "uses_attribute"),
            ("Association", ArtifactKind::Association, "uses_association"),
            ("Enumeration", ArtifactKind::Enumeration, "uses_enumeration"),
            (
                "AllowedModuleRoles",
                ArtifactKind::ModuleRole,
                "allowed_role",
            ),
        ];
        for (field, kind, _) in cases {
            let key = format!("target-{field}");
            builder.add(artifact(&key, kind));
            let document = mxrs_bson::doc! { "Nested": [10, { field: format!("Sales.{key}") }], "Caption": format!("Sales.{key}"), "UnknownField": format!("Sales.{key}") };
            builder.document_references("source", &document, "ignored");
        }
        builder.document_references("source", &mxrs_bson::doc! { "Entity": "", "Name": "Sales.source", "$ID": "id-source", "Documentation": "Sales.target-Microflow", "Microflow": null }, "ignored");
        let index = builder.finish().unwrap();
        assert!(index.diagnostics().is_empty());
        assert_eq!(index.references().count(), cases.len());
        for (field, _, relation) in cases {
            assert!(
                index
                    .references()
                    .any(|reference| reference.to == format!("target-{field}")
                        && reference.relation == relation)
            );
        }
        assert!(matches!(
            index.resolve("Shared"),
            Err(SemanticError::Ambiguous(_))
        ));
        assert!(index.incoming("missing").is_err());
        assert!(index.outgoing("missing").is_err());
        assert!(index.impact("missing").is_err());
    }

    #[test]
    fn homonyms_are_narrowed_by_reference_kind_and_missing_targets_are_diagnostics() {
        let mut builder = IndexBuilder::default();
        builder.add(artifact("flow", ArtifactKind::Microflow));
        builder.add(artifact("entity", ArtifactKind::Entity));
        builder.document_references("flow", &mxrs_bson::doc! { "Microflow": "Shared", "Entity": "Sales.Missing", "Nanoflow": "System.BuiltIn" }, "flow");
        let index = builder.finish().unwrap();
        assert_eq!(index.callees("flow").unwrap()[0].key, "flow");
        assert_eq!(index.diagnostics().len(), 1);
        assert_eq!(index.diagnostics()[0].code, "unresolved_reference");
        assert!(!index.analyze().valid());

        // A page's allowed roles are module roles of the model.
        let mut builder = IndexBuilder::default();
        builder.add(artifact("page", ArtifactKind::Page));
        builder.document_references(
            "page",
            &mxrs_bson::doc! { "AllowedRoles": ["Sales.Ghost"] },
            "page",
        );
        let index = builder.finish().unwrap();
        assert_eq!(index.diagnostics()[0].code, "unresolved_reference");
        assert!(
            index.diagnostics()[0]
                .message
                .contains("allowed_role \"Sales.Ghost\"")
        );

        let mut builder = IndexBuilder::default();
        builder.add(artifact("first", ArtifactKind::Entity));
        builder.add(artifact("second", ArtifactKind::Entity));
        builder.document_references("first", &mxrs_bson::doc! { "Entity": "Shared" }, "ignored");
        let index = builder.finish().unwrap();
        assert_eq!(index.diagnostics()[0].code, "ambiguous_reference");
        assert_eq!(index.references().count(), 0);
    }

    #[test]
    fn duplicate_artifacts_and_search_order_are_deterministic() {
        let mut builder = IndexBuilder::default();
        builder.add(artifact("first", ArtifactKind::Entity));
        builder.add(artifact("first", ArtifactKind::Entity));
        builder.add(artifact("second", ArtifactKind::Microflow));
        builder.pending("first".into(), "".into(), "ignored");
        builder.pending("first".into(), "absent".into(), "ignored");
        builder.pending("first".into(), "Shared".into(), "ignored");
        let index = builder.finish().unwrap();
        assert_eq!(index.diagnostics()[0].code, "duplicate_artifact");
        assert!(index.search("", 10).is_empty());
        assert!(index.search("unknown", 10).is_empty());
        assert!(index.search("order", 0).is_empty());
        assert_eq!(index.search("order", 1)[0].score, 5);
        assert_eq!(index.search("first", 10)[0].artifact.key, "first");
        assert_eq!(
            index
                .search("Sales", 10)
                .iter()
                .map(|hit| hit.artifact.key.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(index.require("ID-FIRST").unwrap().key, "first");
        assert!(index.resolve("unknown").unwrap().is_none());
    }

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
