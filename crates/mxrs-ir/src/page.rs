//! Page/widget IR — a small, storage-independent semantic tree, the same
//! shape `flow.rs`'s [`crate::flow::Activity`] uses for microflow bodies
//! rather than the low-level Mendix Forms metamodel (`mxrs-forms::Node`)
//! that `mxrs-writer` compiles this into.
//!
//! **First-slice scope, loud not silent about what's outside it** (mirrors
//! `mxrs-exporter`'s own crate doc style for naming gaps):
//!
//! - **Native/structural widgets only**: [`WidgetDecl::Container`]
//!   (`Forms$DivContainer`), [`WidgetDecl::LayoutGrid`] (rows of weighted
//!   columns), [`WidgetDecl::Text`] (static caption, `Forms$DynamicText`),
//!   [`WidgetDecl::Button`] (`Forms$ActionButton`, caption plus
//!   [`ButtonAction::None`] or [`ButtonAction::ClosePage`]). Pluggable
//!   widgets (Data Grid 2/Gallery/ComboBox/the generic
//!   `CustomWidgets$CustomWidget` bundle) and native *data-bound* widgets
//!   (TextBox/CheckBox/DatePicker/... bound to an entity attribute via a
//!   `Forms$DataView` entity context) have no `WidgetDecl` variant yet —
//!   see `mxrs-writer::page_compiler`'s crate doc for the fuller reasoning
//!   and the follow-up this unblocks.
//! - **No microflow/nanoflow-calling actions**: a button can only close the
//!   page or do nothing. Wiring a button to call a microflow needs the
//!   same `MicroflowRef<M>`/`MicroflowMarker` machinery `mxrs-dsl::flow`
//!   already uses for `call_microflow` — straightforward to add later, but
//!   flows are explicitly out of scope for this pass (pages only).
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonAction {
    #[default]
    None,
    ClosePage,
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
