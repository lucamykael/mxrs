//! The widget packages installed beside the model (`widgets/*.mpk`) and the
//! definition each widget id resolves to.
//!
//! A widget migrates only when exactly one readable package defines it.
//! A package mxrb would read with a property type of no known kind is not
//! readable here: mxrb would store that property with no type at all, and
//! [`mxrs_widget_package`] refuses it by name instead, which blocks the
//! widget as an `invalid_widget_package`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mxrs_pluggable::WidgetType;
use mxrs_widget_package::{WidgetPackage, WidgetPackageError};
use sha2::{Digest, Sha256};

use super::{IssueKind, MigrationIssue, tree};

/// A widget id no package declares, used to read every definition a package
/// holds without matching one.
const VALIDATION_WIDGET_ID: &str = "__mxrb_package_validation__";

/// The definition of a widget id, and the SHA-256 of the package declaring
/// it.
#[derive(Debug)]
pub(super) struct Installed {
    pub(super) widget: WidgetType,
    pub(super) digest: Option<String>,
}

/// The installed widget packages and what each widget id resolved to.
pub(super) struct Packages {
    paths: Vec<PathBuf>,
    invalid: HashSet<PathBuf>,
    definitions: HashMap<String, Option<Rc<Installed>>>,
    failures: HashMap<String, (IssueKind, String)>,
}

impl Packages {
    /// The packages `root/widgets/*.mpk` names, in path order.
    pub(super) fn discover(root: &Path) -> Self {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(root.join("widgets"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.starts_with('.') && name.ends_with(".mpk"))
            })
            .collect();
        paths.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
        Self {
            paths,
            invalid: HashSet::new(),
            definitions: HashMap::new(),
            failures: HashMap::new(),
        }
    }

    /// Reads every package once; each one that cannot be read blocks the
    /// plan and is left out of every lookup.
    pub(super) fn validate(&mut self, issues: &mut Vec<MigrationIssue>) {
        for path in &self.paths {
            let error = match WidgetPackage::new(path).definition(VALIDATION_WIDGET_ID) {
                Ok(_) | Err(WidgetPackageError::UnknownPropertyType { .. }) => continue,
                Err(error) => error,
            };
            self.invalid.insert(path.clone());
            issues.push(MigrationIssue {
                unit_id: String::new(),
                path: format!(".widgets.{}", file_name(path)),
                kind: IssueKind::InvalidWidgetPackage,
                message: format!("cannot read {}: {}", relative(path), cause(&error)),
            });
        }
    }

    /// The one installed definition of `widget_id`, or nothing — and then
    /// [`Packages::failure`] says why.
    pub(super) fn definition(&mut self, widget_id: &str) -> Option<Rc<Installed>> {
        if let Some(resolved) = self.definitions.get(widget_id) {
            return resolved.clone();
        }
        let mut candidates = Vec::new();
        for path in self
            .paths
            .iter()
            .filter(|path| !self.invalid.contains(*path))
        {
            match WidgetPackage::new(path).definition(widget_id) {
                Ok(Some(widget)) => candidates.push((path.clone(), Ok(widget))),
                Ok(None) => {}
                Err(error @ WidgetPackageError::UnknownPropertyType { .. }) => {
                    candidates.push((path.clone(), Err(error)));
                }
                // `validate` already reported every package it could not
                // read; one failing now is skipped the same way.
                Err(_) => {}
            }
        }
        let resolved = match candidates.as_slice() {
            [(path, Ok(widget))] => Some(Rc::new(Installed {
                widget: widget.clone(),
                digest: digest(path),
            })),
            [(path, Err(error))] => {
                self.failures.insert(
                    widget_id.to_string(),
                    (
                        IssueKind::InvalidWidgetPackage,
                        format!("cannot read {}: {}", relative(path), cause(error)),
                    ),
                );
                None
            }
            [] => None,
            several => {
                let paths: Vec<String> = several.iter().map(|(path, _)| relative(path)).collect();
                self.failures.insert(
                    widget_id.to_string(),
                    (
                        IssueKind::AmbiguousWidgetDefinition,
                        format!(
                            "multiple installed MPKs define {}: {}",
                            tree::inspect_str(widget_id),
                            paths.join(", ")
                        ),
                    ),
                );
                None
            }
        };
        self.definitions
            .insert(widget_id.to_string(), resolved.clone());
        resolved
    }

    /// Why `widget_id` resolved to no definition.
    pub(super) fn failure(&self, widget_id: &str) -> (IssueKind, String) {
        self.failures.get(widget_id).cloned().unwrap_or_else(|| {
            (
                IssueKind::MissingWidgetDefinition,
                format!("no installed MPK defines {}", tree::inspect_str(widget_id)),
            )
        })
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A package's path relative to the project directory.
fn relative(path: &Path) -> String {
    format!("widgets/{}", file_name(path))
}

/// What went wrong reading a package, without the package path the error
/// already carries.
fn cause(error: &WidgetPackageError) -> String {
    match error {
        WidgetPackageError::Package { message, .. } => message.clone(),
        WidgetPackageError::Xml { message, .. } => {
            format!("widget definition is not valid XML: {message}")
        }
        WidgetPackageError::UnknownPropertyType { key, type_name, .. } => format!(
            "widget property {} has unrecognized type {}",
            tree::inspect_str(key),
            tree::inspect_str(type_name)
        ),
    }
}

fn digest(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}
