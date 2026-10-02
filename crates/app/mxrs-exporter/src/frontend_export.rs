//! The user interface a model declares, written as its frontend's
//! TypeScript: the files `mxrs-frontend` reads back on every build.

use mxrs_model::navigation::{NavigationIcon, NavigationItem};

/// Data as TypeScript writes it, its keys in the order they are written.
pub(crate) enum Ts {
    Text(String),
    Number(i64),
    Object(Vec<(String, Ts)>),
    Array(Vec<Ts>),
}

impl Ts {
    fn text(value: &str) -> Self {
        Ts::Text(value.to_string())
    }
}

/// `frontend/src/navigation/index.ts`: the model's navigation profiles as
/// the data that declares them.
pub(crate) fn render_navigation(navigation: &mxrs_model::Navigation) -> String {
    // Profiles keep the model's order: a build writes them in this one.
    let profiles = navigation
        .profiles
        .iter()
        .map(|profile| {
            let mut object = vec![("name".to_string(), Ts::text(&profile.name))];
            if profile.kind != "Responsive" {
                object.push(("kind".into(), Ts::text(&profile.kind)));
            }
            if !profile.app_title.is_empty() {
                object.push(("title".into(), localized(&profile.app_title)));
            }
            for (key, value) in [
                ("homePage", &profile.home_page),
                ("homeMicroflow", &profile.home_microflow),
                ("signInPage", &profile.sign_in_page),
            ] {
                if let Some(value) = value {
                    object.push((key.into(), Ts::text(value)));
                }
            }
            if !profile.role_homes.is_empty() {
                let homes = profile
                    .role_homes
                    .iter()
                    .map(|home| {
                        let mut object = vec![(
                            "role".to_string(),
                            Ts::text(home.role.as_deref().unwrap_or_default()),
                        )];
                        if let Some(page) = &home.page {
                            object.push(("page".into(), Ts::text(page)));
                        }
                        if let Some(microflow) = &home.microflow {
                            object.push(("microflow".into(), Ts::text(microflow)));
                        }
                        Ts::Object(object)
                    })
                    .collect();
                object.push(("homes".into(), Ts::Array(homes)));
            }
            if !profile.menu_items.is_empty() {
                object.push((
                    "items".into(),
                    Ts::Array(profile.menu_items.iter().map(item).collect()),
                ));
            }
            Ts::Object(object)
        })
        .collect();
    let declared = Ts::Object(vec![("profiles".to_string(), Ts::Array(profiles))]);
    format!(
        "// The project's navigation profiles. mxrs reads this file into the model\n// on every build: edit it as the application's navigation.\nimport type {{ Navigation }} from \"@/types/navigation\";\n\nexport default {} satisfies Navigation;\n",
        typescript(&declared, 0)
    )
}

fn item(item: &NavigationItem) -> Ts {
    let caption = match item.caption.get("en_US") {
        Some(text) if item.caption.len() == 1 => Ts::text(text),
        _ => localized(&item.caption),
    };
    let mut object = vec![("caption".to_string(), caption)];
    if let Some(page) = &item.page {
        object.push(("page".into(), Ts::text(page)));
    }
    if let Some(microflow) = &item.microflow {
        object.push(("microflow".into(), Ts::text(microflow)));
    }
    match &item.icon {
        Some(NavigationIcon::Glyph(glyph)) => object.push((
            "icon".into(),
            Ts::Object(vec![("glyph".into(), Ts::text(glyph))]),
        )),
        Some(NavigationIcon::Code(code)) => object.push((
            "icon".into(),
            Ts::Object(vec![("code".into(), Ts::Number(*code))]),
        )),
        None => {}
    }
    if !item.items.is_empty() {
        object.push((
            "items".into(),
            Ts::Array(item.items.iter().map(self::item).collect()),
        ));
    }
    Ts::Object(object)
}

fn localized(texts: &std::collections::BTreeMap<String, String>) -> Ts {
    Ts::Object(
        texts
            .iter()
            .map(|(language, text)| (language.clone(), Ts::text(text)))
            .collect(),
    )
}

/// `value` as TypeScript writes data: two-space indentation, keys bare
/// where they are identifiers, a trailing comma after every element.
pub(crate) fn typescript(value: &Ts, indent: usize) -> String {
    let pad = "  ".repeat(indent + 1);
    let close = "  ".repeat(indent);
    match value {
        Ts::Text(text) => serde_json::Value::String(text.clone()).to_string(),
        Ts::Number(number) => number.to_string(),
        Ts::Array(items) if items.is_empty() => "[]".to_string(),
        Ts::Array(items) => {
            let mut out = String::from("[\n");
            for item in items {
                out.push_str(&format!("{pad}{},\n", typescript(item, indent + 1)));
            }
            out.push_str(&format!("{close}]"));
            out
        }
        Ts::Object(entries) if entries.is_empty() => "{}".to_string(),
        Ts::Object(entries) => {
            let mut out = String::from("{\n");
            for (key, value) in entries {
                let identifier = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                    && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                let key = if identifier {
                    key.clone()
                } else {
                    serde_json::Value::String(key.clone()).to_string()
                };
                out.push_str(&format!("{pad}{key}: {},\n", typescript(value, indent + 1)));
            }
            out.push_str(&format!("{close}}}"));
            out
        }
    }
}
