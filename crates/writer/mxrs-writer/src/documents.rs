//! Incremental re-sync for microflow, page, enumeration, constant,
//! regular-expression, and scheduled-event `Documents` units — mirrors the
//! corresponding slices of `Writer#write_documents`/`#upsert_document` (mxrb's
//! own method upserts pages/microflows/nanoflows/rules/menus/enumerations/
//! constants/scheduled_events; of those this crate still has no DSL/model
//! surface for rules and menus, widened incrementally like the rest of the
//! codebase — see `mxrs_ir::page`'s doc comment for exactly which
//! pages/widgets are and aren't covered yet).
//!
//! Unlike domain-model entity/association sync, this is **upsert-only**:
//! `microflows` is not treated as the module's complete authoritative
//! microflow list — mxrb's own `write_documents` never deletes an existing
//! document just because it's absent from a given call's declared list, and
//! this matches that. A microflow whose name matches an existing
//! `Microflows$Microflow` `Documents` unit keeps its `$ID`; everything else
//! about the document is fully re-derived from the declared `MicroflowDecl`
//! (`mxrs_model::Microflow::to_bson()` is already fully declarative, unlike
//! mxrb's sparser Ruby declarations — which is why mxrb needs a whole
//! `__mxrb_*_declared` sentinel dance in `merge_existing_document` that this
//! doesn't: there's no partial-field-preserve case to handle, same
//! principle already used for attribute/association reconciliation in
//! `domain.rs`).
//!
//! Imported enumerations, constants, and regular expressions may live under
//! arbitrarily nested `Projects$Folder` units. Their lookup therefore walks
//! the containment tree and preserves the original unit ID and container when
//! applying an editable declaration.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mxrs_bson::{Bson, DateTime, Document, build_array, doc, extract_id, parse_array};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::flow::MicroflowDecl;
use mxrs_ir::page::PageDecl;
use mxrs_ir::{
    ConstantDecl, ConstantType, EnumerationDecl, ExportLevel, OnOverlap, RegularExpressionDecl,
    ScheduleUnit, ScheduledEventDecl,
};
use mxrs_model::Microflow;
use mxrs_mpr::MprFile;

use crate::error::Result;
use crate::{flow_compiler, page_compiler};

pub fn synchronize_microflows(
    mpr: &mut MprFile,
    module_id: &str,
    microflows: &[MicroflowDecl],
) -> Result<()> {
    let root_id = mpr
        .root_unit()?
        .ok_or(crate::WriterError::MissingRootUnit)?
        .unit_id;
    let identity = ProjectIdentity::from_project_root(&root_id)?;
    let module_unit = mpr
        .unit(module_id)?
        .ok_or_else(|| crate::WriterError::MissingModuleUnit(module_id.to_string()))?;
    let module_doc = mpr.parse_contents(&module_unit)?;
    let module_name = module_doc
        .get_str("Name")
        .map_err(|_| crate::WriterError::MissingModuleName(module_id.to_string()))?;
    synchronize_microflows_with_identity(mpr, module_id, module_name, microflows, identity)
}

pub(crate) fn synchronize_microflows_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    microflows: &[MicroflowDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    synchronize_flows_with_identity(
        mpr,
        module_id,
        module_name,
        microflows,
        identity,
        "Microflows$Microflow",
        ArtifactKind::Microflow,
    )
}

/// Upserts Cargo-native nanoflows by name while preserving imported IDs and
/// leaving undeclared/opaque nanoflows untouched.
pub fn synchronize_nanoflows(
    mpr: &mut MprFile,
    module_id: &str,
    nanoflows: &[MicroflowDecl],
) -> Result<()> {
    let root_id = mpr
        .root_unit()?
        .ok_or(crate::WriterError::MissingRootUnit)?
        .unit_id;
    let identity = ProjectIdentity::from_project_root(&root_id)?;
    let module_unit = mpr
        .unit(module_id)?
        .ok_or_else(|| crate::WriterError::MissingModuleUnit(module_id.to_string()))?;
    let module_doc = mpr.parse_contents(&module_unit)?;
    let module_name = module_doc
        .get_str("Name")
        .map_err(|_| crate::WriterError::MissingModuleName(module_id.to_string()))?;
    synchronize_nanoflows_with_identity(mpr, module_id, module_name, nanoflows, identity)
}

