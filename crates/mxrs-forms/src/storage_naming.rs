//! Maps editor-schema names to v11 MPR field names. Exceptional names are
//! explicit and evidence-backed; the rest follow Mendix PascalCase storage.
//!
//! Ports `Mxrb::Forms::StorageNaming` from `lib/mxrb/forms/storage_naming.rb`.

use crate::catalog::Property;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageProperty {
    pub name: String,
    pub observed: bool,
}

const TYPE_ALIASES: &[(&str, &str)] = &[
    ("DivContainer", "DivContainer"),
    ("DropDownButton", "MobileDropDownButton"),
    ("DynamicImageViewer", "ImageViewer"),
    ("GridColumn", "DataGridColumn"),
    ("LayoutCallArgument", "FormCallArgument"),
    ("MicroflowClientAction", "MicroflowAction"),
    ("NoClientAction", "NoAction"),
    ("PageClientAction", "FormAction"),
    ("PageSettings", "FormSettings"),
    ("SelectButton", "DataGridSelectButton"),
    ("TableCell", "DbTableCell"),
    ("TabContainer", "TabControl"),
];

const LEGACY_TYPE_ALIASES: &[(&str, &str)] = &[
    ("FormForSpecialization", "PageForSpecialization"),
    ("NewGridDatabaseSource", "GridXPathSource"),
    ("NewListViewDatabaseSource", "ListViewXPathSource"),
    ("NewSelectorDatabaseSource", "SelectorXPathSource"),
];

/// `((declared_by, property name), storage field name)`.
const ALIASES: &[((&str, &str), &str)] = &[
    (
        ("AssociationWidget", "selectPageSettings"),
        "PopupFormSettings",
    ),
    (
        ("AttributeWidgetWithPlaceholder", "placeholderTemplate"),
        "Placeholder",
    ),
    (("Button", "caption"), "CaptionTemplate"),
    (("ControlBar", "items"), "NewButtons"),
    (("ControlBarButton", "caption"), "CaptionTemplate"),
    (("DataGrid", "caption"), "CaptionTemplate"),
    (("ColumnGrid", "tooltipPage"), "TooltipForm"),
    (
        ("DropDownSearchField", "allowMultipleSelect"),
        "AllowMultiSelect",
    ),
    (("GridControlBar", "defaultButton"), "DefaultButtonPointer"),
    (("GridColumn", "width"), "WidthValue"),
    (("GridNewButton", "pageSettings"), "FormSettings"),
    (("DataGridAddButton", "pageSettings"), "FormSettings"),
    (("GridSortItem", "sortDirection"), "SortOrder"),
    (("GroupBox", "caption"), "CaptionTemplate"),
    (("LayoutCall", "layout"), "Form"),
    (("ListViewTemplate", "specialization"), "Entity"),
    (("Page", "allowedRoles"), "AllowedModuleRoles"),
    (("Page", "layoutCall"), "FormCall"),
    (("PageClientAction", "pageSettings"), "FormSettings"),
    (("PageSettings", "page"), "Form"),
    (("PageForSpecialization", "pageSettings"), "FormSettings"),
    (
        ("ReferenceSelector", "gotoPageSettings"),
        "GotoFormSettings",
    ),
    (
        ("ReferenceSetSelector", "xPathConstraint"),
        "SelectableXPathConstraint",
    ),
    (("ScrollContainer", "center"), "CenterRegion"),
    (("SnippetCall", "snippet"), "Form"),
    (("SnippetCallWidget", "snippetCall"), "FormCall"),
    (("Table", "columns"), "ColumnWidths"),
    (("TableColumn", "width"), "Value"),
    (("TabContainer", "defaultPage"), "DefaultPagePointer"),
];

const LEGACY_ALIASES: &[((&str, &str), &str)] = &[
    (("DataView", "editability"), "Editable"),
    (
        ("GridXPathSource", "xPathConstraint"),
        "DatabaseConstraints",
    ),
    (
        ("SelectorXPathSource", "xPathConstraint"),
        "DatabaseConstraints",
    ),
    (
        ("XPathSourceBase", "xPathConstraint"),
        "DatabaseConstraints",
    ),
];

pub fn resolve(property: &Property) -> StorageProperty {
    match ALIASES.iter().find(|((declared_by, name), _)| {
        *declared_by == property.declared_by && *name == property.name
    }) {
        Some((_, alias)) => StorageProperty {
            name: (*alias).to_string(),
            observed: true,
        },
        None => StorageProperty {
            name: pascal_case(&property.name),
            observed: false,
        },
    }
}

/// Every storage field name that might hold `property`'s value: the
/// resolved (possibly aliased) name first, then the legacy alias if any,
/// then the plain PascalCase convention — in the order a decoder should
/// probe an actual document.
pub fn candidates(property: &Property) -> Vec<String> {
    let mut result = vec![resolve(property).name];
    if let Some((_, legacy)) = LEGACY_ALIASES.iter().find(|((declared_by, name), _)| {
        *declared_by == property.declared_by && *name == property.name
    }) {
        result.push((*legacy).to_string());
    }
    result.push(pascal_case(&property.name));
    result.dedup();
    result
}

pub fn pascal_case(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn storage_type_name(schema_name: &str) -> String {
    TYPE_ALIASES
        .iter()
        .find(|(schema, _)| *schema == schema_name)
        .map(|(_, storage)| (*storage).to_string())
        .unwrap_or_else(|| schema_name.to_string())
}

pub fn schema_type_name(storage_name: &str) -> String {
    if let Some((schema, _)) = LEGACY_TYPE_ALIASES
        .iter()
        .find(|(_, storage)| *storage == storage_name)
    {
        return (*schema).to_string();
    }
    if let Some((schema, _)) = TYPE_ALIASES
        .iter()
        .find(|(_, storage)| *storage == storage_name)
    {
        return (*schema).to_string();
    }
    storage_name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Cardinality, Property};

    fn property(declared_by: &str, name: &str) -> Property {
        Property {
            name: name.to_string(),
            ruby_name: crate::catalog::ruby_name(name),
            declared_by: declared_by.to_string(),
            type_name: "string".into(),
            targets: vec![],
            cardinality: Cardinality::One,
            optional: false,
            default_value: None,
            reference: None,
        }
    }

    #[test]
    fn resolve_uses_evidence_backed_alias_when_present() {
        let storage = resolve(&property("Button", "caption"));
        assert_eq!(storage.name, "CaptionTemplate");
        assert!(storage.observed);
    }

    #[test]
    fn resolve_falls_back_to_pascal_case_convention() {
        let storage = resolve(&property("Page", "url"));
        assert_eq!(storage.name, "Url");
        assert!(!storage.observed);
    }

    #[test]
    fn storage_type_name_and_schema_type_name_round_trip() {
        assert_eq!(storage_type_name("TabContainer"), "TabControl");
        assert_eq!(schema_type_name("TabControl"), "TabContainer");
        assert_eq!(storage_type_name("SomeUnaliasedType"), "SomeUnaliasedType");
    }

    #[test]
    fn legacy_type_aliases_only_resolve_in_the_schema_direction() {
        assert_eq!(schema_type_name("GridXPathSource"), "NewGridDatabaseSource");
    }
}
