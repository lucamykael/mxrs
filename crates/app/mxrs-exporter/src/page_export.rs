//! Detects pages built entirely from the widget vocabulary
//! `mxrs-dsl`/`mxrs-writer` can author (see `mxrs_ir::page`'s doc comment)
//! and renders them as real, compiled `mxrs-dsl` source: a standalone
//! `pub fn <page>() -> ::mxrs_ir::page::PageDecl` per detected page, plus
//! the `src/domain/mod.rs` wiring (see `lib.rs::render`) that pushes each
//! one into its module and returns it from `build()`.
//!
//! **Now wired into the live `build()`/`.mpr` output**, same as domain
//! entities. Two things had to be true first, both now closed:
//!
//! 1. **`layout` had to be reliably recoverable.** `mxrs_model::page::Page`
//!    now exposes `layout_id` *and* `layout_parameter` (the
//!    `LayoutCallArgument.Parameter` by-name reference), read together by
//!    `mxrs_model::page::extract_layout`/`extract_layout_parameter` off the
//!    real `LayoutCall`-based storage shape (`doc.FormCall.Form` /
//!    `doc.FormCall.Arguments[0].Parameter` — see those functions' doc
//!    comments for the storage-naming evidence). A page whose widgets exist
//!    but whose layout isn't resolvable is still treated as unconvertible
//!    (opaque), rather than emitting a `PageDecl` `compile_page` would
//!    reject at write time (`WriterError::PageWidgetsRequireLayout`).
//! 2. **`LayoutGrid` decode had to be lossless.** `mxrs_model::page::
//!    layout_grid_widget` now nests each column's actual widgets (via
//!    synthetic `"layout_grid_row"`/`"layout_grid_column"` wrapper
//!    `Widget`s) instead of a `"widget_count"` summary, so this module can
//!    walk back to `WidgetDecl::LayoutGrid`.
//!
//! **Still narrower than a full page**: no conditional visibility/dynamic
//! classes, no security roles. A page using any of those stays exactly as
//! opaque as before — served from the generated snapshot, contributing
//! nothing to `build()`.
//!
//! **`WidgetDecl::DataView`/attribute-bound widgets
//! (`TextBox`/`CheckBox`/`DatePicker`/`DropDown`), pluggable widgets
//! (`DataGrid2`/`Gallery`/`ComboBox`), and flow-calling button actions
//! (`ButtonAction::CallMicroflow`/`CallNanoflow`) are authoring-only — not
//! detected on import**, even though `mxrs-writer::page_compiler` fully
//! supports writing them (see that module's tests). Reason, concretely:
//! `mxrs_model::page::parse_action` and `data_view_widget` decode a *local*
//! (module-unqualified) handler name and a merged body+footer widget list
//! respectively — neither is enough to reconstruct what this IR needs (a
//! fully qualified `"Module.Name"` flow reference; a body list provably
//! free of footer widgets, which the decoded shape can't distinguish); and
//! `mxrs_model::page::pluggable_widget` never decodes a `CustomWidget`'s
//! `Type.WidgetId` at all, so there's no way to even tell a Data Grid 2
//! instance apart from any other pluggable widget from the decoded shape
//! this module consumes. Detecting these on import risks either emitting a
//! `PageDecl` with an unresolvable target, silently misplacing a footer
//! widget into the body, or misclassifying an arbitrary third-party widget
//! as one of these three — a real correctness risk, not just unfinished
//! breadth, so `try_convert_page`/`try_convert_widget` leave any page
//! containing them exactly as opaque as before rather than guess.
//! `render_widget` still matches every `WidgetDecl` variant exhaustively (a
//! `TODO` comment for these, mirroring the precedent `WidgetDecl::LayoutGrid`'s
//! own comment set before *its* import detection existed) so adding a new
//! variant here can't silently forget to update
//! the renderer.

use std::fmt::Write as _;

use mxrs_bson::Document;
use mxrs_ir::page::{
    ButtonAction, LayoutGridColumnDecl, LayoutGridRowDecl, LayoutRef, PageDecl, WidgetDecl,
};
use mxrs_model::Module;
use mxrs_model::page::{Page, Widget};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageExportReport {
    /// Pages built entirely from the detected widget vocabulary — wired
    /// directly into `build()` via `src/domain/pages.rs`.
    pub typed_candidates: usize,
    /// Every other page (pluggable widgets, data binding, conditional
    /// visibility, security roles, unresolvable layout, ...) — stays
    /// exactly as opaque as before this pass, served from the generated
    /// snapshot.
    pub opaque: usize,
}