pub(crate) fn synchronize_nanoflows_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    nanoflows: &[MicroflowDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    synchronize_flows_with_identity(
        mpr,
        module_id,
        module_name,
        nanoflows,
        identity,
        "Microflows$Nanoflow",
        ArtifactKind::Nanoflow,
    )
}

#[allow(clippy::too_many_arguments)]
fn synchronize_flows_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    declarations: &[MicroflowDecl],
    identity: ProjectIdentity,
    native_type: &str,
    artifact_kind: ArtifactKind,
) -> Result<()> {
    let existing_by_name: HashMap<String, String> = mpr
        .children_of(module_id)?
        .into_iter()
        .filter(|u| u.containment_name == "Documents")
        .filter_map(|u| {
            let doc = mpr.parse_contents(&u).ok()?;
            if doc.get_str("$Type").ok()? != native_type {
                return None;
            }
            let name = doc.get_str("Name").ok()?.to_string();
            Some((name, u.unit_id))
        })
        .collect();

    for decl in declarations {
        let (objects, flows) = flow_compiler::build_microflow_graph(
            &decl.activities,
            &decl.rescue_activities,
            decl.return_expression.as_deref(),
        );
        let existing_id = existing_by_name.get(&decl.name).cloned();
        let id = existing_id.clone().unwrap_or_else(|| {
            identity.artifact_id(artifact_kind, &format!("{module_name}.{}", decl.name))
        });
        let microflow = Microflow {
            id: Some(id.clone()),
            name: Some(decl.name.clone()),
            documentation: decl.documentation.clone(),
            return_variable_name: "ReturnValue".into(),
            allow_concurrent_execution: true,
            apply_entity_access: false,
            mark_as_used: false,
            excluded: false,
            export_level: "Hidden".into(),
            allowed_module_roles: vec![],
            parameters: vec![],
            return_type_document: crate::flow_compiler::return_type_document(
                decl.return_type.as_ref(),
            ),
            return_type: None,
            objects,
            flows,
        };
        let doc = microflow.to_bson_as(native_type);
        match existing_id {
            Some(id) => {
                mpr.update_unit(&id, doc)?;
            }
            None => {
                mpr.insert_unit(module_id, "Documents", doc, Some(&id))?;
            }
        }
    }
    Ok(())
}

/// Standalone entry point mirroring `synchronize_microflows`: resolves the
/// project identity and module name from `mpr`/`module_id` itself, for
/// callers that don't already have a `ProjectIdentity` in hand (tests,
/// mostly — `project::synchronize_project` calls
/// `synchronize_pages_with_identity` directly since it already has both).
pub fn synchronize_pages(
    mpr: &mut MprFile,
    module_id: &str,
    mendix_version: &str,
    pages: &[PageDecl],
) -> Result<()> {
    let (identity, module_name) = module_identity(mpr, module_id)?;
    synchronize_pages_with_identity(
        mpr,
        module_id,
        &module_name,
        mendix_version,
        pages,
        identity,
    )
}

pub fn synchronize_layouts(
    mpr: &mut MprFile,
    module_id: &str,
    mendix_version: &str,
    layouts: &[mxrs_ir::LayoutDecl],
) -> Result<()> {
    let (identity, module_name) = module_identity(mpr, module_id)?;
    crate::layout_compiler::synchronize_layouts_with_identity(
        mpr,
        module_id,
        &module_name,
        mendix_version,
        layouts,
        identity,
    )
}

