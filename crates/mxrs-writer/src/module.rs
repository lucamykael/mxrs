//! Persists one `mxrs_ir::ModuleDecl` into an already-created `.mpr`: the
//! `Projects$Module` unit itself, its `DomainModel` unit, and one `Documents`
//! unit per microflow. Mirrors the fresh-project slice of `Writer#module_doc`
//! + `#write_domain_model` + `#write_documents`.

use std::collections::HashSet;

use mxrs_ir::declaration::ModuleDecl;
use mxrs_model::Microflow;
use mxrs_mpr::MprFile;

use crate::error::Result;
use crate::{domain, flow_compiler};

pub fn write_module(
    mpr: &mut MprFile,
    project_root_id: &str,
    decl: &ModuleDecl,
    known_entities: &HashSet<String>,
) -> Result<()> {
    let module_id = uuid::Uuid::new_v4().to_string();
    let module = mxrs_model::Module {
        id: module_id.clone(),
        name: Some(decl.name.clone()),
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
    };
    mpr.insert_unit(project_root_id, "Modules", module.to_bson(), Some(&module_id))?;

    let (domain_model, _entity_ids) = domain::build_domain_model(&decl.name, &decl.entities, known_entities)?;
    let domain_model_id = uuid::Uuid::new_v4().to_string();
    mpr.insert_unit(&module_id, "DomainModel", domain_model.to_bson(&domain_model_id), Some(&domain_model_id))?;

    for mf in &decl.microflows {
        let (objects, flows) = flow_compiler::build_microflow_graph(&mf.activities, mf.return_expression.as_deref());
        let microflow = Microflow {
            id: None,
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
        mpr.insert_unit(&module_id, "Documents", microflow.to_bson(), None)?;
    }

    Ok(())
}
