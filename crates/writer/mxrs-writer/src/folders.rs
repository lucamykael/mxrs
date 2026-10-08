//! Where a module's declared documents live. A declaration states the
//! folder of its module a document is in (`Orders/Admin`), or nothing for
//! the module's root; a build makes each folder the model lacks — under a
//! stable identity of its path — and moves every document it declares into
//! its own. A folder no declaration names any more is kept as it is, and
//! what the build does not declare stays where the model has it.

use std::collections::HashMap;

use mxrs_bson::doc;
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::{ModuleDecl, ProjectDecl};
use mxrs_mpr::MprFile;

use crate::Result;

const FOLDER: &str = "Projects$Folder";

/// Places the documents every module of `project` declares in the folders
/// it states, answering what moved out of a folder to its module's root —
/// which a project declared before folders were stated would otherwise do
/// silently.
pub(crate) fn place_documents(
    mpr: &mut MprFile,
    modules: &HashMap<String, String>,
    project: &ProjectDecl,
    identity: ProjectIdentity,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    for decl in &project.modules {
        let Some(module_id) = modules.get(&decl.name) else {
            continue;
        };
        place_module(mpr, module_id, decl, identity, &mut warnings)?;
    }
    Ok(warnings)
}

/// One module's folders, by their container and name, and its documents
/// by name, each with the unit holding it and the folder it is in.
#[derive(Default)]
struct Tree {
    folders: HashMap<(String, String), String>,
    paths: HashMap<String, String>,
    documents: HashMap<String, (String, String)>,
}

impl Tree {
    fn read(mpr: &MprFile, module_id: &str) -> Result<Self> {
        let mut tree = Tree::default();
        let mut pending = vec![(module_id.to_string(), String::new())];
        while let Some((container, path)) = pending.pop() {
            for unit in mpr.children_of(&container)? {
                let Ok(document) = mpr.parse_contents(&unit) else {
                    continue;
                };
                let name = document.get_str("Name").unwrap_or_default().to_string();
                match unit.containment_name.as_str() {
                    "Folders" if document.get_str("$Type").ok() == Some(FOLDER) => {
                        let segment = mxrs_ir::folder_segment(&name);
                        let inner = if path.is_empty() {
                            segment
                        } else {
                            format!("{path}/{segment}")
                        };
                        tree.folders
                            .insert((container.clone(), name), unit.unit_id.clone());
                        tree.paths.insert(unit.unit_id.clone(), inner.clone());
                        pending.push((unit.unit_id, inner));
                    }
                    "Documents" if !name.is_empty() => {
                        tree.documents
                            .insert(name, (unit.unit_id, container.clone()));
                    }
                    _ => {}
                }
            }
        }
        Ok(tree)
    }
}

fn place_module(
    mpr: &mut MprFile,
    module_id: &str,
    decl: &ModuleDecl,
    identity: ProjectIdentity,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mut tree = Tree::read(mpr, module_id)?;
    for name in decl.document_names() {
        let Some((unit, container)) = tree.documents.get(name).cloned() else {
            continue;
        };
        let target = match decl.folders.get(name) {
            Some(path) => folder(mpr, module_id, &decl.name, path, &mut tree, identity)?,
            None => module_id.to_string(),
        };
        if target == container {
            continue;
        }
        if target == module_id
            && let Some(path) = tree.paths.get(&container)
        {
            warnings.push(format!(
                "{}.{name} moves out of the folder `{path}` to its module's root: its declaration states no `folder`",
                decl.name
            ));
        }
        mpr.relocate_unit(&unit, &target, "Documents")?;
    }
    Ok(())
}

/// The folder at `path` inside the module, made — each folder of the path
/// the model lacks — when it is not there.
fn folder(
    mpr: &mut MprFile,
    module_id: &str,
    module: &str,
    path: &str,
    tree: &mut Tree,
    identity: ProjectIdentity,
) -> Result<String> {
    let mut container = module_id.to_string();
    let mut walked = String::new();
    for name in mxrs_ir::folder_names(path) {
        let segment = mxrs_ir::folder_segment(&name);
        walked = if walked.is_empty() {
            segment
        } else {
            format!("{walked}/{segment}")
        };
        let key = (container.clone(), name.clone());
        container = match tree.folders.get(&key) {
            Some(id) => id.clone(),
            None => {
                let id = identity.artifact_id(ArtifactKind::Folder, &format!("{module}/{walked}"));
                mpr.insert_unit(
                    &container,
                    "Folders",
                    doc! { "$ID": id.clone(), "$Type": FOLDER, "Name": name },
                    Some(&id),
                )?;
                tree.folders.insert(key, id.clone());
                tree.paths.insert(id.clone(), walked.clone());
                id
            }
        };
    }
    Ok(container)
}
