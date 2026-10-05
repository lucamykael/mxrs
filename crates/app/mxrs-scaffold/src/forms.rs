//! Where a scaffolded page or layout goes: the frontend, as the TSX that
//! states its document — the file an import would have written for it.
//!
//! The scaffold builds what the page is (`PageDecl`), has the writer make
//! the document the model would store, and states that document with the
//! elements `frontend/src/mxrs/elements.ts` declares. An element the
//! project does not have yet is added to that file, after the ones it has:
//! their defaults are what the project's pages were written against.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use mxrs_frontend::forms::{self, Shapes, Vocabulary};
use mxrs_ir::page::{ButtonAction, DataSourceDecl, LayoutDecl, LayoutRef, PageDecl, WidgetDecl};

use crate::templates::{RefreshAction, humanize, snake_case};
use crate::transaction::Transaction;
use crate::{Result, ScaffoldError};

/// The files that declare forms in a frontend, by their paths under
/// `frontend/src/`.
pub(crate) struct Declared {
    /// `mxrs/elements.ts` whole, when the forms needed elements it lacked.
    pub(crate) elements: Option<String>,
    /// The pluggable widgets the forms use and the project did not define.
    pub(crate) widgets: Vec<(String, String)>,
    pub(crate) forms: Vec<(String, String)>,
}

fn invalid(path: &str, reason: impl ToString) -> ScaffoldError {
    ScaffoldError::InvalidProjectSource {
        path: path.to_string(),
        reason: reason.to_string(),
    }
}

/// States `stored` — each a module and the document of one of its forms —
/// as TSX, with the elements `elements` declares and `widgets` defines
/// (each a file's name and source) and whatever more they need.
pub(crate) fn declare(
    elements: Option<&str>,
    widgets: &[(String, String)],
    stored: &[(&str, mxrs_bson::Document)],
) -> Result<Declared> {
    let elements_path = "frontend/src/mxrs/elements.ts";
    let shapes = match elements {
        Some(source) => forms::read_elements(source, elements_path)
            .map_err(|error| invalid(elements_path, error))?,
        None => Shapes::default(),
    };
    let mut defined = Vec::with_capacity(widgets.len());
    for (file, source) in widgets {
        defined
            .push(forms::read_widget(source, file, &shapes).map_err(|error| invalid(file, error))?);
    }
    let vocabulary = Vocabulary {
        shapes,
        widgets: defined,
    };
    let mut documents = Vec::with_capacity(stored.len());
    for (module, document) in stored {
        let name = document.get_str("Name").unwrap_or_default().to_string();
        let stated = mxrs_writer::stated_document(document)
            .map_err(|reason| invalid(&format!("{module}.{name}"), reason))?;
        documents.push((*module, name, stated));
    }
    let references: Vec<&mxrs_ir::NativeDocument> =
        documents.iter().map(|(_, _, document)| document).collect();
    let extension =
        forms::extend(&vocabulary, &references).map_err(|reason| invalid(elements_path, reason))?;
    let mut declared = Declared {
        elements: None,
        widgets: Vec::new(),
        forms: Vec::new(),
    };
    if !extension.elements.is_empty() {
        let mut text = match elements {
            Some(source) => source.trim_end().to_string() + "\n",
            None => forms::render_elements(&Shapes::default()),
        };
        text.push_str(&extension.elements);
        declared.elements = Some(text);
    }
    for widget in &extension.widgets {
        let file = format!("{}/{}.tsx", forms::WIDGETS_FOLDER, widget.name);
        let source = forms::render_widget(widget, &extension.vocabulary)
            .map_err(|reason| invalid(&file, reason))?;
        declared.widgets.push((file, source));
    }
    for (module, name, document) in &documents {
        let folder = forms::folder(&document.ty)
            .ok_or_else(|| invalid(name, "it is not a page, layout or snippet"))?;
        let file = format!("{folder}/{}/{name}.tsx", snake_case(module));
        let source = forms::render_form(module, document, &extension.vocabulary)
            .map_err(|reason| invalid(&file, reason))?;
        // What a build will read is the text.
        match forms::read_form(&source, &file, &extension.vocabulary) {
            Ok((read_module, read)) if &read_module == module && &read.document == document => {}
            Ok(_) => return Err(invalid(&file, "its TSX reads back differently")),
            Err(error) => return Err(invalid(&file, error)),
        }
        declared.forms.push((file, source));
    }
    Ok(declared)
}

