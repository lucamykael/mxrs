use std::collections::HashMap;
use std::path::Path;

use mxrs_bson::Document;
use mxrs_mpr::{MprFile, RawUnit};

use crate::{JavaGenError, Result};

#[derive(Debug, Clone)]
pub(crate) struct SourceUnit {
    pub document: Document,
    pub module_name: Option<String>,
}

pub(crate) struct SourceModel {
    pub version: String,
    pub units: Vec<SourceUnit>,
}

impl SourceModel {
    pub fn read(path: &Path) -> Result<Self> {
        let mpr = MprFile::open(path, true)?;
        let version = mpr.mendix_version()?.ok_or(JavaGenError::MissingVersion)?;
        let raw_units = mpr.all_units()?;
        let mut documents = HashMap::with_capacity(raw_units.len());
        for raw in &raw_units {
            documents.insert(raw.unit_id.clone(), mpr.parse_contents(raw)?);
        }
        let raw_index: HashMap<&str, &RawUnit> = raw_units
            .iter()
            .map(|raw| (raw.unit_id.as_str(), raw))
            .collect();
        let mut cache = HashMap::new();
        let units = raw_units
            .iter()
            .map(|raw| SourceUnit {
                document: documents.get(&raw.unit_id).cloned().unwrap_or_default(),
                module_name: resolve_module(raw, &raw_index, &documents, &mut cache),
            })
            .collect();
        Ok(Self { version, units })
    }

    pub fn units_of<'a>(&'a self, native_type: &'a str) -> impl Iterator<Item = &'a SourceUnit> {
        self.units
            .iter()
            .filter(move |unit| unit.document.get_str("$Type").ok() == Some(native_type))
    }
}

fn resolve_module(
    raw: &RawUnit,
    raw_index: &HashMap<&str, &RawUnit>,
    documents: &HashMap<String, Document>,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    if let Some(value) = cache.get(&raw.unit_id) {
        return value.clone();
    }
    let document = documents.get(&raw.unit_id)?;
    if matches!(
        document.get_str("$Type").ok(),
        Some("Projects$ModuleImpl" | "Projects$Module")
    ) {
        let value = document.get_str("Name").ok().map(str::to_string);
        cache.insert(raw.unit_id.clone(), value.clone());
        return value;
    }
    if raw.container_id == raw.unit_id {
        cache.insert(raw.unit_id.clone(), None);
        return None;
    }
    let parent = raw_index.get(raw.container_id.as_str())?;
    let value = resolve_module(parent, raw_index, documents, cache);
    cache.insert(raw.unit_id.clone(), value.clone());
    value
}
