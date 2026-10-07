//! Read-only compiler/runtime compatibility audit for a Mendix project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mxrs_bson::Document;
use mxrs_compiler_flow::FlowCompiler;
use mxrs_compiler_flow::nanoflow::{NanoflowCompiler, NanoflowDiagnostic};
use mxrs_compiler_widgets::ProjectPageBundleCompiler;
use mxrs_model::Project;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum PreflightError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error(transparent)]
    Validation(#[from] mxrs_mpr::MprError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Error,
}

/// One thing a native compiler or runtime does not take, as mxrb's
/// `CompatibilityFinding` says it: identical findings are one, with how
/// many times it was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub category: &'static str,
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub location: String,
    pub message: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreflightStats {
    pub units: usize,
    pub pages: usize,
    pub layouts: usize,
    pub nanoflows: usize,
    pub microflows: usize,
    pub flows: usize,
    pub code_actions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreflightReport {
    pub path: PathBuf,
    pub mendix_version: Option<String>,
    pub compatible: bool,
    pub stats: PreflightStats,
    pub findings: Vec<Finding>,
}

impl PreflightReport {
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Warning)
            .count()
    }
}

pub fn analyze(path: impl AsRef<Path>) -> Result<PreflightReport, PreflightError> {
    let path = std::path::absolute(path.as_ref())?;
    let project = Project::open(&path, true)?;
    let units = project.all_units()?;
    let documents = owned_documents(&project, &units)?;
    let mut findings = Vec::new();

    // What mxrs compiles natively is Mendix 11's model: an older one is
    // said once, and nothing else of it is audited.
    let version = project.mendix_version()?;
    let supported = version
        .as_deref()
        .is_some_and(|version| version.split('.').next() == Some("11"));
    if !supported {
        let version = version.as_deref().unwrap_or("unknown");
        findings.push(Finding {
            severity: Severity::Error,
            category: "version",
            artifact_type: format!("Mendix {version}"),
            location: "project".to_string(),
            message: format!("native compilation supports Mendix 11.x; got {version}"),
            count: 1,
        });
    }

    let validation = crate::validate::validate(&path)?;
    findings.extend(validation.errors.into_iter().map(|message| Finding {
        severity: Severity::Error,
        category: "storage",
        artifact_type: "MPR".to_string(),
        location: "project".to_string(),
        message,
        count: 1,
    }));
    findings.extend(validation.warnings.into_iter().map(|message| Finding {
        severity: Severity::Warning,
        category: "storage",
        artifact_type: "MPR".to_string(),
        location: "project".to_string(),
        message,
        count: 1,
    }));

    if supported {
        analyze_pages(&project, &mut findings);
        analyze_flows(&project, &documents, &path, &mut findings);
    }
    let findings = collapsed(findings);

    let stats = PreflightStats {
        units: units.len(),
        pages: count_type(&documents, "Forms$Page"),
        layouts: count_type(&documents, "Forms$Layout"),
        nanoflows: count_type(&documents, "Microflows$Nanoflow"),
        microflows: count_type(&documents, "Microflows$Microflow"),
        flows: documents
            .iter()
            .filter(|(_, document)| {
                matches!(
                    document.get_str("$Type").ok(),
                    Some("Microflows$Microflow" | "Microflows$Nanoflow" | "Microflows$Rule")
                )
            })
            .count(),
        code_actions: documents
            .iter()
            .filter(|(_, document)| {
                matches!(
                    document.get_str("$Type").ok(),
                    Some("JavaActions$JavaAction" | "JavaScriptActions$JavaScriptAction")
                )
            })
            .count(),
    };
    let compatible = findings
        .iter()
        .all(|finding| finding.severity != Severity::Error);
    Ok(PreflightReport {
        path,
        mendix_version: version,
        compatible,
        stats,
        findings,
    })
}

