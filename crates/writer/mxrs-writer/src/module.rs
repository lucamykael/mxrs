//! Persists one `mxrs_ir::ModuleDecl` into an already-created `.mpr`: the
//! `Projects$Module` unit itself, its `DomainModel` unit, and one `Documents`
//! unit per microflow. Mirrors the fresh-project slice of `Writer#module_doc`
//! + `#write_domain_model` + `#write_documents`.

use std::collections::HashSet;

use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::declaration::ModuleDecl;
use mxrs_mpr::MprFile;

use crate::error::Result;
use crate::{documents, domain, security};

pub fn write_module(
    mpr: &mut MprFile,
    project_root_id: &str,
    decl: &ModuleDecl,
    known_entities: &HashSet<String>,
    identity: ProjectIdentity,
    mendix_version: &str,
) -> Result<()> {
    let module_id = insert_bare_module(mpr, project_root_id, &decl.name, identity)?;

    let (domain_model, _entity_ids) =
        domain::build_domain_model(&decl.name, &decl.entities, known_entities, identity)?;
    let domain_model_id = identity.artifact_id(ArtifactKind::DomainModel, &decl.name);
    mpr.insert_unit(
        &module_id,
        "DomainModel",
        domain_model.to_bson(&domain_model_id),
        Some(&domain_model_id),
    )?;

    security::synchronize_module_security(
        mpr,
        &module_id,
        &decl.name,
        decl.roles.as_deref().unwrap_or_default(),
        identity,
    )?;

    documents::synchronize_microflows_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.microflows,
        identity,
    )?;
    documents::synchronize_nanoflows_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.nanoflows,
        identity,
    )?;
    documents::synchronize_rules_with_identity(mpr, &module_id, &decl.name, &decl.rules, identity)?;

    documents::synchronize_enumerations_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.enumerations,
        identity,
    )?;

    documents::synchronize_oql_view_sources_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.oql_view_sources,
        identity,
    )?;

    documents::synchronize_constants_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.constants,
        identity,
    )?;

    documents::synchronize_regular_expressions_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.regular_expressions,
        identity,
    )?;

    documents::synchronize_task_queues_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.task_queues,
        identity,
    )?;

    documents::synchronize_scheduled_events_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.scheduled_events,
        identity,
    )?;

    documents::synchronize_menus_with_identity(mpr, &module_id, &decl.name, &decl.menus, identity)?;

    crate::layout_compiler::synchronize_layouts_with_identity(
        mpr,
        &module_id,
        &decl.name,
        mendix_version,
        &decl.layouts,
        identity,
    )?;
    documents::synchronize_pages_with_identity(
        mpr,
        &module_id,
        &decl.name,
        mendix_version,
        &decl.pages,
        identity,
    )?;
    crate::native::synchronize_forms_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.forms,
        identity,
    )?;
    crate::native::synchronize_javascript_actions_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.javascript_actions,
        identity,
    )?;
    crate::native::synchronize_java_actions_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.java_actions,
        identity,
    )?;
    crate::native::synchronize_json_structures_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.json_structures,
        identity,
    )?;
    crate::native::synchronize_published_rest_services_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.published_rest_services,
        identity,
    )?;
    crate::native::synchronize_data_sets_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.data_sets,
        identity,
    )?;
    documents::synchronize_image_collections_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.image_collections,
        identity,
    )?;

    Ok(())
}

/// A `Projects$Module` unit with no `DomainModel`/`Documents` children yet
/// — used both by fresh creation (`write_module`, populating those right
/// after) and by `project::synchronize_project` (upserting a module that's
/// new to an existing project, before delegating to
/// `domain::synchronize_domain_model`/`documents::synchronize_microflows`).
pub(crate) fn insert_bare_module(
    mpr: &mut MprFile,
    project_root_id: &str,
    name: &str,
    identity: ProjectIdentity,
) -> Result<String> {
    let module_id = identity.artifact_id(ArtifactKind::Module, name);
    let module = mxrs_model::Module {
        id: module_id.clone(),
        name: Some(name.to_string()),
        sort_index: None,
        from_app_store: false,
        app_store_guid: None,
        app_store_version: None,
        export_level: "Hidden".into(),
        domain_model: None,
        pages: vec![],
        microflows: vec![],
        nanoflows: vec![],
        rules: vec![],
        menus: vec![],
        module_roles: vec![],
        artifact_units: vec![],
    };
    mpr.insert_unit(
        project_root_id,
        "Modules",
        module.to_bson(),
        Some(&module_id),
    )?;
    insert_module_settings(mpr, &module_id, name, identity)?;
    Ok(module_id)
}

/// The settings Studio Pro gives a module it creates: its version, how it
/// is exported, and no Java dependencies. Mendix 10 and later keep them in a
/// unit of their own, which every module of such a project has.
fn insert_module_settings(
    mpr: &mut MprFile,
    module_id: &str,
    name: &str,
    identity: ProjectIdentity,
) -> Result<()> {
    let Some(version) = mpr.mendix_version()? else {
        return Ok(());
    };
    let major = version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok());
    if major.is_none_or(|major| major < 10) {
        return Ok(());
    }
    let id = identity.artifact_id(ArtifactKind::ModuleSettings, name);
    let mut document = mxrs_bson::doc! {
        "$ID": id.clone(),
        "$Type": "Projects$ModuleSettings",
        "BasedOnVersion": "",
        "ExportLevel": "Source",
        "ExtensionName": "",
        "JarDependencies": mxrs_bson::build_array(Vec::new(), 2),
        "ProtectedModuleType": "AddOn",
        "SolutionIdentifier": "",
        "Version": "1.0.0",
    };
    // What the version's own schema adds to them.
    mxrs_schema::apply_document(&version, &mut document);
    mpr.insert_unit(module_id, "ModuleSettings", document, Some(&id))?;
    Ok(())
}