fn module_identity(mpr: &MprFile, module_id: &str) -> Result<(ProjectIdentity, String)> {
    let root_id = mpr
        .root_unit()?
        .ok_or(crate::WriterError::MissingRootUnit)?
        .unit_id;
    let identity = ProjectIdentity::from_project_root(&root_id)?;
    let module_unit = mpr
        .unit(module_id)?
        .ok_or_else(|| crate::WriterError::MissingModuleUnit(module_id.to_string()))?;
    let module_doc = mpr.parse_contents(&module_unit)?;
    let module_name = module_doc
        .get_str("Name")
        .map_err(|_| crate::WriterError::MissingModuleName(module_id.to_string()))?;
    Ok((identity, module_name.to_string()))
}

/// Upserts pages by name, same upsert-only policy as
/// `synchronize_microflows_with_identity`: a page absent from `pages` isn't
/// deleted (it stays whatever it was — typically an opaque unit restored
/// verbatim from an imported snapshot, see `mxrs-project`'s
/// `rebuild_imported_project`). A page's own `$ID` is preserved when its
/// name matches an existing `Forms$Page`/`Pages$Page` `Documents` unit,
/// exactly like a microflow's; everything else about the document is fully
/// re-derived from `PageDecl` via `page_compiler::compile_page` (no
/// partial-field-preserve case, same reasoning `documents.rs`'s own module
/// doc gives for microflows).
///
/// Building a `mxrs-forms::Catalog` parses the embedded schema JSON fresh
/// each call (no caching upstream) — skipped entirely when `pages` is
/// empty so a page-less module (still the common case for most existing
/// projects) pays nothing for this.
pub(crate) fn synchronize_pages_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    mendix_version: &str,
    pages: &[PageDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    if pages.is_empty() {
        return Ok(());
    }
    let catalog = Rc::new(mxrs_forms::Catalog::for_version(mendix_version)?);

    let existing_by_name: HashMap<String, String> = mpr
        .children_of(module_id)?
        .into_iter()
        .filter(|u| u.containment_name == "Documents")
        .filter_map(|u| {
            let doc = mpr.parse_contents(&u).ok()?;
            let type_name = doc.get_str("$Type").ok()?;
            if type_name != "Forms$Page" && type_name != "Pages$Page" {
                return None;
            }
            let name = doc.get_str("Name").ok()?.to_string();
            Some((name, u.unit_id))
        })
        .collect();

    for decl in pages {
        let existing_id = existing_by_name.get(&decl.name).cloned();
        let id = existing_id.clone().unwrap_or_else(|| {
            identity.artifact_id(ArtifactKind::Page, &format!("{module_name}.{}", decl.name))
        });
        let mut document = page_compiler::compile_page(&catalog, decl)?;
        document.insert("$ID", id.clone());
        match existing_id {
            Some(id) => {
                mpr.update_unit(&id, document)?;
            }
            None => {
                mpr.insert_unit(module_id, "Documents", document, Some(&id))?;
            }
        }
    }
    Ok(())
}

pub(crate) fn synchronize_enumerations_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    enumerations: &[EnumerationDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    if enumerations.is_empty() {
        return Ok(());
    }
    let existing_by_name = existing_documents_by_name(mpr, module_id, "Enumerations$Enumeration")?;

    for declaration in enumerations {
        let qualified_name = format!("{module_name}.{}", declaration.name);
        let existing = existing_by_name.get(&declaration.name);
        let enumeration_id = existing
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::Enumeration, &qualified_name));
        let document = enumeration_document(
            declaration,
            &qualified_name,
            &enumeration_id,
            existing.map(|(_, document)| document),
            identity,
        );
        if existing.is_some() {
            mpr.update_unit(&enumeration_id, document)?;
        } else {
            mpr.insert_unit(module_id, "Documents", document, Some(&enumeration_id))?;
        }
    }
    Ok(())
}

