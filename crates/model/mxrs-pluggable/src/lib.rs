//! Typed model and bidirectional BSON codec for Mendix pluggable
//! (`CustomWidgets`) widget packages — the mechanism Data Grid 2, Gallery,
//! ComboBox, and third-party custom widgets use in Mendix 11.
//!
//! Ports `lib/mxrb/pluggable/{catalog,mpr_codec}.rb` from mxrb: schema
//! decode plus instance decode for every value kind mxrb's own
//! `Pluggable::MprCodec#decode_value` supports, via a dependency-inversion
//! seam ([`embedded::EmbeddedFormsDecoder`]) that `mxrs-forms::MprCodec`
//! implements. The symmetric writer uses
//! [`embedded::EmbeddedFormsEncoder`]; schema, object, nested widget, and
//! every value kind now round-trip through the typed representation. See
//! `mpr_codec`'s module doc for the architecture and real-project oracle
//! results.
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
pub use embedded::{EmbeddedFormsDecoder, EmbeddedFormsEncoder};
pub use error::{PluggableError, Result};
pub use mpr_codec::{
    EncodeContext, SchemaContext, decode_object, decode_widget, decode_widget_type, encode_object,
    encode_widget, encode_widget_type,
};
pub use node::{
    Assignment, DataSource, DataSourceValue, ObjectNode, ReferenceTarget, Value, WidgetItem,
    WidgetNode,
};
