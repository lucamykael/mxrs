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
//! document just because it is absent from the declared list. A matching flow
//! retains its unit ID and folder. Its declaration replaces the body and
//! signature; native header metadata and matching parameter/type/collection
//! IDs and fields outside the typed surface survive synchronization.
//!
//! Imported enumerations, constants, and regular expressions may live under
//! arbitrarily nested `Projects$Folder` units. Their lookup therefore walks
//! the containment tree and preserves the original unit ID and container when
//! applying an editable declaration.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mxrs_bson::{Bson, DateTime, Document, build_array, extract_id, parse_array};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::flow::MicroflowDecl;
use mxrs_ir::page::PageDecl;
use mxrs_ir::{
    ConstantDecl, ConstantType, EnumerationDecl, ExportLevel, LocalizedText, MenuActionDecl,
    MenuDecl, MenuIconDecl, MenuItemDecl, OnOverlap, OqlViewSourceDecl, RegularExpressionDecl,
    ScheduleUnit, ScheduledEventDecl, ScheduledEventSchedule,
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
    crate::flow_contract::validate_documents(mpr, module_name, microflows, false)?;
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
    crate::flow_contract::validate_documents(mpr, module_name, nanoflows, true)?;
    synchronize_nanoflows_with_identity(mpr, module_id, module_name, nanoflows, identity)
}

