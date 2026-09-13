//! Builder-closure front end for [`mxrs_ir::page::PageDecl`] — the "locked
//! shape" decision for Cargo-native pages, chosen over a `#[derive(MxPage)]`
//! struct derive.
//!
//! Entities are naturally a flat field list (`#[derive(MxEntity)]` fits
//! because a struct's fields *are* the attribute list). A page is a
//! *tree* of widgets with no natural 1:1 Rust struct-field mapping, so this
//! crate instead extends the same nested-closure-builder shape already
//! locked for microflows (`mxrs-dsl::flow::FlowBuilder::decision`'s
//! `then`/`otherwise` closures) and modules/entities (`crate::module`,
//! `crate::entity`) — see this crate's own doc comment for that Phase 3
//! decision. `ModuleBuilder::page` is the ordinary entry point
//! (`module.page("OrderOverview", |p| { ... })`); `PageBuilder::new` is
//! also public so generated code (e.g. a future `mxrs-exporter` widening)
//! can construct a `PageDecl` without a `ModuleBuilder` in scope.
//!
//! **Not part of this pass**: a `project! {}` textual-macro `page { }`
//! block. Parsing a widget tree out of macro token trees is real
//! additional `mxrs-macros` surface (see that crate's doc for its own
//! locked-scope precedent); pages are authored by calling this builder API
//! from ordinary Rust, not through macro sugar, until a later pass decides
//! that's worth it. See `mxrs_ir::page` for the first-slice widget
//! vocabulary and every other deferred gap (pluggable widgets, data-bound
//! widgets, button actions beyond close-page).

use mxrs_ir::page::{
    ButtonAction, LayoutGridColumnDecl, LayoutGridRowDecl, LayoutRef, PageDecl, WidgetDecl,
};

pub struct PageBuilder {
    decl: PageDecl,
}

impl PageBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        PageBuilder {
            decl: PageDecl::new(name),
        }
    }

    pub fn into_decl(self) -> PageDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    pub fn url(&mut self, url: impl Into<String>) -> &mut Self {
        self.decl.url = url.into();
        self
    }

    pub fn title(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.title = Some(text.into());
        self
    }

    /// `qualified_name` is the existing Layout document's `"Module.Name"`;
    /// `parameter` is the name of the placeholder on that layout widgets
    /// attach to (e.g. `"Main"`). See `mxrs_ir::page`'s doc comment for why
    /// this isn't marker-checked.
    pub fn layout(
        &mut self,
        qualified_name: impl Into<String>,
        parameter: impl Into<String>,
    ) -> &mut Self {
        self.decl.layout = Some(LayoutRef::new(qualified_name, parameter));
        self
    }

    pub fn container(&mut self, configure: impl FnOnce(&mut ContainerBuilder)) -> &mut Self {
        push_container(&mut self.decl.widgets, configure);
        self
    }

    pub fn text(&mut self, caption: impl Into<String>) -> &mut Self {
        push_text(&mut self.decl.widgets, caption);
        self
    }

    pub fn button(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut ButtonBuilder),
    ) -> &mut Self {
        push_button(&mut self.decl.widgets, caption, configure);
        self
    }

    pub fn layout_grid(&mut self, configure: impl FnOnce(&mut LayoutGridBuilder)) -> &mut Self {
        push_layout_grid(&mut self.decl.widgets, configure);
        self
    }
}

pub struct ContainerBuilder {
    name: Option<String>,
    class: Option<String>,
    style: Option<String>,
    children: Vec<WidgetDecl>,
}

impl ContainerBuilder {
    fn new() -> Self {
        ContainerBuilder {
            name: None,
            class: None,
            style: None,
            children: vec![],
        }
    }

    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    pub fn class(&mut self, class: impl Into<String>) -> &mut Self {
        self.class = Some(class.into());
        self
    }

    pub fn style(&mut self, style: impl Into<String>) -> &mut Self {
        self.style = Some(style.into());
        self
    }

    pub fn container(&mut self, configure: impl FnOnce(&mut ContainerBuilder)) -> &mut Self {
        push_container(&mut self.children, configure);
        self
    }

    pub fn text(&mut self, caption: impl Into<String>) -> &mut Self {
        push_text(&mut self.children, caption);
        self
    }

    pub fn button(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut ButtonBuilder),
    ) -> &mut Self {
        push_button(&mut self.children, caption, configure);
        self
    }

    pub fn layout_grid(&mut self, configure: impl FnOnce(&mut LayoutGridBuilder)) -> &mut Self {
        push_layout_grid(&mut self.children, configure);
        self
    }

    fn into_decl(self) -> WidgetDecl {
        WidgetDecl::Container {
            name: self.name,
            class: self.class,
            style: self.style,
            children: self.children,
        }
    }
}

pub struct ButtonBuilder {
    name: Option<String>,
    caption: String,
    class: Option<String>,
    action: ButtonAction,
}

impl ButtonBuilder {
    fn new(caption: impl Into<String>) -> Self {
        ButtonBuilder {
            name: None,
            caption: caption.into(),
            class: None,
            action: ButtonAction::None,
        }
    }

    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    pub fn class(&mut self, class: impl Into<String>) -> &mut Self {
        self.class = Some(class.into());
        self
    }

    /// The only built-in action this first slice supports besides doing
    /// nothing on click — see `mxrs_ir::page`'s doc comment for why calling
    /// a microflow/nanoflow isn't covered yet.
    pub fn close_page(&mut self) -> &mut Self {
        self.action = ButtonAction::ClosePage;
        self
    }

