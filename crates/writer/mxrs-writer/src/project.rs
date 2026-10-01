//! Top-level entry points: create a fresh `.mpr` from an
//! `mxrs_ir::ProjectDecl`, or update an *existing* one in place.
//! `write_project` mirrors `Writer#create_project!` + iterating
//! `Writer#module_doc` per declared module; `synchronize_project` mirrors
//! the incremental-resync half of the same Ruby method — the "go back to
//! Mendix" side of the round trip `mxrs-exporter` opens the other side of
//! (an existing project read out as editable Rust source). See
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory.

use std::collections::HashSet;
use std::path::Path;

use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::declaration::ProjectDecl;
use mxrs_mpr::MprFile;

use crate::error::{Result, WriterError};
use crate::{documents, domain, module, navigation, scaffold, security};

pub fn write_project(path: impl AsRef<Path>, project: &ProjectDecl) -> Result<()> {
    crate::flow_contract::validate_project(project, &[])?;
    validate_lifecycle_handlers(project, &HashSet::new())?;
    validate_oql_sources(project, &HashSet::new())?;
    let path = path.as_ref();
    let schema_hash = mxrs_schema::schema_hash(&project.mendix_version)
        .ok_or_else(|| WriterError::UnsupportedVersion(project.mendix_version.clone()))?;
    let logical_name = path
        .file_stem()
        .map(|name| name.to_string_lossy())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Project".into());
    let identity = ProjectIdentity::for_project(&logical_name);
    let root_id = identity.project_root_id();
    let mut mpr =
        MprFile::create_with_root_id(path, &project.mendix_version, schema_hash, &root_id)?;
    let root_id = mpr
        .root_unit()?
        .ok_or(WriterError::MissingRootUnit)?
        .unit_id;

    scaffold::write_default_project_units(&mut mpr, &root_id, &project.mendix_version, identity)?;

    let known_entities = known_entities(project);

    for decl in &project.modules {
        module::write_module(
            &mut mpr,
            &root_id,
            decl,
            &known_entities,
            identity,
            &project.mendix_version,
        )?;
    }
    security::synchronize_declared_security(&mut mpr, &root_id, project, identity)?;
    if let Some(declaration) = &project.navigation {
        navigation::synchronize_navigation(&mut mpr, &root_id, declaration, identity)?;
    }
    Ok(())
}

