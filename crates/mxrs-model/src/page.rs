//! Page documents (`Pages$Page` / `Forms$Page`) and their widget tree.
//! Ports `lib/mxrb/model/page.rb` from mxrb.
//!
//! Scope note: every `CustomWidgets$CustomWidget` (Studio Pro's pluggable
//! "Custom Widget" wrapper — which in Mendix 11 backs not just marketplace
//! widgets but several of Studio Pro's own modern built-ins like Data Grid 2,
//! Gallery and ComboBox) decodes to a shallow [`Widget`] (type "pluggable" +
//! name + `native_type`, nested widgets still recursed into `children`)
//! rather than mxrb's fully typed per-widget option shape. The pluggable
//! `Object`/`Type` custom-property model is explicitly deferred post-MVP as
//! `mxrs-pluggable` — see `decisions/mxrs-rust-rewrite-plan.md` in this
//! project's ai-memory, and `mxrs-forms`'s own `FormsError::PluggableNotSupported`.
//! Every native (non-pluggable) widget kind mxrb supports — including
//! composite ones like DataGrid, TabControl, Table and LayoutGrid — is fully
//! ported.

use mxrs_bson::{Bson, Document};

use crate::support::{
    docs_any, get_any, get_bool_any, get_doc_any, get_i32_any, get_id_any, get_str_any, items_any,
};

#[derive(Debug, Clone, Default)]
pub struct Widget {
    pub widget_type: String,
    pub name: Option<String>,
    pub options: Document,
    pub events: Vec<Document>,
    pub children: Vec<Widget>,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub id: Option<String>,
    pub name: Option<String>,
    pub documentation: String,
    pub url: String,
    pub layout_id: Option<String>,
    pub title: String,
    pub popup_width: i32,
    pub popup_height: i32,
    pub popup_resizable: bool,
    pub excluded: bool,
    pub export_level: String,
    pub appearance_class: String,
    pub appearance_style: String,
    pub allowed_module_roles: Vec<String>,
    pub parameters: Vec<Document>,
    pub widgets: Vec<Widget>,
    pub data_source: Option<Document>,
    storage_type: String,
}

impl Page {
    pub fn from_bson(doc: &Document) -> Self {
        let storage_type = get_str_any(doc, &["$Type"]).unwrap_or_else(|| "Forms$Page".into());
        let appearance = get_doc_any(doc, &["Appearance"]).unwrap_or_default();

        let roots = widget_roots(doc);
        let mut widgets = Vec::new();
        parse_widgets(&roots, &mut widgets);
        let data_source = canonical_root_data_source(&roots);

        Page {
            id: get_id_any(doc, &["$ID"]),
            name: get_str_any(doc, &["Name", "name"]),
            documentation: get_str_any(doc, &["Documentation", "documentation"])
                .unwrap_or_default(),
            url: get_str_any(doc, &["Url", "URL", "url"]).unwrap_or_default(),
            title: extract_text(get_any(doc, &["Title", "title"])),
            layout_id: extract_layout(doc),
            popup_width: get_i32_any(doc, &["PopupWidth"]).unwrap_or(0),
            popup_height: get_i32_any(doc, &["PopupHeight"]).unwrap_or(0),
            popup_resizable: get_bool_any(doc, &["PopupResizable"]).unwrap_or(false),
            excluded: get_bool_any(doc, &["Excluded"]).unwrap_or(false),
            export_level: get_str_any(doc, &["ExportLevel"]).unwrap_or_else(|| "Hidden".into()),
            appearance_class: get_str_any(&appearance, &["Class"]).unwrap_or_default(),
            appearance_style: get_str_any(&appearance, &["Style"]).unwrap_or_default(),
            allowed_module_roles: string_items(
                doc,
                &["AllowedModuleRoles", "AllowedRoles", "allowedModuleRoles"],
            ),
            parameters: docs_any(doc, &["Parameters", "parameters"]),
            widgets,
            data_source,
            storage_type,
        }
    }

    pub fn to_bson(&self) -> Document {
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        mxrs_bson::doc! {
            "$ID": id,
            "$Type": self.storage_type.clone(),
            "Name": self.name.clone(),
            "Documentation": self.documentation.clone(),
            "Url": self.url.clone(),
            "Title": mxrs_bson::doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Texts$Text", "Translations": mxrs_bson::build_array(vec![], 1) },
            "Layout": self.layout_id.clone(),
            "MarkAsUsed": false,
            "Excluded": self.excluded,
            "AllowedModuleRoles": mxrs_bson::build_array(self.allowed_module_roles.iter().cloned().map(Bson::String).collect(), 1),
            "Parameters": mxrs_bson::build_array(self.parameters.iter().cloned().map(Bson::Document).collect(), 3),
            "PopupWidth": self.popup_width,
            "PopupHeight": self.popup_height,
            "PopupResizable": self.popup_resizable,
            "ExportLevel": self.export_level.clone(),
        }
    }
}