/// One page detected as buildable from `mxrs-dsl`'s native/structural
/// widget vocabulary, ready to be rendered both as a standalone function
/// (`pages.rs`) and as a `src/domain/mod.rs` wiring statement that pushes
/// it into `module_name`'s `ModuleDecl.pages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedPage {
    pub module_name: String,
    pub function_name: String,
    pub decl: PageDecl,
}

/// Detects every convertible page across every module. Pure detection, no
/// rendering — shared by `render_pages_module` (the `pages.rs` text) and
/// `lib.rs::render` (the `build()` wiring), so the two can never drift
/// apart on which pages qualify.
pub fn convert_pages(modules: &[Module]) -> (Vec<ConvertedPage>, PageExportReport) {
    let mut report = PageExportReport::default();
    let mut pages = Vec::new();
    for module in modules {
        let module_name = module.name.clone().unwrap_or_else(|| "Unnamed".into());
        for page in &module.pages {
            match try_convert_page(page) {
                Some(decl) => {
                    report.typed_candidates += 1;
                    pages.push(ConvertedPage {
                        module_name: module_name.clone(),
                        function_name: to_snake_case(&decl.name),
                        decl,
                    });
                }
                None => report.opaque += 1,
            }
        }
    }
    (pages, report)
}

/// Renders every converted page into one `src/domain/pages.rs` source file
/// of real, compiled `pub fn` page-builders. Returns `None` when there is
/// nothing to show (no point writing an empty file).
pub fn render_pages_module(pages: &[ConvertedPage]) -> Option<String> {
    if pages.is_empty() {
        return None;
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Generated by `mxrs-exporter`. Each function below reproduces one page"
    );
    let _ = writeln!(
        out,
        "// detected as buildable from mxrs-dsl's native/structural widget"
    );
    let _ = writeln!(
        out,
        "// vocabulary (see mxrs_ir::page's doc comment) and is wired into"
    );
    let _ = writeln!(
        out,
        "// `build()` by src/domain/mod.rs — edit freely, same as the rest of"
    );
    let _ = writeln!(out, "// this crate's generated source.");
    out.push('\n');
    for page in pages {
        out.push_str(&render_page_function(page));
        out.push('\n');
    }
    Some(out)
}

fn try_convert_page(page: &Page) -> Option<PageDecl> {
    let name = page.name.clone()?;
    if page.data_source.is_some() {
        return None;
    }
    if !page.allowed_module_roles.is_empty() {
        return None;
    }
    if !page.appearance_class.is_empty() || !page.appearance_style.is_empty() {
        return None;
    }
    let widgets = page
        .widgets
        .iter()
        .map(try_convert_widget)
        .collect::<Option<Vec<_>>>()?;

    let layout = match (&page.layout_id, &page.layout_parameter) {
        (Some(qualified_name), Some(parameter)) => {
            Some(LayoutRef::new(qualified_name.clone(), parameter.clone()))
        }
        _ => None,
    };
    if layout.is_none() && !widgets.is_empty() {
        // `compile_page` requires a layout whenever a page has widgets
        // (`WriterError::PageWidgetsRequireLayout`) — a page whose layout
        // isn't resolvable can never round-trip through this IR, so treat
        // it as opaque rather than emit a `PageDecl` that would fail at
        // write time.
        return None;
    }

    let mut decl = PageDecl::new(name);
    decl.documentation = page.documentation.clone();
    decl.url = page.url.clone();
    if !page.title.is_empty() {
        decl.title = Some(page.title.clone());
    }
    decl.layout = layout;
    decl.widgets = widgets;
    Some(decl)
}

