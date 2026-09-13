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
//! that's worth it. See `mxrs_ir::page` for the current widget vocabulary
//! and every remaining deferred gap (`ReferenceSelector`, per-widget
//! configuration on `data_grid_2`/`gallery`/`combo_box`, call-argument
//! mappings on microflow/nanoflow actions).
//!
//! **`DataViewBuilder`'s entry point takes the data source up front**
//! (`data_view_from_microflow`/`data_view_from_nanoflow`), not as a call
//! inside the configure closure the way `PageBuilder::layout` is a separate
//! statement from `.container(...)`. `WidgetDecl::DataView.source` is a
//! required field with no sensible default (there's no such thing as a
//! `Forms$DataView` without a `dataSource`) — requiring it as a parameter
//! makes "configured a data view with no source" a compile error instead of
//! a runtime one, the same "push bugs into `cargo build`" preference this
//! whole crate follows for association/microflow targets.

use mxrs_ir::markers::{
    AttributeMarker, MicroflowMarker, MicroflowRef, NanoflowMarker, NanoflowRef,
};
use mxrs_ir::page::{
    ButtonAction, DataSourceDecl, LayoutGridColumnDecl, LayoutGridRowDecl, LayoutRef, PageDecl,
    WidgetDecl,
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

    pub fn text_with(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut TextBuilder),
    ) -> &mut Self {
        push_text_with(&mut self.decl.widgets, caption, configure);
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

    pub fn data_view_from_microflow<M: MicroflowMarker>(
        &mut self,
        target: MicroflowRef<M>,
        configure: impl FnOnce(&mut DataViewBuilder),
    ) -> &mut Self {
        push_data_view(
            &mut self.decl.widgets,
            DataSourceDecl::Microflow(target.qualified_name()),
            configure,
        );
        self
    }

    pub fn data_view_from_nanoflow<N: NanoflowMarker>(
        &mut self,
        target: NanoflowRef<N>,
        configure: impl FnOnce(&mut DataViewBuilder),
    ) -> &mut Self {
        push_data_view(
            &mut self.decl.widgets,
            DataSourceDecl::Nanoflow(target.qualified_name()),
            configure,
        );
        self
    }

    pub fn data_grid_2(
        &mut self,
        configure: impl FnOnce(&mut PluggableWidgetBuilder),
    ) -> &mut Self {
        push_pluggable_widget(
            &mut self.decl.widgets,
            PluggableWidgetKind::DataGrid2,
            configure,
        );
        self
    }

    pub fn gallery(&mut self, configure: impl FnOnce(&mut PluggableWidgetBuilder)) -> &mut Self {
        push_pluggable_widget(
            &mut self.decl.widgets,
            PluggableWidgetKind::Gallery,
            configure,
        );
        self
    }

    pub fn combo_box(&mut self, configure: impl FnOnce(&mut PluggableWidgetBuilder)) -> &mut Self {
        push_pluggable_widget(
            &mut self.decl.widgets,
            PluggableWidgetKind::ComboBox,
            configure,
        );
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

    pub fn text_with(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut TextBuilder),
    ) -> &mut Self {
        push_text_with(&mut self.children, caption, configure);
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

    pub fn data_view_from_microflow<M: MicroflowMarker>(
        &mut self,
        target: MicroflowRef<M>,
        configure: impl FnOnce(&mut DataViewBuilder),
    ) -> &mut Self {
        push_data_view(
            &mut self.children,
            DataSourceDecl::Microflow(target.qualified_name()),
            configure,
        );
        self
    }

    pub fn data_view_from_nanoflow<N: NanoflowMarker>(
        &mut self,
        target: NanoflowRef<N>,
        configure: impl FnOnce(&mut DataViewBuilder),
    ) -> &mut Self {
        push_data_view(
            &mut self.children,
            DataSourceDecl::Nanoflow(target.qualified_name()),
            configure,
        );
        self
    }

    pub fn data_grid_2(
        &mut self,
        configure: impl FnOnce(&mut PluggableWidgetBuilder),
    ) -> &mut Self {
        push_pluggable_widget(
            &mut self.children,
            PluggableWidgetKind::DataGrid2,
            configure,
        );
        self
    }

    pub fn gallery(&mut self, configure: impl FnOnce(&mut PluggableWidgetBuilder)) -> &mut Self {
        push_pluggable_widget(&mut self.children, PluggableWidgetKind::Gallery, configure);
        self
    }

    pub fn combo_box(&mut self, configure: impl FnOnce(&mut PluggableWidgetBuilder)) -> &mut Self {
        push_pluggable_widget(&mut self.children, PluggableWidgetKind::ComboBox, configure);
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

    pub fn close_page(&mut self) -> &mut Self {
        self.action = ButtonAction::ClosePage;
        self
    }

    /// Marker-checked, mirroring `FlowBuilder::call_microflow`'s
    /// `MicroflowRef<M>` target — see `mxrs_ir::page`'s doc comment for why
    /// there's no call-argument mapping yet (a page-widget analog of a gap
    /// `FlowBuilder`'s own call activities already have a typed-`Expr`
    /// fix for, not ported here).
    pub fn call_microflow<M: MicroflowMarker>(&mut self, target: MicroflowRef<M>) -> &mut Self {
        self.action = ButtonAction::CallMicroflow(target.qualified_name());
        self
    }

    /// See [`ButtonBuilder::call_microflow`]'s doc comment; the nanoflow
    /// equivalent, via `NanoflowRef`.
    pub fn call_nanoflow<N: NanoflowMarker>(&mut self, target: NanoflowRef<N>) -> &mut Self {
        self.action = ButtonAction::CallNanoflow(target.qualified_name());
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

    pub fn text_with(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut TextBuilder),
    ) -> &mut Self {
        push_text_with(&mut self.children, caption, configure);
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

    pub fn data_view_from_microflow<M: MicroflowMarker>(
        &mut self,
        target: MicroflowRef<M>,
        configure: impl FnOnce(&mut DataViewBuilder),
    ) -> &mut Self {
        push_data_view(
            &mut self.children,
            DataSourceDecl::Microflow(target.qualified_name()),
            configure,
        );
        self
    }

    pub fn data_view_from_nanoflow<N: NanoflowMarker>(
        &mut self,
        target: NanoflowRef<N>,
        configure: impl FnOnce(&mut DataViewBuilder),
    ) -> &mut Self {
        push_data_view(
            &mut self.children,
            DataSourceDecl::Nanoflow(target.qualified_name()),
            configure,
        );
        self
    }

    pub fn data_grid_2(
        &mut self,
        configure: impl FnOnce(&mut PluggableWidgetBuilder),
    ) -> &mut Self {
        push_pluggable_widget(
            &mut self.children,
            PluggableWidgetKind::DataGrid2,
            configure,
        );
        self
    }

    pub fn gallery(&mut self, configure: impl FnOnce(&mut PluggableWidgetBuilder)) -> &mut Self {
        push_pluggable_widget(&mut self.children, PluggableWidgetKind::Gallery, configure);
        self
    }

    pub fn combo_box(&mut self, configure: impl FnOnce(&mut PluggableWidgetBuilder)) -> &mut Self {
        push_pluggable_widget(&mut self.children, PluggableWidgetKind::ComboBox, configure);
        self
    }
}

pub struct DataViewBuilder {
    name: Option<String>,
    source: DataSourceDecl,
    children: Vec<WidgetDecl>,
}

impl DataViewBuilder {
    fn new(source: DataSourceDecl) -> Self {
        DataViewBuilder {
            name: None,
            source,
            children: vec![],
        }
    }

    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
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

    pub fn text_with(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut TextBuilder),
    ) -> &mut Self {
        push_text_with(&mut self.children, caption, configure);
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

    /// Renders the bound attribute's raw value as editable text — Mendix's
    /// `Forms$TextBox`. `attribute` is relative to this data view's own
    /// entity (`WidgetDecl::TextBox`'s doc comment) — see this module's
    /// doc comment for why nothing here cross-checks it against the
    /// microflow/nanoflow this data view was constructed from.
    pub fn text_box<A: AttributeMarker>(&mut self) -> &mut Self {
        self.text_box_with::<A>(|_| {});
        self
    }

    pub fn text_box_with<A: AttributeMarker>(
        &mut self,
        configure: impl FnOnce(&mut AttributeWidgetBuilder),
    ) -> &mut Self {
        self.push_attribute_widget::<A>(AttributeWidgetKind::TextBox, configure);
        self
    }

    pub fn check_box<A: AttributeMarker>(&mut self) -> &mut Self {
        self.check_box_with::<A>(|_| {});
        self
    }

    pub fn check_box_with<A: AttributeMarker>(
        &mut self,
        configure: impl FnOnce(&mut AttributeWidgetBuilder),
    ) -> &mut Self {
        self.push_attribute_widget::<A>(AttributeWidgetKind::CheckBox, configure);
        self
    }

    pub fn date_picker<A: AttributeMarker>(&mut self) -> &mut Self {
        self.date_picker_with::<A>(|_| {});
        self
    }

    pub fn date_picker_with<A: AttributeMarker>(
        &mut self,
        configure: impl FnOnce(&mut AttributeWidgetBuilder),
    ) -> &mut Self {
        self.push_attribute_widget::<A>(AttributeWidgetKind::DatePicker, configure);
        self
    }

    pub fn drop_down<A: AttributeMarker>(&mut self) -> &mut Self {
        self.drop_down_with::<A>(|_| {});
        self
    }

    pub fn drop_down_with<A: AttributeMarker>(
        &mut self,
        configure: impl FnOnce(&mut AttributeWidgetBuilder),
    ) -> &mut Self {
        self.push_attribute_widget::<A>(AttributeWidgetKind::DropDown, configure);
        self
    }

    fn push_attribute_widget<A: AttributeMarker>(
        &mut self,
        kind: AttributeWidgetKind,
        configure: impl FnOnce(&mut AttributeWidgetBuilder),
    ) {
        let mut builder = AttributeWidgetBuilder::new();
        configure(&mut builder);
        let fields = (builder.name, A::NAME.to_string(), builder.class);
        self.children.push(match kind {
            AttributeWidgetKind::TextBox => WidgetDecl::TextBox {
                name: fields.0,
                attribute: fields.1,
                class: fields.2,
            },
            AttributeWidgetKind::CheckBox => WidgetDecl::CheckBox {
                name: fields.0,
                attribute: fields.1,
                class: fields.2,
            },
            AttributeWidgetKind::DatePicker => WidgetDecl::DatePicker {
                name: fields.0,
                attribute: fields.1,
                class: fields.2,
            },
            AttributeWidgetKind::DropDown => WidgetDecl::DropDown {
                name: fields.0,
                attribute: fields.1,
                class: fields.2,
            },
        });
    }

    fn into_decl(self) -> WidgetDecl {
        WidgetDecl::DataView {
            name: self.name,
            source: self.source,
            children: self.children,
        }
    }
}

pub struct TextBuilder {
    name: Option<String>,
    class: Option<String>,
}

impl TextBuilder {
    fn new() -> Self {
        Self {
            name: None,
            class: None,
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
}

enum AttributeWidgetKind {
    TextBox,
    CheckBox,
    DatePicker,
    DropDown,
}

pub struct AttributeWidgetBuilder {
    name: Option<String>,
    class: Option<String>,
}

impl AttributeWidgetBuilder {
    fn new() -> Self {
        Self {
            name: None,
            class: None,
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
}

fn push_container(widgets: &mut Vec<WidgetDecl>, configure: impl FnOnce(&mut ContainerBuilder)) {
    let mut builder = ContainerBuilder::new();
    configure(&mut builder);
    widgets.push(builder.into_decl());
}

fn push_text(widgets: &mut Vec<WidgetDecl>, caption: impl Into<String>) {
    push_text_with(widgets, caption, |_| {});
}

fn push_text_with(
    widgets: &mut Vec<WidgetDecl>,
    caption: impl Into<String>,
    configure: impl FnOnce(&mut TextBuilder),
) {
    let mut builder = TextBuilder::new();
    configure(&mut builder);
    widgets.push(WidgetDecl::Text {
        name: builder.name,
        caption: caption.into(),
        class: builder.class,
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

fn push_data_view(
    widgets: &mut Vec<WidgetDecl>,
    source: DataSourceDecl,
    configure: impl FnOnce(&mut DataViewBuilder),
) {
    let mut builder = DataViewBuilder::new(source);
    configure(&mut builder);
    widgets.push(builder.into_decl());
}

/// Which of Mendix's three pluggable built-ins `PluggableWidgetBuilder` is
/// configuring — see `mxrs_ir::page`'s doc comment for why all three share
/// the exact same (name/class-only) builder shape today.
enum PluggableWidgetKind {
    DataGrid2,
    Gallery,
    ComboBox,
}

/// Shared builder behind `.data_grid_2`/`.gallery`/`.combo_box` — all three
/// compile to the same shape (see `mxrs_ir::page::WidgetDecl::DataGrid2`'s
/// doc comment for why there's nothing to configure yet beyond
/// name/appearance), so one builder type serves all three rather than
/// tripling identical boilerplate.
pub struct PluggableWidgetBuilder {
    name: Option<String>,
    class: Option<String>,
}

impl PluggableWidgetBuilder {
    fn new() -> Self {
        PluggableWidgetBuilder {
            name: None,
            class: None,
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
}

fn push_pluggable_widget(
    widgets: &mut Vec<WidgetDecl>,
    kind: PluggableWidgetKind,
    configure: impl FnOnce(&mut PluggableWidgetBuilder),
) {
    let mut builder = PluggableWidgetBuilder::new();
    configure(&mut builder);
    widgets.push(match kind {
        PluggableWidgetKind::DataGrid2 => WidgetDecl::DataGrid2 {
            name: builder.name,
            class: builder.class,
        },
        PluggableWidgetKind::Gallery => WidgetDecl::Gallery {
            name: builder.name,
            class: builder.class,
        },
        PluggableWidgetKind::ComboBox => WidgetDecl::ComboBox {
            name: builder.name,
            class: builder.class,
        },
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
            c.text_with("Manage your orders", |text| {
                text.name("helpText");
                text.class("text-muted");
            });
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
        assert!(matches!(
            &children[0],
            WidgetDecl::Text { name, caption, class }
                if name.as_deref() == Some("helpText")
                    && caption == "Manage your orders"
                    && class.as_deref() == Some("text-muted")
        ));
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

    struct Order;
    impl mxrs_ir::markers::EntityMarker for Order {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "Order";
    }

    struct OrderNumber;
    impl AttributeMarker for OrderNumber {
        type Entity = Order;
        const NAME: &'static str = "Number";
    }

    struct OrderIsPaid;
    impl AttributeMarker for OrderIsPaid {
        type Entity = Order;
        const NAME: &'static str = "IsPaid";
    }

    struct ActGetOrder;
    impl MicroflowMarker for ActGetOrder {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "ACT_GetOrder";
    }

    struct ActSubmitOrder;
    impl MicroflowMarker for ActSubmitOrder {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "ACT_SubmitOrder";
    }

    struct NfValidateOrder;
    impl NanoflowMarker for NfValidateOrder {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "NF_ValidateOrder";
    }

    #[test]
    fn builds_a_data_view_with_attribute_bound_widgets_and_a_microflow_calling_button() {
        let mut page = PageBuilder::new("OrderDetail");
        page.layout("Atlas_Core.ApplicationLayout", "Main");
        page.data_view_from_microflow(MicroflowRef::<ActGetOrder>::new(), |dv| {
            dv.text_box_with::<OrderNumber>(|widget| {
                widget.name("numberInput");
                widget.class("form-control");
            });
            dv.check_box::<OrderIsPaid>();
            dv.button("Submit", |b| {
                b.call_microflow(MicroflowRef::<ActSubmitOrder>::new());
            });
        });
        let decl = page.into_decl();

        let WidgetDecl::DataView {
            source, children, ..
        } = &decl.widgets[0]
        else {
            panic!("expected a data view");
        };
        assert_eq!(
            *source,
            DataSourceDecl::Microflow("Sales.ACT_GetOrder".to_string())
        );
        assert_eq!(children.len(), 3);
        assert!(matches!(
            &children[0],
            WidgetDecl::TextBox { name, attribute, class }
                if name.as_deref() == Some("numberInput")
                    && attribute == "Number"
                    && class.as_deref() == Some("form-control")
        ));
        assert!(matches!(
            &children[1],
            WidgetDecl::CheckBox { attribute, .. } if attribute == "IsPaid"
        ));
        assert!(matches!(
            &children[2],
            WidgetDecl::Button { action: ButtonAction::CallMicroflow(name), .. }
                if name == "Sales.ACT_SubmitOrder"
        ));
    }

    #[test]
    fn a_button_can_call_a_nanoflow() {
        let mut button = ButtonBuilder::new("Validate");
        button.call_nanoflow(NanoflowRef::<NfValidateOrder>::new());
        let decl = button.into_decl();
        assert!(matches!(
            decl,
            WidgetDecl::Button { action: ButtonAction::CallNanoflow(name), .. }
                if name == "Sales.NF_ValidateOrder"
        ));
    }

    #[test]
    fn builds_a_page_with_data_grid_2_gallery_and_combo_box() {
        let mut page = PageBuilder::new("Dashboard");
        page.layout("Atlas_Core.ApplicationLayout", "Main");
        page.data_grid_2(|dg| {
            dg.name("ordersGrid");
            dg.class("orders-grid");
        });
        page.gallery(|_| {});
        page.combo_box(|_| {});
        let decl = page.into_decl();

        assert_eq!(decl.widgets.len(), 3);
        assert!(matches!(
            &decl.widgets[0],
            WidgetDecl::DataGrid2 { name, class }
                if name.as_deref() == Some("ordersGrid") && class.as_deref() == Some("orders-grid")
        ));
        assert!(matches!(&decl.widgets[1], WidgetDecl::Gallery { .. }));
        assert!(matches!(&decl.widgets[2], WidgetDecl::ComboBox { .. }));
    }
}
