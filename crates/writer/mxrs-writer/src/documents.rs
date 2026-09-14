//! Incremental re-sync for microflow, page, and enumeration `Documents`
//! units — mirrors the corresponding slices of `Writer#write_documents`/
//! `#upsert_document` (mxrb's
//! own method upserts pages/microflows/nanoflows/rules/menus/enumerations/
//! constants/scheduled_events; this crate now has DSL/model surface for
//! microflows, native/structural pages, and enumerations, widened
//! incrementally like the rest of the codebase — see `mxrs_ir::page`'s doc
//! comment for exactly which pages/widgets are and aren't covered yet).
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
//! Deliberately doesn't look inside `Folders` containers (mirrors
//! `collect_documents`'s recursion in mxrb): mxrs-writer itself never
//! creates folders, so every `Documents` unit it's responsible for is a
//! direct child of the module.

use std::collections::HashMap;
use std::rc::Rc;

use mxrs_bson::{Bson, Document, build_array, doc, extract_id, parse_array};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::EnumerationDecl;
use mxrs_ir::flow::MicroflowDecl;
use mxrs_ir::page::PageDecl;
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
    let existing_by_name: HashMap<String, (String, Document)> = mpr
        .children_of(module_id)?
        .into_iter()
        .filter(|unit| unit.containment_name == "Documents")
        .filter_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            if document.get_str("$Type").ok()? != "Enumerations$Enumeration" {
                return None;
            }
            let name = document.get_str("Name").ok()?.to_string();
            Some((name, (unit.unit_id, document)))
        })
        .collect();

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
            value_document.insert(
                "Image",
                value_document.get_str("Image").unwrap_or("").to_string(),
            );
            value_document.insert(
                "ExportLevel",
                value_document
                    .get_str("ExportLevel")
                    .unwrap_or("Hidden")
                    .to_string(),
            );
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
    document.insert("Excluded", document.get_bool("Excluded").unwrap_or(false));
    document.insert(
        "ExportLevel",
        document
            .get_str("ExportLevel")
            .unwrap_or("Hidden")
            .to_string(),
    );
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
            Bson::Document(doc! {
                "$ID": translation_id,
                "$Type": "Texts$Translation",
                "LanguageCode": language.clone(),
                "Text": text.clone(),
            })
        })
        .collect();
    doc! {
        "$ID": caption_id,
        "$Type": "Texts$Text",
        "Items": build_array(translations, previous_items.marker),
    }
}
