//! What a flow's declaration says it is related to, held against what the
//! built model says.
//!
//! `#[microflow(calls(...), uses(...), used_by(...))]` writes nothing into
//! the model: the flow's body and the documents that refer to it already
//! say all of it. The lists are there for whoever reads the declaration —
//! so they are only worth having while they are true, and a build says
//! where they are not.

use std::collections::BTreeSet;
use std::path::Path;

use mxrs_ir::{MicroflowDecl, ProjectDecl};

use crate::error::Result;

/// One line per relation a declaration states differently from the model
/// built at `path`. Nothing is read when no flow states a relation.
pub fn relation_warnings(path: impl AsRef<Path>, project: &ProjectDecl) -> Result<Vec<String>> {
    let flows: Vec<(String, &MicroflowDecl)> = project
        .modules
        .iter()
        .flat_map(|module| {
            module
                .microflows
                .iter()
                .chain(&module.nanoflows)
                .filter(|flow| flow.relations.is_stated())
                .map(move |flow| (format!("{}.{}", module.name, flow.name), flow))
        })
        .collect();
    if flows.is_empty() {
        return Ok(Vec::new());
    }
    let model = mxrs_model::Project::open(path, true)?;
    let actual = mxrs_model::relations::flow_relations(&model)?;
    let mut warnings = Vec::new();
    for (name, flow) in flows {
        let Some(actual) = actual.get(&name) else {
            continue;
        };
        for (relation, declared, actual, omitted, stale) in [
            (
                "calls",
                &flow.relations.calls,
                &actual.calls,
                "which it calls",
                "which it does not call",
            ),
            (
                "uses",
                &flow.relations.uses,
                &actual.uses,
                "which it uses",
                "which it does not use",
            ),
            (
                "used_by",
                &flow.relations.used_by,
                &actual.used_by,
                "which uses it",
                "which does not use it",
            ),
        ] {
            let Some(declared) = declared else {
                continue;
            };
            let declared: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
            for missing in actual
                .iter()
                .filter(|name| !declared.contains(name.as_str()))
            {
                warnings.push(format!(
                    "{name}: {relation}(...) leaves out {missing}, {omitted}"
                ));
            }
            for extra in declared.iter().filter(|name| !actual.contains(**name)) {
                warnings.push(format!("{name}: {relation}(...) lists {extra}, {stale}"));
            }
        }
    }
    Ok(warnings)
}

/// Says on standard error where the declarations and the model built at
/// `path` disagree about how the project's flows are related. The lists
/// write nothing, so failing to check them never fails the build.
pub(crate) fn report(path: &Path, project: &ProjectDecl) {
    match relation_warnings(path, project) {
        Ok(warnings) => {
            for warning in warnings {
                eprintln!("[mxrs] warning: {warning}");
            }
        }
        Err(error) => {
            eprintln!("[mxrs] warning: could not check the flows' stated relations: {error}")
        }
    }
}
