//! Design properties the project's theme renamed.
//!
//! Each `themesource/<module>/{web,native}/design-properties.json` lists the
//! design properties a theme offers, and an entry names what it was called
//! before in `oldNames` — on the property, on an option, or (for a
//! `Spacing` property) on each side of each size. A stored design property
//! under an old name takes the new one; a legacy spacing option
//! (`"Spacing bottom" = "Outer medium"`) becomes one side of the compound
//! spacing property, which is created when the widget has none yet. An old
//! name two entries claim differently is left alone, and a legacy spacing
//! option the compound already sets differently is a
//! `conflicting_design_property` issue.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mxrs_bson::{Bson, Document, doc};
use serde_json::Value;

use super::{IssueKind, UnitScope, tree};

/// The side of a compound spacing property a legacy option becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpacingTarget {
    pub(super) key: String,
    pub(super) child: String,
    pub(super) option: String,
}

/// The new name of a design property, and of its option when that was
/// renamed too (`Some(None)` clears the option).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AliasTarget {
    pub(super) key: String,
    pub(super) option: Option<Option<String>>,
}

/// An old `(key, option)` pair, the option absent for a renamed key alone.
type AliasSource = (String, Option<String>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Alias {
    Target(AliasTarget),
    Conflict,
}

/// The renames the project's theme declares, read on first use.
pub(super) struct DesignMappings {
    root: PathBuf,
    loaded: Option<Loaded>,
}

#[derive(Default)]
pub(super) struct Loaded {
    pub(super) spacing: HashMap<String, SpacingTarget>,
    pub(super) aliases: HashMap<AliasSource, AliasTarget>,
}

impl DesignMappings {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            loaded: None,
        }
    }

    /// Migrates the `DesignProperties` of `owner` in place.
    pub(super) fn migrate(&mut self, owner: &mut Document, path: &str, scope: &mut UnitScope<'_>) {
        let root = &self.root;
        let loaded = self.loaded.get_or_insert_with(|| Loaded::read(root));
        loaded.migrate(owner, path, scope);
    }
}

impl Loaded {
    /// Reads every theme catalog under `root`; one that is not JSON is
    /// skipped.
    fn read(root: &Path) -> Self {
        let mut loaded = Self::default();
        let mut aliases = HashMap::new();
        for document in catalogs(root) {
            let Value::Object(groups) = document else {
                continue;
            };
            for properties in groups.values() {
                let Value::Array(properties) = properties else {
                    continue;
                };
                for property in properties {
                    collect_spacing(property, &mut loaded.spacing);
                    collect_aliases(property, &mut aliases);
                }
            }
        }
        loaded.aliases = aliases
            .into_iter()
            .filter_map(|(source, alias)| match alias {
                Alias::Target(target) => Some((source, target)),
                Alias::Conflict => None,
            })
            .collect();
        loaded
    }

