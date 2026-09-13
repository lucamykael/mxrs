//! Compiles `mxrs_ir::page::PageDecl`/`WidgetDecl` (the small,
//! storage-independent page IR — see that module's doc comment for the
//! exact widget vocabulary and every deferred gap) into a `Forms$Page`
//! document, via `mxrs-forms`'s already-lossless, schema-driven
//! `Node`/`MprCodec` rather than hand-rolled BSON (reuses the mature Forms
//! metamodel codec instead of re-deriving it — the same reasoning
//! `domain.rs`/`flow_compiler.rs` apply to `mxrs-model`/`mxrs-bson`).
//!
//! Mirrors `flow_compiler`'s role: an author-level IR is compiled into a
//! *low-level* structure the storage layer understands, the same
//! relationship `flow_compiler::build_microflow_graph` has to `Activity`.
//! Nested element `$ID`s are freshly randomized on every compile
//! (`mxrs-forms::MprCodec::encode` always mints one via
//! `uuid::Uuid::new_v4()` — it has no way to accept a caller-supplied id),
//! exactly like `flow_compiler`'s activity graph: only the page's own
//! top-level identity needs to survive across rebuilds, because the whole
//! document is fully re-derived from the declared `PageDecl` on every
//! write, never diffed widget-by-widget. The caller
//! (`documents::synchronize_pages_with_identity`) overwrites the returned
//! document's `$ID` with the page's stable identity after this returns —
//! mirroring how `documents::enumeration_document` and
//! `module::write_module`'s microflow loop assign their own top-level `$ID`
//! after building the rest of the document.
//!
//! **Widgets require a `layout`, loudly**: a `Forms$Page` document has no
//! `Widgets` property of its own — every real Mendix page attaches its
//! widget tree to a `LayoutCallArgument` inside `LayoutCall.arguments`,
//! keyed to one of the referenced Layout's own parameter names (confirmed
//! against the embedded `forms-11.12.1.json` schema, and matches how
//! `mxrs-model::page::form_call_widgets` reads widgets back out of exactly
//! this shape). A page that declares widgets but no `layout` is therefore
//! a loud [`crate::WriterError::PageWidgetsRequireLayout`], never a
//! silently empty page — rule #1's "loud, not silent" gap policy applied
//! to a structural impossibility rather than an unimplemented feature.
//!
//! **Verified against a real Studio-Pro-produced document, not just this
//! crate's own round trip**: `mxrs-forms/tests/fixtures/native_page.bson`
//! (decoded in that crate's own test suite) is a real exported
//! `Forms$Page` that itself omits `Widgets`/`LayoutCall`/`Appearance`
//! entirely — confirming `MprCodec` doesn't require every schema-"required"
//! property to be present to produce a well-formed document; this compiler
//! likewise only ever sets the properties this IR captures. What's *not*
//! verified (no way to drive Studio Pro from this environment): that Studio
//! Pro itself opens a page built this way without complaint. Only that
//! `MprCodec` accepts what this compiler emits and decodes it back
//! losslessly (see `page_compiler` tests in `mxrs-writer`'s own test
//! suite).

use std::rc::Rc;

use mxrs_bson::Document;
use mxrs_forms::catalog::{Catalog, ReferenceKind};
use mxrs_forms::node::{Node, Value};
use mxrs_forms::values::{AttributeReference, Reference, Text};
use mxrs_ir::page::{
    ButtonAction, DataSourceDecl, LayoutGridColumnDecl, LayoutGridRowDecl, PageDecl, WidgetDecl,
};

use crate::error::{Result, WriterError};

