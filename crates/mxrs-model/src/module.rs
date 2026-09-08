//! Modules (`Projects$Module`, `ContainmentName: "Modules"`) and their
//! children (domain model, pages, microflows, menus, security roles).
//! Ports `lib/mxrb/model/module.rb` from mxrb.
//!
//! Deliberately not ported: the infrastructure/application/presentation/
//! asset/domain "document route" tables (mappings, endpoints, layouts,
//! datasets, enumerations, constants, ...) — those exist in mxrb to power
//! semantic-index/compiler subsystems that are out of scope for this crate
//! (`mxrs-model` is the `Project`/`Module`/`Entity`/`Page`/`Microflow` graph
//! per the phased plan); `compare.rb`'s own `module_summary` doesn't read
//! them either.

use mxrs_bson::Document;
use mxrs_mpr::{MprFile, RawUnit};

use crate::association::Association;
use crate::domain_model::DomainModel;
use crate::entity::Entity;
use crate::error::Result;
use crate::menu::Menu;
use crate::microflow::Microflow;
use crate::page::Page;
use crate::support::{docs_any, get_bool_any, get_i32_any, get_id_any, get_str_any};

#[derive(Debug, Clone)]
pub struct ModuleRole {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub id: String,
    pub name: Option<String>,
    pub sort_index: Option<i32>,
    pub from_app_store: bool,
    pub app_store_guid: Option<String>,
    pub app_store_version: Option<String>,
    pub export_level: String,
    pub domain_model: Option<DomainModel>,
    pub pages: Vec<Page>,
    pub microflows: Vec<Microflow>,
    pub nanoflows: Vec<Microflow>,
    pub rules: Vec<Microflow>,
    pub menus: Vec<Menu>,
    pub module_roles: Vec<ModuleRole>,
}

impl Module {
    pub fn entities(&self) -> &[Entity] {
        self.domain_model.as_ref().map(|d| d.entities.as_slice()).unwrap_or(&[])
    }

    pub fn associations(&self) -> Vec<&Association> {
        self.domain_model.as_ref().map(|d| d.all_associations().collect()).unwrap_or_default()
    }

    pub fn load(mpr: &MprFile, raw: &RawUnit) -> Result<Module> {
        let doc = mpr.parse_contents(raw)?;
        let name = get_str_any(&doc, &["Name"]);

        let domain_model = mpr
            .units_by_containment("DomainModel")?
            .into_iter()
            .find(|u| u.container_id == raw.unit_id)
            .map(|u| -> Result<DomainModel> {
                let dm_doc = mpr.parse_contents(&u)?;
                Ok(DomainModel::from_bson(&dm_doc, name.as_deref()))
            })
            .transpose()?;

        let mut pages = Vec::new();
        let mut microflows = Vec::new();
        let mut nanoflows = Vec::new();
        let mut rules = Vec::new();
        let mut menus = Vec::new();
        for unit in collect_documents(mpr, &raw.unit_id)? {
            let d = mpr.parse_contents(&unit)?;
            match get_str_any(&d, &["$Type"]).as_deref() {
                Some("Pages$Page") | Some("Forms$Page") => pages.push(Page::from_bson(&d)),
                Some("Microflows$Microflow") => microflows.push(Microflow::from_bson(&d)),
                Some("Microflows$Nanoflow") => nanoflows.push(Microflow::from_bson(&d)),
                Some("Microflows$Rule") => rules.push(Microflow::from_bson(&d)),
                Some("Menus$MenuDocument") => menus.push(Menu::from_bson(&d)),
                _ => {}
            }
        }

        let module_roles = mpr
            .children_of(&raw.unit_id)?
            .into_iter()
            .find(|u| u.containment_name == "ModuleSecurity")
            .map(|u| -> Result<Vec<ModuleRole>> {
                let security_doc = mpr.parse_contents(&u)?;
                Ok(docs_any(&security_doc, &["ModuleRoles"])
                    .iter()
                    .map(|r| ModuleRole {
                        id: get_id_any(r, &["$ID"]),
                        name: get_str_any(r, &["Name"]),
                        description: get_str_any(r, &["Description"]).unwrap_or_default(),
                    })
                    .collect())
            })
            .transpose()?
            .unwrap_or_default();

        Ok(Module {
            id: raw.unit_id.clone(),
            name,
            sort_index: get_i32_any(&doc, &["SortIndex", "NewSortIndex"]),
            from_app_store: get_bool_any(&doc, &["FromAppStore"]).unwrap_or(false),
            app_store_guid: get_str_any(&doc, &["AppStoreGuid"]),
            app_store_version: get_str_any(&doc, &["AppStoreVersion"]),
            export_level: get_str_any(&doc, &["ExportLevel"]).unwrap_or_else(|| "Hidden".into()),
            domain_model,
            pages,
            microflows,
            nanoflows,
            rules,
            menus,
            module_roles,
        })
    }

    pub fn to_bson(&self) -> Document {
        mxrs_bson::doc! {
            "$ID": self.id.clone(),
            "$Type": "Projects$Module",
            "Name": self.name.clone(),
            "SortIndex": self.sort_index.unwrap_or(0),
            "FromAppStore": self.from_app_store,
            "ExportLevel": self.export_level.clone(),
        }
    }
}

/// Documents live in `ContainmentName = "Documents"` recursively under a
/// module (nested inside `"Folders"` containers) — mirrors
/// `Module#collect_documents`.
fn collect_documents(mpr: &MprFile, parent_id: &str) -> Result<Vec<RawUnit>> {
    let mut result = Vec::new();
    for raw in mpr.children_of(parent_id)? {
        match raw.containment_name.as_str() {
            "Documents" => result.push(raw),
            "Folders" => result.extend(collect_documents(mpr, &raw.unit_id)?),
            _ => {}
        }
    }
    Ok(result)
}