/// Upserts rules by name as microflows are, written with the fields a rule
/// stores: no roles and no concurrency of its own, as a decision runs it.
pub(crate) fn synchronize_rules_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    rules: &[MicroflowDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    synchronize_flows_with_identity(
        mpr,
        module_id,
        module_name,
        rules,
        identity,
        "Microflows$Rule",
        ArtifactKind::Rule,
    )
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

/// Whether a flow document already allows exactly `roles`, in whatever
/// order it lists them: the order is the model's, not something a
/// declaration states.
fn same_roles(document: &mxrs_bson::Document, roles: &[String]) -> bool {
    let stored: std::collections::BTreeSet<&str> = match document.get("AllowedModuleRoles") {
        Some(mxrs_bson::Bson::Array(items)) => {
            items.iter().filter_map(mxrs_bson::Bson::as_str).collect()
        }
        _ => return false,
    };
    stored == roles.iter().map(String::as_str).collect()
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
    let existing_by_name = existing_documents_by_name(mpr, module_id, native_type)?;
    let mut updates = Vec::new();

    for decl in declarations {
        let (objects, flows) = flow_compiler::build_flow_graph(
            &decl.activities,
            &decl.rescue_activities,
            decl.return_expression.as_deref(),
            native_type == "Microflows$Nanoflow",
        );
        let previous = existing_by_name.get(&decl.name);
        let existing_id = previous.map(|(id, _)| id.clone());
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
            allowed_module_roles: decl.allowed_roles.clone().unwrap_or_default(),
            parameters: vec![],
            return_type_document: crate::flow_compiler::return_type_document(
                decl.return_type.as_ref(),
            ),
            return_type: None,
            objects,
            flows,
        };
        let fresh = microflow.to_bson_as(native_type);
        let mut doc = previous
            .map(|(_, doc)| doc.clone())
            .unwrap_or_else(|| fresh.clone());
        // Matching linear bodies keep native node identities and layout.
        let merged = previous.and_then(|(_, old)| crate::flow_graph::merge(old, &fresh));
        let same_parameters =
            previous.is_some_and(|(_, old)| crate::flow_graph::same_parameters(old, decl));
        doc.insert("Name", decl.name.clone());
        doc.insert("Documentation", decl.documentation.clone());
        // Who may run the flow is the declaration's to say when it says it;
        // otherwise the model keeps the roles it has.
        if let Some(roles) = &decl.allowed_roles
            && !same_roles(&doc, roles)
        {
            doc.insert(
                "AllowedModuleRoles",
                mxrs_bson::build_array(
                    roles.iter().cloned().map(mxrs_bson::Bson::String).collect(),
                    1,
                ),
            );
        }
        doc.insert(
            "MicroflowReturnType",
            crate::flow_graph::merge_return_type(&doc, &fresh),
        );
        if let Some(collection) = merged {
            doc.insert("ObjectCollection", collection);
        } else {
            doc.insert(
                "ObjectCollection",
                fresh.get("ObjectCollection").expect("objects").clone(),
            );
            doc.insert("Flows", fresh.get("Flows").expect("flows").clone());
            if let Some((_, old)) = previous {
                // A structural edit rebuilds the graph: the notes stay, the
                // rest of the drawing is new, and the build says so.
                let notes = old
                    .get_document("ObjectCollection")
                    .map(crate::flow_graph::annotations)
                    .unwrap_or_default();
                eprintln!(
                    "[mxrs] warning: {module_name}.{}: the body's structure changed, so its \
                     graph was rebuilt; {} annotation(s) kept without the lines that attached \
                     them, and activity captions, documentation, colours and layout reset",
                    decl.name,
                    notes.len()
                );
                doc.get_document_mut("ObjectCollection")
                    .expect("objects")
                    .get_array_mut("Objects")
                    .expect("object array")
                    .extend(notes.into_iter().map(mxrs_bson::Bson::Document));
            }
            if same_parameters && let Some((_, old)) = previous {
                // Parameters are graph objects; retain them even when a
                // structural body edit requires a fresh activity graph.
                let legacy = old
                    .get_document("ObjectCollection")
                    .ok()
                    .and_then(|c| c.get("Objects"))
                    .and_then(crate::flow_graph::documents)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|o| o.get_str("$Type").ok() == Some("Microflows$MicroflowParameter"))
                    .cloned()
                    .map(mxrs_bson::Bson::Document);
                doc.get_document_mut("ObjectCollection")
                    .expect("objects")
                    .get_array_mut("Objects")
                    .expect("object array")
                    .extend(legacy);
            }
        }
        if !same_parameters {
            let parameters =
                crate::flow_parameters::lower(decl, &id, previous.map(|(_, doc)| doc), identity)?;
            crate::flow_parameters::place(&mut doc, parameters);
        } else if crate::flow_parameters::collected(&doc) {
            let parameters = Microflow::from_bson(&doc)
                .parameters
                .into_iter()
                .map(mxrs_bson::Bson::Document)
                .collect();
            crate::flow_parameters::place(&mut doc, parameters);
        }
        if native_type == "Microflows$Rule" {
            // A decision runs a rule: who may run it and how often is the
            // flow's that asks.
            doc.remove("AllowConcurrentExecution");
            doc.remove("AllowedModuleRoles");
        }
        updates.push((existing_id, id, doc));
    }
    for (existing_id, id, doc) in updates {
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
    // mxrb's write-time widget-schema synchronization roots the MPK lookup
    // at the target's own directory (`WidgetPackage.find(File.dirname(@path))`).
    let packages_root = mpr.path().parent().map(std::path::Path::to_path_buf);

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
        let mut document = page_compiler::compile_page(&catalog, decl, packages_root.as_deref())?;
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

/// Writes the image collections a module declares: a stored one keeps its
/// identities and its images theirs, matched by name, and is left as stored
/// when it says what is stored.
pub(crate) fn synchronize_image_collections_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    declarations: &[mxrs_ir::ImageCollectionDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    if declarations.is_empty() {
        return Ok(());
    }
    let existing = existing_documents_by_name(mpr, module_id, "Images$ImageCollection")?;
    for declaration in declarations {
        let qualified_name = format!("{module_name}.{}", declaration.name);
        let previous = existing.get(&declaration.name);
        let id = previous.map(|(id, _)| id.clone()).unwrap_or_else(|| {
            identity.artifact_id(ArtifactKind::ImageCollection, &qualified_name)
        });
        let stored_images: HashMap<String, String> = previous
            .and_then(|(_, document)| document.get_array("Images").ok())
            .map(|items| {
                items
                    .iter()
                    .filter_map(Bson::as_document)
                    .filter_map(|image| {
                        Some((
                            image.get_str("Name").ok()?.to_string(),
                            extract_id(image.get("$ID")?)?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let images = declaration
            .images
            .iter()
            .map(|image| {
                let image_id = stored_images.get(&image.name).cloned().unwrap_or_else(|| {
                    identity.artifact_id(
                        ArtifactKind::Image,
                        &format!("{qualified_name}.{}", image.name),
                    )
                });
                Bson::Document(mxrs_bson::doc! {
                    "$ID": blob(&image_id),
                    "$Type": "Images$Image",
                    "Image": Bson::Binary(mxrs_bson::Binary {
                        subtype: mxrs_bson::BinarySubtype::Generic,
                        bytes: image.data.clone(),
                    }),
                    "ImageFormat": image.format.as_str(),
                    "Name": image.name.clone(),
                })
            })
            .collect();
        let document = mxrs_bson::doc! {
            "$ID": blob(&id),
            "$Type": "Images$ImageCollection",
            "Documentation": declaration.documentation.clone(),
            "Excluded": declaration.excluded,
            "ExportLevel": match declaration.export_level {
                ExportLevel::Hidden => "Hidden",
                ExportLevel::Published => "Published",
            },
            "Images": build_array(images, 3),
            "Name": declaration.name.clone(),
        };
        match previous {
            Some((_, stored)) if *stored == document => {}
            Some(_) => {
                mpr.update_unit(&id, document)?;
            }
            None => {
                mpr.insert_unit(module_id, "Documents", document, Some(&id))?;
            }
        }
    }
    Ok(())
}

/// A stored identity: the binary form Mendix keeps it in.
fn blob(id: &str) -> Bson {
    match mxrs_bson::uuid_to_blob(id) {
        Ok(bytes) => Bson::Binary(mxrs_bson::Binary {
            subtype: mxrs_bson::BinarySubtype::Generic,
            bytes: bytes.to_vec(),
        }),
        Err(_) => Bson::String(id.to_string()),
    }
}

pub(crate) fn synchronize_oql_view_sources_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    declarations: &[OqlViewSourceDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    if declarations.is_empty() {
        return Ok(());
    }
    let existing =
        existing_documents_by_name(mpr, module_id, "DomainModels$ViewEntitySourceDocument")?;
    for declaration in declarations {
        let qualified_name = format!("{module_name}.{}", declaration.name);
        let previous = existing.get(&declaration.name);
        let id = previous
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::OqlViewSource, &qualified_name));
        let mut document = previous
            .map(|(_, document)| document.clone())
            .unwrap_or_default();
        document.insert("$ID", id.clone());
        document.insert("$Type", "DomainModels$ViewEntitySourceDocument");
        document.insert("Name", declaration.name.clone());
        document.insert("Oql", declaration.query.clone());
        // A version that stores none of these leaves them out: a stored
        // document gains one only when the declaration says other than its
        // default.
        let stored = previous.is_some();
        let mut option = |key: &str, value: Bson, default: bool| {
            if !stored || document.contains_key(key) || !default {
                document.insert(key, value);
            }
        };
        option(
            "Documentation",
            Bson::String(declaration.documentation.clone()),
            declaration.documentation.is_empty(),
        );
        option(
            "Excluded",
            Bson::Boolean(declaration.excluded),
            !declaration.excluded,
        );
        option(
            "ExportLevel",
            Bson::String(
                match declaration.export_level {
                    ExportLevel::Hidden => "Hidden",
                    ExportLevel::Published => "Published",
                }
                .to_string(),
            ),
            declaration.export_level == ExportLevel::Hidden,
        );
        if previous.is_some() {
            mpr.update_unit(&id, document)?;
        } else {
            mpr.insert_unit(module_id, "Documents", document, Some(&id))?;
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
pub(crate) fn existing_documents_by_name(
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

/// Mirrors mxrb's `ArtifactDocuments#task_queue`; retained native fields stay
/// outside the editable declaration, including fields in the nested config.
pub(crate) fn synchronize_task_queues_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    declarations: &[mxrs_ir::TaskQueueDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    use mxrs_ir::{TaskQueueConfig, TaskQueueScope};

    if declarations.is_empty() {
        return Ok(());
    }
    let existing_by_name = existing_documents_by_name(mpr, module_id, "Queues$Queue")?;
    let mut declared = HashSet::new();
    for declaration in declarations {
        let invalid = |reason: &str| crate::WriterError::InvalidTaskQueue {
            name: format!("{module_name}.{}", declaration.name),
            reason: reason.to_string(),
        };
        if declaration.name.trim().is_empty() {
            return Err(invalid("name must not be empty"));
        }
        if !declared.insert(declaration.name.as_str()) {
            return Err(crate::WriterError::DuplicateTaskQueue {
                module_name: module_name.to_string(),
                name: declaration.name.clone(),
            });
        }
        match &declaration.config {
            TaskQueueConfig::Fixed { parallelism }
                if *parallelism == 0 || *parallelism > i32::MAX as u32 =>
            {
                return Err(invalid("parallelism must be between 1 and 2147483647"));
            }
            TaskQueueConfig::Dynamic {
                parallelism_expression,
                ..
            } if parallelism_expression.trim().is_empty() => {
                return Err(invalid("parallelism expression must not be empty"));
            }
            _ => {}
        }
        let qualified_name = format!("{module_name}.{}", declaration.name);
        let existing = existing_by_name.get(&declaration.name);
        let id = existing.map_or_else(
            || identity.artifact_id(ArtifactKind::TaskQueue, &qualified_name),
            |(id, _)| id.clone(),
        );
        let mut document = existing
            .map(|(_, document)| document.clone())
            .unwrap_or_default();
        let mut config = match document.get("Config") {
            Some(Bson::Document(config))
                if config.get_str("$Type").ok() == Some("Queues$BasicQueueConfig") =>
            {
                config.clone()
            }
            Some(_) => {
                return Err(invalid(
                    "existing queue configuration is not a supported basic config",
                ));
            }
            None if existing.is_some() => {
                return Err(invalid("existing queue configuration is missing"));
            }
            None => Document::new(),
        };
        let config_id = config.get("$ID").and_then(extract_id).unwrap_or_else(|| {
            identity.artifact_id(ArtifactKind::TaskQueueConfig, &qualified_name)
        });
        config.insert("$ID", config_id);
        config.insert("$Type", "Queues$BasicQueueConfig");
        match &declaration.config {
            TaskQueueConfig::Fixed { parallelism } => {
                config.remove("ParallelismExpression");
                config.remove("ClusterWide");
                let value = if matches!(config.get("Parallelism"), Some(Bson::Int64(_))) {
                    Bson::Int64(i64::from(*parallelism))
                } else {
                    Bson::Int32(*parallelism as i32)
                };
                config.insert("Parallelism", value);
            }
            TaskQueueConfig::Dynamic {
                parallelism_expression,
                scope,
            } => {
                config.remove("Parallelism");
                config.insert("ParallelismExpression", parallelism_expression.clone());
                config.insert("ClusterWide", *scope == TaskQueueScope::ClusterWide);
            }
        }
        document.insert("$ID", id.clone());
        document.insert("$Type", "Queues$Queue");
        document.insert("Name", declaration.name.clone());
        document.insert("Documentation", declaration.documentation.clone());
        document.insert("Excluded", declaration.excluded);
        document.insert(
            "ExportLevel",
            match declaration.export_level {
                ExportLevel::Hidden => "Hidden",
                ExportLevel::Published => "Published",
            },
        );
        document.insert("Config", config);
        if existing.is_some() {
            mpr.update_unit(&id, document)?;
        } else {
            mpr.insert_unit(module_id, "Documents", document, Some(&id))?;
        }
    }
    Ok(())
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
        )?;
        if existing.is_some() {
            mpr.update_unit(&event_id, document)?;
        } else {
            mpr.insert_unit(module_id, "Documents", document, Some(&event_id))?;
        }
    }
    Ok(())
}

fn validate_scheduled_event(declaration: &ScheduledEventDecl) -> Result<()> {
    if declaration.microflow.trim().is_empty() && declaration.enabled {
        return Err(crate::WriterError::ScheduledEventWithoutMicroflow(
            declaration.name.clone(),
        ));
    }
    if declaration.interval < 0 {
        return Err(crate::WriterError::InvalidScheduleInterval {
            name: declaration.name.clone(),
            interval: declaration.interval,
        });
    }
    validate_schedule_value(
        declaration,
        "multiplier",
        |schedule| match schedule {
            ScheduledEventSchedule::Minute { multiplier }
            | ScheduledEventSchedule::Hour { multiplier, .. } => Some(*multiplier),
            _ => None,
        },
        1..=i64::MAX,
    )?;
    validate_schedule_value(
        declaration,
        "minute offset",
        |schedule| match schedule {
            ScheduledEventSchedule::Hour { minute_offset, .. } => Some(*minute_offset),
            _ => None,
        },
        0..=59,
    )?;
    validate_schedule_value(
        declaration,
        "hour of day",
        |schedule| match schedule {
            ScheduledEventSchedule::Day { hour_of_day, .. }
            | ScheduledEventSchedule::Week { hour_of_day, .. } => Some(*hour_of_day),
            _ => None,
        },
        0..=23,
    )?;
    validate_schedule_value(
        declaration,
        "minute of hour",
        |schedule| match schedule {
            ScheduledEventSchedule::Day { minute_of_hour, .. }
            | ScheduledEventSchedule::Week { minute_of_hour, .. } => Some(*minute_of_hour),
            _ => None,
        },
        0..=59,
    )?;
    Ok(())
}

fn validate_schedule_value(
    declaration: &ScheduledEventDecl,
    field: &'static str,
    value: impl FnOnce(&ScheduledEventSchedule) -> Option<i64>,
    range: std::ops::RangeInclusive<i64>,
) -> Result<()> {
    let Some(value) = value(&declaration.schedule) else {
        return Ok(());
    };
    if range.contains(&value) {
        Ok(())
    } else {
        Err(crate::WriterError::InvalidScheduledEventScheduleValue {
            name: declaration.name.clone(),
            field,
            value,
        })
    }
}

fn scheduled_event_document(
    declaration: &ScheduledEventDecl,
    module_name: &str,
    qualified_name: &str,
    event_id: &str,
    existing: Option<&Document>,
    identity: ProjectIdentity,
) -> Result<Document> {
    let schedule_id = existing
        .and_then(|document| document.get_document("Schedule").ok())
        .and_then(|document| document.get("$ID"))
        .and_then(extract_id)
        .unwrap_or_else(|| {
            identity.artifact_id(ArtifactKind::ScheduledEventSchedule, qualified_name)
        });
    let start_date_time = DateTime::parse_rfc3339_str(&declaration.start_at).map_err(|_| {
        crate::WriterError::InvalidScheduledEventStart {
            name: declaration.name.clone(),
            value: declaration.start_at.clone(),
        }
    })?;

    let mut document = existing.cloned().unwrap_or_default();
    document.insert("$ID", event_id);
    document.insert("$Type", "ScheduledEvents$ScheduledEvent");
    document.insert("Name", declaration.name.clone());
    document.insert("Documentation", declaration.documentation.clone());
    document.insert("Excluded", declaration.excluded);
    document.insert(
        "ExportLevel",
        match declaration.export_level {
            ExportLevel::Hidden => "Hidden",
            ExportLevel::Published => "Published",
        },
    );
    document.insert(
        "Microflow",
        qualify_microflow(module_name, &declaration.microflow),
    );
    document.insert("StartDateTime", start_date_time);
    document.insert("TimeZone", declaration.time_zone.clone());
    document.insert(
        "Schedule",
        schedule_document(
            declaration,
            &schedule_id,
            existing.and_then(|document| document.get_document("Schedule").ok()),
        ),
    );
    document.insert("OnOverlap", on_overlap_name(declaration.on_overlap));
    document.insert("Enabled", declaration.enabled);
    document.insert("IntervalType", interval_type_name(declaration.unit));
    document.insert("Interval", declaration.interval);
    Ok(document)
}

/// Stores the microflow reference qualified. mxrb's scheduler qualifies an
/// unqualified name against the owning module when it reads one, so both
/// spellings resolve — writing the qualified form means the document is
/// unambiguous on its own.
fn qualify_microflow(module_name: &str, microflow: &str) -> String {
    if microflow.trim().is_empty() {
        String::new()
    } else if microflow.contains('.') {
        microflow.to_string()
    } else {
        format!("{module_name}.{microflow}")
    }
}

/// Lowers the closed modern schedule variants independently of the retained
/// legacy interval fields.
fn schedule_document(
    declaration: &ScheduledEventDecl,
    schedule_id: &str,
    previous: Option<&Document>,
) -> Bson {
    let expected_type = match declaration.schedule {
        ScheduledEventSchedule::None => return Bson::Null,
        ScheduledEventSchedule::Minute { .. } => "ScheduledEvents$MinuteSchedule",
        ScheduledEventSchedule::Hour { .. } => "ScheduledEvents$HourSchedule",
        ScheduledEventSchedule::Day { .. } => "ScheduledEvents$DaySchedule",
        ScheduledEventSchedule::Week { .. } => "ScheduledEvents$WeekSchedule",
    };
    let mut document = previous
        .filter(|document| document.get_str("$Type").ok() == Some(expected_type))
        .cloned()
        .unwrap_or_default();
    document.insert("$ID", schedule_id);
    document.insert("$Type", expected_type);
    match declaration.schedule {
        ScheduledEventSchedule::None => unreachable!("returned above"),
        ScheduledEventSchedule::Minute { multiplier } => {
            document.insert("Multiplier", multiplier);
        }
        ScheduledEventSchedule::Hour {
            multiplier,
            minute_offset,
        } => {
            document.insert("Multiplier", multiplier);
            document.insert("MinuteOffset", minute_offset);
        }
        ScheduledEventSchedule::Day {
            hour_of_day,
            minute_of_hour,
        } => {
            document.insert("HourOfDay", hour_of_day);
            document.insert("MinuteOfHour", minute_of_hour);
        }
        ScheduledEventSchedule::Week {
            hour_of_day,
            minute_of_hour,
            monday,
            tuesday,
            wednesday,
            thursday,
            friday,
            saturday,
            sunday,
        } => {
            document.insert("HourOfDay", hour_of_day);
            document.insert("MinuteOfHour", minute_of_hour);
            document.insert("Monday", monday);
            document.insert("Tuesday", tuesday);
            document.insert("Wednesday", wednesday);
            document.insert("Thursday", thursday);
            document.insert("Friday", friday);
            document.insert("Saturday", saturday);
            document.insert("Sunday", sunday);
        }
    }
    Bson::Document(document)
}

/// Mirrors the complete legacy interval vocabulary.
fn interval_type_name(unit: ScheduleUnit) -> &'static str {
    match unit {
        ScheduleUnit::Milliseconds => "Millisecond",
        ScheduleUnit::Seconds => "Second",
        ScheduleUnit::Minutes => "Minute",
        ScheduleUnit::Hours => "Hour",
        ScheduleUnit::Days => "Day",
        ScheduleUnit::Weeks => "Week",
        ScheduleUnit::Months => "Month",
        ScheduleUnit::Years => "Year",
    }
}

fn on_overlap_name(on_overlap: OnOverlap) -> &'static str {
    match on_overlap {
        OnOverlap::SkipNext => "SkipNext",
        OnOverlap::DelayNext => "DelayNext",
    }
}

/// Upserts complete standalone `Menus$MenuDocument` declarations. Nested
/// identities and Mendix array markers are retained on imported documents;
/// fresh menus receive deterministic private identities.
pub(crate) fn synchronize_menus_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    menus: &[MenuDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    if menus.is_empty() {
        return Ok(());
    }
    let existing_by_name = existing_documents_by_name(mpr, module_id, "Menus$MenuDocument")?;
    let mut declared = HashSet::new();
    for declaration in menus {
        if !declared.insert(declaration.name.as_str()) {
            return Err(crate::WriterError::DuplicateMenu {
                module_name: module_name.to_string(),
                name: declaration.name.clone(),
            });
        }
        let qualified_name = format!("{module_name}.{}", declaration.name);
        let existing = existing_by_name.get(&declaration.name);
        let id = existing
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::Menu, &qualified_name));
        let document = menu_document(
            declaration,
            module_name,
            &qualified_name,
            &id,
            existing.map(|(_, document)| document),
            identity,
        );
        if existing.is_some() {
            mpr.update_unit(&id, document)?;
        } else {
            mpr.insert_unit(module_id, "Documents", document, Some(&id))?;
        }
    }
    Ok(())
}

fn menu_document(
    declaration: &MenuDecl,
    module_name: &str,
    qualified_name: &str,
    id: &str,
    previous: Option<&Document>,
    identity: ProjectIdentity,
) -> Document {
    let mut document = previous.cloned().unwrap_or_default();
    document.insert("$ID", id);
    document.insert("$Type", "Menus$MenuDocument");
    document.insert("Name", declaration.name.clone());
    document.insert("Documentation", declaration.documentation.clone());
    document.insert("Excluded", declaration.excluded);
    document.insert("ExportLevel", export_level_name(declaration.export_level));

    let previous_collection = previous.and_then(|value| value.get_document("ItemCollection").ok());
    let mut collection = previous_collection.cloned().unwrap_or_default();
    nested_id(
        &mut collection,
        identity,
        ArtifactKind::MenuCollection,
        qualified_name,
    );
    collection.insert("$Type", "Menus$MenuItemCollection");
    collection.insert(
        "Items",
        menu_items_document(
            &declaration.items,
            previous_collection.and_then(|value| value.get("Items")),
            module_name,
            qualified_name,
            identity,
        ),
    );
    document.insert("ItemCollection", collection);
    document
}

fn menu_items_document(
    declarations: &[MenuItemDecl],
    previous: Option<&Bson>,
    module_name: &str,
    key: &str,
    identity: ProjectIdentity,
) -> Bson {
    let (marker, previous_items) = bson_document_array(previous, 3);
    let items = declarations
        .iter()
        .enumerate()
        .map(|(index, declaration)| {
            let item_key = format!("{key}:item:{index}");
            Bson::Document(menu_item_document(
                declaration,
                previous_items.get(index),
                module_name,
                &item_key,
                identity,
            ))
        })
        .collect();
    Bson::Array(build_array(items, marker))
}

fn menu_item_document(
    declaration: &MenuItemDecl,
    previous: Option<&Document>,
    module_name: &str,
    key: &str,
    identity: ProjectIdentity,
) -> Document {
    let mut document = previous.cloned().unwrap_or_default();
    nested_id(&mut document, identity, ArtifactKind::MenuItem, key);
    document.insert("$Type", "Menus$MenuItem");
    document.insert(
        "Caption",
        menu_text_document(
            &declaration.caption,
            previous.and_then(|value| value.get_document("Caption").ok()),
            &format!("{key}:caption"),
            identity,
        ),
    );
    document.insert(
        "AlternativeText",
        declaration
            .alternative_text
            .as_ref()
            .map_or(Bson::Null, |text| {
                Bson::Document(menu_text_document(
                    text,
                    previous.and_then(|value| value.get_document("AlternativeText").ok()),
                    &format!("{key}:alternative"),
                    identity,
                ))
            }),
    );
    document.insert(
        "Action",
        menu_action_document(
            &declaration.action,
            previous.and_then(|value| value.get_document("Action").ok()),
            module_name,
            &format!("{key}:action"),
            identity,
        ),
    );
    document.insert(
        "Icon",
        menu_icon_document(
            declaration.icon.as_ref(),
            previous.and_then(|value| value.get_document("Icon").ok()),
            &format!("{key}:icon"),
            identity,
        ),
    );
    document.insert(
        "Items",
        menu_items_document(
            &declaration.items,
            previous.and_then(|value| value.get("Items")),
            module_name,
            key,
            identity,
        ),
    );
    document
}

fn menu_icon_document(
    declaration: Option<&MenuIconDecl>,
    previous: Option<&Document>,
    key: &str,
    identity: ProjectIdentity,
) -> Bson {
    let Some(declaration) = declaration else {
        return Bson::Null;
    };
    let expected_type = match declaration {
        MenuIconDecl::Glyph(_) => "Forms$GlyphIcon",
        MenuIconDecl::Image(_) => "Forms$IconCollectionIcon",
    };
    let mut document = previous
        .filter(|value| value.get_str("$Type").ok() == Some(expected_type))
        .cloned()
        .unwrap_or_default();
    nested_id(&mut document, identity, ArtifactKind::MenuIcon, key);
    document.insert("$Type", expected_type);
    match declaration {
        MenuIconDecl::Glyph(code) => document.insert("Code", *code),
        MenuIconDecl::Image(image) => document.insert("Image", image.clone()),
    };
    Bson::Document(document)
}

fn menu_action_document(
    declaration: &MenuActionDecl,
    previous: Option<&Document>,
    module_name: &str,
    key: &str,
    identity: ProjectIdentity,
) -> Document {
    let expected_type = match declaration {
        MenuActionDecl::None { .. } => "Forms$NoAction",
        MenuActionDecl::OpenPage { .. } => "Forms$FormAction",
        MenuActionDecl::CreateObjectAndOpenPage { .. } => "Forms$CreateObjectClientAction",
    };
    let mut action = previous
        .filter(|value| value.get_str("$Type").ok() == Some(expected_type))
        .cloned()
        .unwrap_or_default();
    nested_id(&mut action, identity, ArtifactKind::MenuAction, key);
    action.insert("$Type", expected_type);
    match declaration {
        MenuActionDecl::None {
            disabled_during_execution,
        } => {
            action.insert("DisabledDuringExecution", *disabled_during_execution);
        }
        MenuActionDecl::OpenPage {
            page,
            disabled_during_execution,
            pages_to_close,
            title_override,
        } => {
            action.insert("DisabledDuringExecution", *disabled_during_execution);
            action.insert(
                "FormSettings",
                menu_form_settings(
                    page,
                    title_override.as_ref(),
                    previous.and_then(|value| value.get_document("FormSettings").ok()),
                    module_name,
                    &format!("{key}:settings"),
                    identity,
                ),
            );
            action.insert(
                "NumberOfPagesToClose2",
                pages_to_close_string(*pages_to_close),
            );
            action.insert(
                "PagesForSpecializations",
                empty_array_preserving_marker(
                    previous.and_then(|value| value.get("PagesForSpecializations")),
                    2,
                ),
            );
        }
        MenuActionDecl::CreateObjectAndOpenPage {
            entity,
            page,
            disabled_during_execution,
            pages_to_close,
            title_override,
        } => {
            action.insert("DisabledDuringExecution", *disabled_during_execution);
            let previous_ref = previous.and_then(|value| value.get_document("EntityRef").ok());
            let mut entity_ref = previous_ref.cloned().unwrap_or_default();
            nested_id(
                &mut entity_ref,
                identity,
                ArtifactKind::MenuActionSettings,
                &format!("{key}:entity"),
            );
            entity_ref.insert("$Type", "DomainModels$DirectEntityRef");
            entity_ref.insert("Entity", qualify_reference(module_name, entity));
            action.insert("EntityRef", entity_ref);
            action.insert(
                "NumberOfPagesToClose2",
                pages_to_close_string(*pages_to_close),
            );
            action.insert(
                "PageSettings",
                menu_form_settings(
                    page,
                    title_override.as_ref(),
                    previous.and_then(|value| value.get_document("PageSettings").ok()),
                    module_name,
                    &format!("{key}:settings"),
                    identity,
                ),
            );
        }
    }
    action
}

fn menu_form_settings(
    page: &str,
    title_override: Option<&LocalizedText>,
    previous: Option<&Document>,
    module_name: &str,
    key: &str,
    identity: ProjectIdentity,
) -> Document {
    let mut settings = previous.cloned().unwrap_or_default();
    nested_id(
        &mut settings,
        identity,
        ArtifactKind::MenuActionSettings,
        key,
    );
    settings.insert("$Type", "Forms$FormSettings");
    settings.insert("Form", qualify_reference(module_name, page));
    settings.insert(
        "ParameterMappings",
        empty_array_preserving_marker(previous.and_then(|value| value.get("ParameterMappings")), 2),
    );
    settings.insert(
        "TitleOverride",
        title_override.map_or(Bson::Null, |translations| {
            let previous_template =
                previous.and_then(|value| value.get_document("TitleOverride").ok());
            let mut template = previous_template.cloned().unwrap_or_default();
            nested_id(
                &mut template,
                identity,
                ArtifactKind::MenuTextTemplate,
                &format!("{key}:title"),
            );
            template.insert("$Type", "Microflows$TextTemplate");
            template.insert(
                "Parameters",
                empty_array_preserving_marker(
                    previous_template.and_then(|value| value.get("Parameters")),
                    2,
                ),
            );
            template.insert(
                "Text",
                menu_text_document(
                    translations,
                    previous_template.and_then(|value| value.get_document("Text").ok()),
                    &format!("{key}:title:text"),
                    identity,
                ),
            );
            Bson::Document(template)
        }),
    );
    settings
}

fn menu_text_document(
    translations: &LocalizedText,
    previous: Option<&Document>,
    key: &str,
    identity: ProjectIdentity,
) -> Document {
    let mut text = previous.cloned().unwrap_or_default();
    nested_id(&mut text, identity, ArtifactKind::MenuText, key);
    text.insert("$Type", "Texts$Text");
    let (marker, prior_items) =
        bson_document_array(previous.and_then(|value| value.get("Items")), 3);
    let prior_by_language = prior_items
        .iter()
        .filter_map(|item| Some((item.get_str("LanguageCode").ok()?.to_string(), item)))
        .collect::<HashMap<_, _>>();
    let items = translations
        .iter()
        .map(|(language, value)| {
            let mut translation = prior_by_language
                .get(language)
                .map(|item| (*item).clone())
                .unwrap_or_default();
            nested_id(
                &mut translation,
                identity,
                ArtifactKind::Translation,
                &format!("{key}:{language}"),
            );
            translation.insert("$Type", "Texts$Translation");
            translation.insert("LanguageCode", language.clone());
            translation.insert("Text", value.clone());
            Bson::Document(translation)
        })
        .collect();
    text.insert("Items", build_array(items, marker));
    text
}

fn bson_document_array(value: Option<&Bson>, default_marker: i32) -> (i32, Vec<Document>) {
    let Some(Bson::Array(values)) = value else {
        return (default_marker, vec![]);
    };
    let parsed = parse_array(Some(values));
    let documents = parsed
        .items
        .into_iter()
        .filter_map(|value| value.as_document().cloned())
        .collect();
    (parsed.marker, documents)
}

fn empty_array_preserving_marker(previous: Option<&Bson>, default_marker: i32) -> Bson {
    let marker = previous
        .and_then(Bson::as_array)
        .map(|values| parse_array(Some(values)).marker)
        .unwrap_or(default_marker);
    Bson::Array(build_array(vec![], marker))
}

fn nested_id(document: &mut Document, identity: ProjectIdentity, kind: ArtifactKind, key: &str) {
    if document.get("$ID").and_then(extract_id).is_none() {
        document.insert("$ID", identity.artifact_id(kind, key));
    }
}

fn qualify_reference(module_name: &str, reference: &str) -> String {
    if reference.contains('.') {
        reference.to_string()
    } else {
        format!("{module_name}.{reference}")
    }
}

fn pages_to_close_string(value: Option<u32>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn export_level_name(value: ExportLevel) -> &'static str {
    match value {
        ExportLevel::Hidden => "Hidden",
        ExportLevel::Published => "Published",
    }
}
