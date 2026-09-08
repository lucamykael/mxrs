//! Menu documents (`Menus$MenuDocument`). Ports `lib/mxrb/model/menu.rb`.

use mxrs_bson::Document;

use crate::support::{docs_any, get_doc_any, get_str_any};

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub name: Option<String>,
    pub caption: String,
    pub page: Option<String>,
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone)]
pub struct Menu {
    pub name: Option<String>,
    pub items: Vec<MenuItem>,
}

impl Menu {
    pub fn from_bson(doc: &Document) -> Self {
        let collection = get_doc_any(doc, &["ItemCollection"]).unwrap_or_default();
        Menu {
            name: get_str_any(doc, &["Name"]),
            items: docs_any(&collection, &["Items"]).iter().map(parse_menu_item).collect(),
        }
    }
}

fn parse_menu_item(doc: &Document) -> MenuItem {
    MenuItem {
        name: get_str_any(doc, &["Name"]),
        caption: extract_text(doc, &["Caption"]),
        page: get_doc_any(doc, &["Action"])
            .and_then(|a| get_doc_any(&a, &["FormSettings"]))
            .and_then(|f| get_str_any(&f, &["Form"])),
        items: docs_any(doc, &["Items"]).iter().map(parse_menu_item).collect(),
    }
}

fn extract_text(doc: &Document, keys: &[&str]) -> String {
    let Some(obj) = get_doc_any(doc, keys) else { return String::new() };
    let translation = docs_any(&obj, &["Items", "Translations"]).into_iter().next();
    let Some(translation) = translation else { return String::new() };
    get_str_any(&translation, &["Text", "Translation"]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::{doc, Bson};

    #[test]
    fn decodes_nested_menu_items_with_page_reference() {
        let child = doc! {
            "Name": "child_item",
            "Action": { "FormSettings": { "Form": "OrderOverview" } },
        };
        let root = doc! {
            "Name": "MainMenu",
            "ItemCollection": { "Items": mxrs_bson::build_array(vec![Bson::Document(child)], 3) },
        };
        let menu = Menu::from_bson(&root);
        assert_eq!(menu.name.as_deref(), Some("MainMenu"));
        assert_eq!(menu.items[0].page.as_deref(), Some("OrderOverview"));
    }
}