fn try_convert_widget(widget: &Widget) -> Option<WidgetDecl> {
    match widget.widget_type.as_str() {
        "container" => {
            if !widget.events.is_empty() || !only_keys(&widget.options, &["class", "style"]) {
                return None;
            }
            let children = widget
                .children
                .iter()
                .map(try_convert_widget)
                .collect::<Option<Vec<_>>>()?;
            Some(WidgetDecl::Container {
                name: widget.name.clone(),
                class: string_option(&widget.options, "class"),
                style: string_option(&widget.options, "style"),
                children,
            })
        }
        "text" => {
            if !widget.events.is_empty()
                || !only_keys(&widget.options, &["class", "style", "caption"])
            {
                return None;
            }
            let caption = string_option(&widget.options, "caption")?;
            Some(WidgetDecl::Text {
                name: widget.name.clone(),
                caption,
                class: string_option(&widget.options, "class"),
            })
        }
        "button" => {
            if !only_keys(&widget.options, &["class", "style", "caption"]) {
                return None;
            }
            let action = match widget.events.as_slice() {
                [] => ButtonAction::None,
                [event]
                    if event.get_str("kind").ok() == Some("action")
                        && event.get_str("handler").ok() == Some("close_page")
                        && event.get_str("event").ok() == Some("on_click") =>
                {
                    ButtonAction::ClosePage
                }
                _ => return None,
            };
            Some(WidgetDecl::Button {
                name: widget.name.clone(),
                caption: string_option(&widget.options, "caption").unwrap_or_default(),
                class: string_option(&widget.options, "class"),
                action,
            })
        }
        "layout_grid" => try_convert_layout_grid(widget),
        _ => None,
    }
}

/// Walks the `"layout_grid_row"`/`"layout_grid_column"` wrapper tree
/// `mxrs_model::page::layout_grid_widget` now nests real children under
/// (see that function's doc comment) back into `WidgetDecl::LayoutGrid`.
/// Only desktop-weight columns with no tablet/phone override convert —
/// `WidgetDecl::LayoutGrid` has no per-breakpoint weight concept yet (see
/// `mxrs_ir::page`), so a column that actually uses one stays opaque rather
/// than silently dropping it.
fn try_convert_layout_grid(widget: &Widget) -> Option<WidgetDecl> {
    if !widget.events.is_empty() || !only_keys(&widget.options, &[]) {
        return None;
    }
    let rows = widget
        .children
        .iter()
        .map(try_convert_layout_grid_row)
        .collect::<Option<Vec<_>>>()?;
    Some(WidgetDecl::LayoutGrid {
        name: widget.name.clone(),
        rows,
    })
}

fn try_convert_layout_grid_row(row: &Widget) -> Option<LayoutGridRowDecl> {
    if row.widget_type != "layout_grid_row" || !row.events.is_empty() {
        return None;
    }
    let columns = row
        .children
        .iter()
        .map(try_convert_layout_grid_column)
        .collect::<Option<Vec<_>>>()?;
    Some(LayoutGridRowDecl { columns })
}

fn try_convert_layout_grid_column(column: &Widget) -> Option<LayoutGridColumnDecl> {
    if column.widget_type != "layout_grid_column" || !column.events.is_empty() {
        return None;
    }
    let weight = column.options.get_i32("desktop").ok()?;
    let is_uniform_breakpoint = |key: &str| {
        column
            .options
            .get_i32(key)
            .ok()
            .is_some_and(|value| value == weight)
    };
    if !is_uniform_breakpoint("tablet") || !is_uniform_breakpoint("phone") {
        return None;
    }
    let children = column
        .children
        .iter()
        .map(try_convert_widget)
        .collect::<Option<Vec<_>>>()?;
    Some(LayoutGridColumnDecl { weight, children })
}

fn only_keys(document: &Document, allowed: &[&str]) -> bool {
    document.keys().all(|key| allowed.contains(&key.as_str()))
}

fn string_option(document: &Document, key: &str) -> Option<String> {
    document.get_str(key).ok().map(str::to_string)
}

fn render_page_function(page: &ConvertedPage) -> String {
    let decl = &page.decl;
    let mut out = String::new();
    let _ = writeln!(out, "/// `{}.{}`", page.module_name, decl.name);
    let _ = writeln!(
        out,
        "pub fn {}() -> ::mxrs_ir::page::PageDecl {{",
        page.function_name
    );
    let _ = writeln!(
        out,
        "    let mut p = ::mxrs_dsl::PageBuilder::new({:?});",
        decl.name
    );
    if !decl.documentation.is_empty() {
        let _ = writeln!(out, "    p.documentation({:?});", decl.documentation);
    }
    if !decl.url.is_empty() {
        let _ = writeln!(out, "    p.url({:?});", decl.url);
    }
    if let Some(title) = &decl.title {
        let _ = writeln!(out, "    p.title({title:?});");
    }
    if let Some(layout) = &decl.layout {
        let _ = writeln!(
            out,
            "    p.layout({:?}, {:?});",
            layout.qualified_name, layout.parameter
        );
    }
    for widget in &decl.widgets {
        out.push_str(&render_widget(widget, 1, "p"));
    }
    let _ = writeln!(out, "    p.into_decl()");
    let _ = writeln!(out, "}}");
    out
}

