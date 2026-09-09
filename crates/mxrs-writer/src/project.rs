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

use mxrs_ir::declaration::ProjectDecl;
use mxrs_mpr::MprFile;

use crate::error::{Result, WriterError};
use crate::{documents, domain, module, scaffold};

pub fn write_project(path: impl AsRef<Path>, project: &ProjectDecl) -> Result<()> {
    let schema_hash = mxrs_schema::schema_hash(&project.mendix_version)
        .ok_or_else(|| WriterError::UnsupportedVersion(project.mendix_version.clone()))?;
    let mut mpr = MprFile::create(path, &project.mendix_version, schema_hash)?;
    let root_id = mpr
        .root_unit()?
        .ok_or(WriterError::MissingRootUnit)?
        .unit_id;

    scaffold::write_default_project_units(&mut mpr, &root_id, &project.mendix_version)?;

    let known_entities = known_entities(project);

    for decl in &project.modules {
        module::write_module(&mut mpr, &root_id, decl, &known_entities)?;
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
    let mut mpr = MprFile::open(path, false)?;
    let root_id = mpr
        .root_unit()?
        .ok_or(WriterError::MissingRootUnit)?
        .unit_id;

    let existing_modules_by_name = existing_module_ids_by_name(&mpr, &root_id)?;
    let known_entities = known_entities(project);

    for decl in &project.modules {
        let module_id = match existing_modules_by_name.get(&decl.name) {
            Some(id) => id.clone(),
            None => {
                let module_id = module::insert_bare_module(&mut mpr, &root_id, &decl.name)?;
                // `synchronize_domain_model` resyncs an *existing*
                // `DomainModel` unit (it errors with `MissingDomainModel`
                // otherwise) — a module that's new to this project needs
                // an empty one first, same as `write_module` inserts one
                // right after the bare module unit for fresh creation.
                let empty_domain_model = mxrs_model::DomainModel {
                    documentation: String::new(),
                    entities: vec![],
                    associations: vec![],
                    cross_associations: vec![],
                };
                let domain_model_id = uuid::Uuid::new_v4().to_string();
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
        documents::synchronize_microflows(&mut mpr, &module_id, &decl.microflows)?;
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
