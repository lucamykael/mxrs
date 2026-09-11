//! Incremental re-sync for microflow `Documents` units — mirrors the
//! microflow slice of `Writer#write_documents`/`#upsert_document` (mxrb's
//! own method upserts pages/microflows/nanoflows/rules/menus/enumerations/
//! constants/scheduled_events; this crate only has DSL/model surface for
//! microflows so far, same narrowing every other pass in this codebase has
//! made — widen incrementally as those grow a typed surface).
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

use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::flow::MicroflowDecl;
use mxrs_model::Microflow;
use mxrs_mpr::MprFile;

use crate::error::Result;
use crate::flow_compiler;

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
    let existing_by_name: HashMap<String, String> = mpr
        .children_of(module_id)?
        .into_iter()
        .filter(|u| u.containment_name == "Documents")
        .filter_map(|u| {
            let doc = mpr.parse_contents(&u).ok()?;
            if doc.get_str("$Type").ok()? != "Microflows$Microflow" {
                return None;
            }
            let name = doc.get_str("Name").ok()?.to_string();
            Some((name, u.unit_id))
        })
        .collect();

    for decl in microflows {
        let (objects, flows) = flow_compiler::build_microflow_graph(
            &decl.activities,
            &decl.rescue_activities,
            decl.return_expression.as_deref(),
        );
        let existing_id = existing_by_name.get(&decl.name).cloned();
        let id = existing_id.clone().unwrap_or_else(|| {
            identity.artifact_id(
                ArtifactKind::Microflow,
                &format!("{module_name}.{}", decl.name),
            )
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
            return_type_document: None,
            return_type: None,
            objects,
            flows,
        };
        let doc = microflow.to_bson();
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