fn enumeration_document(
    declaration: &EnumerationDecl,
    qualified_name: &str,
    enumeration_id: &str,
    existing: Option<&Document>,
    identity: ProjectIdentity,
) -> Document {
    let existing_values = parse_array(
        existing
            .and_then(|document| document.get_array("Values").ok())
            .map(Vec::as_slice),
    );
    let existing_by_name: HashMap<&str, &Document> = existing_values
        .items
        .iter()
        .filter_map(Bson::as_document)
        .filter_map(|value| Some((value.get_str("Name").ok()?, value)))
        .collect();
    let values = declaration
        .values
        .iter()
        .map(|value| {
            let value_key = format!("{qualified_name}.{}", value.name);
            let previous = existing_by_name.get(value.name.as_str()).copied();
            let value_id = previous
                .and_then(|document| document.get("$ID"))
                .and_then(extract_id)
                .unwrap_or_else(|| {
                    identity.artifact_id(ArtifactKind::EnumerationValue, &value_key)
                });
            let mut value_document = previous.cloned().unwrap_or_default();
            value_document.insert("$ID", value_id);
            value_document.insert("$Type", "Enumerations$EnumerationValue");
            value_document.insert("Name", value.name.clone());
            if previous.is_none() {
                value_document.insert("Image", "");
                value_document.insert("ExportLevel", "Hidden");
            }
            value_document.insert(
                "Caption",
                caption_document(value, &value_key, previous, identity),
            );
            Bson::Document(value_document)
        })
        .collect();

    let mut document = existing.cloned().unwrap_or_default();
    document.insert("$ID", enumeration_id);
    document.insert("$Type", "Enumerations$Enumeration");
    document.insert("Name", declaration.name.clone());
    document.insert("Documentation", declaration.documentation.clone());
    if existing.is_none() {
        document.insert("Excluded", false);
        document.insert("ExportLevel", "Hidden");
    }
    document.insert("Values", build_array(values, existing_values.marker));
    document
}

fn caption_document(
    value: &mxrs_ir::EnumerationValueDecl,
    value_key: &str,
    previous_value: Option<&Document>,
    identity: ProjectIdentity,
) -> Document {
    let previous = previous_value.and_then(|document| document.get_document("Caption").ok());
    let caption_id = previous
        .and_then(|document| document.get("$ID"))
        .and_then(extract_id)
        .unwrap_or_else(|| identity.artifact_id(ArtifactKind::EnumerationCaption, value_key));
    let previous_items = parse_array(
        previous
            .and_then(|document| document.get_array("Items").ok())
            .map(Vec::as_slice),
    );
    let previous_by_language: HashMap<&str, &Document> = previous_items
        .items
        .iter()
        .filter_map(Bson::as_document)
        .filter_map(|item| Some((item.get_str("LanguageCode").ok()?, item)))
        .collect();
    let translations = value
        .captions
        .iter()
        .map(|(language, text)| {
            let previous = previous_by_language.get(language.as_str()).copied();
            let translation_key = format!("{value_key}.{language}");
            let translation_id = previous
                .and_then(|document| document.get("$ID"))
                .and_then(extract_id)
                .unwrap_or_else(|| {
                    identity.artifact_id(ArtifactKind::Translation, &translation_key)
                });
            let mut translation = previous.cloned().unwrap_or_default();
            translation.insert("$ID", translation_id);
            translation.insert("$Type", "Texts$Translation");
            translation.insert("LanguageCode", language.clone());
            translation.insert("Text", text.clone());
            Bson::Document(translation)
        })
        .collect();
    let mut caption = previous.cloned().unwrap_or_default();
    caption.insert("$ID", caption_id);
    caption.insert("$Type", "Texts$Text");
    caption.insert("Items", build_array(translations, previous_items.marker));
    caption
}