    fn into_decl(self) -> WidgetDecl {
        WidgetDecl::Button {
            name: self.name,
            caption: self.caption,
            class: self.class,
            action: self.action,
        }
    }
}

pub struct LayoutGridBuilder {
    name: Option<String>,
    rows: Vec<LayoutGridRowDecl>,
}

impl LayoutGridBuilder {
    fn new() -> Self {
        LayoutGridBuilder {
            name: None,
            rows: vec![],
        }
    }

    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    pub fn row(&mut self, configure: impl FnOnce(&mut LayoutGridRowBuilder)) -> &mut Self {
        let mut builder = LayoutGridRowBuilder::new();
        configure(&mut builder);
        self.rows.push(LayoutGridRowDecl {
            columns: builder.columns,
        });
        self
    }
}

pub struct LayoutGridRowBuilder {
    columns: Vec<LayoutGridColumnDecl>,
}

impl LayoutGridRowBuilder {
    fn new() -> Self {
        LayoutGridRowBuilder { columns: vec![] }
    }

    pub fn column(
        &mut self,
        weight: i32,
        configure: impl FnOnce(&mut LayoutGridColumnBuilder),
    ) -> &mut Self {
        let mut builder = LayoutGridColumnBuilder::new(weight);
        configure(&mut builder);
        self.columns.push(LayoutGridColumnDecl {
            weight: builder.weight,
            children: builder.children,
        });
        self
    }
}

pub struct LayoutGridColumnBuilder {
    weight: i32,
    children: Vec<WidgetDecl>,
}

impl LayoutGridColumnBuilder {
    fn new(weight: i32) -> Self {
        LayoutGridColumnBuilder {
            weight,
            children: vec![],
        }
    }

    pub fn container(&mut self, configure: impl FnOnce(&mut ContainerBuilder)) -> &mut Self {
        push_container(&mut self.children, configure);
        self
    }

    pub fn text(&mut self, caption: impl Into<String>) -> &mut Self {
        push_text(&mut self.children, caption);
        self
    }

    pub fn button(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut ButtonBuilder),
    ) -> &mut Self {
        push_button(&mut self.children, caption, configure);
        self
    }

    pub fn layout_grid(&mut self, configure: impl FnOnce(&mut LayoutGridBuilder)) -> &mut Self {
        push_layout_grid(&mut self.children, configure);
        self
    }
}

fn push_container(widgets: &mut Vec<WidgetDecl>, configure: impl FnOnce(&mut ContainerBuilder)) {
    let mut builder = ContainerBuilder::new();
    configure(&mut builder);
    widgets.push(builder.into_decl());
}

fn push_text(widgets: &mut Vec<WidgetDecl>, caption: impl Into<String>) {
    widgets.push(WidgetDecl::Text {
        name: None,
        caption: caption.into(),
        class: None,
    });
}

fn push_button(
    widgets: &mut Vec<WidgetDecl>,
    caption: impl Into<String>,
    configure: impl FnOnce(&mut ButtonBuilder),
) {
    let mut builder = ButtonBuilder::new(caption);
    configure(&mut builder);
    widgets.push(builder.into_decl());
}

fn push_layout_grid(widgets: &mut Vec<WidgetDecl>, configure: impl FnOnce(&mut LayoutGridBuilder)) {
    let mut builder = LayoutGridBuilder::new();
    configure(&mut builder);
    widgets.push(WidgetDecl::LayoutGrid {
        name: builder.name,
        rows: builder.rows,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_page_with_nested_containers_text_and_a_close_page_button() {
        let mut page = PageBuilder::new("OrderOverview");
        page.title("Orders");
        page.url("orderoverview");
        page.layout("Atlas_Core.ApplicationLayout", "Main");
        page.container(|c| {
            c.class("row");
            c.text("Manage your orders");
            c.button("Close", |b| {
                b.close_page();
            });
        });
        let decl = page.into_decl();

        assert_eq!(decl.name, "OrderOverview");
        assert_eq!(decl.title.as_deref(), Some("Orders"));
        assert_eq!(decl.url, "orderoverview");
        let layout = decl.layout.expect("layout set");
        assert_eq!(layout.qualified_name, "Atlas_Core.ApplicationLayout");
        assert_eq!(layout.parameter, "Main");
        assert_eq!(decl.widgets.len(), 1);
        let WidgetDecl::Container {
            class, children, ..
        } = &decl.widgets[0]
        else {
            panic!("expected a container");
        };
        assert_eq!(class.as_deref(), Some("row"));
        assert_eq!(children.len(), 2);
        assert!(
            matches!(&children[0], WidgetDecl::Text { caption, .. } if caption == "Manage your orders")
        );
        assert!(matches!(
            &children[1],
            WidgetDecl::Button { action: ButtonAction::ClosePage, caption, .. } if caption == "Close"
        ));
    }

    #[test]
    fn builds_a_layout_grid_with_weighted_columns() {
        let mut page = PageBuilder::new("Dashboard");
        page.layout_grid(|grid| {
            grid.row(|row| {
                row.column(1, |col| {
                    col.text("Left");
                });
                row.column(2, |col| {
                    col.text("Right");
                });
            });
        });
        let decl = page.into_decl();

        let WidgetDecl::LayoutGrid { rows, .. } = &decl.widgets[0] else {
            panic!("expected a layout grid");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].columns.len(), 2);
        assert_eq!(rows[0].columns[0].weight, 1);
        assert_eq!(rows[0].columns[1].weight, 2);
    }
}
