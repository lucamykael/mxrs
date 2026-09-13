//! Typed instance of a pluggable widget: its inline schema, outer storage
//! baseline, assigned object, and every property-value shape. Ports
//! `Mxrb::Pluggable::{Node,ObjectNode}` from `lib/mxrb/pluggable/node.rb` —
//! deliberately not the `method_missing`-based Ruby ergonomics on top of
//! it (`mxrs-dsl`'s job, same boundary `mxrs-forms`'s own `Node` already
//! draws).

use crate::catalog::PropertyType;
use crate::catalog::WidgetType;

/// A complete custom-widget instance: its inline schema and assigned
/// object. Keeping both is essential for encoding because every stored
/// property/value points back into that inline schema by generated UUID.
#[derive(Debug, Clone)]
pub struct WidgetNode<N> {
    pub widget_type: WidgetType,
    pub object: ObjectNode<N>,
    /// Original outer storage document, when this node was decoded from
    /// BSON. The encoder uses it to preserve outer properties and the
    /// exact inline-schema identity expected by Studio Pro.
    pub storage_baseline: Option<mxrs_bson::Document>,
    /// Extra top-level fields merged onto the outer `CustomWidgets$CustomWidget`
    /// document when encoding *without* a `storage_baseline` — i.e. a
    /// freshly authored widget, not a decoded one (a decoded widget's
    /// `storage_baseline` already carries every original field verbatim,
    /// `extra` included, so this only matters on first write). Ports a real
    /// behavior of `Mxrb::Writer#pluggable_widget_doc`'s fallback path
    /// (`lib/mxrb/writer.rb`'s `configure_fallback_data_grid!`/
    /// `configure_fallback_combo_box!`): when the real Studio-Pro-side
    /// widget-package property schema isn't available to hydrate from (the
    /// normal case for any project not opened by an actual Studio Pro
    /// install — true for every `mxrs`-authored project), mxrb still emits
    /// a *recognizable, structurally valid* widget by writing a small set
    /// of native-widget-shaped fields directly onto the wrapper document
    /// (e.g. a Data Grid 2's `DataSource`/`Columns`/`ToolBar`, mirroring
    /// `Forms$DataGrid`'s own shape) rather than attempting to fabricate
    /// the widget's real (and, without Studio Pro, unknowable) pluggable
    /// property schema. Studio Pro recognizes the `WidgetId` on next open
    /// and "hydrates" the real schema itself. `mxrs-writer::page_compiler`
    /// ports the same fallback for the same reason.
    pub extra: mxrs_bson::Document,
}

impl<N> WidgetNode<N> {
    pub fn new(widget_type: WidgetType, object: ObjectNode<N>) -> Self {
        Self {
            widget_type,
            object,
            storage_baseline: None,
            extra: mxrs_bson::Document::new(),
        }
    }
}

impl<N: PartialEq> PartialEq for WidgetNode<N> {
    fn eq(&self, other: &Self) -> bool {
        self.widget_type == other.widget_type && self.object == other.object
    }
}

/// A decoded pluggable-widget property value. Self-contained kinds live
/// directly in this enum; kinds containing native Forms nodes use the
/// generic `N` supplied through the embedded codec traits.
/// The polymorphic `target` a semantic [`Value::Reference`] wraps —
/// mirrors `Pluggable.reference(kind, target)`'s duck-typed `target` in
/// mxrb: a bare storage path for `File`/`Form`/`Image`/`Microflow`/
/// `Nanoflow` and an `Association` routed through `AttributeRef`, or a
/// structured domain-model reference for an `Association` routed through
/// `EntityRef`.
#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceTarget {
    Path(String),
    Attribute(mxrs_forms_refs::AttributeReference),
    Entity(mxrs_forms_refs::EntityReference),
}