/// Indexes a module's existing `Documents` units of one `$Type` by `Name`.
///
/// The enumeration/page/microflow paths each grew their own copy of this
/// filter; constants and scheduled events share it rather than adding two
/// more.
fn existing_documents_by_name(
    mpr: &mut MprFile,
    module_id: &str,
    document_type: &str,
) -> Result<HashMap<String, (String, Document)>> {
    let mut documents = HashMap::new();
    let mut pending = vec![module_id.to_string()];
    let mut visited = HashSet::new();
    while let Some(parent) = pending.pop() {
        for unit in mpr.children_of(&parent)? {
            if !visited.insert(unit.unit_id.clone()) {
                continue;
            }
            pending.push(unit.unit_id.clone());
            if unit.containment_name != "Documents" {
                continue;
            }
            let document = mpr.parse_contents(&unit)?;
            if document.get_str("$Type").ok() != Some(document_type) {
                continue;
            }
            if let Ok(name) = document.get_str("Name") {
                documents.insert(name.to_string(), (unit.unit_id, document));
            }
        }
    }
    Ok(documents)
}

/// Upserts `Constants$Constant` documents, mirroring `Writer#constant_doc`.
///
/// Like the enumeration path this is upsert-only: a constant absent from
/// `constants` is left alone rather than deleted.
pub(crate) fn synchronize_constants_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    constants: &[ConstantDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    if constants.is_empty() {
        return Ok(());
    }
    let existing_by_name = existing_documents_by_name(mpr, module_id, "Constants$Constant")?;
    let mut declared = HashSet::new();
    for declaration in constants {
        if !declared.insert(declaration.name.as_str()) {
            return Err(crate::WriterError::DuplicateConstant {
                module_name: module_name.to_string(),
                name: declaration.name.clone(),
            });
        }
        let qualified_name = format!("{module_name}.{}", declaration.name);
        let existing = existing_by_name.get(&declaration.name);
        let constant_id = existing
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::Constant, &qualified_name));
        let document = constant_document(
            declaration,
            &qualified_name,
            &constant_id,
            existing.map(|(_, document)| document),
            identity,
        );
        if existing.is_some() {
            mpr.update_unit(&constant_id, document)?;
        } else {
            mpr.insert_unit(module_id, "Documents", document, Some(&constant_id))?;
        }
    }
    Ok(())
}

fn constant_document(
    declaration: &ConstantDecl,
    qualified_name: &str,
    constant_id: &str,
    existing: Option<&Document>,
    identity: ProjectIdentity,
) -> Document {
    // The `Type` sub-document keeps its own `$ID` across re-syncs even when
    // the declared type changes: Studio Pro treats it as the same slot, and
    // reassigning the ID would make an otherwise field-level edit look like a
    // replaced node.
    let type_id = existing
        .and_then(|document| document.get_document("Type").ok())
        .and_then(|document| document.get("$ID"))
        .and_then(extract_id)
        .unwrap_or_else(|| identity.artifact_id(ArtifactKind::ConstantType, qualified_name));

    let mut document = existing.cloned().unwrap_or_default();
    document.insert("$ID", constant_id);
    document.insert("$Type", "Constants$Constant");
    document.insert("Name", declaration.name.clone());
    document.insert("Documentation", declaration.documentation.clone());
    if existing.is_none() {
        document.insert("Excluded", false);
        document.insert("ExportLevel", "Hidden");
    }
    document.insert("ExposedToClient", declaration.exposed_to_client);
    let mut type_document = existing
        .and_then(|document| document.get_document("Type").ok())
        .cloned()
        .unwrap_or_default();
    type_document.insert("$ID", type_id);
    type_document.insert("$Type", constant_type_name(declaration.constant_type));
    document.insert("Type", type_document);
    if let Some(value) = &declaration.value {
        document.insert("DefaultValue", value.clone());
    }
    document
}

/// Mirrors mxrb's `Writer::CONSTANT_TYPE_MAP`.
fn constant_type_name(constant_type: ConstantType) -> &'static str {
    match constant_type {
        ConstantType::String => "DataTypes$StringType",
        ConstantType::Integer => "DataTypes$IntegerType",
        ConstantType::Boolean => "DataTypes$BooleanType",
        ConstantType::Decimal => "DataTypes$DecimalType",
        ConstantType::DateTime => "DataTypes$DateTimeType",
    }
}

