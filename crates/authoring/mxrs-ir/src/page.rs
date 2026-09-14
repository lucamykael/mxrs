//! Page/widget IR — a small, storage-independent semantic tree, the same
//! shape `flow.rs`'s [`crate::flow::Activity`] uses for microflow bodies
//! rather than the low-level Mendix Forms metamodel (`mxrs-forms::Node`)
//! that `mxrs-writer` compiles this into.
//!
//! **Widget vocabulary, loud not silent about what's outside it** (mirrors
//! `mxrs-exporter`'s own crate doc style for naming gaps):
//!
//! - **Structural**: [`WidgetDecl::Container`] (`Forms$DivContainer`),
//!   [`WidgetDecl::LayoutGrid`] (rows of weighted columns), [`WidgetDecl::Text`]
//!   (static caption, `Forms$DynamicText`).
//! - **Native data-bound**: [`WidgetDecl::DataView`] (`Forms$DataView`,
//!   the entity context every attribute-bound widget needs — see
//!   `mxrs-writer::page_compiler`'s doc comment for why a `Forms$Page` has
//!   no other way to give a widget an entity to bind to) sourced from an
//!   existing microflow or nanoflow ([`DataSourceDecl`]), containing
//!   [`WidgetDecl::TextBox`]/[`WidgetDecl::CheckBox`]/[`WidgetDecl::DatePicker`]/
//!   [`WidgetDecl::DropDown`] — each bound to an attribute via
//!   `crate::markers::AttributeMarker`, the same marker-checked-target
//!   pattern `EntityBuilder::association`'s `Ref<M>` already established for
//!   domain associations. Only a one-sided guarantee, same caveat
//!   `AssociationMarker`'s own doc comment already names for `From`: nothing
//!   here checks that an attribute widget's `AttributeMarker::Entity`
//!   actually matches the entity its enclosing `DataView` sources — the
//!   builder has no compile-time handle on that at the point a widget is
//!   pushed. **`ReferenceSelector`** (association-bound, not
//!   attribute-bound — a different marker shape, `AssociationMarker`
//!   instead of `AttributeMarker`, and Mendix's `AssociationWidget` base
//!   instead of `AttributeWidget`/`MemberWidget`) and every other native
//!   widget kind `mxrs-model::page`'s decode side already recognizes
//!   (`ReferenceSetSelector`, `FileManager`, image/navigation/menu
//!   widgets, ...) have no `WidgetDecl` variant yet — a real, separately
//!   trackable follow-up, not implemented here.
//! - **Pluggable widgets**: [`WidgetDecl::DataGrid2`]/[`WidgetDecl::Gallery`]/
//!   [`WidgetDecl::ComboBox`] — Mendix 11's own modern built-ins, which are
//!   `CustomWidgets$CustomWidget` instances under the hood (see
//!   `mxrs-model::page`'s module doc). **Deliberately name/class only, no
//!   per-widget configuration** (columns, data source, entity, bound
//!   attribute, ...) — a genuine blocker, not an unfinished-breadth cut:
//!   see `mxrs-writer::page_compiler`'s doc comment for why. The generic
//!   third-party `CustomWidgets$CustomWidget` bundle (an arbitrary
//!   marketplace widget by id) has no `WidgetDecl` variant at all yet.
//! - **Button actions**: [`ButtonAction::None`], [`ButtonAction::ClosePage`],
//!   plus [`ButtonAction::CallMicroflow`]/[`ButtonAction::CallNanoflow`] —
//!   marker-checked via `crate::markers::MicroflowRef`/`NanoflowRef`, the
//!   same target-checking `mxrs-dsl::flow::FlowBuilder::call_microflow`
//!   established for microflow activities. No call-argument mappings yet
//!   (a microflow/nanoflow parameter binding needs the same typed-`Expr`
//!   machinery `mxrs-dsl::flow`'s own call activities use — out of scope
//!   for this pass, a page-widget analog of that same deferred gap).
//! - **`layout` is a plain, unchecked by-name reference** (`"Module.Name"`
//!   for the Layout document, plus the Layout's own parameter name widgets
//!   attach to, e.g. `"Main"`) — not marker-checked the way
//!   [`crate::markers::Ref`] checks entity associations. This mirrors a
//!   real limitation in the oracle itself: mxrb's own DSL
//!   (`lib/mxrb/dsl/builder.rb`) has never had a typed `Layout`
//!   declaration either, just a raw string (see
//!   `lib/mxrb/scaffold/templates.rb`'s `layout "#{module_name}.ApplicationLayout"`).
//!   [`LayoutDecl`] authors native web layouts and named placeholders;
//!   references are still validated at the writer boundary, not Rust paths.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ButtonAction {
    #[default]
    None,
    ClosePage,
    /// Qualified microflow name (e.g. `"Sales.ACT_SubmitOrder"`) — callers
    /// go through `mxrs-dsl::ButtonBuilder::call_microflow::<M: MicroflowMarker>()`
    /// for the marker-checked entry point; this IR itself only stores the
    /// resolved name, the same split `AssociationDecl.target` already draws
    /// between the builder's marker requirement and the underlying
    /// storage-independent string (see `crate::markers`' doc comment).
    CallMicroflow(String),
    /// Qualified nanoflow name — see [`ButtonAction::CallMicroflow`]'s doc
    /// comment; the nanoflow equivalent, via `NanoflowRef`.
    CallNanoflow(String),
    /// Commits the enclosing data view's object. Carries no target: the
    /// action operates on whatever context the button sits in, which is why
    /// it needs no marker the way the two call variants do.
    SaveChanges,
    /// Rolls the enclosing data view's object back to its last committed
    /// state — [`ButtonAction::SaveChanges`]'s counterpart.
    CancelChanges,
}