/// Adds `stored` to the project's frontend: the forms' files, and what
/// `elements.ts` and `widgets/` gain for them.
pub(crate) fn add_forms(
    transaction: &mut Transaction,
    root: &Path,
    stored: &[(&str, mxrs_bson::Document)],
) -> Result<()> {
    let source = root.join("frontend/src");
    let elements_path = source.join("mxrs/elements.ts");
    let elements = transaction.content(&elements_path)?;
    let mut widgets = Vec::new();
    let folder = source.join(forms::WIDGETS_FOLDER);
    if let Ok(entries) = std::fs::read_dir(&folder) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|file| file.extension().is_some_and(|extension| extension == "tsx"))
            .collect();
        files.sort();
        for file in files {
            if let Some(text) = transaction.content(&file)? {
                widgets.push((file.display().to_string(), text));
            }
        }
    }
    let declared = declare(elements.as_deref(), &widgets, stored)?;
    if let Some(text) = declared.elements {
        if elements.is_some() {
            transaction.write(&elements_path, text)?;
        } else {
            transaction.create(&elements_path, text)?;
        }
    }
    for (file, text) in declared.widgets.into_iter().chain(declared.forms) {
        transaction.create(source.join(file), text)?;
    }
    Ok(())
}

fn catalog(version: &str) -> Result<Rc<mxrs_forms::Catalog>> {
    mxrs_forms::Catalog::for_version(version)
        .map(Rc::new)
        .map_err(|error| ScaffoldError::InvalidVersion(format!("{version}: {error}")))
}

/// The document the model stores for `page`.
pub(crate) fn page_document(version: &str, page: &PageDecl) -> Result<mxrs_bson::Document> {
    mxrs_writer::page_compiler::compile_page(&catalog(version)?, page, None)
        .map_err(|error| invalid(&page.name, error))
}

/// The document the model stores for `layout`.
pub(crate) fn layout_document(version: &str, layout: &LayoutDecl) -> Result<mxrs_bson::Document> {
    mxrs_writer::layout_compiler::compile_layout(&catalog(version)?, layout, None)
        .map_err(|error| invalid(&layout.name, error))
}

/// The application's shell: its header, its navigation and the place a
/// page's content goes.
pub(crate) fn presentation_layout() -> LayoutDecl {
    let mut layout = LayoutDecl::new("ApplicationLayout");
    layout.class = Some("mxrb-application-shell".to_string());
    layout.widgets.push(WidgetDecl::ApplicationShell {
        title: "ApplicationLayout".to_string(),
        navigation: Some("Responsive".to_string()),
    });
    layout
}

/// The layout scaffolded pages are shown in: one placeholder.
pub(crate) fn application_layout(parameter: &str) -> LayoutDecl {
    let mut layout = LayoutDecl::new("ApplicationLayout");
    layout.widgets.push(WidgetDecl::LayoutPlaceholder {
        name: parameter.to_string(),
    });
    layout
}

fn text(caption: &str) -> WidgetDecl {
    WidgetDecl::Text {
        name: None,
        caption: caption.to_string(),
        class: None,
    }
}

fn container(name: &str, class: &str, children: Vec<WidgetDecl>) -> WidgetDecl {
    WidgetDecl::Container {
        name: Some(name.to_string()),
        class: Some(class.to_string()),
        style: None,
        children,
    }
}

fn button(name: &str, caption: &str, action: ButtonAction) -> WidgetDecl {
    WidgetDecl::Button {
        name: Some(name.to_string()),
        caption: caption.to_string(),
        class: None,
        action,
    }
}

fn page(module_name: &str, name: &str, parameter: &str, title: &str, roles: &[String]) -> PageDecl {
    let mut page = PageDecl::new(name);
    page.title = Some(title.to_string());
    page.layout = Some(LayoutRef::new(
        format!("{module_name}.ApplicationLayout"),
        parameter,
    ));
    page.allowed_module_roles = roles.to_vec();
    page
}