fn render_widget(widget: &WidgetDecl, indent: usize, receiver: &str) -> String {
    let pad = "    ".repeat(indent);
    let mut out = String::new();
    match widget {
        WidgetDecl::Container {
            name,
            class,
            style,
            children,
        } => {
            let _ = writeln!(out, "{pad}{receiver}.container(|w| {{");
            if let Some(name) = name {
                let _ = writeln!(out, "{pad}    w.name({name:?});");
            }
            if let Some(class) = class {
                let _ = writeln!(out, "{pad}    w.class({class:?});");
            }
            if let Some(style) = style {
                let _ = writeln!(out, "{pad}    w.style({style:?});");
            }
            for child in children {
                out.push_str(&render_widget(child, indent + 1, "w"));
            }
            let _ = writeln!(out, "{pad}}});");
        }
        WidgetDecl::LayoutGrid { name, rows } => {
            let _ = writeln!(out, "{pad}{receiver}.layout_grid(|grid| {{");
            if let Some(name) = name {
                let _ = writeln!(out, "{pad}    grid.name({name:?});");
            }
            for row in rows {
                let _ = writeln!(out, "{pad}    grid.row(|row| {{");
                for column in &row.columns {
                    let _ = writeln!(out, "{pad}        row.column({}, |col| {{", column.weight);
                    for child in &column.children {
                        out.push_str(&render_widget(child, indent + 3, "col"));
                    }
                    let _ = writeln!(out, "{pad}        }});");
                }
                let _ = writeln!(out, "{pad}    }});");
            }
            let _ = writeln!(out, "{pad}}});");
        }
        WidgetDecl::Text { caption, .. } => {
            let _ = writeln!(out, "{pad}{receiver}.text({caption:?});");
        }
        WidgetDecl::Button {
            caption, action, ..
        } => {
            let _ = writeln!(out, "{pad}{receiver}.button({caption:?}, |b| {{");
            match action {
                ButtonAction::ClosePage => {
                    let _ = writeln!(out, "{pad}    b.close_page();");
                }
                ButtonAction::None => {}
                ButtonAction::CallMicroflow(_) | ButtonAction::CallNanoflow(_) => {
                    // Unreachable from `try_convert_widget` today (see this
                    // module's doc comment: flow-calling buttons are
                    // authoring-only, not detected on import) — kept
                    // exhaustive rather than `unreachable!()`, same
                    // precedent `WidgetDecl::LayoutGrid`'s own comment set
                    // before its import detection existed.
                    let _ = writeln!(
                        out,
                        "{pad}    // TODO: flow-calling button import detection isn't supported yet"
                    );
                }
            }
            let _ = writeln!(out, "{pad}}});");
        }
        WidgetDecl::DataView { .. } => {
            // Unreachable from `try_convert_widget` today — see this
            // module's doc comment for why data-bound widgets are
            // authoring-only, not detected on import.
            let _ = writeln!(
                out,
                "{pad}// TODO: data view import detection isn't supported yet"
            );
        }
        WidgetDecl::TextBox { .. }
        | WidgetDecl::CheckBox { .. }
        | WidgetDecl::DatePicker { .. }
        | WidgetDecl::DropDown { .. } => {
            let _ = writeln!(
                out,
                "{pad}// TODO: attribute-bound widget import detection isn't supported yet"
            );
        }
        WidgetDecl::DataGrid2 { .. } | WidgetDecl::Gallery { .. } | WidgetDecl::ComboBox { .. } => {
            // Unreachable from `try_convert_widget` today: `mxrs_model::page::pluggable_widget`
            // only decodes a `CustomWidgets$CustomWidget`'s outer `Name`, not
            // its `Type.WidgetId` — there's no way to tell a Data Grid 2
            // instance apart from a Gallery, ComboBox, or arbitrary
            // third-party widget from the decoded shape this module
            // consumes (a real, separately trackable `mxrs-model` gap, not
            // fixed here).
            let _ = writeln!(
                out,
                "{pad}// TODO: pluggable widget import detection isn't supported yet"
            );
        }
    }
    out
}