/// What a [`WidgetDecl::DataView`] fetches its context object from. Both
/// variants store the flow's already-resolved qualified name — see
/// [`ButtonAction::CallMicroflow`]'s doc comment for why the IR itself
/// stays marker-free while `mxrs-dsl::DataViewBuilder` requires
/// `MicroflowMarker`/`NanoflowMarker`.
///
/// Object page parameters can be inherited through [`DataSourceDecl::Context`].
/// Other parameter data types remain fail-closed until their authoring types
/// are represented here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSourceDecl {
    Microflow(String),
    Nanoflow(String),
    /// The named object parameter inherited by this page.
    Context {
        parameter: String,
        entity: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageParameterDecl {
    pub name: String,
    pub entity: String,
    pub required: bool,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetDecl {
    /// Only legal inside a layout, never an ordinary page body.
    LayoutPlaceholder { name: String },
    Container {
        name: Option<String>,
        class: Option<String>,
        style: Option<String>,
        children: Vec<WidgetDecl>,
    },
    LayoutGrid {
        name: Option<String>,
        rows: Vec<LayoutGridRowDecl>,
    },
    Text {
        name: Option<String>,
        caption: String,
        class: Option<String>,
    },
    Button {
        name: Option<String>,
        caption: String,
        class: Option<String>,
        action: ButtonAction,
    },
    /// The entity context every attribute-bound widget below needs — see
    /// this module's doc comment.
    DataView {
        name: Option<String>,
        source: DataSourceDecl,
        children: Vec<WidgetDecl>,
    },
    TextBox {
        name: Option<String>,
        /// The bound attribute's plain name (`AttributeMarker::NAME`), not
        /// a qualified path — matches mxrb's own `Forms$TextBox` shape,
        /// where the attribute is always relative to the enclosing
        /// `DataView`'s own entity, never a cross-entity path.
        attribute: String,
        class: Option<String>,
    },
    CheckBox {
        name: Option<String>,
        attribute: String,
        class: Option<String>,
    },
    DatePicker {
        name: Option<String>,
        attribute: String,
        class: Option<String>,
    },
    DropDown {
        name: Option<String>,
        attribute: String,
        class: Option<String>,
    },
    /// Mendix's official React Data Grid 2 (`com.mendix.widget.web.datagrid.Datagrid`).
    /// See this module's doc comment for why there's no column/data-source
    /// configuration yet.
    DataGrid2 {
        name: Option<String>,
        class: Option<String>,
    },
    /// Mendix's official Gallery widget (`com.mendix.widget.web.gallery.Gallery`).
    Gallery {
        name: Option<String>,
        class: Option<String>,
    },
    /// Mendix's official Combo Box widget (`com.mendix.widget.web.combobox.Combobox`).
    ComboBox {
        name: Option<String>,
        class: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutGridColumnDecl {
    pub weight: i32,
    pub children: Vec<WidgetDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutGridRowDecl {
    pub columns: Vec<LayoutGridColumnDecl>,
}

/// A Layout document reference plus the parameter (placeholder) name on
/// that layout the page's widgets attach to — see this module's doc
/// comment for why this is a plain string pair, not a marker type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutRef {
    pub qualified_name: String,
    pub parameter: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LayoutKind {
    #[default]
    Responsive,
    Tablet,
    Phone,
    ModalPopup,
    Popup,
    Legacy,
}

/// Web layouts use the same native widget tree as pages, with placeholders
/// marking where a page's layout-call arguments are inserted. Native-mobile
/// layouts and nested layout calls require separate authoring support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutDecl {
    pub name: String,
    pub documentation: String,
    pub excluded: bool,
    pub export_level: String,
    pub canvas_width: i32,
    pub canvas_height: i32,
    pub class: Option<String>,
    pub style: Option<String>,
    pub kind: LayoutKind,
    pub widgets: Vec<WidgetDecl>,
}

impl LayoutDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            excluded: false,
            export_level: "Hidden".into(),
            canvas_width: 800,
            canvas_height: 600,
            class: None,
            style: None,
            kind: LayoutKind::Responsive,
            widgets: vec![],
        }
    }
}

impl LayoutRef {
    pub fn new(qualified_name: impl Into<String>, parameter: impl Into<String>) -> Self {
        LayoutRef {
            qualified_name: qualified_name.into(),
            parameter: parameter.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageDecl {
    pub name: String,
    pub documentation: String,
    pub url: String,
    /// Displayed page title. Defaults to `name` when `None`.
    pub title: Option<String>,
    /// Static page-level appearance. Dynamic widget appearance remains a
    /// separate expression-bearing concern.
    pub class: Option<String>,
    pub style: Option<String>,
    /// Qualified module-role names allowed to open the page. An empty list
    /// follows Mendix's unrestricted/default page behavior.
    pub allowed_module_roles: Vec<String>,
    pub popup_width: i32,
    pub popup_height: i32,
    pub popup_resizable: bool,
    pub excluded: bool,
    /// Mendix document export level (`Hidden` or `API`).
    pub export_level: String,
    pub parameters: Vec<PageParameterDecl>,
    pub layout: Option<LayoutRef>,
    pub widgets: Vec<WidgetDecl>,
}

impl PageDecl {
    pub fn new(name: impl Into<String>) -> Self {
        PageDecl {
            name: name.into(),
            documentation: String::new(),
            url: String::new(),
            title: None,
            class: None,
            style: None,
            allowed_module_roles: vec![],
            popup_width: 0,
            popup_height: 0,
            popup_resizable: false,
            excluded: false,
            export_level: "Hidden".to_string(),
            parameters: vec![],
            layout: None,
            widgets: vec![],
        }
    }
}
