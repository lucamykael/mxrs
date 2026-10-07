//! `mxrs frontend migrate` — previews, and under `--apply` writes, the
//! frontend migrations a model needs after its installed widget packages,
//! theme or Mendix version moved on. Ports `Mxrb::Frontend::Migrator` and
//! `MigrationPlan` (`lib/mxrb/frontend/migrator.rb`).
//!
//! Three migrations run over every unit:
//!
//! - a pluggable widget (`CustomWidgets$CustomWidget`) is rebound to the
//!   schema its installed `widgets/*.mpk` declares, each configured value
//!   kept by its property key ([`widget`]);
//! - a layout-grid row whose desktop column weights do not add up to twelve
//!   is normalized when that is lossless ([`layout`]);
//! - a design property the theme renamed (`oldNames` in
//!   `themesource/*/{web,native}/design-properties.json`) takes its new name,
//!   and a legacy spacing option is folded into its compound property
//!   ([`design`]).
//!
//! The plan fails closed: anything it cannot migrate losslessly is an
//! [`MigrationIssue`], and a plan with an issue is not safe and never writes.
//! Applying a safe plan updates its units in one transaction, each only if
//! its `ContentsHash` is still the one the preview read.

mod design;
mod layout;
mod packages;
mod template;
mod tree;
mod widget;

#[cfg(test)]
mod tests;

use std::fmt;
use std::path::{Path, PathBuf};

use mxrs_bson::{Bson, Document};
use mxrs_mpr::{MprError, MprFile};
use serde::Serialize;

use design::DesignMappings;
use packages::Packages;

/// The Mendix majors whose frontend model this migrator knows.
const SUPPORTED_MAJORS: [i64; 2] = [10, 11];

/// What blocks a migration from being applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// The model's Mendix version is outside [`SUPPORTED_MAJORS`].
    UnsupportedVersion,
    /// An installed `.mpk` cannot be read.
    InvalidWidgetPackage,
    /// No installed package defines a widget the model uses.
    MissingWidgetDefinition,
    /// More than one installed package defines a widget the model uses.
    AmbiguousWidgetDefinition,
    /// A widget lacks its `Type` or its `Object`.
    MalformedWidget,
    /// A widget's stored schema has fields neither Mendix nor the package
    /// declares.
    UnknownWidgetSchema,
    /// A widget's stored object has fields neither Mendix nor the package
    /// declares.
    UnknownWidgetObject,
    /// A stored property points at no property type of its schema.
    UnknownPropertyPointer,
    /// The installed schema dropped a property the widget configures.
    RemovedConfiguredWidgetProperty,
    /// A stored property value is not a document.
    MalformedWidgetValue,
    /// A configured nested object has no schema to rebind it to.
    ChangedWidgetObject,
    /// A configured property changed its type with no lossless conversion.
    ChangedWidgetProperty,
    /// A layout row's weights cannot be normalized without guessing.
    UnsafeLayoutWeights,
    /// A legacy spacing option disagrees with the compound value it folds
    /// into.
    ConflictingDesignProperty,
}

impl IssueKind {
    /// The kind as the report names it (`missing_widget_definition`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "unsupported_version",
            Self::InvalidWidgetPackage => "invalid_widget_package",
            Self::MissingWidgetDefinition => "missing_widget_definition",
            Self::AmbiguousWidgetDefinition => "ambiguous_widget_definition",
            Self::MalformedWidget => "malformed_widget",
            Self::UnknownWidgetSchema => "unknown_widget_schema",
            Self::UnknownWidgetObject => "unknown_widget_object",
            Self::UnknownPropertyPointer => "unknown_property_pointer",
            Self::RemovedConfiguredWidgetProperty => "removed_configured_widget_property",
            Self::MalformedWidgetValue => "malformed_widget_value",
            Self::ChangedWidgetObject => "changed_widget_object",
            Self::ChangedWidgetProperty => "changed_widget_property",
            Self::UnsafeLayoutWeights => "unsafe_layout_weights",
            Self::ConflictingDesignProperty => "conflicting_design_property",
        }
    }
}

impl fmt::Display for IssueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One reason the plan is not safe, located by its unit and the path to the
/// offending node inside the unit's document (`$.Widgets[1].Object`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MigrationIssue {
    pub unit_id: String,
    pub path: String,
    pub kind: IssueKind,
    pub message: String,
}