/// `N` is the type an embedded (non-pluggable) Forms element decodes to —
/// always `mxrs_forms::Node` in practice, injected via
/// [`crate::embedded::EmbeddedFormsDecoder`] rather than named directly
/// here, to avoid a dependency cycle (`mxrs-forms` already depends on
/// this crate). See `mpr_codec`'s module doc for which kinds need it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value<N> {
    /// An absent optional reference (mxrb's `nil` — a `File`/`Form`/
    /// `Image`/`Microflow`/`Nanoflow`-kind property with no target set).
    Null,
    Boolean(bool),
    Integer(i64),
    /// `Decimal`/`String`/`Enumeration` all store `PrimitiveValue` as a
    /// raw string in mxrb too (`Decimal.coerce` only normalizes
    /// formatting for later arithmetic, which this crate has no need
    /// for) — kept as `String` here rather than introducing a decimal
    /// type this crate doesn't otherwise use.
    Primitive(String),
    Selection(String),
    Expression(String),
    XPathConstraint(String),
    /// `TranslatableString` — self-contained (`Texts$Text`/`Texts$Translation`
    /// are read directly, not through `mxrs-forms`). One `(language, text)`
    /// pair per translation, same order as stored.
    Text(Vec<(String, String)>),
    /// An `Attribute`-kind value's `AttributeRef` — ports the `when
    /// 'Attribute'` arm of mxrb's `decode_value`, which returns
    /// `forms_codec.decode_attribute_reference_value(...)` directly (not
    /// wrapped in `Pluggable.reference`).
    AttributeReference(mxrs_forms_refs::AttributeReference),
    /// An `Entity`-kind value's `EntityRef` — ports the `when 'Entity'`
    /// arm the same way.
    EntityReference(mxrs_forms_refs::EntityReference),
    /// Covers `File`/`Form`/`Image`/`Microflow`/`Nanoflow`, and
    /// `Association` (routed through either `EntityRef` or `AttributeRef`
    /// — mxrb's `decode_semantic_reference` wraps both in the same
    /// `Pluggable.reference(kind, target)` shape, so `target`'s shape
    /// depends on which branch decoded it).
    Reference {
        kind: String,
        target: ReferenceTarget,
    },
    /// A nested pluggable object (`Object`-kind property) — `None` means
    /// an unset single (non-list) nested object, mirroring mxrb's
    /// `items.first` on an empty collection.
    Object(Option<Box<ObjectNode<N>>>),
    ObjectList(Vec<ObjectNode<N>>),
    System,
    /// A `TextTemplate`-kind value's embedded `Texts$TextTemplate`
    /// element — ports `decode_optional_forms(document['TextTemplate'])`,
    /// i.e. `forms_codec.decode_embedded(...)`. `Null` (not this variant)
    /// when the field is absent, same convention as `Attribute`/`Entity`.
    TextTemplate(N),
    /// An `Action`-kind value's embedded element — ports
    /// `decode_optional_forms(document['Action'])` the same way.
    Action(N),
    /// An `Icon`-kind value's embedded element — ports
    /// `decode_optional_forms(document['Icon'])` the same way.
    Icon(N),
    /// A `DataSource`-kind value — ports `decode_data_source` in full
    /// (mxrb's `XPathSource` branch, or a source of some other `$Type`
    /// decoded whole via the injected [`crate::embedded::EmbeddedFormsDecoder`],
    /// mirroring `forms_codec.decode_embedded(source, ...)`). `None`
    /// mirrors mxrb's `nil` (no source and no variable at all).
    DataSource(Option<DataSourceValue<N>>),
    /// A `Widgets`-kind value — one [`WidgetItem`] per entry, mirroring
    /// mxrb's `decode_widgets`: a `CustomWidgets$CustomWidget` item
    /// recurses into this crate's own complete widget codec, retaining the
    /// nested schema and outer storage baseline; any
    /// other item routes through the injected `EmbeddedFormsDecoder`
    /// (mxrb's `forms_codec.decode_embedded`).
    Widgets(Vec<WidgetItem<N>>),
}

/// See [`Value::DataSource`]. Ports mxrb's `decode_data_source` return
/// shape: either a `CustomWidgets$CustomWidgetXPathSource`/
/// `...DatabaseSource` decoded field-by-field (`XPath`), or a source of
/// some other `$Type` decoded whole via `forms_codec.decode_embedded`
/// (`Embedded`) — mxrb returns either shape untyped (duck-typed), this
/// enum makes the two cases explicit.
#[derive(Debug, Clone, PartialEq)]
pub enum DataSourceValue<N> {
    XPath(DataSource<N>),
    Embedded(N),
}

/// See [`DataSourceValue::XPath`]. Ports every field of mxrb's
/// `XPathSource`, now that `sort_bar`/`source_variable` route through the
/// injected `EmbeddedFormsDecoder` instead of being unreachable.
#[derive(Debug, Clone, PartialEq)]
pub struct DataSource<N> {
    pub entity: Option<mxrs_forms_refs::EntityReference>,
    pub constraint: String,
    pub sort_bar: Option<N>,
    pub source_variable: Option<N>,
    pub force_full_objects: bool,
}

/// See [`Value::Widgets`].
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetItem<N> {
    /// A nested `CustomWidgets$CustomWidget` item, decoded via this
    /// crate's own schema/object decode (mxrb's `decode(item)` branch),
    /// retaining the nested schema for encoding.
    Pluggable(Box<WidgetNode<N>>),
    /// Any other (native Forms) item, decoded via the injected
    /// `EmbeddedFormsDecoder` (mxrb's `forms_codec.decode_embedded`
    /// branch).
    Native(N),
}

/// One assigned property on an [`ObjectNode`]. Ports the read side of
/// `Pluggable::ObjectNode::Assignment`; `source_variable` mirrors mxrb's
/// per-assignment `SourceVariable` (present on any property whose
/// `value_type.kind` isn't `DataSource`) but is left as an opaque raw
/// document rather than a decoded `Forms` node — decoding it needs the
/// same `mxrs-forms` primitives this module's `Value` gaps do.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment<N> {
    pub property: PropertyType,
    pub value: Value<N>,
    pub source_variable: Option<mxrs_bson::Document>,
}

/// A decoded pluggable-widget object instance — the `Object` field of a
/// `CustomWidgets$CustomWidget` (or a nested `Object`-kind property).
/// Ports the read side of `Pluggable::ObjectNode`.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectNode<N> {
    assignments: Vec<Assignment<N>>,
}

impl<N> Default for ObjectNode<N> {
    fn default() -> Self {
        Self {
            assignments: Vec::new(),
        }
    }
}

impl<N> ObjectNode<N> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn assignments(&self) -> &[Assignment<N>] {
        &self.assignments
    }

    pub fn push(&mut self, assignment: Assignment<N>) {
        self.assignments.push(assignment);
    }

    pub fn get(&self, key: &str) -> Option<&Value<N>> {
        self.assignments
            .iter()
            .find(|assignment| assignment.property.key == key)
            .map(|assignment| &assignment.value)
    }
}