/// Updates an *already-created* `.mpr` (typically one `mxrs-exporter` read
/// out of, that the caller edited via `mxrs-dsl`/`project! {}` and wants
/// written back) from a full `ProjectDecl`: per module, upserts the
/// `Projects$Module` unit itself (by name — creating one if the module is
/// new), then delegates to `domain::synchronize_domain_model` and
/// `documents::synchronize_microflows`, which already handle add/rename/
/// remove with `$ID` preservation on a name match. Unlike `write_project`,
/// this doesn't touch `ProjectSettings`/`Security`/`Navigation` scaffolding
/// (those already exist on a real project — `write_project`'s scaffold
/// step is only for a brand-new file with nothing in it yet) and doesn't
/// delete a module that's simply absent from `project` (mirrors
/// `synchronize_microflows`'s own upsert-only stance: an absent module
/// might be intentionally untouched, e.g. one only ever edited directly in
/// Studio Pro, not through `mxrs-exporter`).
pub fn synchronize_project(path: impl AsRef<Path>, project: &ProjectDecl) -> Result<()> {
    let path = path.as_ref();
    let existing_project = mxrs_model::Project::open(path, true)?;
    let existing_modules = existing_project.modules()?;
    let existing_microflows: HashSet<String> = existing_modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.clone().unwrap_or_else(|| "Unnamed".to_string());
            module
                .microflows
                .iter()
                .filter_map(move |flow| Some(format!("{module_name}.{}", flow.name.as_deref()?)))
        })
        .collect();
    let existing_oql_sources: HashSet<String> = existing_modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.clone().unwrap_or_else(|| "Unnamed".to_string());
            module.artifact_units.iter().filter_map(move |document| {
                if document.get_str("$Type").ok() != Some("DomainModels$ViewEntitySourceDocument") {
                    return None;
                }
                Some(format!("{module_name}.{}", document.get_str("Name").ok()?))
            })
        })
        .collect();
    // An association may point at an entity this build does not declare: a
    // module the project only edits in Studio Pro, or one it installed and
    // therefore declares nothing of. Synchronizing writes *into* this model,
    // so what the model already holds is known too. `write_project` starts
    // from nothing and keeps asking the declaration alone.
    let existing_entities: HashSet<String> = existing_modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.clone().unwrap_or_else(|| "Unnamed".to_string());
            module
                .entities()
                .iter()
                .filter_map(|entity| entity.name.clone())
                .map(move |name| format!("{module_name}.{name}"))
                .collect::<Vec<_>>()
        })
        .collect();
    crate::flow_contract::validate_project(project, &existing_modules)?;
    drop(existing_project);
    validate_lifecycle_handlers(project, &existing_microflows)?;
    validate_oql_sources(project, &existing_oql_sources)?;
    let mut mpr = MprFile::open(path, false)?;
    let root_id = mpr
        .root_unit()?
        .ok_or(WriterError::MissingRootUnit)?
        .unit_id;
    let identity = ProjectIdentity::from_project_root(&root_id)?;

    let existing_modules_by_name = existing_module_ids_by_name(&mpr, &root_id)?;
    let mut known_entities = known_entities(project);
    known_entities.extend(existing_entities);

    for decl in &project.modules {
        let module_id = match existing_modules_by_name.get(&decl.name) {
            Some(id) => id.clone(),
            None => {
                let module_id =
                    module::insert_bare_module(&mut mpr, &root_id, &decl.name, identity)?;
                // `synchronize_domain_model` resyncs an *existing*
                // `DomainModel` unit (it errors with `MissingDomainModel`
                // otherwise) — a module that's new to this project needs
                // an empty one first, same as `write_module` inserts one
                // right after the bare module unit for fresh creation.
                let empty_domain_model = mxrs_model::DomainModel {
                    id: None,
                    native_type: None,
                    documentation: String::new(),
                    entities: vec![],
                    associations: vec![],
                    cross_associations: vec![],
                };
                let domain_model_id = identity.artifact_id(ArtifactKind::DomainModel, &decl.name);
                mpr.insert_unit(
                    &module_id,
                    "DomainModel",
                    empty_domain_model.to_bson(&domain_model_id),
                    Some(&domain_model_id),
                )?;
                module_id
            }
        };
        domain::synchronize_domain_model(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.entities,
            &known_entities,
        )?;
        if let Some(roles) = &decl.roles {
            security::synchronize_module_security(
                &mut mpr, &module_id, &decl.name, roles, identity,
            )?;
        }
        documents::synchronize_microflows_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.microflows,
            identity,
        )?;
        documents::synchronize_nanoflows_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.nanoflows,
            identity,
        )?;
        documents::synchronize_enumerations_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.enumerations,
            identity,
        )?;
        documents::synchronize_oql_view_sources_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.oql_view_sources,
            identity,
        )?;
        documents::synchronize_constants_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.constants,
            identity,
        )?;
        documents::synchronize_regular_expressions_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.regular_expressions,
            identity,
        )?;
        documents::synchronize_task_queues_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.task_queues,
            identity,
        )?;
        documents::synchronize_scheduled_events_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.scheduled_events,
            identity,
        )?;
        documents::synchronize_menus_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &decl.menus,
            identity,
        )?;
        crate::layout_compiler::synchronize_layouts_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &project.mendix_version,
            &decl.layouts,
            identity,
        )?;
        documents::synchronize_pages_with_identity(
            &mut mpr,
            &module_id,
            &decl.name,
            &project.mendix_version,
            &decl.pages,
            identity,
        )?;
    }
    security::synchronize_declared_security(&mut mpr, &root_id, project, identity)?;
    if let Some(declaration) = &project.navigation {
        navigation::synchronize_navigation(&mut mpr, &root_id, declaration, identity)?;
    }
    Ok(())
}