/// Compiles a page into a `Forms$Page` document. The returned document's
/// `$ID` is a fresh random UUID (from `MprCodec::encode`) — callers that
/// need a stable identity must overwrite it, see this module's doc comment.
pub fn compile_page(catalog: &Rc<Catalog>, decl: &PageDecl) -> Result<Document> {
    if decl.layout.is_none() && !decl.widgets.is_empty() {
        return Err(WriterError::PageWidgetsRequireLayout(decl.name.clone()));
    }

    let mut page = Node::new("Page", catalog.clone())?;
    page.set("name", Value::String(decl.name.clone()))?;
    let title = decl.title.clone().unwrap_or_else(|| decl.name.clone());
    page.set("title", Value::Text(Text::from_plain(title)))?;
    if !decl.documentation.is_empty() {
        page.set("documentation", Value::String(decl.documentation.clone()))?;
    }
    if !decl.url.is_empty() {
        page.set("url", Value::String(decl.url.clone()))?;
    }

    if let Some(layout) = &decl.layout {
        let mut layout_call = Node::new("LayoutCall", catalog.clone())?;
        layout_call.set(
            "layout",
            Value::Reference(Reference {
                target: layout.qualified_name.clone(),
                kind: ReferenceKind::ByName,
            }),
        )?;
        let mut argument = Node::new("LayoutCallArgument", catalog.clone())?;
        argument.set(
            "parameter",
            Value::Reference(Reference {
                target: layout.parameter.clone(),
                kind: ReferenceKind::ByName,
            }),
        )?;
        let mut counter = 0u32;
        let widgets = decl
            .widgets
            .iter()
            .map(|widget| compile_widget(catalog, widget, &mut counter))
            .collect::<Result<Vec<_>>>()?;
        argument.set("widgets", Value::List(widgets))?;
        layout_call.set("arguments", Value::List(vec![Value::Node(argument)]))?;
        page.set("layoutCall", Value::Node(layout_call))?;
    }

    let codec = mxrs_forms::MprCodec::new(catalog.clone());
    Ok(codec.encode(&page)?)
}

fn compile_widget(catalog: &Rc<Catalog>, widget: &WidgetDecl, counter: &mut u32) -> Result<Value> {
    match widget {
        WidgetDecl::Container {
            name,
            class,
            style,
            children,
        } => {
            let mut node = Node::new("DivContainer", catalog.clone())?;
            node.set(
                "name",
                Value::String(widget_name(name, "container", counter)),
            )?;
            if let Some(appearance) = appearance_node(catalog, class.as_deref(), style.as_deref())?
            {
                node.set("appearance", Value::Node(appearance))?;
            }
            let compiled = children
                .iter()
                .map(|child| compile_widget(catalog, child, counter))
                .collect::<Result<Vec<_>>>()?;
            node.set("widgets", Value::List(compiled))?;
            Ok(Value::Node(node))
        }
        WidgetDecl::LayoutGrid { name, rows } => {
            let mut node = Node::new("LayoutGrid", catalog.clone())?;
            node.set(
                "name",
                Value::String(widget_name(name, "layoutGrid", counter)),
            )?;
            let compiled_rows = rows
                .iter()
                .map(|row| compile_layout_grid_row(catalog, row, counter))
                .collect::<Result<Vec<_>>>()?;
            node.set("rows", Value::List(compiled_rows))?;
            Ok(Value::Node(node))
        }
        WidgetDecl::Text {
            name,
            caption,
            class,
        } => {
            let mut node = Node::new("DynamicText", catalog.clone())?;
            node.set("name", Value::String(widget_name(name, "text", counter)))?;
            node.set("content", Value::Node(client_template(catalog, caption)?))?;
            if let Some(appearance) = appearance_node(catalog, class.as_deref(), None)? {
                node.set("appearance", Value::Node(appearance))?;
            }
            Ok(Value::Node(node))
        }
        WidgetDecl::Button {
            name,
            caption,
            class,
            action,
        } => {
            let mut node = Node::new("ActionButton", catalog.clone())?;
            node.set("name", Value::String(widget_name(name, "button", counter)))?;
            node.set("caption", Value::Node(client_template(catalog, caption)?))?;
            node.set("action", Value::Node(button_action_node(catalog, action)?))?;
            if let Some(appearance) = appearance_node(catalog, class.as_deref(), None)? {
                node.set("appearance", Value::Node(appearance))?;
            }
            Ok(Value::Node(node))
        }
        WidgetDecl::DataView {
            name,
            source,
            children,
        } => {
            let mut node = Node::new("DataView", catalog.clone())?;
            node.set(
                "name",
                Value::String(widget_name(name, "dataView", counter)),
            )?;
            node.set(
                "dataSource",
                Value::Node(data_source_node(catalog, source)?),
            )?;
            let compiled = children
                .iter()
                .map(|child| compile_widget(catalog, child, counter))
                .collect::<Result<Vec<_>>>()?;
            node.set("widgets", Value::List(compiled))?;
            Ok(Value::Node(node))
        }
        WidgetDecl::TextBox {
            name,
            attribute,
            class,
        } => attribute_widget_node(
            catalog, "TextBox", "textBox", name, attribute, class, counter,
        ),
        WidgetDecl::CheckBox {
            name,
            attribute,
            class,
        } => attribute_widget_node(
            catalog, "CheckBox", "checkBox", name, attribute, class, counter,
        ),
        WidgetDecl::DatePicker {
            name,
            attribute,
            class,
        } => attribute_widget_node(
            catalog,
            "DatePicker",
            "datePicker",
            name,
            attribute,
            class,
            counter,
        ),
        WidgetDecl::DropDown {
            name,
            attribute,
            class,
        } => attribute_widget_node(
            catalog, "DropDown", "dropDown", name, attribute, class, counter,
        ),
    }
}

