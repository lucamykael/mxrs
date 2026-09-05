//! Maps a `ContainmentName` to its expected `$Type` prefix for quick type
//! detection, mirroring `CONTAINMENT_TO_TYPE` in `bson_codec.rb`.
//!
//! Returns `None` both for unknown containment names and for `"Documents"`,
//! which is deliberately ambiguous (could be a Page, Microflow, etc. —
//! callers must read `$Type` directly instead).

pub fn containment_to_type(containment_name: &str) -> Option<&'static str> {
    match containment_name {
        "Modules" => Some("Projects$Module"),
        "DomainModel" => Some("DomainModels$DomainModel"),
        "Documents" => None,
        "Folders" => Some("Projects$Folder"),
        "ModuleSecurity" => Some("Security$ModuleSecurity"),
        "Settings" => Some("Settings$ProjectSettings"),
        "Security" => Some("Security$ProjectSecurity"),
        "NavigationDocuments" => Some("Navigation$NavigationDocument"),
        "SystemTexts" => Some("Texts$SystemTextCollection"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_containments() {
        assert_eq!(containment_to_type("Modules"), Some("Projects$Module"));
        assert_eq!(
            containment_to_type("DomainModel"),
            Some("DomainModels$DomainModel")
        );
    }

    #[test]
    fn returns_none_for_ambiguous_and_unknown_containments() {
        assert_eq!(containment_to_type("Documents"), None);
        assert_eq!(containment_to_type("SomethingNew"), None);
    }
}
