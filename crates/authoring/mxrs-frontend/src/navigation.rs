//! `frontend/src/navigation/index.ts`: the project's navigation profiles.
//!
//! ```ts
//! import type { Navigation } from "@/types/navigation";
//!
//! export default {
//!   profiles: [
//!     {
//!       name: "Responsive",
//!       title: { en_US: "Shop" },
//!       homePage: "Main.Home",
//!       homes: [{ role: "Anonymous", page: "Main.Login" }],
//!       items: [
//!         { caption: "Orders", page: "Sales.Orders", icon: { glyph: "list" } },
//!         { caption: { en_US: "Admin", nl_NL: "Beheer" }, items: [ ... ] },
//!       ],
//!     },
//!   ],
//! } satisfies Navigation;
//! ```

use std::collections::BTreeMap;

use mxrs_ir::{
    NavigationDecl, NavigationIconDecl, NavigationItemDecl, NavigationProfileDecl, RoleHomeDecl,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Navigation {
    profiles: Vec<Profile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Profile {
    name: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    title: BTreeMap<String, String>,
    #[serde(default)]
    home_page: Option<String>,
    #[serde(default)]
    home_microflow: Option<String>,
    #[serde(default)]
    sign_in_page: Option<String>,
    #[serde(default)]
    homes: Vec<RoleHome>,
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoleHome {
    role: String,
    #[serde(default)]
    page: Option<String>,
    #[serde(default)]
    microflow: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Item {
    caption: Caption,
    #[serde(default)]
    page: Option<String>,
    #[serde(default)]
    microflow: Option<String>,
    #[serde(default)]
    icon: Option<Icon>,
    #[serde(default)]
    items: Vec<Item>,
}

/// A caption in English alone is its text; one in other languages names
/// each language.
#[derive(Deserialize)]
#[serde(untagged)]
enum Caption {
    Text(String),
    Localized(BTreeMap<String, String>),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Icon {
    #[serde(default)]
    glyph: Option<String>,
    #[serde(default)]
    code: Option<i64>,
}

/// The navigation `value` declares, or why it does not declare one.
pub(crate) fn navigation(value: serde_json::Value) -> Result<NavigationDecl, String> {
    let declared: Navigation = serde_json::from_value(value).map_err(|error| error.to_string())?;
    let profiles = declared
        .profiles
        .into_iter()
        .map(|profile| {
            if profile.home_page.is_some() && profile.home_microflow.is_some() {
                return Err(format!(
                    "profile {:?} has both a home page and a home microflow",
                    profile.name
                ));
            }
            let mut declared = NavigationProfileDecl::new(&profile.name);
            if let Some(kind) = profile.kind {
                declared.kind = kind;
            }
            declared.app_title = profile.title;
            declared.home_page = profile.home_page;
            declared.home_microflow = profile.home_microflow;
            declared.sign_in_page = profile.sign_in_page;
            declared.role_homes = profile
                .homes
                .into_iter()
                .map(|home| {
                    if home.page.is_some() == home.microflow.is_some() {
                        return Err(format!(
                            "the home of role {:?} is a page or a microflow, one of them",
                            home.role
                        ));
                    }
                    Ok(RoleHomeDecl {
                        user_role: home.role,
                        page: home.page,
                        microflow: home.microflow,
                    })
                })
                .collect::<Result<_, _>>()?;
            declared.items = profile
                .items
                .into_iter()
                .map(item)
                .collect::<Result<_, _>>()?;
            Ok(declared)
        })
        .collect::<Result<_, _>>()?;
    Ok(NavigationDecl { profiles })
}

fn item(item: Item) -> Result<NavigationItemDecl, String> {
    let caption = match item.caption {
        Caption::Text(text) => BTreeMap::from([("en_US".to_string(), text)]),
        Caption::Localized(captions) => captions,
    };
    if item.page.is_some() && item.microflow.is_some() {
        return Err(format!(
            "the item {caption:?} opens a page or calls a microflow, one of them"
        ));
    }
    let icon = match item.icon {
        None => None,
        Some(Icon {
            glyph: Some(glyph),
            code: None,
        }) => Some(NavigationIconDecl::Glyph(glyph)),
        Some(Icon {
            glyph: None,
            code: Some(code),
        }) => Some(NavigationIconDecl::Code(code)),
        Some(_) => {
            return Err(format!(
                "the icon of {caption:?} is a glyph or a code, one of them"
            ));
        }
    };
    Ok(NavigationItemDecl {
        caption,
        page: item.page,
        microflow: item.microflow,
        icon,
        items: item
            .items
            .into_iter()
            .map(self::item)
            .collect::<Result<_, _>>()?,
    })
}
