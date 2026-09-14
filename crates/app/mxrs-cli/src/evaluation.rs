//! Declarative static model evaluations backed by the semantic index.

use std::path::Path;

use mxrs_semantic::{ArtifactKind, SemanticIndex};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    #[error("cannot read evaluation {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid evaluation {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error(transparent)]
    Semantic(#[from] mxrs_semantic::SemanticError),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    checks: Vec<Check>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Check {
    NoCallCycles {
        #[serde(default)]
        severity: Severity,
    },
    NoMissingInternalReferences {
        #[serde(default)]
        severity: Severity,
    },
    Artifact {
        name: String,
        kind: Option<ArtifactKind>,
        #[serde(default)]
        severity: Severity,
    },
    Reference {
        from: String,
        to: String,
        relation: Option<String>,
        #[serde(default)]
        severity: Severity,
    },
    ForbidDependency {
        from: String,
        to: String,
        #[serde(default)]
        severity: Severity,
    },
    RequireDependency {
        from: String,
        to: String,
        #[serde(default)]
        severity: Severity,
    },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    #[default]
    Error,
    Warning,
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub severity: Severity,
    pub passed: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct EvaluationResult {
    pub checks: Vec<CheckResult>,
    pub score: f64,
    pub passed: bool,
}

pub fn evaluate(
    project: impl AsRef<Path>,
    definition: impl AsRef<Path>,
) -> Result<EvaluationResult, EvaluationError> {
    let definition_path = definition.as_ref();
    let source =
        std::fs::read_to_string(definition_path).map_err(|source| EvaluationError::Io {
            path: definition_path.display().to_string(),
            source,
        })?;
    let definition: Definition =
        serde_json::from_str(&source).map_err(|source| EvaluationError::Json {
            path: definition_path.display().to_string(),
            source,
        })?;
    let project = mxrs_model::Project::open(project, true)?;
    let index = SemanticIndex::build(&project)?;
    let analysis = index.analyze();
    let checks = definition
        .checks
        .iter()
        .map(|check| execute(check, &index, &analysis))
        .collect::<Vec<_>>();
    let passed_count = checks.iter().filter(|check| check.passed).count();
    let score = if checks.is_empty() {
        100.0
    } else {
        (passed_count as f64 * 10_000.0 / checks.len() as f64).round() / 100.0
    };
    let passed = checks
        .iter()
        .all(|check| check.passed || check.severity == Severity::Warning);
    Ok(EvaluationResult {
        checks,
        score,
        passed,
    })
}

fn execute(
    check: &Check,
    index: &SemanticIndex,
    analysis: &mxrs_semantic::Analysis,
) -> CheckResult {
    let (name, severity, failure) = match check {
        Check::NoCallCycles { severity } => (
            "model has no call cycles".to_string(),
            *severity,
            (!analysis.call_cycles.is_empty())
                .then(|| format!("{} call cycle(s) found", analysis.call_cycles.len())),
        ),
        Check::NoMissingInternalReferences { severity } => {
            let missing = analysis
                .diagnostics
                .iter()
                .filter(|item| item.code == "unresolved_reference")
                .count();
            (
                "model has no missing internal references".to_string(),
                *severity,
                (missing != 0).then(|| format!("{missing} internal reference(s) missing")),
            )
        }
        Check::Artifact {
            name,
            kind,
            severity,
        } => {
            let found = index.resolve(name).ok().flatten();
            let valid = found.is_some_and(|artifact| kind.is_none_or(|kind| artifact.kind == kind));
            (
                format!("artifact {name} exists"),
                *severity,
                (!valid).then(|| {
                    format!(
                        "missing {} {name}",
                        kind.map_or("artifact", ArtifactKind::as_str)
                    )
                }),
            )
        }
        Check::Reference {
            from,
            to,
            relation,
            severity,
        } => {
            let source = index.resolve(from).ok().flatten();
            let target = index.resolve(to).ok().flatten();
            let found = source.zip(target).is_some_and(|(source, target)| {
                index.references().any(|reference| {
                    reference.from == source.key
                        && reference.to == target.key
                        && relation
                            .as_ref()
                            .is_none_or(|relation| &reference.relation == relation)
                })
            });
            (
                format!("{from} references {to}"),
                *severity,
                (!found).then(|| format!("reference {from} -> {to} not found")),
            )
        }
        Check::ForbidDependency { from, to, severity } => {
            let dependency = analysis
                .module_dependencies
                .iter()
                .find(|item| &item.from == from && &item.to == to);
            (
                format!("{from} does not depend on {to}"),
                *severity,
                dependency.map(|item| format!("{} forbidden reference(s)", item.references.len())),
            )
        }
        Check::RequireDependency { from, to, severity } => (
            format!("{from} depends on {to}"),
            *severity,
            (!analysis
                .module_dependencies
                .iter()
                .any(|item| &item.from == from && &item.to == to))
            .then(|| "required module dependency not found".to_string()),
        ),
    };
    CheckResult {
        name,
        severity,
        passed: failure.is_none(),
        message: failure.unwrap_or_else(|| "passed".to_string()),
    }
}
