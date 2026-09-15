//! Evidence-based inventory of Cargo-native authoring coverage.
//!
//! Reading a document, compiling it for a runtime, and being able to author
//! it from Rust are deliberately different claims. This audit calls a unit
//! `typed` only when import emits an editable declaration and the existing
//! exporter has proved that recompiling it preserves every source field.
//! Everything else remains lossless through `model/imported`, but is reported
//! as either a partial typed projection or preserved-only data.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mxrs_model::Project;
use serde::Serialize;

use crate::{ExportError, Result, page_export, round_trip_gaps};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortabilityStatus {
    Typed,
    Partial,
    Preserved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortabilityFamily {
    pub native_type: String,
    pub total: usize,
    pub typed: usize,
    pub partial: usize,
    pub preserved: usize,
    pub status: PortabilityStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortabilitySummary {
    pub total_units: usize,
    pub typed_units: usize,
    pub partial_units: usize,
    pub preserved_units: usize,
    pub typed_round_trip_gaps: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortabilityReport {
    pub path: PathBuf,
    pub mendix_version: Option<String>,
    /// True means the imported model units have an exact preservation path.
    /// It does not mean every document has a typed Rust authoring surface.
    pub model_lossless: bool,
    pub fully_typed: bool,
    pub requires_imported_model: bool,
    pub summary: PortabilitySummary,
    pub families: Vec<PortabilityFamily>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentRoundTripReport {
    pub source_units: usize,
    pub rebuilt_units: usize,
    pub candidate_units: usize,
    pub byte_identical_units: usize,
    pub passed: bool,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UnitSnapshot {
    unit_id: String,
    container_id: String,
    containment_name: String,
    document: mxrs_bson::Document,
    bytes: Vec<u8>,
}

pub fn audit_portability(path: impl AsRef<Path>) -> Result<PortabilityReport> {
    let path = std::path::absolute(path.as_ref()).map_err(|source| ExportError::Io {
        path: path.as_ref().display().to_string(),
        source,
    })?;
    let project = Project::open(&path, true)?;
    let units = project.all_units()?;
    let mut counts = BTreeMap::<String, usize>::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        let native_type = document
            .get_str("$Type")
            .unwrap_or("<missing $Type>")
            .to_string();
        *counts.entry(native_type).or_default() += 1;
    }

    let mut modules = project.modules()?;
    modules.sort_by(|left, right| left.name.cmp(&right.name));
    let typed_round_trip_gaps = round_trip_gaps(&modules).len();
    let (mut typed_pages, mut page_report) = page_export::convert_pages_for_version(
        &modules,
        project.mendix_version()?.as_deref().unwrap_or(""),
    );
    page_export::protect_referenced_page_elements(
        &project,
        &modules,
        &mut typed_pages,
        &mut page_report,
    )?;
    let mut typed_pages_by_type = BTreeMap::<String, usize>::new();
    for page in &typed_pages {
        *typed_pages_by_type
            .entry(page.source_type().to_string())
            .or_default() += 1;
    }
    let (_, editable_documents) = super::render_documents_module(
        &project,
        project.mendix_version()?.as_deref().unwrap_or(""),
    )?;
    let converted_flows = super::flow_export::collect(&project, &modules)?;

    let mut families = Vec::with_capacity(counts.len());
    for (native_type, total) in counts {
        let editable = editable_documents
            .get(native_type.as_str())
            .copied()
            .unwrap_or(0)
            + converted_flows
                .iter()
                .filter(|flow| flow.native_type == native_type)
                .count();
        let typed = typed_pages_by_type.get(&native_type).copied().unwrap_or(0)
            + if matches!(
                native_type.as_str(),
                "RegularExpressions$RegularExpression"
                    | "Queues$Queue"
                    | "ScheduledEvents$ScheduledEvent"
                    | "Menus$MenuDocument"
            ) {
                editable
            } else {
                0
            };
        let (partial, preserved, reason) = if typed > 0 {
            (
                0,
                total.saturating_sub(typed),
                if matches!(
                    native_type.as_str(),
                    "RegularExpressions$RegularExpression"
                        | "Queues$Queue"
                        | "ScheduledEvents$ScheduledEvent"
                        | "Menus$MenuDocument"
                ) {
                    "the complete semantic document is emitted as typed Rust and byte-exactly round-tripped"
                        .to_string()
                } else {
                    "typed pages are recompiled and field-compared; remaining pages are preserved byte-for-byte"
                        .to_string()
                },
            )
        } else if editable > 0 {
            (
                editable,
                total.saturating_sub(editable),
                "editable Rust declarations are emitted; fields outside the authoring IR remain preserved"
                    .to_string(),
            )
        } else if let Some(reason) = partial_projection_reason(&native_type) {
            (total, 0, reason.to_string())
        } else {
            (
                0,
                total,
                "no editable Rust declaration is emitted; the imported model preserves the unit byte-for-byte"
                    .to_string(),
            )
        };
        let status = if typed == total {
            PortabilityStatus::Typed
        } else if typed > 0 || partial > 0 {
            PortabilityStatus::Partial
        } else {
            PortabilityStatus::Preserved
        };
        families.push(PortabilityFamily {
            native_type,
            total,
            typed,
            partial,
            preserved,
            status,
            reason,
        });
    }
    families.sort_by(|left, right| {
        right
            .preserved
            .cmp(&left.preserved)
            .then(right.partial.cmp(&left.partial))
            .then(left.native_type.cmp(&right.native_type))
    });

    let summary = PortabilitySummary {
        total_units: units.len(),
        typed_units: families.iter().map(|family| family.typed).sum(),
        partial_units: families.iter().map(|family| family.partial).sum(),
        preserved_units: families.iter().map(|family| family.preserved).sum(),
        typed_round_trip_gaps,
    };
    let fully_typed = summary.partial_units == 0 && summary.preserved_units == 0;
    Ok(PortabilityReport {
        path,
        mendix_version: project.mendix_version()?,
        model_lossless: true,
        fully_typed,
        requires_imported_model: !fully_typed,
        summary,
        families,
    })
}

/// Rebuilds a temporary copy and applies only the document declarations the
/// importer emits today. Every enumeration and constant is compared by
/// qualified name, unit identity, containment and raw BSON bytes.
pub fn verify_editable_document_round_trip(
    path: impl AsRef<Path>,
) -> Result<DocumentRoundTripReport> {
    let source = Project::open(path.as_ref(), true)?;
    let declaration = super::editable_project_declaration(&source)?;
    let before = editable_unit_snapshots(&source, &declaration)?;
    let source_units = source.all_units()?.len();
    drop(source);

    let directory = tempfile::tempdir().map_err(|source| ExportError::Io {
        path: std::env::temp_dir().display().to_string(),
        source,
    })?;
    let imported = directory.path().join("imported");
    let rebuilt = directory.path().join("rebuilt.mpr");
    mxrs_project::capture_imported_project(path.as_ref(), &imported)?;
    mxrs_project::restore_imported_project(&imported, &rebuilt)?;
    mxrs_writer::synchronize_project_documents(&rebuilt, &declaration)?;
    {
        let mut project = Project::open(&rebuilt, false)?;
        let modules = project.modules()?;
        for declared in &declaration.modules {
            let module = modules
                .iter()
                .find(|m| m.name.as_deref() == Some(declared.name.as_str()))
                .expect("source module");
            mxrs_writer::documents::synchronize_microflows(
                project.mpr_mut(),
                &module.id,
                &declared.microflows,
            )?;
            mxrs_writer::documents::synchronize_nanoflows(
                project.mpr_mut(),
                &module.id,
                &declared.nanoflows,
            )?;
        }
    }

    let rebuilt_project = Project::open(&rebuilt, true)?;
    let rebuilt_units = rebuilt_project.all_units()?.len();
    let after = editable_unit_snapshots(&rebuilt_project, &declaration)?;
    let candidate_units = before.values().map(Vec::len).sum();
    let mut byte_identical_units = 0;
    let mut failures = Vec::new();
    for (name, expected) in &before {
        let actual = after.get(name).map(Vec::as_slice).unwrap_or_default();
        if expected.len() != 1 || actual.len() != 1 {
            failures.push(format!(
                "{name}: expected one source and one rebuilt unit, found {} and {}",
                expected.len(),
                actual.len()
            ));
            continue;
        }
        if expected[0] == actual[0] {
            byte_identical_units += 1;
        } else {
            let detail = snapshot_difference(&expected[0], &actual[0]);
            failures.push(format!("{name}: {detail}"));
        }
    }
    for name in after.keys() {
        if !before.contains_key(name) {
            failures.push(format!("{name}: rebuilt an undeclared extra unit"));
        }
    }
    if source_units != rebuilt_units {
        failures.push(format!(
            "project unit count changed from {source_units} to {rebuilt_units}"
        ));
    }
    failures.sort();
    failures.dedup();
    Ok(DocumentRoundTripReport {
        source_units,
        rebuilt_units,
        candidate_units,
        byte_identical_units,
        passed: failures.is_empty() && byte_identical_units == candidate_units,
        failures,
    })
}

fn editable_unit_snapshots(
    project: &Project,
    declaration: &mxrs_ir::ProjectDecl,
) -> Result<BTreeMap<String, Vec<UnitSnapshot>>> {
    let expected =
        declaration
            .modules
            .iter()
            .flat_map(|module| {
                module
                    .enumerations
                    .iter()
                    .map(|document| {
                        (
                            module.name.clone(),
                            "Enumerations$Enumeration",
                            document.name.clone(),
                        )
                    })
                    .chain(module.constants.iter().map(|document| {
                        (
                            module.name.clone(),
                            "Constants$Constant",
                            document.name.clone(),
                        )
                    }))
                    .chain(module.regular_expressions.iter().map(|document| {
                        (
                            module.name.clone(),
                            "RegularExpressions$RegularExpression",
                            document.name.clone(),
                        )
                    }))
                    .chain(module.task_queues.iter().map(|document| {
                        (module.name.clone(), "Queues$Queue", document.name.clone())
                    }))
                    .chain(module.scheduled_events.iter().map(|document| {
                        (
                            module.name.clone(),
                            "ScheduledEvents$ScheduledEvent",
                            document.name.clone(),
                        )
                    }))
                    .chain(module.microflows.iter().map(|document| {
                        (
                            module.name.clone(),
                            "Microflows$Microflow",
                            document.name.clone(),
                        )
                    }))
                    .chain(module.nanoflows.iter().map(|document| {
                        (
                            module.name.clone(),
                            "Microflows$Nanoflow",
                            document.name.clone(),
                        )
                    }))
                    .chain(module.menus.iter().map(|document| {
                        (
                            module.name.clone(),
                            "Menus$MenuDocument",
                            document.name.clone(),
                        )
                    }))
            })
            .collect::<std::collections::HashSet<_>>();
    let units = project.all_units()?;
    let module_by_id = project
        .modules()?
        .into_iter()
        .filter_map(|module| Some((module.id, module.name?)))
        .collect::<BTreeMap<_, _>>();
    let parent_by_id = units
        .iter()
        .map(|unit| (unit.unit_id.clone(), unit.container_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut snapshots = BTreeMap::<String, Vec<UnitSnapshot>>::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        let Some(native_type) = document.get_str("$Type").ok() else {
            continue;
        };
        if !matches!(
            native_type,
            "Enumerations$Enumeration"
                | "Constants$Constant"
                | "RegularExpressions$RegularExpression"
                | "Queues$Queue"
                | "ScheduledEvents$ScheduledEvent"
                | "Menus$MenuDocument"
                | "Microflows$Microflow"
                | "Microflows$Nanoflow"
        ) {
            continue;
        }
        let Some(name) = document.get_str("Name").ok() else {
            continue;
        };
        let Some(module) = owning_module(&unit.container_id, &parent_by_id, &module_by_id) else {
            continue;
        };
        if !expected.contains(&(module.clone(), native_type, name.to_string())) {
            continue;
        }
        let key = format!("{module}.{name} ({native_type})");
        let bytes = project
            .mpr()
            .content_bytes(unit)
            .map_err(mxrs_model::ModelError::from)?
            .unwrap_or_default();
        snapshots.entry(key).or_default().push(UnitSnapshot {
            unit_id: unit.unit_id.clone(),
            container_id: unit.container_id.clone(),
            containment_name: unit.containment_name.clone(),
            document,
            bytes,
        });
    }
    for values in snapshots.values_mut() {
        values.sort_by(|left, right| left.unit_id.cmp(&right.unit_id));
    }
    Ok(snapshots)
}

fn snapshot_difference(expected: &UnitSnapshot, actual: &UnitSnapshot) -> String {
    if expected.unit_id != actual.unit_id {
        return format!(
            "unit ID changed from {} to {}",
            expected.unit_id, actual.unit_id
        );
    }
    if expected.container_id != actual.container_id {
        return format!(
            "container changed from {} to {}",
            expected.container_id, actual.container_id
        );
    }
    if expected.containment_name != actual.containment_name {
        return format!(
            "containment changed from {} to {}",
            expected.containment_name, actual.containment_name
        );
    }
    if let Some(path) = first_document_difference("$", &expected.document, &actual.document) {
        return format!("BSON value changed at {path}");
    }
    let offset = expected
        .bytes
        .iter()
        .zip(&actual.bytes)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| expected.bytes.len().min(actual.bytes.len()));
    format!(
        "BSON encoding changed at byte {offset} ({} bytes became {} bytes)",
        expected.bytes.len(),
        actual.bytes.len()
    )
}

fn first_document_difference(
    path: &str,
    expected: &mxrs_bson::Document,
    actual: &mxrs_bson::Document,
) -> Option<String> {
    let expected_keys = expected.keys().collect::<Vec<_>>();
    let actual_keys = actual.keys().collect::<Vec<_>>();
    if expected_keys != actual_keys {
        let first = expected_keys
            .iter()
            .zip(&actual_keys)
            .position(|(left, right)| left != right)
            .unwrap_or_else(|| expected_keys.len().min(actual_keys.len()));
        let expected_key = expected_keys
            .get(first)
            .map(|key| key.as_str())
            .unwrap_or("<missing>");
        let actual_key = actual_keys
            .get(first)
            .map(|key| key.as_str())
            .unwrap_or("<missing>");
        return Some(format!(
            "{path}.$keys[{first}] ({expected_key:?} became {actual_key:?})"
        ));
    }
    for key in expected_keys {
        let child = format!("{path}.{key}");
        let difference = first_bson_difference(&child, &expected[key], &actual[key]);
        if difference.is_some() {
            return difference;
        }
    }
    None
}

fn first_bson_difference(
    path: &str,
    expected: &mxrs_bson::Bson,
    actual: &mxrs_bson::Bson,
) -> Option<String> {
    match (expected, actual) {
        (mxrs_bson::Bson::Document(left), mxrs_bson::Bson::Document(right)) => {
            first_document_difference(path, left, right)
        }
        (mxrs_bson::Bson::Array(left), mxrs_bson::Bson::Array(right)) => {
            if left.len() != right.len() {
                return Some(format!("{path}.$length"));
            }
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                if let Some(difference) =
                    first_bson_difference(&format!("{path}[{index}]"), left, right)
                {
                    return Some(difference);
                }
            }
            None
        }
        _ if expected == actual => None,
        _ => Some(path.to_string()),
    }
}

fn owning_module(
    container: &str,
    parents: &BTreeMap<String, String>,
    modules: &BTreeMap<String, String>,
) -> Option<String> {
    let mut current = container;
    for _ in 0..64 {
        if let Some(module) = modules.get(current) {
            return Some(module.clone());
        }
        let parent = parents.get(current)?;
        if parent == current {
            return None;
        }
        current = parent;
    }
    None
}

fn partial_projection_reason(native_type: &str) -> Option<&'static str> {
    match native_type {
        "Projects$Module" | "Projects$ModuleImpl" => Some(
            "module identity participates in typed declarations; module metadata remains preserved",
        ),
        "DomainModels$DomainModel" => Some(
            "entities, attributes and associations are editable; unsupported domain fields remain preserved",
        ),
        "Security$ModuleSecurity" => {
            Some("module roles are editable; unmodeled security fields remain preserved")
        }
        "Security$ProjectSecurity" => Some(
            "core project security and user roles are editable; unmodeled fields remain preserved",
        ),
        "Navigation$NavigationDocument" => Some(
            "profiles, homes and menu items are editable; unmodeled navigation fields remain preserved",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Portability.mpr");
        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
            module.enumeration("Status", |enumeration| {
                enumeration.value("Open");
            });
            module.constant("Limit", |constant| {
                constant.value("10");
            });
            module.regular_expression("Code", "[A-Z]+", |regular_expression| {
                regular_expression.documentation("Uppercase code");
            });
            module.microflow("Save", |_| {});
            module.scheduled_event("Nightly", "Save", mxrs_ir::ScheduleUnit::Days, |_| {});
            module.menu("Main", |menu| {
                menu.item("Home", |item| {
                    item.page("Home");
                });
            });
        });
        mxrs_writer::write_project(&path, &project.build()).unwrap();
        (directory, path)
    }

    #[test]
    fn distinguishes_typed_partial_and_preserved_units() {
        let (_directory, path) = fixture();
        let report = audit_portability(path).unwrap();
        let family = |kind: &str| {
            report
                .families
                .iter()
                .find(|family| family.native_type == kind)
                .unwrap()
        };
        assert_eq!(
            family("DomainModels$DomainModel").status,
            PortabilityStatus::Partial
        );
        assert_eq!(
            family("Microflows$Microflow").status,
            PortabilityStatus::Partial
        );
        assert_eq!(
            family("RegularExpressions$RegularExpression").status,
            PortabilityStatus::Typed
        );
        assert_eq!(
            family("ScheduledEvents$ScheduledEvent").status,
            PortabilityStatus::Typed
        );
        assert_eq!(
            family("Menus$MenuDocument").status,
            PortabilityStatus::Typed
        );
        assert!(report.model_lossless);
        assert!(!report.fully_typed);
        assert!(report.requires_imported_model);
        assert_eq!(
            report.summary.total_units,
            report.summary.typed_units
                + report.summary.partial_units
                + report.summary.preserved_units
        );
    }

    #[test]
    fn json_names_the_preservation_boundary_without_ids() {
        let (_directory, path) = fixture();
        let value = serde_json::to_value(audit_portability(path).unwrap()).unwrap();
        assert_eq!(value["model_lossless"], true);
        assert_eq!(value["requires_imported_model"], true);
        assert!(value["families"][0]["status"].is_string());
        assert!(!value.to_string().contains("unit_id"));
    }

    #[test]
    fn verifies_editable_documents_by_identity_containment_and_bytes() {
        let (_directory, path) = fixture();
        let report = verify_editable_document_round_trip(path).unwrap();
        assert!(report.passed, "{:?}", report.failures);
        assert_eq!(report.source_units, report.rebuilt_units);
        assert_eq!(report.candidate_units, 6);
        assert_eq!(report.byte_identical_units, 6);
    }
}
