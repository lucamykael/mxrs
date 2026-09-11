//! Typed reader for modern and legacy native Mendix navigation documents.
//! Ports `lib/mxrb/model/navigation.rb`.

use mxrs_bson::Document;

use crate::support::{docs_any, get_doc_any, get_id_any, get_str_any};

const LEGACY_PROFILES: &[(&str, &str)] = &[
    ("DesktopProfile", "Desktop"),
    ("TabletProfile", "Tablet"),
    ("PhoneProfile", "Phone"),
    ("OfflinePhoneProfile", "OfflinePhone"),
    ("HybridPhoneProfile6", "HybridPhone"),
    ("HybridTabletProfile6", "HybridTablet"),
];

#[derive(Debug, Clone, Default)]
pub struct RoleHome {
    pub role: Option<String>,
    pub page: Option<String>,
    pub microflow: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NavigationItem {
    pub caption: std::collections::BTreeMap<String, String>,
    pub page: Option<String>,
    pub microflow: Option<String>,
    pub icon: Option<String>,
    pub items: Vec<NavigationItem>,
}

#[derive(Debug, Clone)]
pub struct NavigationProfile {
    pub name: String,
    pub kind: String,
    pub app_icon: Option<Document>,
    pub app_title: std::collections::BTreeMap<String, String>,
    pub home_page: Option<String>,
    pub home_microflow: Option<String>,
    pub sign_in_page: Option<String>,
    pub role_homes: Vec<RoleHome>,
    pub menu_items: Vec<NavigationItem>,
}

impl NavigationProfile {
    pub fn offline(&self) -> bool {
        self.kind.to_lowercase().contains("offline")
    }

    fn from_bson(doc: &Document) -> Self {
        let role_homes = docs_any(doc, &["HomeItems", "RoleBasedHomePages"])
            .iter()
            .map(|home| RoleHome {
                role: reference(home, &["UserRole"]),
                page: reference(home, &["Page"]),
                microflow: reference(home, &["Microflow"]),
            })
            .collect();
        let menu = get_doc_any(doc, &["Menu", "MenuItemCollection"]).unwrap_or_default();

        NavigationProfile {
            name: get_str_any(doc, &["Name"]).unwrap_or_default(),
            kind: get_str_any(doc, &["Kind"]).unwrap_or_default(),
            app_icon: get_doc_any(doc, &["AppIcon"]),
            app_title: text_translations(get_doc_any(doc, &["AppTitle"]).as_ref()),
            home_page: get_doc_any(doc, &["HomePage"]).and_then(|h| reference(&h, &["Page"])),
            home_microflow: get_doc_any(doc, &["HomePage"])
                .and_then(|h| reference(&h, &["Microflow"])),
            sign_in_page: get_doc_any(doc, &["LoginPageSettings"])
                .and_then(|s| reference(&s, &["Form"])),
            role_homes,
            menu_items: docs_any(&menu, &["Items"])
                .iter()
                .map(navigation_item)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Navigation {
    pub profiles: Vec<NavigationProfile>,
}

impl Navigation {
    pub fn from_bson(raw_document: Option<&Document>) -> Self {
        let Some(raw) = raw_document else {
            return Navigation::default();
        };
        let modern = docs_any(raw, &["Profiles"]);
        let documents = if modern.is_empty() {
            legacy_profile_documents(raw)
        } else {
            modern
        };
        Navigation {
            profiles: documents.iter().map(NavigationProfile::from_bson).collect(),
        }
    }

    pub fn empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

fn legacy_profile_documents(raw: &Document) -> Vec<Document> {
    LEGACY_PROFILES
        .iter()
        .filter_map(|(key, name)| {
            let mut value = get_doc_any(raw, &[key])?;
            if get_str_any(&value, &["Name"]).is_none() {
                value.insert("Name", *name);
            }
            Some(value)
        })
        .collect()
}

fn reference(doc: &Document, keys: &[&str]) -> Option<String> {
    let id = get_id_any(doc, keys)?;
    if id.is_empty() { None } else { Some(id) }
}

fn text_translations(text: Option<&Document>) -> std::collections::BTreeMap<String, String> {
    let Some(text) = text else {
        return Default::default();
    };
    docs_any(text, &["Translations", "Items"])
        .iter()
        .filter_map(|t| {
            let code = get_str_any(t, &["LanguageCode"])?;
            let value = get_str_any(t, &["Text"]).unwrap_or_default();
            if value.is_empty() {
                None
            } else {
                Some((code, value))
            }
        })
        .collect()
}

fn navigation_item(doc: &Document) -> NavigationItem {
    let action = get_doc_any(doc, &["Action"]).unwrap_or_default();
    NavigationItem {
        caption: text_translations(get_doc_any(doc, &["Caption"]).as_ref()),
        page: get_doc_any(&action, &["FormSettings"]).and_then(|s| reference(&s, &["Form"])),
        microflow: get_doc_any(&action, &["MicroflowSettings"])
            .and_then(|s| reference(&s, &["Microflow"])),
        icon: get_doc_any(doc, &["Icon"]).and_then(|i| get_str_any(&i, &["Code"])),
        items: docs_any(doc, &["Items"])
            .iter()
            .map(navigation_item)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    #[test]
    fn empty_raw_document_yields_empty_navigation() {
        let nav = Navigation::from_bson(None);
        assert!(nav.empty());
    }

    #[test]
    fn falls_back_to_legacy_profiles_when_no_modern_profiles_array() {
        let raw = doc! { "DesktopProfile": { "Kind": "Responsive" } };
        let nav = Navigation::from_bson(Some(&raw));
        assert_eq!(nav.profiles.len(), 1);
        assert_eq!(nav.profiles[0].name, "Desktop");
    }

    #[test]
    fn decodes_home_page_reference() {
        let page_id = uuid::Uuid::new_v4().to_string();
        let raw = doc! {
            "Profiles": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(doc! {
                "Name": "Responsive",
                "HomePage": { "Page": page_id.clone() },
            })], 3),
        };
        let nav = Navigation::from_bson(Some(&raw));
        assert_eq!(nav.profiles[0].home_page.as_deref(), Some(page_id.as_str()));
    }
}