    pub(super) fn migrate(&self, owner: &mut Document, path: &str, scope: &mut UnitScope<'_>) {
        let raw = owner.get("DesignProperties");
        let raw_marker = tree::marker(raw);
        // Every design property this owner ends with, by position: its own
        // first, then the compounds created for it.
        let mut arena: Vec<Bson> = tree::items(raw).to_vec();
        if arena.is_empty() {
            return;
        }
        let mut changed = self.rename_aliases(&mut arena);
        let mut compounds: HashMap<String, usize> = arena
            .iter()
            .enumerate()
            .filter(|(_, item)| is_compound(item))
            .filter_map(|(index, item)| match tree::field(item, "Key") {
                Some(Bson::String(key)) => Some((key.clone(), index)),
                _ => None,
            })
            .collect();
        let mut retained: Vec<usize> = Vec::new();
        for index in 0..arena.len() {
            let Some(old_name) = option_name(&arena[index]) else {
                retained.push(index);
                continue;
            };
            let Some(target) = self.spacing.get(&old_name) else {
                retained.push(index);
                continue;
            };
            let compound = *compounds.entry(target.key.clone()).or_insert_with(|| {
                arena.push(Bson::Document(compound_property(&target.key)));
                arena.len() - 1
            });
            let properties =
                tree::items(tree::dig(Some(&arena[compound]), &["Value", "Properties"]));
            let existing = properties
                .iter()
                .find(|property| tree::is_str(tree::field(property, "Key"), &target.child));
            match existing {
                Some(existing)
                    if !tree::is_str(
                        tree::dig(Some(existing), &["Value", "Option"]),
                        &target.option,
                    ) =>
                {
                    scope.issue(
                        path,
                        IssueKind::ConflictingDesignProperty,
                        format!(
                            "{} conflicts with {}",
                            tree::inspect_str(&old_name),
                            tree::inspect_str(&target.child)
                        ),
                    );
                    retained.push(index);
                    continue;
                }
                Some(_) => {}
                None => {
                    let mut properties = properties.to_vec();
                    properties.push(Bson::Document(option_property(
                        &target.child,
                        &target.option,
                    )));
                    if let Some(Bson::Document(value)) = compound_value(&mut arena[compound]) {
                        value.insert("Properties", tree::array(properties, 2));
                    }
                }
            }
            if !retained
                .iter()
                .any(|kept| tree::same(&arena[*kept], &arena[compound]))
            {
                retained.push(compound);
            }
            changed += 1;
        }
        if changed == 0 {
            return;
        }
        let properties = retained.iter().map(|index| arena[*index].clone()).collect();
        owner.insert("DesignProperties", tree::array(properties, raw_marker));
        scope.counts.design_properties += changed;
    }

    /// Renames, in place, each design property whose key, or key and
    /// option, the theme renamed; returns how many it renamed.
    fn rename_aliases(&self, items: &mut [Bson]) -> usize {
        let mut renamed = 0;
        for item in items {
            let Bson::Document(item) = item else {
                continue;
            };
            let Some(Bson::String(key)) = item.get("Key") else {
                continue;
            };
            let option = match tree::dig(item.get("Value"), &["Option"]) {
                None | Some(Bson::Null) => None,
                Some(Bson::String(option)) => Some(option.clone()),
                Some(_) => continue,
            };
            let Some(target) = self.aliases.get(&(key.clone(), option)) else {
                continue;
            };
            item.insert("Key", target.key.clone());
            if let Some(option) = &target.option
                && let Ok(value) = item.get_document_mut("Value")
            {
                value.insert("Option", option.clone().map_or(Bson::Null, Bson::String));
            }
            renamed += 1;
        }
        renamed
    }
}

/// The theme catalogs, in path order.
fn catalogs(root: &Path) -> Vec<Value> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(root.join("themesource"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .flat_map(|entry| {
            ["web", "native"]
                .map(|platform| entry.path().join(platform).join("design-properties.json"))
        })
        .filter(|path| path.is_file())
        .collect();
    paths.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    paths
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|source| serde_json::from_str(&source).ok())
        .collect()
}