/// The page `mxrs new` starts a project with.
pub(crate) fn home_page(application: &str) -> PageDecl {
    let mut page = PageDecl::new("Home");
    page.layout = Some(LayoutRef::new("Main.ApplicationLayout", "Main"));
    page.widgets
        .push(text(&format!("Welcome to {application}")));
    page
}

/// A page with its title and nothing else.
pub(crate) fn plain_page(
    module_name: &str,
    name: &str,
    parameter: &str,
    roles: &[String],
) -> PageDecl {
    let title = humanize(name);
    let mut page = page(module_name, name, parameter, &title, roles);
    page.widgets.push(text(&title));
    page
}

/// The attributes of the entity a data-backed page binds, as the model
/// names them.
const CHAIN_ATTRIBUTES: [&str; 3] = ["Reference", "Total", "Active"];

/// A page from a catalogued template, with the Refresh button of its chain
/// when it has one.
pub(crate) fn templated_page(
    module_name: &str,
    name: &str,
    parameter: &str,
    template: &str,
    refresh: Option<RefreshAction>,
    roles: &[String],
) -> PageDecl {
    let title = humanize(name);
    let mut page = page(module_name, name, parameter, &title, roles);
    let refresh = refresh.map(|refresh| {
        button(
            "refresh",
            "Refresh",
            match refresh {
                RefreshAction::Microflow => {
                    ButtonAction::CallMicroflow(format!("{module_name}.ACT_Refresh{name}"))
                }
                RefreshAction::Nanoflow => {
                    ButtonAction::CallNanoflow(format!("{module_name}.NAN_Refresh{name}"))
                }
            },
        )
    });
    let header = |subtitle: &str| {
        container(
            "pageHeader",
            "mxrs-page-header",
            vec![text(&title), text(subtitle)],
        )
    };
    match template {
        "starter" => {
            page.widgets
                .push(header("Page generated from the starter template"));
            page.widgets.extend(refresh);
        }
        "blank" => {
            page.widgets.push(container(
                "content",
                "mxrs-page-content",
                refresh.into_iter().collect(),
            ));
        }
        "dashboard" => {
            page.widgets.push(header("Dashboard overview"));
            let cards = [
                (
                    "primary",
                    "PRIMARY",
                    "0",
                    "Connect this card to your domain data.",
                ),
                (
                    "secondary",
                    "SECONDARY",
                    "0",
                    "Replace this metric with a business signal.",
                ),
                (
                    "activity",
                    "ACTIVITY",
                    "Ready",
                    "Add charts, lists or actions here.",
                ),
            ]
            .iter()
            .map(|(slot, label, value, help)| {
                container(
                    &format!("{slot}Metric"),
                    "mxrs-card",
                    vec![text(label), text(value), text(help)],
                )
            })
            .collect();
            page.widgets
                .push(container("dashboard", "mxrs-dashboard-grid", cards));
            page.widgets.extend(refresh);
        }
        _ => {
            let [reference, total, active] = CHAIN_ATTRIBUTES;
            let bound = |attribute: &str| (None, attribute.to_string(), None);
            let (name_1, attribute_1, class_1) = bound(reference);
            let (name_2, attribute_2, class_2) = bound(total);
            let (name_3, attribute_3, class_3) = bound(active);
            let mut children = vec![
                header("Executable page scaffold"),
                WidgetDecl::TextBox {
                    name: name_1,
                    attribute: attribute_1,
                    class: class_1,
                },
                WidgetDecl::TextBox {
                    name: name_2,
                    attribute: attribute_2,
                    class: class_2,
                },
                WidgetDecl::CheckBox {
                    name: name_3,
                    attribute: attribute_3,
                    class: class_3,
                },
                button("save", "Save", ButtonAction::SaveChanges),
                button("cancel", "Cancel", ButtonAction::CancelChanges),
            ];
            children.extend(refresh);
            page.widgets.push(WidgetDecl::DataView {
                name: None,
                source: DataSourceDecl::Microflow(format!("{module_name}.ACT_Load{name}")),
                children,
            });
        }
    }
    page
}