fn analyze_pages(project: &Project, findings: &mut Vec<Finding>) {
    let compiler = match ProjectPageBundleCompiler::new(project) {
        Ok(compiler) => compiler,
        Err(error) => {
            findings.push(error_finding("compiler", "project", "project", error));
            return;
        }
    };
    for result in compiler
        .compile_pages()
        .into_iter()
        .chain(compiler.compile_layouts())
    {
        match result {
            Ok(bundle) => {
                for artifact_type in bundle.unsupported_widgets {
                    findings.push(Finding {
                        severity: Severity::Error,
                        category: "widget",
                        artifact_type: artifact_type.clone(),
                        location: bundle.qualified_name.clone(),
                        message: format!(
                            "{artifact_type} is not compiled by the native web renderer"
                        ),
                        count: 1,
                    });
                }
                for artifact_type in bundle.unsupported_custom_widgets {
                    findings.push(Finding {
                        severity: Severity::Error,
                        category: "custom_widget",
                        artifact_type: artifact_type.clone(),
                        location: bundle.qualified_name.clone(),
                        message: format!(
                            "{artifact_type} is not compiled by the native web renderer"
                        ),
                        count: 1,
                    });
                }
            }
            Err(error) => findings.push(error_finding("page", "Forms artifact", "project", error)),
        }
    }
}

fn analyze_flows(
    project: &Project,
    documents: &[(String, Document)],
    path: &Path,
    findings: &mut Vec<Finding>,
) {
    let compiler = match FlowCompiler::new(project, &[]) {
        Ok(compiler) => compiler,
        Err(error) => {
            findings.push(error_finding("compiler", "flow index", "project", error));
            return;
        }
    };
    for (module, document) in documents {
        let artifact_type = document.get_str("$Type").unwrap_or("");
        let name = document.get_str("Name").unwrap_or("Unnamed");
        let location = if module.is_empty() {
            name.to_string()
        } else {
            format!("{module}.{name}")
        };
        let result = match artifact_type {
            "Microflows$Microflow" | "Microflows$Nanoflow" | "Microflows$Rule" => {
                compiler.compile_flow(document, module).map(|_| ())
            }
            "JavaActions$JavaAction" | "JavaScriptActions$JavaScriptAction" => compiler
                .compile_code_action(document, (!module.is_empty()).then_some(module.as_str()))
                .map(|_| ()),
            _ => continue,
        };
        if let Err(error) = result {
            findings.push(error_finding("flow", artifact_type, &location, error));
        }
    }
    for diagnostic in compiler.diagnostics() {
        findings.push(Finding {
            severity: Severity::Warning,
            category: "flow",
            artifact_type: "DatabaseConnector$DatabaseExecuteAction".to_string(),
            location: match &diagnostic {
                mxrs_compiler_flow::FlowDiagnostic::UnconfiguredWrite { action_id, .. } => {
                    action_id.clone()
                }
            },
            message: format!("{diagnostic:?}"),
            count: 1,
        });
    }

    let mut nanoflows = NanoflowCompiler::new(compiler.index(), path.parent());
    let mut names = compiler
        .index()
        .nanoflows
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();
    for name in names {
        let _ = nanoflows.reference(&name);
    }
    for unsupported in nanoflows.unsupported() {
        findings.push(Finding {
            severity: Severity::Error,
            category: "nanoflow",
            artifact_type: unsupported.node_type.clone(),
            location: unsupported.flow.clone(),
            message: format!(
                "{} is not compiled by the native nanoflow runtime",
                unsupported.node_type
            ),
            count: 1,
        });
    }
    for diagnostic in nanoflows.diagnostics() {
        let (location, artifact_type, message) = match diagnostic {
            NanoflowDiagnostic::UnreachableNode {
                flow,
                node_id,
                node_type,
            } => (
                flow.clone(),
                node_type.clone(),
                format!("unreachable node {node_id}"),
            ),
            NanoflowDiagnostic::AmbiguousBinaryLeftOperand { flow, expression } => (
                flow.clone(),
                "expression".to_string(),
                format!("ambiguous binary left operand in {expression}"),
            ),
        };
        findings.push(Finding {
            severity: Severity::Warning,
            category: "nanoflow",
            artifact_type,
            location,
            message,
            count: 1,
        });
    }
}

/// The findings as mxrb reports them: identical ones are one, counted, by
/// severity, category, location and type.
fn collapsed(findings: Vec<Finding>) -> Vec<Finding> {
    let mut collapsed: Vec<Finding> = Vec::new();
    for finding in findings {
        let same = |kept: &&mut Finding| {
            kept.severity == finding.severity
                && kept.category == finding.category
                && kept.artifact_type == finding.artifact_type
                && kept.location == finding.location
                && kept.message == finding.message
        };
        match collapsed.iter_mut().find(same) {
            Some(kept) => kept.count += finding.count,
            None => collapsed.push(finding),
        }
    }
    collapsed.sort_by(|left, right| {
        (
            severity_name(&left.severity),
            left.category,
            &left.location,
            &left.artifact_type,
            &left.message,
        )
            .cmp(&(
                severity_name(&right.severity),
                right.category,
                &right.location,
                &right.artifact_type,
                &right.message,
            ))
    });
    collapsed
}