/// A JSON value as a list: a list itself, nothing for null, any other
/// scalar alone.
fn listed(value: Option<&Value>) -> Vec<&Value> {
    match value {
        None | Some(Value::Null) | Some(Value::Object(_)) => Vec::new(),
        Some(Value::Array(items)) => items.iter().collect(),
        Some(other) => vec![other],
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    listed(value)
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// Records the legacy names of each side of each size of a `Spacing`
/// property; an old name keeps the first side that claimed it.
pub(super) fn collect_spacing(property: &Value, mappings: &mut HashMap<String, SpacingTarget>) {
    let Some(property) = property.as_object() else {
        return;
    };
    if property.get("type").and_then(Value::as_str) != Some("Spacing") {
        return;
    }
    let Some(key) = property.get("name").and_then(Value::as_str) else {
        return;
    };
    for mode in ["margin", "padding"] {
        for size in listed(property.get(mode)) {
            let Some(size) = size.as_object() else {
                continue;
            };
            let Some(option) = size.get("name").and_then(Value::as_str) else {
                continue;
            };
            for direction in ["top", "right", "bottom", "left"] {
                let old_names = size
                    .get(direction)
                    .and_then(Value::as_object)
                    .and_then(|side| side.get("oldNames"));
                for old_name in strings(old_names) {
                    let target = SpacingTarget {
                        key: key.to_string(),
                        child: format!("{mode}-{direction}"),
                        option: option.to_string(),
                    };
                    mappings.entry(old_name).or_insert(target);
                }
            }
        }
    }
}

/// Records the renames of a non-spacing property: its old keys, its
/// options' old names, and every pairing of the two.
fn collect_aliases(property: &Value, mappings: &mut HashMap<AliasSource, Alias>) {
    let Some(property) = property.as_object() else {
        return;
    };
    if property.get("type").and_then(Value::as_str) == Some("Spacing") {
        return;
    }
    let Some(key) = property.get("name").and_then(Value::as_str) else {
        return;
    };
    let old_keys = strings(property.get("oldNames"));
    for old_key in &old_keys {
        add_alias(
            mappings,
            (old_key.clone(), None),
            AliasTarget {
                key: key.to_string(),
                option: None,
            },
        );
    }
    for option in listed(property.get("options")) {
        let Some(option) = option.as_object() else {
            continue;
        };
        let new_option = match option.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => Some(name.clone()),
            Some(_) => continue,
        };
        let target = AliasTarget {
            key: key.to_string(),
            option: Some(new_option.clone()),
        };
        let old_options = strings(option.get("oldNames"));
        for old_option in &old_options {
            add_alias(
                mappings,
                (key.to_string(), Some(old_option.clone())),
                target.clone(),
            );
        }
        for old_key in &old_keys {
            add_alias(
                mappings,
                (old_key.clone(), new_option.clone()),
                target.clone(),
            );
        }
        for old_key in &old_keys {
            for old_option in &old_options {
                add_alias(
                    mappings,
                    (old_key.clone(), Some(old_option.clone())),
                    target.clone(),
                );
            }
        }
    }
}

/// An old name two entries rename differently is a conflict, and stays one.
fn add_alias(mappings: &mut HashMap<AliasSource, Alias>, source: AliasSource, target: AliasTarget) {
    let alias = Alias::Target(target);
    match mappings.get(&source) {
        None => {
            mappings.insert(source, alias);
        }
        Some(existing) if *existing != alias => {
            mappings.insert(source, Alias::Conflict);
        }
        Some(_) => {}
    }
}

/// `Key::Option` of an option design property; nothing for any other.
fn option_name(item: &Bson) -> Option<String> {
    let value = tree::field(item, "Value")?;
    tree::is_str(
        tree::field(value, "$Type"),
        "Forms$OptionDesignPropertyValue",
    )
    .then(|| {
        format!(
            "{}::{}",
            tree::to_text(tree::field(item, "Key")),
            tree::to_text(tree::field(value, "Option"))
        )
    })
}

fn is_compound(item: &Bson) -> bool {
    tree::is_str(
        tree::dig(Some(item), &["Value", "$Type"]),
        "Forms$CompoundDesignPropertyValue",
    )
}

fn compound_value(item: &mut Bson) -> Option<&mut Bson> {
    match item {
        Bson::Document(document) => document.get_mut("Value"),
        _ => None,
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(super) fn compound_property(key: &str) -> Document {
    doc! {
        "$ID": new_id(),
        "$Type": "Forms$DesignPropertyValue",
        "Key": key,
        "Value": {
            "$ID": new_id(),
            "$Type": "Forms$CompoundDesignPropertyValue",
            "Properties": tree::array(Vec::new(), 2),
        },
    }
}

pub(super) fn option_property(key: &str, option: &str) -> Document {
    doc! {
        "$ID": new_id(),
        "$Type": "Forms$DesignPropertyValue",
        "Key": key,
        "Value": {
            "$ID": new_id(),
            "$Type": "Forms$OptionDesignPropertyValue",
            "Option": option,
        },
    }
}