/// `OrderOverview` -> `order_overview`. A defensive, not exhaustive, name
/// sanitizer mirroring `mxrs-exporter`'s own `sanitize_ident` for the
/// domain-model renderer: non-alphanumeric characters become `_`.
fn to_snake_case(name: &str) -> String {
    let mut out = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else if ch.is_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(children: Vec<Widget>) -> Widget {
        Widget {
            widget_type: "container".into(),
            name: Some("row".into()),
            options: mxrs_bson::doc! { "class": "row" },
            events: vec![],
            children,
        }
    }

    fn text(caption: &str) -> Widget {
        Widget {
            widget_type: "text".into(),
            name: Some("hint".into()),
            options: mxrs_bson::doc! { "caption": caption },
            events: vec![],
            children: vec![],
        }
    }

    fn close_button(caption: &str) -> Widget {
        Widget {
            widget_type: "button".into(),
            name: Some("close".into()),
            options: mxrs_bson::doc! { "caption": caption },
            events: vec![
                mxrs_bson::doc! { "kind": "action", "handler": "close_page", "event": "on_click" },
            ],
            children: vec![],
        }
    }

    fn layout_grid_column(weight: i32, children: Vec<Widget>) -> Widget {
        Widget {
            widget_type: "layout_grid_column".into(),
            name: None,
            options: mxrs_bson::doc! { "desktop": weight, "tablet": weight, "phone": weight },
            events: vec![],
            children,
        }
    }

    fn layout_grid_row(columns: Vec<Widget>) -> Widget {
        Widget {
            widget_type: "layout_grid_row".into(),
            name: None,
            options: Document::new(),
            events: vec![],
            children: columns,
        }
    }

    fn layout_grid(rows: Vec<Widget>) -> Widget {
        Widget {
            widget_type: "layout_grid".into(),
            name: Some("grid1".into()),
            options: Document::new(),
            events: vec![],
            children: rows,
        }
    }

    fn bare_page(name: &str, widgets: Vec<Widget>) -> Page {
        page_with_layout(
            name,
            widgets,
            Some(("Atlas_Core.ApplicationLayout", "Main")),
        )
    }

    fn page_with_layout(name: &str, widgets: Vec<Widget>, layout: Option<(&str, &str)>) -> Page {
        let mut doc = mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Forms$Page",
            "Name": name,
        };
        if let Some((qualified_name, parameter)) = layout {
            doc.insert(
                "FormCall",
                mxrs_bson::doc! {
                    "Form": qualified_name,
                    "Arguments": mxrs_bson::build_array(
                        vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                            "Parameter": parameter,
                            "Widgets": mxrs_bson::build_array(
                                widgets.into_iter().map(widget_to_bson).collect(),
                                3,
                            ),
                        })],
                        3,
                    ),
                },
            );
        } else {
            doc.insert(
                "Widgets",
                mxrs_bson::build_array(widgets.into_iter().map(widget_to_bson).collect(), 3),
            );
        }
        Page::from_bson(&doc)
    }

    fn widget_to_bson(widget: Widget) -> mxrs_bson::Bson {
        match widget.widget_type.as_str() {
            "container" => mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Forms$DivContainer",
                "Name": widget.name.unwrap_or_default(),
                "Class": widget.options.get_str("class").unwrap_or("").to_string(),
                "Widgets": mxrs_bson::build_array(
                    widget.children.into_iter().map(widget_to_bson).collect(),
                    3,
                ),
            }),
            "text" => {
                let mut doc = mxrs_bson::doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "Forms$DynamicText",
                    "Name": widget.name.unwrap_or_default(),
                    "Content": widget.options.get_str("caption").unwrap_or("").to_string(),
                };
                if let Ok(expression) = widget.options.get_str("visible") {
                    doc.insert(
                        "ConditionalVisibilitySettings",
                        mxrs_bson::doc! {
                            "$ID": uuid::Uuid::new_v4().to_string(),
                            "$Type": "Forms$ConditionalVisibilitySettings",
                            "Expression": expression.to_string(),
                        },
                    );
                }
                mxrs_bson::Bson::Document(doc)
            }
            "button" => mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Forms$ActionButton",
                "Name": widget.name.unwrap_or_default(),
                "Caption": widget.options.get_str("caption").unwrap_or("").to_string(),
                "Action": { "$Type": "Forms$ClosePageClientAction" },
            }),
            "layout_grid" => mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Forms$LayoutGrid",
                "Name": widget.name.unwrap_or_default(),
                "Rows": mxrs_bson::build_array(
                    widget.children.into_iter().map(row_to_bson).collect(),
                    3,
                ),
            }),
            other => panic!("unsupported test widget type {other}"),
        }
    }

    fn row_to_bson(row: Widget) -> mxrs_bson::Bson {
        mxrs_bson::Bson::Document(mxrs_bson::doc! {
            "Columns": mxrs_bson::build_array(
                row.children.into_iter().map(column_to_bson).collect(),
                3,
            ),
        })
    }

    fn column_to_bson(column: Widget) -> mxrs_bson::Bson {
        mxrs_bson::Bson::Document(mxrs_bson::doc! {
            "Weight": column.options.get_i32("desktop").unwrap_or(-1),
            "TabletWeight": column.options.get_i32("tablet").unwrap_or(-1),
            "PhoneWeight": column.options.get_i32("phone").unwrap_or(-1),
            "Widgets": mxrs_bson::build_array(
                column.children.into_iter().map(widget_to_bson).collect(),
                3,
            ),
        })
    }

    #[test]
    fn a_page_built_from_container_text_and_close_button_converts() {
        let page = bare_page(
            "OrderOverview",
            vec![container(vec![
                text("Manage your orders"),
                close_button("Close"),
            ])],
        );
        let decl = try_convert_page(&page).expect("should convert");
        assert_eq!(decl.name, "OrderOverview");
        assert_eq!(decl.widgets.len(), 1);
        let layout = decl.layout.expect("layout recovered from FormCall");
        assert_eq!(layout.qualified_name, "Atlas_Core.ApplicationLayout");
        assert_eq!(layout.parameter, "Main");
    }

    #[test]
    fn a_page_with_widgets_but_no_resolvable_layout_is_left_opaque() {
        let page = page_with_layout("Broken", vec![text("hi")], None);
        assert!(try_convert_page(&page).is_none());
    }

    #[test]
    fn a_layout_grid_with_uniform_weighted_columns_converts_losslessly() {
        let page = bare_page(
            "Dashboard",
            vec![layout_grid(vec![layout_grid_row(vec![
                layout_grid_column(1, vec![text("Left")]),
                layout_grid_column(2, vec![text("Right")]),
            ])])],
        );
        let decl = try_convert_page(&page).expect("should convert");
        let WidgetDecl::LayoutGrid { rows, .. } = &decl.widgets[0] else {
            panic!("expected a layout grid");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].columns.len(), 2);
        assert_eq!(rows[0].columns[0].weight, 1);
        assert!(matches!(
            &rows[0].columns[0].children[0],
            WidgetDecl::Text { caption, .. } if caption == "Left"
        ));
        assert_eq!(rows[0].columns[1].weight, 2);
    }

    #[test]
    fn a_layout_grid_column_with_a_breakpoint_override_is_left_opaque() {
        let mut column = layout_grid_column(1, vec![text("Left")]);
        column.options.insert("tablet", -2);
        let page = bare_page(
            "Dashboard",
            vec![layout_grid(vec![layout_grid_row(vec![column])])],
        );
        assert!(try_convert_page(&page).is_none());
    }

    #[test]
    fn convert_pages_counts_typed_and_opaque_pages_separately() {
        let convertible = bare_page("Simple", vec![text("hi")]);
        let unsupported = bare_page(
            "WithVisibility",
            vec![Widget {
                widget_type: "text".into(),
                name: Some("hint".into()),
                options: mxrs_bson::doc! { "caption": "hi", "visible": "$currentObject/Active" },
                events: vec![],
                children: vec![],
            }],
        );
        let module = Module {
            id: "m".into(),
            name: Some("Sales".into()),
            sort_index: None,
            from_app_store: false,
            app_store_guid: None,
            app_store_version: None,
            export_level: "Hidden".into(),
            domain_model: None,
            pages: vec![convertible, unsupported],
            microflows: vec![],
            nanoflows: vec![],
            rules: vec![],
            menus: vec![],
            module_roles: vec![],
            artifact_units: vec![],
        };

        let (pages, report) = convert_pages(std::slice::from_ref(&module));
        assert_eq!(report.typed_candidates, 1);
        assert_eq!(report.opaque, 1);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].module_name, "Sales");
        assert_eq!(pages[0].function_name, "simple");

        let source = render_pages_module(&pages).expect("one convertible page should render");
        assert!(source.contains("pub fn simple"));
        assert!(!source.contains("with_visibility"));
    }

    #[test]
    fn a_page_with_a_data_bound_text_widget_is_left_opaque() {
        let page = bare_page(
            "Bound",
            vec![Widget {
                widget_type: "text".into(),
                name: Some("hint".into()),
                options: mxrs_bson::doc! { "attribute": "Order/Number" },
                events: vec![],
                children: vec![],
            }],
        );
        assert!(try_convert_page(&page).is_none());
    }
}
