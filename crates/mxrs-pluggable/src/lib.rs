//! Schema model and BSON schema decode for Mendix pluggable
//! (`CustomWidgets`) widget packages — the mechanism Data Grid 2, Gallery,
//! and ComboBox are built on even for a trivial Mendix 11 page (confirmed
//! directly: `mxrs-forms`'s own crate doc notes encountering one is a
//! deliberate, explicit error today, not silent data loss).
//!
//! Ports the schema half of `lib/mxrb/pluggable/{catalog,mpr_codec}.rb`
//! from mxrb. See `mpr_codec`'s module doc for exactly what's deferred
//! (the widget-*instance* decode/encode half, which needs an instance
//! value model this crate doesn't build yet plus a small expansion of
//! `mxrs-forms`'s public surface) and why.
//!
//! Deliberately not ported at all: `schema_dsl.rb`/`schema_source_emitter.rb`
//! (Ruby-authoring DSL + Ruby codegen for hand-writing widget schemas —
//! `mxrs-dsl`'s job if ever needed, same boundary every other crate in this
//! workspace draws against mxrb's Ruby ergonomics/codegen).

pub mod catalog;
pub mod error;
pub mod mpr_codec;

pub use catalog::{
    ActionVariable, Catalog, EnumerationValue, ObjectType, PropertyType, ReturnType, Translation,
    ValueType, WidgetType,
};
pub use error::{PluggableError, Result};
pub use mpr_codec::{SchemaContext, decode_widget_type};