/// A unit whose document the migrations change, with how many of each
/// migration it holds.
#[derive(Debug, Clone)]
pub struct MigrationChange {
    pub unit_id: String,
    /// The `ContentsHash` the preview read; applying requires it unchanged.
    pub before_hash: Option<String>,
    pub after: Document,
    pub widgets: usize,
    pub layout_rows: usize,
    pub design_properties: usize,
}

/// Why applying a plan failed.
#[derive(Debug, thiserror::Error)]
pub enum FrontendMigrationError {
    #[error(transparent)]
    Mpr(#[from] MprError),
    #[error("cannot resolve {path}: {source}")]
    Path {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("frontend migration is blocked: {0}")]
    Blocked(String),
    #[error("frontend migration plan was already applied")]
    AlreadyApplied,
    #[error("frontend unit disappeared: {0}")]
    UnitDisappeared(String),
    #[error("frontend unit changed after preview: {0}")]
    UnitChanged(String),
}

/// The preview of every frontend migration a model needs.
#[derive(Debug)]
pub struct MigrationPlan {
    path: PathBuf,
    version: String,
    changes: Vec<MigrationChange>,
    issues: Vec<MigrationIssue>,
    applied: bool,
}

/// [`MigrationPlan`] as the command reports it, field for field.
#[derive(Debug, Serialize)]
pub struct MigrationReport<'plan> {
    pub version: &'plan str,
    pub changes: usize,
    pub widgets: usize,
    pub layout_rows: usize,
    pub design_properties: usize,
    pub issues: &'plan [MigrationIssue],
    pub safe: bool,
    pub applied: bool,
}