pub(crate) fn synchronize_regular_expressions_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    declarations: &[RegularExpressionDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    if declarations.is_empty() {
        return Ok(());
    }
    let existing_by_name =
        existing_documents_by_name(mpr, module_id, "RegularExpressions$RegularExpression")?;
    let mut declared = HashSet::new();
    for declaration in declarations {
        if !declared.insert(declaration.name.as_str()) {
            return Err(crate::WriterError::DuplicateRegularExpression {
                module_name: module_name.to_string(),
                name: declaration.name.clone(),
            });
        }
        let qualified_name = format!("{module_name}.{}", declaration.name);
        let existing = existing_by_name.get(&declaration.name);
        let id = existing.map_or_else(
            || identity.artifact_id(ArtifactKind::RegularExpression, &qualified_name),
            |(id, _)| id.clone(),
        );
        let mut document = existing
            .map(|(_, document)| document.clone())
            .unwrap_or_default();
        document.insert("$ID", id.clone());
        document.insert("$Type", "RegularExpressions$RegularExpression");
        document.insert("Documentation", declaration.documentation.clone());
        document.insert("Excluded", declaration.excluded);
        document.insert(
            "ExportLevel",
            match declaration.export_level {
                ExportLevel::Hidden => "Hidden",
                ExportLevel::Published => "Published",
            },
        );
        document.insert("Expression", declaration.expression.clone());
        document.insert("Name", declaration.name.clone());
        if existing.is_some() {
            mpr.update_unit(&id, document)?;
        } else {
            mpr.insert_unit(module_id, "Documents", document, Some(&id))?;
        }
    }
    Ok(())
}

/// Upserts `ScheduledEvents$ScheduledEvent` documents, mirroring
/// `Writer#scheduled_event_doc` + `#scheduled_event_schedule_doc`.
pub(crate) fn synchronize_scheduled_events_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    events: &[ScheduledEventDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    let existing_by_name =
        existing_documents_by_name(mpr, module_id, "ScheduledEvents$ScheduledEvent")?;
    let mut declared = HashSet::new();
    for declaration in events {
        if !declared.insert(declaration.name.as_str()) {
            return Err(crate::WriterError::DuplicateScheduledEvent {
                module_name: module_name.to_string(),
                name: declaration.name.clone(),
            });
        }
        validate_scheduled_event(declaration)?;
        let qualified_name = format!("{module_name}.{}", declaration.name);
        let existing = existing_by_name.get(&declaration.name);
        let event_id = existing
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::ScheduledEvent, &qualified_name));
        let document = scheduled_event_document(
            declaration,
            module_name,
            &qualified_name,
            &event_id,
            existing.map(|(_, document)| document),
            identity,
        );
        if existing.is_some() {
            mpr.update_unit(&event_id, document)?;
        } else {
            mpr.insert_unit(module_id, "Documents", document, Some(&event_id))?;
        }
    }
    Ok(())
}

fn validate_scheduled_event(declaration: &ScheduledEventDecl) -> Result<()> {
    if declaration.microflow.trim().is_empty() {
        return Err(crate::WriterError::ScheduledEventWithoutMicroflow(
            declaration.name.clone(),
        ));
    }
    if declaration.interval <= 0 {
        return Err(crate::WriterError::InvalidScheduleInterval {
            name: declaration.name.clone(),
            interval: declaration.interval,
        });
    }
    if declaration.unit == ScheduleUnit::Days && declaration.interval != 1 {
        return Err(crate::WriterError::UnsupportedDayInterval {
            name: declaration.name.clone(),
            interval: declaration.interval,
        });
    }
    Ok(())
}

/// mxrb defaults `StartDateTime` to `Time.utc(2000, 1, 1)` rather than to the
/// current time, so repeated writes of the same declaration stay byte-stable.
const DEFAULT_START_DATE_TIME_MILLIS: i64 = 946_684_800_000;