/// `source` of a `frontend/src/navigation/index.ts` with an item for
/// `page` added to the items of its Responsive profile — the first profile
/// when none is named that. `None` when the file does not declare its
/// profiles the way an import or `mxrs new` writes them.
pub(crate) fn add_navigation_item(source: &str, caption: &str, page: &str) -> Option<String> {
    let item = format!(
        "{{ caption: {}, page: {}, icon: {{ glyph: \"file\" }} }},",
        serde_json::Value::String(caption.to_string()),
        serde_json::Value::String(page.to_string())
    );
    let profile = source
        .find("name: \"Responsive\"")
        .or_else(|| source.find("name: "))?;
    // The profile's own text: from its name to the brace that closes it.
    let opened = source[..profile].rfind('{')?;
    let indent = source[..opened]
        .rsplit('\n')
        .next()
        .filter(|line| line.trim().is_empty())?
        .len();
    let closing = format!("\n{}}}", " ".repeat(indent));
    let end = profile + source[profile..].find(&closing)?;
    let inner = " ".repeat(indent + 2);
    let body = &source[profile..end];
    if let Some(items) = body.find("items: [") {
        let at = profile + items + "items: [".len();
        // A list written on one line has no place for another line.
        if !source[at..].starts_with('\n') {
            if !source[at..].starts_with(']') {
                return None;
            }
            return Some(format!(
                "{}\n{inner}  {item}\n{inner}{}",
                &source[..at],
                &source[at..]
            ));
        }
        return Some(format!(
            "{}\n{inner}  {item}{}",
            &source[..at],
            &source[at..]
        ));
    }
    Some(format!(
        "{}\n{inner}items: [\n{inner}  {item}\n{inner}],{}",
        &source[..end],
        &source[end..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_navigation_item_joins_the_responsive_profile() {
        let fresh = crate::templates::navigation();
        let once = add_navigation_item(&fresh, "Orders", "Sales.Orders").unwrap();
        assert!(
            once.contains(
                "      homePage: \"Main.Home\",\n      items: [\n        { caption: \"Orders\", page: \"Sales.Orders\", icon: { glyph: \"file\" } },\n      ],\n    },\n"
            ),
            "{once}"
        );
        let twice = add_navigation_item(&once, "Invoices", "Sales.Invoices").unwrap();
        assert!(
            twice.contains(
                "      items: [\n        { caption: \"Invoices\", page: \"Sales.Invoices\", icon: { glyph: \"file\" } },\n        { caption: \"Orders\", page: \"Sales.Orders\", icon: { glyph: \"file\" } },\n      ],\n"
            ),
            "{twice}"
        );
        // A file written another way is left for its author.
        assert_eq!(
            add_navigation_item("export default {};\n", "A", "M.P"),
            None
        );
    }

    #[test]
    fn scaffolded_forms_are_stated_with_the_elements_they_add() {
        let layout = layout_document("11.12.1", &application_layout("Main")).unwrap();
        let home = page_document("11.12.1", &home_page("Shop")).unwrap();
        let first = declare(None, &[], &[("Main", layout), ("Main", home)]).unwrap();
        let elements = first.elements.unwrap();
        assert!(elements.contains("export const Page = element(\"Forms$Page\""));
        let files: Vec<&str> = first.forms.iter().map(|(file, _)| file.as_str()).collect();
        assert_eq!(
            files,
            [
                "components/layout/main/ApplicationLayout.tsx",
                "pages/main/Home.tsx"
            ]
        );
        assert!(
            first.forms[1].1.contains("Welcome to Shop"),
            "{}",
            first.forms[1].1
        );
        // A second page states itself with what the project has, and adds
        // only what it lacks, after it.
        let orders = page_document(
            "11.12.1",
            &templated_page("Sales", "Orders", "Main", "dashboard", None, &[]),
        )
        .unwrap();
        let second = declare(Some(&elements), &[], &[("Sales", orders)]).unwrap();
        let grown = second.elements.unwrap();
        assert!(grown.starts_with(elements.trim_end()), "{grown}");
        assert!(grown.len() > elements.len());
        assert_eq!(second.forms[0].0, "pages/sales/Orders.tsx");
        // Once more needs nothing new.
        let again = page_document(
            "11.12.1",
            &templated_page("Sales", "Stock", "Main", "dashboard", None, &[]),
        )
        .unwrap();
        assert!(
            declare(Some(&grown), &[], &[("Sales", again)])
                .unwrap()
                .elements
                .is_none()
        );
    }
}