fn severity_name(severity: &Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn owned_documents(
    project: &Project,
    units: &[mxrs_mpr::RawUnit],
) -> Result<Vec<(String, Document)>, mxrs_model::ModelError> {
    let module_by_id = project
        .modules()?
        .into_iter()
        .filter_map(|module| Some((module.id, module.name?)))
        .collect::<HashMap<_, _>>();
    let parent_by_id = units
        .iter()
        .map(|unit| (unit.unit_id.clone(), unit.container_id.clone()))
        .collect::<HashMap<_, _>>();
    units
        .iter()
        .map(|unit| {
            let module =
                owning_module(&unit.container_id, &parent_by_id, &module_by_id).unwrap_or_default();
            project
                .mpr()
                .parse_contents(unit)
                .map(|document| (module, document))
                .map_err(mxrs_model::ModelError::from)
        })
        .collect()
}

fn owning_module(
    container: &str,
    parents: &HashMap<String, String>,
    modules: &HashMap<String, String>,
) -> Option<String> {
    let mut current = container;
    for _ in 0..64 {
        if let Some(name) = modules.get(current) {
            return Some(name.clone());
        }
        let parent = parents.get(current)?;
        if parent == current {
            return None;
        }
        current = parent;
    }
    None
}

fn count_type(documents: &[(String, Document)], artifact_type: &str) -> usize {
    documents
        .iter()
        .filter(|(_, document)| document.get_str("$Type").ok() == Some(artifact_type))
        .count()
}

fn error_finding(
    category: &'static str,
    artifact_type: impl Into<String>,
    location: impl Into<String>,
    error: impl std::fmt::Display,
) -> Finding {
    Finding {
        severity: Severity::Error,
        category,
        artifact_type: artifact_type.into(),
        location: location.into(),
        message: error.to_string(),
        count: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_project_is_compatible_and_reports_real_inventory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("minimal.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
            module.microflow("Start", |_| {});
        });
        mxrs_writer::write_project(&path, &builder.build()).unwrap();
        let report = analyze(&path).unwrap();
        assert!(report.compatible, "{:?}", report.findings);
        assert_eq!(report.mendix_version.as_deref(), Some("11.12.1"));
        assert_eq!(report.stats.flows, 1);
        assert!(report.stats.units >= 3);
        assert_eq!(report.errors(), 0);
        assert_eq!(report.stats.microflows, 1);
    }

    /// Identical findings are one, counted, ordered as mxrb orders them:
    /// errors before warnings, then by category, location and type.
    #[test]
    fn identical_findings_are_one_and_counted() {
        let finding = |severity, category, location: &str| Finding {
            severity,
            category,
            artifact_type: "Forms$Gallery".to_string(),
            location: location.to_string(),
            message: "not compiled".to_string(),
            count: 1,
        };
        let findings = collapsed(vec![
            finding(Severity::Warning, "flow", "Sales.A"),
            finding(Severity::Error, "widget", "Sales.B"),
            finding(Severity::Error, "widget", "Sales.B"),
            finding(Severity::Error, "page", "Sales.C"),
        ]);
        assert_eq!(
            findings
                .iter()
                .map(|finding| (finding.category, finding.count))
                .collect::<Vec<_>>(),
            [("page", 1), ("widget", 2), ("flow", 1)]
        );
    }

    /// A model of a Mendix version mxrs does not compile is said to be
    /// that, once, and nothing else of it is audited.
    #[test]
    fn a_model_of_another_major_version_is_one_version_finding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("older.mpr");
        mxrs_writer::write_project(&path, &mxrs_dsl::ProjectBuilder::new("11.12.1").build())
            .unwrap();
        let mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        mpr.raw_query("UPDATE _MetaData SET _ProductVersion = '10.24.0'")
            .unwrap();
        drop(mpr);
        let report = analyze(&path).unwrap();
        assert!(!report.compatible);
        let version = &report.findings[0];
        assert_eq!(
            (version.category, version.artifact_type.as_str()),
            ("version", "Mendix 10.24.0")
        );
        assert!(
            report
                .findings
                .iter()
                .all(|finding| finding.category != "widget")
        );
    }
}