fn scheduled_event_document(
    declaration: &ScheduledEventDecl,
    module_name: &str,
    qualified_name: &str,
    event_id: &str,
    existing: Option<&Document>,
    identity: ProjectIdentity,
) -> Document {
    let schedule_id = existing
        .and_then(|document| document.get_document("Schedule").ok())
        .and_then(|document| document.get("$ID"))
        .and_then(extract_id)
        .unwrap_or_else(|| {
            identity.artifact_id(ArtifactKind::ScheduledEventSchedule, qualified_name)
        });
    // A start date already in the model is the author's, not ours: only a
    // document we are creating gets the default epoch.
    let start_date_time = existing
        .and_then(|document| document.get_datetime("StartDateTime").ok().copied())
        .unwrap_or_else(|| DateTime::from_millis(DEFAULT_START_DATE_TIME_MILLIS));

    let mut document = existing.cloned().unwrap_or_default();
    document.insert("$ID", event_id);
    document.insert("$Type", "ScheduledEvents$ScheduledEvent");
    document.insert("Name", declaration.name.clone());
    document.insert("Documentation", declaration.documentation.clone());
    document.insert("Excluded", document.get_bool("Excluded").unwrap_or(false));
    document.insert(
        "ExportLevel",
        document
            .get_str("ExportLevel")
            .unwrap_or("Hidden")
            .to_string(),
    );
    document.insert(
        "Microflow",
        qualify_microflow(module_name, &declaration.microflow),
    );
    document.insert("StartDateTime", start_date_time);
    document.insert("TimeZone", declaration.time_zone.clone());
    document.insert("Schedule", schedule_document(declaration, &schedule_id));
    document.insert("OnOverlap", on_overlap_name(declaration.on_overlap));
    document.insert("Enabled", declaration.enabled);
    document.insert("IntervalType", interval_type_name(declaration.unit));
    document.insert("Interval", declaration.interval);
    document
}

/// Stores the microflow reference qualified. mxrb's scheduler qualifies an
/// unqualified name against the owning module when it reads one, so both
/// spellings resolve — writing the qualified form means the document is
/// unambiguous on its own.
fn qualify_microflow(module_name: &str, microflow: &str) -> String {
    if microflow.contains('.') {
        microflow.to_string()
    } else {
        format!("{module_name}.{microflow}")
    }
}

/// Mirrors `Writer#scheduled_event_schedule_doc`. `interval` is already
/// validated by [`validate_scheduled_event`].
fn schedule_document(declaration: &ScheduledEventDecl, schedule_id: &str) -> Document {
    match declaration.unit {
        ScheduleUnit::Minutes => doc! {
            "$ID": schedule_id,
            "$Type": "ScheduledEvents$MinuteSchedule",
            "Multiplier": declaration.interval,
        },
        ScheduleUnit::Hours => doc! {
            "$ID": schedule_id,
            "$Type": "ScheduledEvents$HourSchedule",
            "Multiplier": declaration.interval,
            "MinuteOffset": 0_i32,
        },
        ScheduleUnit::Days => doc! {
            "$ID": schedule_id,
            "$Type": "ScheduledEvents$DaySchedule",
            "HourOfDay": 0_i32,
            "MinuteOfHour": 0_i32,
        },
    }
}

/// Mirrors the reachable subset of mxrb's `SCHEDULED_EVENT_INTERVAL_MAP` —
/// see [`ScheduleUnit`] for why the other five values have no surface.
fn interval_type_name(unit: ScheduleUnit) -> &'static str {
    match unit {
        ScheduleUnit::Minutes => "Minute",
        ScheduleUnit::Hours => "Hour",
        ScheduleUnit::Days => "Day",
    }
}

fn on_overlap_name(on_overlap: OnOverlap) -> &'static str {
    match on_overlap {
        OnOverlap::SkipNext => "SkipNext",
        OnOverlap::DelayNext => "DelayNext",
    }
}
