//! Schema model and BSON schema decode for Mendix pluggable
//! (`CustomWidgets`) widget packages — the mechanism Data Grid 2, Gallery,
//! and ComboBox are built on even for a trivial Mendix 11 page (confirmed
//! directly: `mxrs-forms`'s own crate doc notes encountering one is a
//! deliberate, explicit error today, not silent data loss).
//!
//! Ports `lib/mxrb/pluggable/{catalog,mpr_codec}.rb` from mxrb: schema
//! decode plus instance decode for every value kind mxrb's own
//! `Pluggable::MprCodec#decode_value` supports, via a dependency-inversion
//! seam ([`embedded::EmbeddedFormsDecoder`]) that `mxrs-forms::MprCodec`
//! implements. See `mpr_codec`'s module doc for exactly which small slice
//! (non-self-contained `DataSource`/`Widgets` shapes) and which whole
//! side (`encode_object`/`encode_value`, the writer direction) are still
//! open, and the real numbers behind that.
//!
//! Deliberately not ported at all: `schema_dsl.rb`/`schema_source_emitter.rb`
//! (Ruby-authoring DSL + Ruby codegen for hand-writing widget schemas —
//! `mxrs-dsl`'s job if ever needed, same boundary every other crate in this
//! workspace draws against mxrb's Ruby ergonomics/codegen).

pub mod catalog;
pub mod embedded;
pub mod error;
pub mod mpr_codec;
pub mod node;

pub use catalog::{
    ActionVariable, Catalog, EnumerationValue, ObjectType, PropertyType, ReturnType, Translation,
    ValueType, WidgetType,
};
pub use embedded::EmbeddedFormsDecoder;
pub use error::{PluggableError, Result};
pub use mpr_codec::{SchemaContext, decode_object, decode_widget_type};
pub use node::{Assignment, ObjectNode, ReferenceTarget, Value};
