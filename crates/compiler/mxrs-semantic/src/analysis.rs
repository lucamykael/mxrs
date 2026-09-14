use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::{Artifact, Diagnostic, Reference, SemanticIndex};

#[derive(Debug, Serialize)]
pub struct CallCycle {
    pub artifacts: Vec<Artifact>,
    pub references: Vec<Reference>,
}

#[derive(Debug, Serialize)]
pub struct ModuleDependency {
    pub from: String,
    pub to: String,
    pub references: Vec<Reference>,
    pub source_artifacts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Analysis {
    pub scope: &'static str,
    pub limitations: &'static [&'static str],
    pub diagnostics: Vec<Diagnostic>,
    pub call_cycles: Vec<CallCycle>,
    pub module_dependencies: Vec<ModuleDependency>,
}

impl Analysis {
    pub fn valid(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == "error")
    }
}

impl SemanticIndex {
    pub fn analyze(&self) -> Analysis {
        let call_cycles = self.call_cycles();
        let mut diagnostics = self.diagnostics.clone();
        diagnostics.extend(call_cycles.iter().map(|cycle| Diagnostic {
            severity: "error".into(),
            code: "call_cycle".into(),
            message: format!(
                    "recursive call component: {}",
                    cycle
                        .artifacts
                        .iter()
                        .map(|artifact| artifact.qualified_name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
        }));
        diagnostics.sort_by(|left, right| {
            (&left.severity, &left.code, &left.message).cmp(&(
                &right.severity,
                &right.code,
                &right.message,
            ))
        });
        Analysis {
            scope: "explicit_reference_graph",
            limitations: &[
                "Expression-embedded and opaque widget references are not analyzed.",
                "Bare attributes shared by entities in one module need entity context or qualified names.",
                "Unused artifacts, architecture rules and Studio Pro consistency checks are not covered.",
            ],
            diagnostics,
            call_cycles,
            module_dependencies: self.module_dependencies(),
        }
    }

    /// Iterative SCC traversal avoids exhausting the native stack on long flow
    /// chains. Like mxrb's analyzer, only `calls` edges establish recursion.
    /// Component edge extraction scans the graph once, including when every
    /// node is an independent self-cycle, instead of rescanning per component.
    pub fn call_cycles(&self) -> Vec<CallCycle> {
        let mut forward: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut reverse: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for reference in self
            .references
            .iter()
            .filter(|reference| reference.relation == "calls")
        {
            forward
                .entry(&reference.from)
                .or_default()
                .push(&reference.to);
            forward.entry(&reference.to).or_default();
            reverse
                .entry(&reference.to)
                .or_default()
                .push(&reference.from);
        }
        let mut visited = BTreeSet::new();
        let mut order = Vec::new();
        for &root in forward.keys() {
            let mut stack = vec![(root, false)];
            while let Some((node, expanded)) = stack.pop() {
                if expanded {
                    order.push(node);
                } else if visited.insert(node) {
                    stack.push((node, true));
                    stack.extend(forward[node].iter().rev().map(|&next| (next, false)));
                }
            }
        }
        visited.clear();
        let mut cycles = Vec::new();
        let mut cyclic_membership = HashMap::new();
        for root in order.into_iter().rev() {
            if visited.contains(root) {
                continue;
            }
            let mut component = BTreeSet::new();
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                if visited.insert(node) {
                    component.insert(node);
                    stack.extend(reverse.get(node).into_iter().flatten().copied());
                }
            }
            if component.len() == 1 && !forward[root].contains(&root) {
                continue;
            }
            let mut artifacts: Vec<_> = component
                .iter()
                .filter_map(|key| self.artifacts.get(*key))
                .cloned()
                .collect();
            artifacts.sort_by(|left, right| {
                left.qualified_name
                    .cmp(&right.qualified_name)
                    .then(left.key.cmp(&right.key))
            });
            for node in component {
                cyclic_membership.insert(node, cycles.len());
            }
            cycles.push(CallCycle {
                artifacts,
                references: Vec::new(),
            });
        }
        // BTreeSet iteration keeps each component's output deterministic; the
        // hash table is only a lookup, never an output ordering source.
        for reference in &self.references {
            if reference.relation == "calls"
                && let Some(&component) = cyclic_membership.get(reference.from.as_str())
                && cyclic_membership.get(reference.to.as_str()) == Some(&component)
            {
                cycles[component].references.push(reference.clone());
            }
        }
        cycles.sort_by(|left, right| {
            left.artifacts
                .iter()
                .map(|artifact| &artifact.key)
                .cmp(right.artifacts.iter().map(|artifact| &artifact.key))
        });
        cycles
    }

    pub fn module_dependencies(&self) -> Vec<ModuleDependency> {
        let mut grouped: BTreeMap<(&str, &str), Vec<Reference>> = BTreeMap::new();
        for reference in &self.references {
            let (Some(from), Some(to)) = (
                self.artifacts
                    .get(&reference.from)
                    .and_then(|artifact| artifact.module.as_deref()),
                self.artifacts
                    .get(&reference.to)
                    .and_then(|artifact| artifact.module.as_deref()),
            ) else {
                continue;
            };
            if from != to {
                grouped
                    .entry((from, to))
                    .or_default()
                    .push(reference.clone());
            }
        }
        grouped
            .into_iter()
            .map(|((from, to), references)| {
                let source_artifacts: BTreeSet<_> = references
                    .iter()
                    .map(|reference| reference.from.clone())
                    .collect();
                ModuleDependency {
                    from: from.to_string(),
                    to: to.to_string(),
                    references,
                    source_artifacts: source_artifacts.into_iter().collect(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtifactKind, IndexBuilder};

    fn graph(edges: &[(&str, &str, &str)]) -> SemanticIndex {
        let mut builder = IndexBuilder::default();
        let names: BTreeSet<_> = edges
            .iter()
            .flat_map(|(from, to, _)| [*from, *to])
            .collect();
        for name in names {
            builder.add(Artifact {
                key: name.into(),
                id: None,
                name: name.into(),
                qualified_name: name.into(),
                module: name.split_once('.').map(|(module, _)| module.to_string()),
                kind: ArtifactKind::Microflow,
                documentation: String::new(),
            });
        }
        for (from, to, relation) in edges {
            builder.reference_keys(from, to, relation);
        }
        builder.finish().unwrap()
    }

    #[test]
    fn separates_cycles_from_diamonds_containment_and_one_way_dependencies() {
        let index = graph(&[
            ("A.One", "B.Two", "calls"),
            ("B.Two", "A.One", "calls"),
            ("B.Two", "C.Three", "calls"),
            ("C.Three", "D.Four", "calls"),
            ("C.Three", "E.Five", "calls"),
            ("E.Five", "D.Four", "calls"),
            ("D.Four", "A.One", "contains"),
        ]);
        let analysis = index.analyze();
        assert!(!analysis.valid());
        assert_eq!(analysis.call_cycles.len(), 1);
        assert_eq!(
            analysis.call_cycles[0]
                .artifacts
                .iter()
                .map(|artifact| artifact.key.as_str())
                .collect::<Vec<_>>(),
            ["A.One", "B.Two"]
        );
        assert_eq!(analysis.call_cycles[0].references.len(), 2);
        assert_eq!(analysis.diagnostics[0].code, "call_cycle");
        assert!(!analysis.limitations.is_empty());
    }

    #[test]
    fn detects_self_calls_but_does_not_treat_references_as_recursion() {
        let index = graph(&[
            ("A.One", "A.One", "calls"),
            ("B.Two", "B.Two", "references"),
        ]);
        assert_eq!(index.call_cycles().len(), 1);
        assert_eq!(index.callers("A.One").unwrap()[0].key, "A.One");
        assert_eq!(index.callees("A.One").unwrap()[0].key, "A.One");
        assert!(index.callers("B.Two").unwrap().is_empty());
        assert!(index.callees("B.Two").unwrap().is_empty());
        assert!(index.callers("missing").is_err());
        assert!(index.callees("missing").is_err());
    }

    #[test]
    fn module_dependencies_group_sources_and_keep_direction() {
        let index = graph(&[
            ("A.One", "B.Two", "calls"),
            ("A.One", "B.Three", "calls"),
            ("B.Two", "A.One", "calls"),
            ("A.One", "A.Four", "calls"),
            ("Project", "A.One", "contains"),
        ]);
        let dependencies = index.module_dependencies();
        assert_eq!(dependencies.len(), 2);
        assert_eq!((&*dependencies[0].from, &*dependencies[0].to), ("A", "B"));
        assert_eq!(dependencies[0].source_artifacts, ["A.One"]);
        assert_eq!(dependencies[0].references.len(), 2);
        assert_eq!((&*dependencies[1].from, &*dependencies[1].to), ("B", "A"));
    }

    #[test]
    fn handles_long_acyclic_chains_without_recursion_or_false_cycles() {
        let names: Vec<_> = (0..10_000).map(|i| format!("M.F{i:05}")).collect();
        let edges: Vec<_> = names
            .windows(2)
            .map(|pair| (pair[0].as_str(), pair[1].as_str(), "calls"))
            .collect();
        let index = graph(&edges);
        assert!(index.call_cycles().is_empty());
        assert!(index.analyze().valid());
        assert_eq!(
            index.impact(names.last().unwrap()).unwrap().len(),
            names.len() - 1
        );
        assert!(index.impact(&names[0]).unwrap().is_empty());
        assert!(graph(&[]).analyze().valid());
    }

    #[test]
    fn thousands_of_independent_cycles_extract_only_their_own_call_edges() {
        let names: Vec<_> = (0..10_000).map(|i| format!("M.F{i:05}")).collect();
        let mut edges: Vec<_> = names
            .iter()
            .map(|name| (name.as_str(), name.as_str(), "calls"))
            .collect();
        // Edges between consecutive components must not merge their cycles.
        // Other relations within a component must not be reported as calls.
        edges.extend(
            names
                .windows(2)
                .map(|pair| (pair[0].as_str(), pair[1].as_str(), "calls")),
        );
        edges.extend(
            names
                .iter()
                .map(|name| (name.as_str(), name.as_str(), "references")),
        );
        let index = graph(&edges);
        let cycles = index.call_cycles();
        assert_eq!(cycles.len(), names.len());
        for (cycle, name) in cycles.iter().zip(&names) {
            assert_eq!(cycle.artifacts.len(), 1);
            assert_eq!(&cycle.artifacts[0].key, name);
            assert_eq!(cycle.references.len(), 1);
            assert_eq!(&cycle.references[0].from, name);
            assert_eq!(&cycle.references[0].to, name);
            assert_eq!(cycle.references[0].relation, "calls");
        }
    }
}