impl MigrationPlan {
    /// Reads the model at `path` and plans its migrations; nothing is
    /// written.
    pub fn build(path: impl AsRef<Path>) -> Result<Self, FrontendMigrationError> {
        let path =
            std::path::absolute(path.as_ref()).map_err(|source| FrontendMigrationError::Path {
                path: path.as_ref().to_path_buf(),
                source,
            })?;
        let mpr = MprFile::open(&path, true)?;
        let version = mpr.mendix_version()?.unwrap_or_default();
        let mut issues = Vec::new();
        if !SUPPORTED_MAJORS.contains(&tree::ruby_to_i(&version)) {
            issues.push(MigrationIssue {
                unit_id: String::new(),
                path: String::new(),
                kind: IssueKind::UnsupportedVersion,
                message: format!(
                    "Mendix {}; supported majors are 10 and 11",
                    tree::inspect_str(&version)
                ),
            });
            return Ok(Self {
                path,
                version,
                changes: Vec::new(),
                issues,
                applied: false,
            });
        }

        let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut migrator = Migrator {
            packages: Packages::discover(&root),
            design: DesignMappings::new(&root),
        };
        migrator.packages.validate(&mut issues);

        let mut changes = Vec::new();
        for unit in mpr.all_units()? {
            let before = mpr.parse_contents(&unit)?;
            let mut after = before.clone();
            let mut scope = UnitScope {
                unit_id: &unit.unit_id,
                issues: &mut issues,
                counts: Counts::default(),
            };
            migrator.migrate_document(&mut after, "$", &mut scope);
            let counts = scope.counts;
            if tree::same_document(&after, &before) {
                continue;
            }
            changes.push(MigrationChange {
                unit_id: unit.unit_id,
                before_hash: unit.contents_hash,
                after,
                widgets: counts.widgets,
                layout_rows: counts.layout_rows,
                design_properties: counts.design_properties,
            });
        }
        Ok(Self {
            path,
            version,
            changes,
            issues,
            applied: false,
        })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn changes(&self) -> &[MigrationChange] {
        &self.changes
    }

    pub fn issues(&self) -> &[MigrationIssue] {
        &self.issues
    }

    /// A plan is safe when nothing blocks it.
    pub fn is_safe(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn is_applied(&self) -> bool {
        self.applied
    }

    pub fn widgets(&self) -> usize {
        self.changes.iter().map(|change| change.widgets).sum()
    }

    pub fn layout_rows(&self) -> usize {
        self.changes.iter().map(|change| change.layout_rows).sum()
    }

    pub fn design_properties(&self) -> usize {
        self.changes
            .iter()
            .map(|change| change.design_properties)
            .sum()
    }

    pub fn report(&self) -> MigrationReport<'_> {
        MigrationReport {
            version: &self.version,
            changes: self.changes.len(),
            widgets: self.widgets(),
            layout_rows: self.layout_rows(),
            design_properties: self.design_properties(),
            issues: &self.issues,
            safe: self.is_safe(),
            applied: self.applied,
        }
    }

    /// The report as the command prints it without `--json`.
    pub fn render_text(&self) -> String {
        let report = self.report();
        let mut text = format!(
            "Version           : {}\n\
             Changes           : {}\n\
             Widgets           : {}\n\
             Layout rows       : {}\n\
             Design properties : {}\n\
             Safe              : {}\n\
             Applied           : {}\n",
            report.version,
            report.changes,
            report.widgets,
            report.layout_rows,
            report.design_properties,
            report.safe,
            report.applied
        );
        for issue in &self.issues {
            text.push_str(&format!(
                "[{}] {}{}: {}\n",
                issue.kind.as_str().to_uppercase(),
                issue.unit_id,
                issue.path,
                issue.message
            ));
        }
        text
    }

    /// Writes every change in one transaction. Refused when the plan is not
    /// safe or was applied already, and rolled back entirely when a unit
    /// disappeared or changed since the preview read it.
    pub fn apply(&mut self) -> Result<(), FrontendMigrationError> {
        if !self.is_safe() {
            return Err(FrontendMigrationError::Blocked(self.blocked_details()));
        }
        if self.applied {
            return Err(FrontendMigrationError::AlreadyApplied);
        }
        let mut mpr = MprFile::open(&self.path, false)?;
        let changes = &self.changes;
        let version = &self.version;
        mpr.transaction(|mpr| {
            for change in changes {
                match mpr.unit(&change.unit_id)? {
                    None => {
                        return Ok(Err(FrontendMigrationError::UnitDisappeared(
                            change.unit_id.clone(),
                        )));
                    }
                    Some(current) if current.contents_hash != change.before_hash => {
                        return Ok(Err(FrontendMigrationError::UnitChanged(
                            change.unit_id.clone(),
                        )));
                    }
                    Some(_) => {}
                }
            }
            // Every unit is checked before the first write, so a refusal
            // above leaves the transaction empty.
            for change in changes {
                let mut after = change.after.clone();
                mxrs_schema::apply_document(version, &mut after);
                mpr.update_unit(&change.unit_id, after)?;
            }
            Ok(Ok(()))
        })??;
        self.applied = true;
        Ok(())
    }

    fn blocked_details(&self) -> String {
        self.issues
            .iter()
            .map(|issue| {
                format!(
                    "{} at {}{}: {}",
                    issue.kind, issue.unit_id, issue.path, issue.message
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// How many migrations of each kind one unit holds.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    widgets: usize,
    layout_rows: usize,
    design_properties: usize,
}

/// The unit being migrated: where its issues go and what it counts.
struct UnitScope<'scope> {
    unit_id: &'scope str,
    issues: &'scope mut Vec<MigrationIssue>,
    counts: Counts,
}

impl UnitScope<'_> {
    fn issue(&mut self, path: &str, kind: IssueKind, message: impl Into<String>) {
        self.issues.push(MigrationIssue {
            unit_id: self.unit_id.to_string(),
            path: path.to_string(),
            kind,
            message: message.into(),
        });
    }
}

/// What the migrations read from the project around the model: its
/// installed widget packages and its theme's design properties.
struct Migrator {
    packages: Packages,
    design: DesignMappings,
}

impl Migrator {
    /// Migrates a node, then each node it holds — after its own migration,
    /// so a rebound widget's new object is walked too.
    fn migrate_document(&mut self, document: &mut Document, path: &str, scope: &mut UnitScope<'_>) {
        if document.contains_key("DesignProperties") {
            self.design.migrate(document, path, scope);
        }
        if tree::has_type(document, "Forms$LayoutGridRow") {
            layout::migrate_row(document, path, scope);
        }
        if tree::has_type(document, "CustomWidgets$CustomWidget") {
            widget::migrate(&mut self.packages, document, path, scope);
        }
        for (key, child) in document.iter_mut() {
            self.migrate_value(child, &format!("{path}.{key}"), scope);
        }
    }

    fn migrate_value(&mut self, value: &mut Bson, path: &str, scope: &mut UnitScope<'_>) {
        match value {
            Bson::Document(document) => self.migrate_document(document, path, scope),
            Bson::Array(items) => {
                for (index, child) in items.iter_mut().enumerate() {
                    self.migrate_value(child, &format!("{path}[{index}]"), scope);
                }
            }
            _ => {}
        }
    }
}
