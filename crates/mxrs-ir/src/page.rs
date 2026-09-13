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
//! - **Pluggable widgets** (Data Grid 2/Gallery/ComboBox/the generic
//!   `CustomWidgets$CustomWidget` bundle) have no `WidgetDecl` variant yet —
//!   see `mxrs-writer::page_compiler`'s crate doc for the fuller reasoning.
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
//!   Layouts have no Cargo-native front end yet, the same deferred-concept
//!   boundary pages/flows/security/navigation already draw.

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
}

/// What a [`WidgetDecl::DataView`] fetches its context object from. Both
/// variants store the flow's already-resolved qualified name — see
/// [`ButtonAction::CallMicroflow`]'s doc comment for why the IR itself
/// stays marker-free while `mxrs-dsl::DataViewBuilder` requires
/// `MicroflowMarker`/`NanoflowMarker`.
///
/// **No page-parameter/context-inherited source yet** (`Forms$DataViewSource`,
/// mxrb's `"context"` decode kind) — that needs `PageDecl` to have a typed
/// parameter list to inherit an object *from*, which doesn't exist yet (see
/// this module's own doc comment on `layout` for the same "no Cargo-native
/// front end yet" boundary). A microflow/nanoflow-sourced `DataView` needs
/// no such parameter, so it's unblocked without that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSourceDecl {
    Microflow(String),
    Nanoflow(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetDecl {
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
            layout: None,
            widgets: vec![],
        }
    }
}
