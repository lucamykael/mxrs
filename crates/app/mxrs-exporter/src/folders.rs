//! The folder of its module each document lives in, as the importer states
//! it: `folder = "Orders/Admin"` on a Rust declaration, a folder of the
//! module's folder for a form of the frontend, `@folder` for a nanoflow.

use std::collections::HashMap;

use mxrs_model::Project;

use crate::Result;

/// Each document in a folder, by its module and name: the path of the
/// folder inside the module. A document at its module's root is not here.
#[derive(Debug, Default, Clone)]
pub(crate) struct Folders(HashMap<(String, String), String>);

impl Folders {
    pub(crate) fn read(project: &Project) -> Result<Self> {
        let units = project.all_units()?;
        let modules: HashMap<String, String> = project
            .modules()?
            .into_iter()
            .filter_map(|module| Some((module.id, module.name?)))
            .collect();
        let mut folders: HashMap<String, (String, String)> = HashMap::new();
        let mut documents = Vec::new();
        for unit in &units {
            let Ok(document) = project.mpr().parse_contents(unit) else {
                continue;
            };
            let name = document.get_str("Name").unwrap_or_default().to_string();
            match (
                document.get_str("$Type").ok(),
                unit.containment_name.as_str(),
            ) {
                (Some("Projects$Folder"), _) => {
                    folders.insert(unit.unit_id.clone(), (unit.container_id.clone(), name));
                }
                (_, "Documents") if !name.is_empty() => {
                    documents.push((unit.container_id.clone(), name));
                }
                _ => {}
            }
        }
        let mut placed = HashMap::new();
        for (container, name) in documents {
            let mut path = Vec::new();
            let mut current = container;
            for _ in 0..64 {
                if let Some(module) = modules.get(&current) {
                    if !path.is_empty() {
                        path.reverse();
                        placed.insert((module.clone(), name.clone()), path.join("/"));
                    }
                    break;
                }
                let Some((parent, folder)) = folders.get(&current) else {
                    break;
                };
                path.push(mxrs_ir::folder_segment(folder));
                current = parent.clone();
            }
        }
        Ok(Self(placed))
    }

    /// The folder `module`'s document `name` is in; `None` at its root.
    pub(crate) fn of(&self, module: &str, name: &str) -> Option<&str> {
        self.0
            .get(&(module.to_string(), name.to_string()))
            .map(String::as_str)
    }

    /// `folder = "..."` for a declaration's attribute, when the document is
    /// in a folder.
    pub(crate) fn argument(&self, module: &str, name: &str) -> Option<String> {
        self.of(module, name)
            .map(|folder| format!("folder = {folder:?}"))
    }
}

/// The arguments of a `#[declaration]` into `module`, placing what it
/// declares in `folder` (an argument of [`Folders::argument`]) when it is
/// in one.
pub(crate) fn declaration_arguments(module: &str, folder: Option<String>) -> String {
    let mut arguments = format!("module = {}", crate::rust_string(module));
    if let Some(folder) = folder {
        arguments.push_str(", ");
        arguments.push_str(&folder);
    }
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declaration_states_its_folder_after_its_module() {
        assert_eq!(declaration_arguments("Sales", None), "module = \"Sales\"");
        assert_eq!(
            declaration_arguments("Sales", Some("folder = \"Orders/Admin\"".to_string())),
            "module = \"Sales\", folder = \"Orders/Admin\""
        );
    }
}
