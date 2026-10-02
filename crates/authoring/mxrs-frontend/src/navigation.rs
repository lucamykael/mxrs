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
#[serde(untagged, expecting = "a caption: a text, or texts by language code")]
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

/// Why a value does not declare a navigation: at which path, and what is
/// wrong there.
pub(crate) type Refusal = (String, String);

/// The navigation `value` declares, or why it does not declare one.
pub(crate) fn navigation(value: &serde_json::Value) -> Result<NavigationDecl, Refusal> {
    let declared: Navigation = serde_path_to_error::deserialize(value)
        .map_err(|error| (error.path().to_string(), error.inner().to_string()))?;
    let mut names = std::collections::HashSet::new();
    let profiles = declared
        .profiles
        .into_iter()
        .enumerate()
        .map(|(index, profile)| {
            let at = format!("profiles[{index}]");
            if !names.insert(profile.name.clone()) {
                return Err((at, format!("a second profile named {:?}", profile.name)));
            }
            if profile.home_page.is_some() && profile.home_microflow.is_some() {
                return Err((at, "a home page or a home microflow, not both".to_string()));
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
                .enumerate()
                .map(|(home_index, home)| {
                    if home.page.is_some() == home.microflow.is_some() {
                        return Err((
                            format!("{at}.homes[{home_index}]"),
                            format!(
                                "the home of role {:?} is a page or a microflow, one of them",
                                home.role
                            ),
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
                .enumerate()
                .map(|(item_index, declared)| item(declared, format!("{at}.items[{item_index}]")))
                .collect::<Result<_, _>>()?;
            Ok(declared)
        })
        .collect::<Result<_, _>>()?;
    Ok(NavigationDecl { profiles })
}

fn item(item: Item, at: String) -> Result<NavigationItemDecl, Refusal> {
    let caption = match item.caption {
        Caption::Text(text) => BTreeMap::from([("en_US".to_string(), text)]),
        Caption::Localized(captions) => captions,
    };
    if item.page.is_some() && item.microflow.is_some() {
        return Err((
            at,
            "an item opens a page or calls a microflow, one of them".to_string(),
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
            return Err((
                format!("{at}.icon"),
                "an icon is a glyph or a code, one of them".to_string(),
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
            .enumerate()
            .map(|(index, child)| self::item(child, format!("{at}.items[{index}]")))
            .collect::<Result<_, _>>()?,
    })
}
