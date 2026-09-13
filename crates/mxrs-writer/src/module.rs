//! Persists one `mxrs_ir::ModuleDecl` into an already-created `.mpr`: the
//! `Projects$Module` unit itself, its `DomainModel` unit, and one `Documents`
//! unit per microflow. Mirrors the fresh-project slice of `Writer#module_doc`
//! + `#write_domain_model` + `#write_documents`.

use std::collections::HashSet;

use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::declaration::ModuleDecl;
use mxrs_model::Microflow;
use mxrs_mpr::MprFile;

use crate::error::Result;
use crate::{documents, domain, flow_compiler};

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

    for mf in &decl.microflows {
        let (objects, flows) = flow_compiler::build_microflow_graph(
            &mf.activities,
            &mf.rescue_activities,
            mf.return_expression.as_deref(),
        );
        let microflow = Microflow {
            id: Some(identity.artifact_id(
                ArtifactKind::Microflow,
                &format!("{}.{}", decl.name, mf.name),
            )),
            name: Some(mf.name.clone()),
            documentation: mf.documentation.clone(),
            return_variable_name: "ReturnValue".into(),
            allow_concurrent_execution: true,
            apply_entity_access: false,
            mark_as_used: false,
            excluded: false,
            export_level: "Hidden".into(),
            allowed_module_roles: vec![],
            parameters: vec![],
            return_type_document: None,
            return_type: None,
            objects,
            flows,
        };
        let microflow_id = microflow.id.clone().expect("assigned above");
        mpr.insert_unit(
            &module_id,
            "Documents",
            microflow.to_bson(),
            Some(&microflow_id),
        )?;
    }

    documents::synchronize_enumerations_with_identity(
        mpr,
        &module_id,
        &decl.name,
        &decl.enumerations,
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
    Ok(module_id)
}
