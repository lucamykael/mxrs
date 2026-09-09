//! Known `.mpr` SQLite tables and their purpose (reverse-engineered from
//! Mendix 9/10 projects + mxcli source inspection). Columns marked "(?)" in
//! the original source are inferred/version-dependent.
//!
//! Ports `Mxrb::Schema::TABLES`/`UNIT_TYPES`/`ATTRIBUTE_TYPES` from
//! `lib/mxrb/schema/tables.rb`.

pub struct TableInfo {
    pub desc: &'static str,
    pub columns: &'static [&'static str],
}

pub const TABLES: &[(&str, TableInfo)] = &[
    (
        "_MetaData",
        TableInfo {
            desc: "Single-row table with project-level metadata",
            columns: &["MetaDataID", "MendixVersion", "ProjectID", "ProjectName"],
        },
    ),
    (
        "Unit",
        TableInfo {
            desc: "Every Mendix artefact (module, entity, page, microflow...) is a Unit",
            columns: &[
                "UnitID",
                "ContainerID",
                "ContainmentName",
                "UnitTypeID",
                "ContentsHash",
                "Contents",
            ],
        },
    ),
    (
        "UnitType",
        TableInfo {
            desc: "Lookup table: numeric ID -> qualified type name",
            columns: &["UnitTypeID", "Name"],
        },
    ),
    (
        "PreferredHash",
        TableInfo {
            desc: "Stores preferred/last-known hash per unit for change detection",
            columns: &["UnitID", "Hash"],
        },
    ),
];

pub fn table_info(name: &str) -> Option<&'static TableInfo> {
    TABLES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, info)| info)
}

/// Known `UnitType` names (qualified Mendix metamodel class names). These
/// appear in `UnitType.Name` and drive how `Contents` is decoded.
pub const UNIT_TYPES: &[&str] = &[
    "Mxmodels.Projects.Project",
    "Mxmodels.Projects.Module",
    "Mxmodels.DomainModels.DomainModel",
    "Mxmodels.DomainModels.Entity",
    "Mxmodels.DomainModels.Attribute",
    "Mxmodels.DomainModels.Association",
    "Mxmodels.Pages.Page",
    "Mxmodels.Pages.Layout",
    "Mxmodels.Pages.Snippet",
    "Mxmodels.Microflows.Microflow",
    "Mxmodels.Microflows.Nanoflow",
    "Mxmodels.Microflows.Rule",
    "Mxmodels.Settings.ProjectSettings",
    "Mxmodels.Settings.RuntimeSettings",
    "Mxmodels.Security.ProjectSecurity",
    "Mxmodels.Security.ModuleSecurity",
    "Mxmodels.Texts.SystemTextCollection",
];

/// Attribute types as used in the serialized `Contents` blob.
pub const ATTRIBUTE_TYPES: &[&str] = &[
    "AutoNumber",
    "Boolean",
    "Currency",
    "DateTime",
    "Decimal",
    "Enum",
    "Float",
    "HashString",
    "Integer",
    "Long",
    "String",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_table_columns() {
        let info = table_info("Unit").unwrap();
        assert!(info.columns.contains(&"ContentsHash"));
    }

    #[test]
    fn returns_none_for_unknown_table() {
        assert!(table_info("NotATable").is_none());
    }
}