/// Shared shape behind `TextBox`/`CheckBox`/`DatePicker`/`DropDown`: all
/// four extend Mendix's `MemberWidget` (directly, or via `AttributeWidget`)
/// and expose exactly one property this compiler needs beyond
/// name/appearance — `attributeRef` — so one function builds all of them,
/// keyed by schema type name. Not folded into `compile_widget`'s own
/// `match` to keep that `match` one arm per `WidgetDecl` variant, matching
/// this file's existing style for `Container`/`Text`/`Button`.
fn attribute_widget_node(
    catalog: &Rc<Catalog>,
    schema_type: &str,
    default_prefix: &str,
    name: &Option<String>,
    attribute: &str,
    class: &Option<String>,
    counter: &mut u32,
) -> Result<Value> {
    let mut node = Node::new(schema_type, catalog.clone())?;
    node.set(
        "name",
        Value::String(widget_name(name, default_prefix, counter)),
    )?;
    node.set(
        "attributeRef",
        Value::AttributeReference(AttributeReference {
            attribute: attribute.to_string(),
            entity_reference: None,
        }),
    )?;
    if let Some(appearance) = appearance_node(catalog, class.as_deref(), None)? {
        node.set("appearance", Value::Node(appearance))?;
    }
    Ok(Value::Node(node))
}

