//! The Project entry point: opens an `.mpr` via `mxrs-mpr` and exposes the
//! module/navigation graph. Ports the reading side of
//! `lib/mxrb/model/project.rb` — the semantic-index/OQL/migration surface
//! documented there belongs to later phases (`mxrs-semantic`, `mxrs-oql`,
//! the writer/migration pipeline), not this crate.

use std::path::Path;

use mxrs_bson::Document;
use mxrs_mpr::{MprFile, RawUnit};

use crate::error::Result;
use crate::module::Module;
use crate::navigation::Navigation;
use crate::support::get_str_any;

pub struct Project {
    mpr: MprFile,
}

impl Project {
    pub fn open(path: impl AsRef<Path>, readonly: bool) -> Result<Self> {
        Ok(Project {
            mpr: MprFile::open(path, readonly)?,
        })
    }

    pub fn mpr(&self) -> &MprFile {
        &self.mpr
    }

    /// Mutable access for the refactoring passes (`mxrs-refactor`), which
    /// rewrite units in place. Reading stays the default: a caller that only
    /// inspects the model should take `&Project` and never reach this.
    pub fn mpr_mut(&mut self) -> &mut MprFile {
        &mut self.mpr
    }

    pub fn name(&self) -> Result<Option<String>> {
        let Some(root) = self.mpr.root_unit()? else {
            return Ok(None);
        };
        Ok(get_str_any(&self.mpr.parse_contents(&root)?, &["Name"]))
    }

    pub fn mendix_version(&self) -> Result<Option<String>> {
        Ok(self.mpr.mendix_version()?)
    }

    pub fn modules(&self) -> Result<Vec<Module>> {
        self.mpr
            .units_by_containment("Modules")?
            .iter()
            .map(|raw| Module::load(&self.mpr, raw))
            .collect()
    }

    pub fn navigation(&self) -> Result<Navigation> {
        Ok(Navigation::from_bson(self.navigation_document()?.as_ref()))
    }

    /// The navigation document as the model stores it, when there is one.
    pub fn navigation_document(&self) -> Result<Option<Document>> {
        for unit in self.mpr.all_units()? {
            let doc = self.mpr.parse_contents(&unit)?;
            if get_str_any(&doc, &["$Type"]).as_deref() == Some("Navigation$NavigationDocument") {
                return Ok(Some(doc));
            }
        }
        Ok(None)
    }

    pub fn all_units(&self) -> Result<Vec<RawUnit>> {
        Ok(self.mpr.all_units()?)
    }
}
