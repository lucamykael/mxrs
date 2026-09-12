//! Decoded *instance* of a pluggable widget's `Object` (its assigned
//! property values), keyed against the [`crate::catalog::ObjectType`]
//! schema already decoded for it. Ports the read side of
//! `Mxrb::Pluggable::ObjectNode` from `lib/mxrb/pluggable/node.rb` —
//! deliberately not the `method_missing`-based Ruby ergonomics on top of
//! it (`mxrs-dsl`'s job, same boundary `mxrs-forms`'s own `Node` already
//! draws).
//!
//! See [`crate::mpr_codec`]'s module doc for exactly which
//! [`Value`] kinds this can decode today and which still need
//! `mxrs-forms` to expose more of its private codec surface first.

use crate::catalog::PropertyType;

/// A decoded pluggable-widget property value. Every variant here is
/// self-contained (needs only this crate + `mxrs-bson`) — kinds whose
/// mxrb behavior delegates to `Forms::MprCodec`'s *private* helpers
/// (`Attribute`, `Entity`, an `Association` routed through `EntityRef`,
/// `TextTemplate`, `Action`, `Icon`, a `DataSource` beyond a bare source
/// variable, or a `Widgets` child that isn't itself a nested
/// `CustomWidgets$CustomWidget`) are not represented here — decoding one
/// returns [`crate::error::PluggableError::NeedsFormsIntegration`]
/// instead of a value, per this workspace's rule that an unported gap
/// must fail loudly, never guess or silently drop data.
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
    /// recurses into this crate's own `decode_widget_type`/`decode_object`
    /// (discarding the nested widget's own schema/outer-properties, same
    /// as `mxrs-forms::decode_pluggable` does for the top-level case); any
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
    /// crate's own schema/object decode (mxrb's `decode(item)` branch).
    Pluggable(ObjectNode<N>),
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