/// Builds the `MicroflowSource`/`NanoflowSource` node behind a
/// [`WidgetDecl::DataView`] — see `DataSourceDecl`'s doc comment for why
/// only these two variants exist yet.
fn data_source_node(catalog: &Rc<Catalog>, source: &DataSourceDecl) -> Result<Node> {
    match source {
        DataSourceDecl::Microflow(qualified_name) => {
            let mut settings = Node::new("MicroflowSettings", catalog.clone())?;
            settings.set(
                "microflow",
                Value::Reference(Reference {
                    target: qualified_name.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            let mut node = Node::new("MicroflowSource", catalog.clone())?;
            node.set("microflowSettings", Value::Node(settings))?;
            Ok(node)
        }
        DataSourceDecl::Nanoflow(qualified_name) => {
            let mut node = Node::new("NanoflowSource", catalog.clone())?;
            node.set(
                "nanoflow",
                Value::Reference(Reference {
                    target: qualified_name.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            Ok(node)
        }
    }
}

fn compile_layout_grid_row(
    catalog: &Rc<Catalog>,
    row: &LayoutGridRowDecl,
    counter: &mut u32,
) -> Result<Value> {
    let mut node = Node::new("LayoutGridRow", catalog.clone())?;
    let columns = row
        .columns
        .iter()
        .map(|column| compile_layout_grid_column(catalog, column, counter))
        .collect::<Result<Vec<_>>>()?;
    node.set("columns", Value::List(columns))?;
    Ok(Value::Node(node))
}

fn compile_layout_grid_column(
    catalog: &Rc<Catalog>,
    column: &LayoutGridColumnDecl,
    counter: &mut u32,
) -> Result<Value> {
    let mut node = Node::new("LayoutGridColumn", catalog.clone())?;
    node.set("weight", Value::Integer(i64::from(column.weight)))?;
    let children = column
        .children
        .iter()
        .map(|child| compile_widget(catalog, child, counter))
        .collect::<Result<Vec<_>>>()?;
    node.set("widgets", Value::List(children))?;
    Ok(Value::Node(node))
}

fn client_template(catalog: &Rc<Catalog>, text: &str) -> Result<Node> {
    let mut node = Node::new("ClientTemplate", catalog.clone())?;
    node.set("template", Value::Text(Text::from_plain(text)))?;
    Ok(node)
}

fn button_action_node(catalog: &Rc<Catalog>, action: &ButtonAction) -> Result<Node> {
    match action {
        ButtonAction::None => Ok(Node::new("NoClientAction", catalog.clone())?),
        ButtonAction::ClosePage => Ok(Node::new("ClosePageClientAction", catalog.clone())?),
        ButtonAction::CallMicroflow(qualified_name) => {
            let mut settings = Node::new("MicroflowSettings", catalog.clone())?;
            settings.set(
                "microflow",
                Value::Reference(Reference {
                    target: qualified_name.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            let mut node = Node::new("MicroflowClientAction", catalog.clone())?;
            node.set("microflowSettings", Value::Node(settings))?;
            Ok(node)
        }
        ButtonAction::CallNanoflow(qualified_name) => {
            let mut node = Node::new("CallNanoflowClientAction", catalog.clone())?;
            node.set(
                "nanoflow",
                Value::Reference(Reference {
                    target: qualified_name.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            Ok(node)
        }
    }
}

fn appearance_node(
    catalog: &Rc<Catalog>,
    class: Option<&str>,
    style: Option<&str>,
) -> Result<Option<Node>> {
    if class.is_none() && style.is_none() {
        return Ok(None);
    }
    let mut node = Node::new("Appearance", catalog.clone())?;
    if let Some(class) = class {
        node.set("class", Value::String(class.to_string()))?;
    }
    if let Some(style) = style {
        node.set("style", Value::String(style.to_string()))?;
    }
    Ok(Some(node))
}

/// Falls back to a sequential default (`"container1"`, `"text2"`, ...) so
/// every widget still gets a unique-within-the-page name even when the
/// author didn't call `.name(...)` — Mendix widget names must be unique per
/// page, though nothing in this compiler enforces that for
/// author-supplied names (a duplicate author-chosen name round-trips
/// through `MprCodec` today; whether Studio Pro accepts it is unverified,
/// same caveat as this module's doc comment).
fn widget_name(name: &Option<String>, default_prefix: &str, counter: &mut u32) -> String {
    if let Some(name) = name {
        return name.clone();
    }
    *counter += 1;
    format!("{default_prefix}{counter}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_ir::page::LayoutRef;

    fn catalog() -> Rc<Catalog> {
        Rc::new(Catalog::for_version("11.12.1").unwrap())
    }

    #[test]
    fn a_page_with_widgets_but_no_layout_is_a_loud_error() {
        let mut decl = PageDecl::new("OrderOverview");
        decl.widgets.push(WidgetDecl::Text {
            name: None,
            caption: "hi".into(),
            class: None,
        });
        let error = compile_page(&catalog(), &decl).unwrap_err();
        assert!(
            matches!(error, WriterError::PageWidgetsRequireLayout(name) if name == "OrderOverview")
        );
    }

    #[test]
    fn a_page_with_no_widgets_compiles_without_a_layout() {
        let decl = PageDecl::new("Empty");
        let document = compile_page(&catalog(), &decl).unwrap();
        assert_eq!(document.get_str("$Type").unwrap(), "Forms$Page");
        assert_eq!(document.get_str("Name").unwrap(), "Empty");
    }

    #[test]
    fn compiled_page_round_trips_through_the_forms_codec() {
        let catalog = catalog();
        let mut decl = PageDecl::new("OrderOverview");
        decl.title = Some("Orders".into());
        decl.url = "orderoverview".into();
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::Container {
            name: Some("row".into()),
            class: Some("row".into()),
            style: None,
            children: vec![
                WidgetDecl::Text {
                    name: None,
                    caption: "Manage your orders".into(),
                    class: None,
                },
                WidgetDecl::Button {
                    name: None,
                    caption: "Close".into(),
                    class: None,
                    action: ButtonAction::ClosePage,
                },
            ],
        });
        decl.widgets.push(WidgetDecl::LayoutGrid {
            name: None,
            rows: vec![LayoutGridRowDecl {
                columns: vec![LayoutGridColumnDecl {
                    weight: 1,
                    children: vec![],
                }],
            }],
        });

        let document = compile_page(&catalog, &decl).unwrap();
        assert_eq!(document.get_str("$Type").unwrap(), "Forms$Page");

        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_encoded = codec.encode(&node).unwrap();
        let re_decoded = codec.decode(&re_encoded).unwrap();
        assert_eq!(node, re_decoded);
    }

    #[test]
    fn a_data_view_with_attribute_bound_widgets_and_flow_calling_buttons_round_trips() {
        let catalog = catalog();
        let mut decl = PageDecl::new("OrderDetail");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::DataView {
            name: Some("orderView".into()),
            source: DataSourceDecl::Microflow("Sales.ACT_GetOrder".into()),
            children: vec![
                WidgetDecl::TextBox {
                    name: None,
                    attribute: "Number".into(),
                    class: None,
                },
                WidgetDecl::CheckBox {
                    name: None,
                    attribute: "IsPaid".into(),
                    class: None,
                },
                WidgetDecl::DatePicker {
                    name: None,
                    attribute: "SubmittedAt".into(),
                    class: None,
                },
                WidgetDecl::DropDown {
                    name: None,
                    attribute: "Status".into(),
                    class: None,
                },
                WidgetDecl::Button {
                    name: None,
                    caption: "Submit".into(),
                    class: None,
                    action: ButtonAction::CallMicroflow("Sales.ACT_SubmitOrder".into()),
                },
                WidgetDecl::Button {
                    name: None,
                    caption: "Validate".into(),
                    class: None,
                    action: ButtonAction::CallNanoflow("Sales.NF_ValidateOrder".into()),
                },
            ],
        });

        let document = compile_page(&catalog, &decl).unwrap();
        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_encoded = codec.encode(&node).unwrap();
        let re_decoded = codec.decode(&re_encoded).unwrap();
        assert_eq!(node, re_decoded);
    }

    #[test]
    fn a_data_view_sourced_from_a_nanoflow_round_trips() {
        let catalog = catalog();
        let mut decl = PageDecl::new("OrderDetail");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::DataView {
            name: None,
            source: DataSourceDecl::Nanoflow("Sales.NF_GetOrder".into()),
            children: vec![],
        });

        let document = compile_page(&catalog, &decl).unwrap();
        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_encoded = codec.encode(&node).unwrap();
        let re_decoded = codec.decode(&re_encoded).unwrap();
        assert_eq!(node, re_decoded);
    }
}