fn string_items(doc: &Document, keys: &[&str]) -> Vec<String> {
    items_any(doc, keys)
        .into_iter()
        .filter_map(|b| match b {
            Bson::String(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn extract_text(value: Option<&Bson>) -> String {
    match value {
        Some(Bson::String(s)) => s.clone(),
        Some(Bson::Document(d)) => {
            if get_str_any(d, &["$Type"]).as_deref() == Some("Forms$ClientTemplate") {
                return extract_text(d.get("Template"));
            }
            let translation = docs_any(d, &["Translations", "Items", "translations"])
                .into_iter()
                .next();
            translation
                .and_then(|t| get_str_any(&t, &["Translation", "Text", "text"]))
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn extract_layout(doc: &Document) -> Option<String> {
    if let Some(form) = get_doc_any(doc, &["FormCall"]).and_then(|f| get_str_any(&f, &["Form"])) {
        return Some(form);
    }
    get_id_any(doc, &["Layout", "LayoutId"])
}

// ── Structural traversal ─────────────────────────────────────────────────

fn widget_roots(doc: &Document) -> Vec<Document> {
    let mut roots = docs_any(doc, &["Widgets", "widgets"]);
    roots.extend(form_call_widgets(get_doc_any(doc, &["FormCall"]).as_ref()));
    roots
}

fn form_call_widgets(form_call: Option<&Document>) -> Vec<Document> {
    let Some(form_call) = form_call else {
        return vec![];
    };
    docs_any(form_call, &["Arguments"])
        .iter()
        .flat_map(child_widgets)
        .collect()
}

fn child_widgets(widget: &Document) -> Vec<Document> {
    let mut result = docs_any(widget, &["Widgets"]);
    result.extend(docs_any(widget, &["FooterWidgets"]));
    for tab in docs_any(widget, &["TabPages"]) {
        result.extend(docs_any(&tab, &["Widgets"]));
    }
    for row in docs_any(widget, &["Rows"]) {
        for column in docs_any(&row, &["Columns"]) {
            result.extend(docs_any(&column, &["Widgets"]));
        }
    }
    result
}

fn is_layout_container(widget: &Document) -> bool {
    !child_widgets(widget).is_empty() || widget.contains_key("Arguments")
}

fn canonical_root_data_source(roots: &[Document]) -> Option<Document> {
    if roots.len() != 1 {
        return None;
    }
    let root = &roots[0];
    let ty = get_str_any(root, &["$Type"]).unwrap_or_default();
    if ty != "Pages$DataView" && ty != "Forms$DataView" {
        return None;
    }
    let source = parse_data_view_source(get_doc_any(root, &["DataSource"]).as_ref());
    if get_str_any(&source, &["kind"]).as_deref() == Some("native") {
        None
    } else {
        Some(source)
    }
}

fn parse_widgets(items: &[Document], target: &mut Vec<Widget>) {
    for widget in items {
        let ty = get_str_any(widget, &["$Type"]).unwrap_or_default();

        if ty == "Pages$DataView" || ty == "Forms$DataView" {
            target.push(data_view_widget(widget));
            continue;
        }
        if ty == "CustomWidgets$CustomWidget" {
            target.push(pluggable_widget(widget));
            continue;
        }
        if ty == "Pages$DataGrid" || ty == "Forms$DataGrid" {
            target.push(data_grid_widget(widget));
            continue;
        }
        if ty == "Pages$TabControl" || ty == "Forms$TabControl" {
            target.push(tab_control_widget(widget));
            continue;
        }
        if ty == "Pages$Table" || ty == "Forms$Table" {
            target.push(table_widget(widget));
            continue;
        }
        if ty == "Forms$LayoutGrid" {
            target.push(layout_grid_widget(widget));
            continue;
        }
        if matches!(
            ty.as_str(),
            "Pages$SnippetCall" | "Forms$SnippetCall" | "Forms$SnippetCallWidget"
        ) {
            let snippet_ref = get_doc_any(widget, &["SnippetSettings"])
                .and_then(|s| get_str_any(&s, &["Snippet"]))
                .or_else(|| {
                    get_doc_any(widget, &["FormCall"]).and_then(|f| get_str_any(&f, &["Form"]))
                })
                .unwrap_or_default();
            target.push(Widget {
                widget_type: "snippet".into(),
                name: Some(get_str_any(widget, &["Name"]).unwrap_or_else(|| "snippet".into())),
                options: mxrs_bson::doc! { "snippet": snippet_ref },
                events: vec![],
                children: vec![],
            });
            continue;
        }
        if matches!(
            ty.as_str(),
            "Pages$Container" | "Forms$Container" | "Forms$DivContainer"
        ) {
            let mut children = Vec::new();
            parse_widgets(&docs_any(widget, &["Widgets"]), &mut children);
            target.push(Widget {
                widget_type: "container".into(),
                name: Some(get_str_any(widget, &["Name"]).unwrap_or_else(|| "container".into())),
                options: appearance_options(widget),
                events: container_events(widget),
                children,
            });
            continue;
        }
        if is_layout_container(widget) {
            parse_widgets(&child_widgets(widget), target);
            for argument in docs_any(widget, &["Arguments"]) {
                parse_widgets(&child_widgets(&argument), target);
            }
            continue;
        }
        let Some(widget_type) = widget_type(&ty) else {
            target.push(native_widget(widget));
            continue;
        };

        let events = [
            ("OnChangeAction", "on_change"),
            ("OnEnterAction", "on_enter"),
            ("OnLeaveAction", "on_leave"),
            ("ClickAction", "on_click"),
            ("Action", "on_click"),
        ]
        .iter()
        .filter_map(|(property, event)| {
            let action = parse_action(get_doc_any(widget, &[property]).as_ref())?;
            Some(with_event(action, event))
        })
        .collect();

        target.push(Widget {
            widget_type: widget_type.to_string(),
            name: get_str_any(widget, &["Name"]),
            options: widget_options(widget, widget_type),
            events,
            children: vec![],
        });
    }
}

/// Fallback for `CustomWidgets$CustomWidget` (Studio Pro's generic
/// pluggable-widget wrapper — see module doc). In Mendix 11 this backs not
/// just marketplace widgets but several of Studio Pro's own modern built-ins
/// (Data Grid 2, Gallery, ComboBox), so it's a meaningfully common case, not
/// just a marketplace edge case. Preserves the type and any nested widgets,
/// without decoding the pluggable `Object`/`Type` custom-property model.
fn pluggable_widget(widget: &Document) -> Widget {
    let mut children = Vec::new();
    parse_widgets(&child_widgets(widget), &mut children);
    Widget {
        widget_type: "pluggable".into(),
        name: Some(get_str_any(widget, &["Name"]).unwrap_or_else(|| "widget".into())),
        options: mxrs_bson::doc! { "native_type": get_str_any(widget, &["$Type"]).unwrap_or_default() },
        events: vec![],
        children,
    }
}

fn native_widget(widget: &Document) -> Widget {
    let mut deep = widget.clone();
    for key in ["$ID", "$Type", "Name"] {
        deep.remove(key);
    }
    let type_metadata = get_doc_any(widget, &["Type"]).unwrap_or_default();
    let widget_id = get_str_any(&type_metadata, &["WidgetId"]).unwrap_or_default();
    let platform = get_str_any(&type_metadata, &["SupportedPlatform"]).unwrap_or_default();

    let mut options = mxrs_bson::doc! {
        "native_type": get_str_any(widget, &["$Type"]).unwrap_or_default(),
        "deep_structure": deep,
    };
    if !widget_id.is_empty() {
        options.insert("widget_id", widget_id);
    }
    if !platform.is_empty() {
        options.insert("platform", platform);
    }

    Widget {
        widget_type: "native_widget".into(),
        name: Some(get_str_any(widget, &["Name"]).unwrap_or_else(|| "widget".into())),
        options,
        events: vec![],
        children: vec![],
    }
}

// ── Data source / data view ──────────────────────────────────────────────

fn parse_data_view_source(source: Option<&Document>) -> Document {
    let Some(source) = source else {
        return mxrs_bson::doc! { "kind": "context" };
    };
    let ty = get_str_any(source, &["$Type"]).unwrap_or_default();
    let force_full = get_bool_any(source, &["ForceFullObjects"]).unwrap_or(false);

    let mut out = match ty.as_str() {
        "Pages$NanoflowSource" | "Forms$NanoflowSource" => {
            let name = get_str_any(source, &["Nanoflow"])
                .or_else(|| {
                    get_doc_any(source, &["NanoflowSettings"])
                        .and_then(|s| get_str_any(&s, &["Nanoflow"]))
                })
                .unwrap_or_default();
            mxrs_bson::doc! { "kind": "nanoflow", "name": name }
        }
        "Pages$MicroflowSource" | "Forms$MicroflowSource" => {
            let settings =
                get_doc_any(source, &["MicroflowSettings"]).unwrap_or_else(|| source.clone());
            let name = get_str_any(&settings, &["Microflow"])
                .or_else(|| get_str_any(source, &["Microflow"]))
                .unwrap_or_default();
            mxrs_bson::doc! { "kind": "microflow", "name": name }
        }
        "Pages$ListenTargetSource" | "Forms$ListenTargetSource" => {
            mxrs_bson::doc! { "kind": "listen", "target": get_str_any(source, &["ListenTarget"]).unwrap_or_default() }
        }
        "Pages$DataViewSource" | "Forms$DataViewSource" => {
            let mut d = mxrs_bson::doc! { "kind": "context" };
            if let Some(p) = get_str_any(source, &["EntityPath"]) {
                d.insert("entity_path", p);
            }
            if let Some(r) = get_id_any(source, &["EntityRef"]) {
                d.insert("entity_ref", r);
            }
            d
        }
        _ => mxrs_bson::doc! { "kind": "native", "native_type": ty },
    };
    if force_full {
        out.insert("force_full_objects", true);
    }
    out
}

fn data_view_widget(widget: &Document) -> Widget {
    let mut body = Vec::new();
    parse_widgets(&docs_any(widget, &["Widgets"]), &mut body);
    let mut footer = Vec::new();
    parse_widgets(&docs_any(widget, &["FooterWidgets"]), &mut footer);
    let mut children = body;
    children.extend(footer);

    let mut options = appearance_options(widget);
    options.remove("visible");
    options.insert(
        "source",
        parse_data_view_source(get_doc_any(widget, &["DataSource"]).as_ref()),
    );
    let no_entity_message = extract_text(get_any(widget, &["NoEntityMessage"]));
    if !no_entity_message.is_empty() {
        options.insert("no_entity_message", no_entity_message);
    }
    options.insert(
        "show_footer",
        get_bool_any(widget, &["ShowFooter"]).unwrap_or(true),
    );

    Widget {
        widget_type: "data_view".into(),
        name: Some(get_str_any(widget, &["Name"]).unwrap_or_else(|| "dataView".into())),
        options,
        events: vec![],
        children,
    }
}

// ── Data grid (native / non-pluggable) ───────────────────────────────────

fn grid_entity(widget: &Document) -> Option<String> {
    get_doc_any(widget, &["DataSource"]).and_then(|d| {
        get_str_any(&d, &["Entity"])
            .or_else(|| get_doc_any(&d, &["EntityRef"]).and_then(|r| get_str_any(&r, &["Entity"])))
    })
}

fn grid_events(widget: &Document) -> Vec<Document> {
    [
        ("OnChangeAction", "on_change"),
        ("OnClickAction", "on_click"),
        ("Action", "on_click"),
    ]
    .iter()
    .filter_map(|(property, event)| {
        let action = parse_action(get_doc_any(widget, &[property]).as_ref())?;
        Some(with_event(action, event))
    })
    .collect()
}

fn parse_search_bar(sb: &Document) -> Option<Document> {
    let fields: Vec<Bson> = docs_any(sb, &["SearchFields"])
        .iter()
        .filter_map(|sf| {
            let attr_ref = get_doc_any(sf, &["AttributeRef"])
                .and_then(|a| get_str_any(&a, &["Attribute"]))
                .or_else(|| get_str_any(sf, &["Attribute", "AttributePath"]))?;
            let attribute = attr_ref.rsplit('/').next().unwrap_or(&attr_ref).to_string();
            let caption = extract_text(get_any(sf, &["Label", "Caption"]));
            Some(Bson::Document(
                mxrs_bson::doc! { "attribute": attribute, "caption": caption },
            ))
        })
        .collect();
    if fields.is_empty() {
        None
    } else {
        Some(mxrs_bson::doc! { "fields": fields })
    }
}

fn parse_toolbar(tb: &Document) -> Option<Document> {
    let buttons: Vec<Bson> = docs_any(tb, &["Buttons"])
        .iter()
        .filter_map(|btn| {
            let ty = get_str_any(btn, &["$Type"])?;
            let label = match ty.as_str() {
                "Pages$GridNewButton" | "Forms$GridNewButton" => "new",
                "Pages$GridDeleteButton" | "Forms$GridDeleteButton" => "delete",
                "Pages$GridSearchButton" | "Forms$GridSearchButton" => "search",
                "Pages$GridExportToExcelButton" | "Forms$GridExportToExcelButton" => "export",
                _ => return None,
            };
            Some(Bson::Document(mxrs_bson::doc! { "type": label, "caption": extract_text(get_any(btn, &["Caption"])) }))
        })
        .collect();
    if buttons.is_empty() {
        None
    } else {
        Some(mxrs_bson::doc! { "buttons": buttons })
    }
}

fn data_grid_widget(widget: &Document) -> Widget {
    let columns: Vec<Bson> = docs_any(widget, &["Columns"])
        .iter()
        .map(|column| {
            let mut c = mxrs_bson::doc! {};
            if let Some(n) = get_str_any(column, &["Name"]) {
                c.insert("name", n);
            }
            if let Some(a) = attribute_path(column) {
                c.insert("attribute", a);
            }
            let caption = extract_text(get_any(column, &["Caption"]));
            if !caption.is_empty() {
                c.insert("caption", caption);
            }
            Bson::Document(c)
        })
        .collect();

    let mut options = mxrs_bson::doc! { "columns": columns };
    if let Some(entity) = grid_entity(widget) {
        options.insert("entity", entity);
    }
    if let Some(sb) = get_doc_any(widget, &["SearchBar"])
        && let Some(parsed) = parse_search_bar(&sb)
    {
        options.insert("search_bar", parsed);
    }
    if let Some(tb) = get_doc_any(widget, &["ToolBar"])
        && let Some(parsed) = parse_toolbar(&tb)
    {
        options.insert("toolbar", parsed);
    }

    Widget {
        widget_type: "data_grid".into(),
        name: get_str_any(widget, &["Name"]),
        options,
        events: grid_events(widget),
        children: vec![],
    }
}

// ── Tab control / table / layout grid ────────────────────────────────────

fn tab_control_widget(widget: &Document) -> Widget {
    let tabs: Vec<Bson> = docs_any(widget, &["TabPages"])
        .iter()
        .map(|tab| {
            let mut widgets = Vec::new();
            parse_widgets(&docs_any(tab, &["Widgets"]), &mut widgets);
            let mut t = mxrs_bson::doc! {};
            if let Some(n) = get_str_any(tab, &["Name"]) {
                t.insert("name", n);
            }
            let caption = extract_text(get_any(tab, &["Caption"]));
            if !caption.is_empty() {
                t.insert("caption", caption);
            }
            t.insert("widget_count", widgets.len() as i32);
            Bson::Document(t)
        })
        .collect();
    Widget {
        widget_type: "tab_control".into(),
        name: get_str_any(widget, &["Name"]),
        options: mxrs_bson::doc! { "tabs": tabs },
        events: vec![],
        children: vec![],
    }
}

fn table_widget(widget: &Document) -> Widget {
    let column_widths: Vec<Bson> = docs_any(widget, &["ColumnWidths"])
        .iter()
        .map(|c| {
            Bson::Document(mxrs_bson::doc! { "width": get_i32_any(c, &["Value"]).unwrap_or(0) })
        })
        .collect();

    let mut cells_by_row: std::collections::BTreeMap<i32, Vec<Document>> = Default::default();
    for cell in docs_any(widget, &["Cells"]) {
        let row = get_i32_any(&cell, &["TopRowIndex"]).unwrap_or(0);
        cells_by_row.entry(row).or_default().push(cell);
    }

    let rows: Vec<Bson> = docs_any(widget, &["Rows"])
        .iter()
        .enumerate()
        .map(|(row_index, _row)| {
            let mut row_cells = cells_by_row
                .get(&(row_index as i32))
                .cloned()
                .unwrap_or_default();
            row_cells.sort_by_key(|c| get_i32_any(c, &["LeftColumnIndex"]).unwrap_or(0));
            let cells: Vec<Bson> = row_cells
                .iter()
                .map(|cell| {
                    let mut widgets = Vec::new();
                    parse_widgets(&docs_any(cell, &["Widgets"]), &mut widgets);
                    Bson::Document(mxrs_bson::doc! {
                        "column": get_i32_any(cell, &["LeftColumnIndex"]).unwrap_or(0),
                        "colspan": get_i32_any(cell, &["Width"]).unwrap_or(1).max(1),
                        "rowspan": get_i32_any(cell, &["Height"]).unwrap_or(1).max(1),
                        "header": get_bool_any(cell, &["IsHeader"]).unwrap_or(false),
                        "widget_count": widgets.len() as i32,
                    })
                })
                .collect();
            Bson::Document(mxrs_bson::doc! { "cells": cells })
        })
        .collect();

    Widget {
        widget_type: "table".into(),
        name: Some(get_str_any(widget, &["Name"]).unwrap_or_else(|| "table".into())),
        options: mxrs_bson::doc! {
            "columns": column_widths,
            "rows": rows,
            "width_unit": get_str_any(widget, &["WidthUnit"]).unwrap_or_else(|| "Weight".into()),
        },
        events: vec![],
        children: vec![],
    }
}

fn layout_grid_weight(value: i32) -> Bson {
    match value {
        -1 => Bson::String("grow".into()),
        -2 => Bson::String("auto".into()),
        n => Bson::Int32(n),
    }
}

fn layout_grid_widget(widget: &Document) -> Widget {
    let rows: Vec<Bson> = docs_any(widget, &["Rows"])
        .iter()
        .map(|row| {
            let columns: Vec<Bson> = docs_any(row, &["Columns"])
                .iter()
                .map(|column| {
                    let mut children = Vec::new();
                    parse_widgets(&docs_any(column, &["Widgets"]), &mut children);
                    Bson::Document(mxrs_bson::doc! {
                        "desktop": layout_grid_weight(get_i32_any(column, &["Weight"]).unwrap_or(-1)),
                        "tablet": layout_grid_weight(get_i32_any(column, &["TabletWeight"]).unwrap_or(-1)),
                        "phone": layout_grid_weight(get_i32_any(column, &["PhoneWeight"]).unwrap_or(-1)),
                        "widget_count": children.len() as i32,
                    })
                })
                .collect();
            Bson::Document(mxrs_bson::doc! { "columns": columns })
        })
        .collect();

    Widget {
        widget_type: "layout_grid".into(),
        name: Some(get_str_any(widget, &["Name"]).unwrap_or_else(|| "layoutGrid".into())),
        options: mxrs_bson::doc! { "rows": rows },
        events: vec![],
        children: vec![],
    }
}

// ── Generic simple-widget option builders ────────────────────────────────

fn appearance_options(widget: &Document) -> Document {
    let appearance = get_doc_any(widget, &["Appearance"]);
    let class_name = appearance
        .as_ref()
        .and_then(|a| get_str_any(a, &["Class"]))
        .or_else(|| get_str_any(widget, &["Class"]))
        .unwrap_or_default();
    let style = appearance
        .as_ref()
        .and_then(|a| get_str_any(a, &["Style"]))
        .or_else(|| get_str_any(widget, &["Style"]))
        .unwrap_or_default();
    let dynamic = appearance
        .as_ref()
        .and_then(|a| get_str_any(a, &["DynamicClasses"]))
        .unwrap_or_default();
    let visible = get_doc_any(widget, &["ConditionalVisibilitySettings"])
        .and_then(|c| get_str_any(&c, &["Expression"]))
        .unwrap_or_default();

    let mut out = Document::new();
    if !class_name.is_empty() {
        out.insert("class", class_name);
    }
    if !style.is_empty() {
        out.insert("style", style);
    }
    if !dynamic.is_empty() {
        out.insert("dynamic_class", dynamic);
    }
    if !visible.is_empty() {
        out.insert("visible", visible);
    }
    out
}

fn container_events(widget: &Document) -> Vec<Document> {
    let action =
        get_doc_any(widget, &["OnClickAction"]).or_else(|| get_doc_any(widget, &["Action"]));
    match parse_action(action.as_ref()) {
        Some(a) => vec![with_event(a, "on_click")],
        None => vec![],
    }
}

fn template_parameters(widget: &Document) -> Vec<String> {
    let Some(template) = get_doc_any(widget, &["Content", "CaptionTemplate", "LabelTemplate"])
    else {
        return vec![];
    };
    docs_any(&template, &["Parameters"])
        .iter()
        .filter_map(|parameter| {
            let expression = get_str_any(parameter, &["Expression"]).unwrap_or_default();
            if !expression.is_empty() {
                return Some(expression);
            }
            let attribute = get_doc_any(parameter, &["AttributeRef"])
                .and_then(|a| get_str_any(&a, &["Attribute"]))
                .and_then(|a| a.rsplit('.').next().map(str::to_string));
            attribute
                .filter(|a| !a.is_empty())
                .map(|a| format!("$currentObject/{a}"))
        })
        .collect()
}

fn attribute_path(widget: &Document) -> Option<String> {
    get_str_any(widget, &["AttributePath"]).or_else(|| {
        get_doc_any(widget, &["AttributeRef"]).and_then(|a| get_str_any(&a, &["Attribute"]))
    })
}

fn widget_caption(widget: &Document, widget_type: &str) -> String {
    if widget_type == "text" {
        return extract_text(get_any(widget, &["Content", "Text", "LabelText"]));
    }
    let value = get_any(widget, &["Caption"])
        .or_else(|| get_any(widget, &["LabelText"]))
        .or_else(|| get_any(widget, &["LabelTemplate"]))
        .cloned()
        .or_else(|| {
            get_doc_any(widget, &["CaptionTemplate"]).and_then(|c| c.get("Template").cloned())
        });
    extract_text(value.as_ref())
}

fn static_image_options(widget: &Document) -> Document {
    let mut out = appearance_options(widget);
    out.insert("image", get_str_any(widget, &["Image"]).unwrap_or_default());
    out.insert(
        "alternative_text",
        extract_text(get_any(widget, &["AlternativeText"])),
    );
    out.insert("width", get_i32_any(widget, &["Width"]).unwrap_or(0));
    out.insert("height", get_i32_any(widget, &["Height"]).unwrap_or(0));
    out.insert(
        "width_unit",
        get_str_any(widget, &["WidthUnit"])
            .unwrap_or_else(|| "Pixels".into())
            .to_lowercase(),
    );
    out.insert(
        "height_unit",
        get_str_any(widget, &["HeightUnit"])
            .unwrap_or_else(|| "Pixels".into())
            .to_lowercase(),
    );
    out.insert(
        "responsive",
        get_bool_any(widget, &["Responsive"]).unwrap_or(true),
    );
    out
}

fn file_manager_options(widget: &Document) -> Document {
    let mut out = appearance_options(widget);
    out.insert(
        "allowed_extensions",
        get_str_any(widget, &["AllowedExtensions"]).unwrap_or_default(),
    );
    out.insert(
        "editable",
        get_str_any(widget, &["Editable"]).unwrap_or_else(|| "Always".into()),
    );
    out.insert(
        "max_file_size",
        get_i32_any(widget, &["MaxFileSize"]).unwrap_or(5),
    );
    out.insert(
        "show_file_in_browser",
        get_bool_any(widget, &["ShowFileInBrowser"]).unwrap_or(false),
    );
    out.insert(
        "mode",
        get_str_any(widget, &["Type"]).unwrap_or_else(|| "Both".into()),
    );
    out.insert("tab_index", get_i32_any(widget, &["TabIndex"]).unwrap_or(0));
    out
}

fn reference_set_selector_options(widget: &Document) -> Document {
    let mut out = appearance_options(widget);
    out.insert(
        "selection",
        get_str_any(widget, &["SelectionMode"]).unwrap_or_else(|| "Multi".into()),
    );
    out.insert(
        "number_of_rows",
        get_i32_any(widget, &["NumberOfRows"]).unwrap_or(20),
    );
    out.insert(
        "selectable_xpath",
        get_str_any(widget, &["SelectableXPathConstraint"]).unwrap_or_default(),
    );
    out.insert(
        "control_bar",
        get_bool_any(widget, &["IsControlBarVisible"]).unwrap_or(false),
    );
    out.insert(
        "select_first",
        get_bool_any(widget, &["SelectFirst"]).unwrap_or(false),
    );
    out.insert(
        "show_empty_rows",
        get_bool_any(widget, &["ShowEmptyRows"]).unwrap_or(false),
    );
    out.insert(
        "paging",
        get_str_any(widget, &["ShowPagingBar"]).unwrap_or_else(|| "YesWithTotalCount".into()),
    );
    out.insert("tab_index", get_i32_any(widget, &["TabIndex"]).unwrap_or(0));
    out.insert(
        "width_unit",
        get_str_any(widget, &["WidthUnit"]).unwrap_or_else(|| "Weight".into()),
    );
    out
}

fn navigation_list_options(widget: &Document) -> Document {
    let mut out = appearance_options(widget);
    out.insert("tab_index", get_i32_any(widget, &["TabIndex"]).unwrap_or(0));
    out
}

fn scroll_container_options(widget: &Document) -> Document {
    let mut out = appearance_options(widget);
    out.insert(
        "alignment",
        get_str_any(widget, &["Alignment"]).unwrap_or_else(|| "Center".into()),
    );
    out.insert(
        "layout_mode",
        get_str_any(widget, &["LayoutMode"]).unwrap_or_else(|| "Headline".into()),
    );
    out.insert(
        "hide_scrollbars",
        get_bool_any(widget, &["NativeHideScrollbars"]).unwrap_or(false),
    );
    out.insert(
        "scroll_behavior",
        get_str_any(widget, &["ScrollBehavior"]).unwrap_or_else(|| "PerRegion".into()),
    );
    out.insert("tab_index", get_i32_any(widget, &["TabIndex"]).unwrap_or(0));
    out.insert("width", get_i32_any(widget, &["Width"]).unwrap_or(0));
    out.insert(
        "width_mode",
        get_str_any(widget, &["WidthMode"]).unwrap_or_else(|| "Auto".into()),
    );
    out
}

fn image_viewer_options(widget: &Document) -> Document {
    let source = get_doc_any(widget, &["DataSource"]).unwrap_or_default();
    let mut out = appearance_options(widget);
    let entity = get_doc_any(&source, &["EntityRef"])
        .and_then(|r| get_str_any(&r, &["Entity"]))
        .or_else(|| get_str_any(&source, &["EntityPath"]));
    if let Some(entity) = entity {
        out.insert("entity", entity);
    }
    out.insert(
        "alternative_text",
        extract_text(get_any(widget, &["AlternativeText"])),
    );
    out.insert(
        "default_image",
        get_str_any(widget, &["DefaultImage"]).unwrap_or_default(),
    );
    out.insert(
        "force_full_objects",
        get_bool_any(&source, &["ForceFullObjects"]).unwrap_or(false),
    );
    out.insert("width", get_i32_any(widget, &["Width"]).unwrap_or(100));
    out.insert("height", get_i32_any(widget, &["Height"]).unwrap_or(100));
    out.insert(
        "width_unit",
        get_str_any(widget, &["WidthUnit"]).unwrap_or_else(|| "Auto".into()),
    );
    out.insert(
        "height_unit",
        get_str_any(widget, &["HeightUnit"]).unwrap_or_else(|| "Auto".into()),
    );
    out.insert(
        "responsive",
        get_bool_any(widget, &["Responsive"]).unwrap_or(true),
    );
    out.insert(
        "show_as_thumbnail",
        get_bool_any(widget, &["ShowAsThumbnail"]).unwrap_or(false),
    );
    out.insert(
        "on_click_enlarge",
        get_bool_any(widget, &["OnClickEnlarge"]).unwrap_or(false),
    );
    out.insert("tab_index", get_i32_any(widget, &["TabIndex"]).unwrap_or(0));
    out
}

fn image_uploader_options(widget: &Document) -> Document {
    let size = get_str_any(widget, &["ThumbnailSize"]).unwrap_or_else(|| "100;75".into());
    let mut parts = size.splitn(2, ';');
    let width: i32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let height: i32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);

    let mut out = appearance_options(widget);
    out.insert(
        "allowed_extensions",
        get_str_any(widget, &["AllowedExtensions"]).unwrap_or_default(),
    );
    out.insert("caption", extract_text(get_any(widget, &["LabelTemplate"])));
    out.insert(
        "editable",
        get_str_any(widget, &["Editable"]).unwrap_or_else(|| "Always".into()),
    );
    out.insert(
        "max_file_size",
        get_i32_any(widget, &["MaxFileSize"]).unwrap_or(5),
    );
    out.insert("thumbnail_width", if width > 0 { width } else { 100 });
    out.insert("thumbnail_height", if height > 0 { height } else { 75 });
    out.insert("tab_index", get_i32_any(widget, &["TabIndex"]).unwrap_or(0));
    out
}

fn menu_widget_options(widget: &Document) -> Document {
    let mut out = appearance_options(widget);
    out.insert(
        "menu",
        get_doc_any(widget, &["MenuSource"])
            .and_then(|m| get_str_any(&m, &["Menu"]))
            .unwrap_or_default(),
    );
    out.insert("tab_index", get_i32_any(widget, &["TabIndex"]).unwrap_or(0));
    out
}

fn widget_type(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "Pages$ActionButton" | "Forms$ActionButton" | "Forms$GridActionButton" => "button",
        "Pages$TextBox" | "Forms$TextBox" => "text_box",
        "Pages$TextArea" | "Forms$TextArea" => "text_area",
        "Pages$CheckBox" | "Forms$CheckBox" => "check_box",
        "Pages$DatePicker" | "Forms$DatePicker" => "date_picker",
        "Pages$ReferenceSelector"
        | "Forms$ReferenceSelector"
        | "Pages$InputReferenceSetSelector"
        | "Forms$InputReferenceSetSelector" => "reference_selector",
        "Pages$DynamicText" | "Forms$DynamicText" | "Pages$Label" | "Forms$Label" => "text",
        "Pages$DropDownWidget" | "Forms$DropDownWidget" | "Forms$DropDown" => "drop_down",
        "Forms$RadioButtonGroup" => "radio_button_group",
        "Forms$StaticImageViewer" => "static_image",
        "Forms$Title" => "page_title",
        "Forms$FileManager" => "file_manager",
        "Forms$ReferenceSetSelector" => "reference_set_selector",
        "Forms$NavigationList" => "navigation_list",
        "Forms$ScrollContainer" => "scroll_container",
        "Forms$ImageViewer" => "image_viewer",
        "Forms$ImageUploader" => "image_uploader",
        "Forms$MenuBar" => "menu_bar",
        "Forms$NavigationTree" => "navigation_tree",
        _ => return None,
    })
}

fn widget_options(widget: &Document, widget_type: &str) -> Document {
    match widget_type {
        "page_title" => appearance_options(widget),
        "static_image" => static_image_options(widget),
        "file_manager" => file_manager_options(widget),
        "reference_set_selector" => reference_set_selector_options(widget),
        "navigation_list" => navigation_list_options(widget),
        "scroll_container" => scroll_container_options(widget),
        "image_viewer" => image_viewer_options(widget),
        "image_uploader" => image_uploader_options(widget),
        "menu_bar" | "navigation_tree" => menu_widget_options(widget),
        _ => {
            let mut options = appearance_options(widget);
            if let Some(a) = attribute_path(widget) {
                options.insert("attribute", a);
            }
            let caption = widget_caption(widget, widget_type);
            if !caption.is_empty() {
                options.insert("caption", caption);
            }
            let parameters = template_parameters(widget);
            if !parameters.is_empty() {
                options.insert(
                    "parameters",
                    parameters.into_iter().map(Bson::String).collect::<Vec<_>>(),
                );
            }
            if widget_type == "text_area"
                && let Some(n) = get_i32_any(widget, &["NumberOfLines"])
            {
                options.insert("lines", n);
            }
            if widget_type == "radio_button_group" {
                options.insert(
                    "horizontal",
                    get_bool_any(widget, &["RenderHorizontal"]).unwrap_or(false),
                );
            }
            options
        }
    }
}

// ── Actions ───────────────────────────────────────────────────────────────

fn local_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn action_arguments(settings: &Document) -> Document {
    let mut out = Document::new();
    for mapping in docs_any(settings, &["ParameterMappings"]) {
        let Some(param) = get_str_any(&mapping, &["Parameter"]) else {
            continue;
        };
        let value = get_str_any(&mapping, &["Expression", "Argument"]).unwrap_or_default();
        out.insert(local_name(&param), value);
    }
    out
}

fn parse_action(action: Option<&Document>) -> Option<Document> {
    let action = action?;
    let ty = get_str_any(action, &["$Type"])?;
    match ty.as_str() {
        "Pages$CallNanoflowClientAction" | "Forms$CallNanoflowClientAction" => {
            let settings =
                get_doc_any(action, &["NanoflowSettings"]).unwrap_or_else(|| action.clone());
            let handler = get_str_any(action, &["Nanoflow"])
                .or_else(|| get_str_any(&settings, &["Nanoflow"]))
                .map(|n| local_name(&n).to_string());
            let handler = handler.filter(|h| !h.is_empty())?;
            Some(
                mxrs_bson::doc! { "kind": "nanoflow", "handler": handler, "arguments": action_arguments(&settings) },
            )
        }
        "Pages$MicroflowClientAction" | "Forms$MicroflowAction" | "Forms$MicroflowClientAction" => {
            let settings =
                get_doc_any(action, &["MicroflowSettings"]).unwrap_or_else(|| action.clone());
            let handler = get_str_any(action, &["Microflow"])
                .or_else(|| get_str_any(&settings, &["Microflow"]))
                .map(|n| local_name(&n).to_string());
            let handler = handler.filter(|h| !h.is_empty())?;
            Some(
                mxrs_bson::doc! { "kind": "microflow", "handler": handler, "arguments": action_arguments(&settings) },
            )
        }
        "Pages$FormAction" | "Forms$FormAction" => {
            let settings = get_doc_any(action, &["FormSettings"]).unwrap_or_else(|| action.clone());
            let handler = get_str_any(&settings, &["Form"]).filter(|h| !h.is_empty())?;
            Some(
                mxrs_bson::doc! { "kind": "page", "handler": handler, "arguments": action_arguments(&settings) },
            )
        }
        "Pages$SaveChangesClientAction" | "Forms$SaveChangesClientAction" => {
            Some(mxrs_bson::doc! { "kind": "action", "handler": "save_changes" })
        }
        "Pages$CancelChangesClientAction" | "Forms$CancelChangesClientAction" => {
            Some(mxrs_bson::doc! { "kind": "action", "handler": "cancel_changes" })
        }
        "Pages$DeleteClientAction" | "Forms$DeleteClientAction" => {
            Some(mxrs_bson::doc! { "kind": "action", "handler": "delete" })
        }
        "Pages$ClosePageClientAction" | "Forms$ClosePageClientAction" => {
            Some(mxrs_bson::doc! { "kind": "action", "handler": "close_page" })
        }
        _ => None,
    }
}

fn with_event(mut action: Document, event: &str) -> Document {
    action.insert("event", event);
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    #[test]
    fn decodes_page_level_fields() {
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Forms$Page",
            "Name": "OrderOverview",
            "Url": "orderoverview",
        };
        let page = Page::from_bson(&d);
        assert_eq!(page.name.as_deref(), Some("OrderOverview"));
        assert_eq!(page.url, "orderoverview");
    }

    #[test]
    fn decodes_a_native_button_and_text_widget() {
        let button = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Forms$ActionButton", "Name": "new_order_button", "Caption": "New order" };
        let text = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Forms$DynamicText", "Name": "hint", "Content": "Manage your orders" };
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Forms$Page",
            "Name": "OrderOverview",
            "Widgets": mxrs_bson::build_array(vec![Bson::Document(button), Bson::Document(text)], 3),
        };
        let page = Page::from_bson(&d);
        assert_eq!(page.widgets.len(), 2);
        assert_eq!(page.widgets[0].widget_type, "button");
        assert_eq!(page.widgets[1].widget_type, "text");
    }

    #[test]
    fn single_data_view_root_becomes_the_page_data_source() {
        let data_view = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Forms$DataView",
            "Name": "dataView1",
            "DataSource": { "$Type": "Forms$MicroflowSource", "Microflow": "Sales.ACT_GetOrder" },
        };
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Forms$Page",
            "Widgets": mxrs_bson::build_array(vec![Bson::Document(data_view)], 3),
        };
        let page = Page::from_bson(&d);
        let source = page.data_source.unwrap();
        assert_eq!(source.get_str("kind").unwrap(), "microflow");
        assert_eq!(source.get_str("name").unwrap(), "Sales.ACT_GetOrder");
    }

    #[test]
    fn layout_container_flattens_children_into_the_parent_list() {
        let inner = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Forms$DynamicText", "Name": "hint", "Content": "hi" };
        let container = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Forms$ScrollContainer",
            "Name": "wrapper",
            "Widgets": mxrs_bson::build_array(vec![Bson::Document(inner)], 3),
        };
        let d = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Forms$Page",
            "Widgets": mxrs_bson::build_array(vec![Bson::Document(container)], 3),
        };
        let page = Page::from_bson(&d);
        assert_eq!(page.widgets.len(), 1);
        assert_eq!(page.widgets[0].widget_type, "text");
    }
}