/// Applies only enumeration, constant, regular-expression, scheduled-event,
/// and standalone-menu
/// declarations to an imported model.
///
/// This narrow entry point is used by the portability verifier: it proves
/// document-family round trips without rewriting unrelated domain, security,
/// navigation, page, or flow units. Modules must already exist because this
/// operation verifies an import rather than scaffolding new application
/// structure.
pub fn synchronize_project_documents(path: impl AsRef<Path>, project: &ProjectDecl) -> Result<()> {
    let mut mpr = MprFile::open(path, false)?;
    let root_id = mpr
        .root_unit()?
        .ok_or(WriterError::MissingRootUnit)?
        .unit_id;
    let identity = ProjectIdentity::from_project_root(&root_id)?;
    let existing_modules_by_name = existing_module_ids_by_name(&mpr, &root_id)?;
    for declaration in &project.modules {
        if declaration.enumerations.is_empty()
            && declaration.oql_view_sources.is_empty()
            && declaration.constants.is_empty()
            && declaration.regular_expressions.is_empty()
            && declaration.task_queues.is_empty()
            && declaration.scheduled_events.is_empty()
            && declaration.menus.is_empty()
        {
            continue;
        }
        let module_id = existing_modules_by_name
            .get(&declaration.name)
            .ok_or_else(|| WriterError::MissingModuleUnit(declaration.name.clone()))?;
        documents::synchronize_enumerations_with_identity(
            &mut mpr,
            module_id,
            &declaration.name,
            &declaration.enumerations,
            identity,
        )?;
        documents::synchronize_oql_view_sources_with_identity(
            &mut mpr,
            module_id,
            &declaration.name,
            &declaration.oql_view_sources,
            identity,
        )?;
        documents::synchronize_constants_with_identity(
            &mut mpr,
            module_id,
            &declaration.name,
            &declaration.constants,
            identity,
        )?;
        documents::synchronize_regular_expressions_with_identity(
            &mut mpr,
            module_id,
            &declaration.name,
            &declaration.regular_expressions,
            identity,
        )?;
        documents::synchronize_task_queues_with_identity(
            &mut mpr,
            module_id,
            &declaration.name,
            &declaration.task_queues,
            identity,
        )?;
        documents::synchronize_scheduled_events_with_identity(
            &mut mpr,
            module_id,
            &declaration.name,
            &declaration.scheduled_events,
            identity,
        )?;
        documents::synchronize_menus_with_identity(
            &mut mpr,
            module_id,
            &declaration.name,
            &declaration.menus,
            identity,
        )?;
    }
    Ok(())
}

fn known_entities(project: &ProjectDecl) -> HashSet<String> {
    project
        .modules
        .iter()
        .flat_map(|m| {
            m.entities
                .iter()
                .map(move |e| format!("{}.{}", m.name, e.name))
        })
        .collect()
}

fn validate_lifecycle_handlers(
    project: &ProjectDecl,
    existing_microflows: &HashSet<String>,
) -> Result<()> {
    let mut microflows: HashSet<String> = project
        .modules
        .iter()
        .flat_map(|module| {
            module
                .microflows
                .iter()
                .map(move |flow| format!("{}.{}", module.name, flow.name))
        })
        .collect();
    microflows.extend(existing_microflows.iter().cloned());
    for module in &project.modules {
        for entity in &module.entities {
            for callback in entity.lifecycle.as_deref().unwrap_or(&[]) {
                if !microflows.contains(&callback.handler) {
                    return Err(WriterError::UnknownLifecycleHandler {
                        entity: format!("{}.{}", module.name, entity.name),
                        event: callback.event.rust_name().to_string(),
                        handler: callback.handler.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_oql_sources(project: &ProjectDecl, existing: &HashSet<String>) -> Result<()> {
    let mut sources = existing.clone();
    for module in &project.modules {
        let mut names = HashSet::new();
        for source in &module.oql_view_sources {
            if !names.insert(source.name.clone()) {
                return Err(WriterError::DuplicateOqlViewSource {
                    module_name: module.name.clone(),
                    name: source.name.clone(),
                });
            }
            sources.insert(format!("{}.{}", module.name, source.name));
        }
    }
    for module in &project.modules {
        for entity in &module.entities {
            let Some(mxrs_ir::EntitySourceDecl::OqlView { source_document }) =
                entity.source.as_ref()
            else {
                continue;
            };
            if entity.persistable {
                return Err(WriterError::PersistableOqlView(format!(
                    "{}.{}",
                    module.name, entity.name
                )));
            }
            let qualified = if source_document.contains('.') {
                source_document.clone()
            } else {
                format!("{}.{}", module.name, source_document)
            };
            if !sources.contains(&qualified) {
                return Err(WriterError::UnknownOqlViewSource {
                    entity: format!("{}.{}", module.name, entity.name),
                    source_name: qualified,
                });
            }
        }
    }
    Ok(())
}

fn existing_module_ids_by_name(
    mpr: &MprFile,
    root_id: &str,
) -> Result<std::collections::HashMap<String, String>> {
    let mut by_name = std::collections::HashMap::new();
    for unit in mpr.children_of(root_id)? {
        if unit.containment_name != "Modules" {
            continue;
        }
        let doc = mpr.parse_contents(&unit)?;
        if let Ok(name) = doc.get_str("Name") {
            by_name.insert(name.to_string(), unit.unit_id);
        }
    }
    Ok(by_name)
}
