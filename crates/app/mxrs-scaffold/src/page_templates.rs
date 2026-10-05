//! The audited page-pattern catalog behind `mxrs page new --template NAME`
//! and `mxrs page templates`. Ported from mxrb's
//! `lib/mxrb/scaffold/page_templates.rb`, including its four entries, their
//! categories, descriptions and the `data_backed` flag that decides whether
//! `--chain` also generates a backing entity and loader.
//!
//! Kept as data rather than folded into `templates.rs` so the catalog the CLI
//! *prints* and the catalog the generator *dispatches on* cannot drift: both
//! read [`ENTRIES`], and an entry with no renderer is a compile error.

use crate::{Result, ScaffoldError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageTemplate {
    pub category: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Whether the template renders a data view. A data-backed template needs
    /// a context object, which is why `--chain` generates an entity and an
    /// `ACT_Load…` microflow for it and not for the others.
    pub data_backed: bool,
}

/// Order is meaningful: it is the order `templates` prints, and categories are
/// grouped in first-appearance order the way mxrb's `group_by` leaves them.
/// The template whose page name is an entity's: its overview and edit pages.
pub const CRUD: &str = "crud";

pub const ENTRIES: &[PageTemplate] = &[
    PageTemplate {
        category: "General",
        name: "starter",
        description: "Title and page header",
        data_backed: false,
    },
    PageTemplate {
        category: "General",
        name: "blank",
        description: "Empty responsive content area",
        data_backed: false,
    },
    PageTemplate {
        category: "Dashboards",
        name: "dashboard",
        description: "Header and three responsive cards",
        data_backed: false,
    },
    PageTemplate {
        category: "Forms",
        name: "form-vertical",
        description: "DataView with vertical inputs and actions",
        data_backed: true,
    },
    PageTemplate {
        category: "Data",
        name: CRUD,
        description: "Overview and edit pages of an entity the project declares (named by the entity)",
        data_backed: false,
    },
];

/// The template `--chain` falls back to when none is named, matching mxrb's
/// `template || 'form-vertical'`.
pub const DEFAULT_CHAIN_TEMPLATE: &str = "form-vertical";

pub fn fetch(name: &str) -> Result<PageTemplate> {
    ENTRIES
        .iter()
        .copied()
        .find(|entry| entry.name == name)
        .ok_or_else(|| ScaffoldError::UnknownPageTemplate(name.to_string()))
}

/// Categories in first-appearance order, each with its entries.
pub fn grouped() -> Vec<(&'static str, Vec<PageTemplate>)> {
    let mut groups: Vec<(&'static str, Vec<PageTemplate>)> = Vec::new();
    for entry in ENTRIES {
        match groups
            .iter_mut()
            .find(|(category, _)| *category == entry.category)
        {
            Some((_, entries)) => entries.push(*entry),
            None => groups.push((entry.category, vec![*entry])),
        }
    }
    groups
}

/// The same box-drawing tree mxrb's `PageTemplates.tree` renders.
pub fn tree() -> String {
    let groups = grouped();
    let mut lines = vec!["Page templates".to_string()];
    for (index, (category, entries)) in groups.iter().enumerate() {
        let last_group = index == groups.len() - 1;
        lines.push(format!(
            "{} {category}",
            if last_group { "└──" } else { "├──" }
        ));
        let indent = if last_group { "    " } else { "│   " };
        for (index, entry) in entries.iter().enumerate() {
            let branch = if index == entries.len() - 1 {
                "└──"
            } else {
                "├──"
            };
            lines.push(format!(
                "{indent}{branch} {} — {}",
                entry.name, entry.description
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_groups_categories_in_first_appearance_order_and_closes_the_last_one() {
        assert_eq!(
            tree(),
            "Page templates\n\
             ├── General\n\
             │   ├── starter — Title and page header\n\
             │   └── blank — Empty responsive content area\n\
             ├── Dashboards\n\
             │   └── dashboard — Header and three responsive cards\n\
             ├── Forms\n\
             │   └── form-vertical — DataView with vertical inputs and actions\n\
             └── Data\n    \
             └── crud — Overview and edit pages of an entity the project declares (named by the entity)"
        );
    }

    #[test]
    fn an_unknown_template_is_named_rather_than_silently_defaulted() {
        assert!(fetch("form-horizontal").is_err());
        assert!(fetch("form-vertical").unwrap().data_backed);
        assert!(!fetch("starter").unwrap().data_backed);
    }
}
