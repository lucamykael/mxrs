//! The model's integration mappings, as declarations a generated project can
//! read and a runtime boundary can apply.
//!
//! An export mapping describes the JSON document a Mendix object graph turns
//! into: which attributes appear, under which keys, and which associated
//! objects nest beneath them. A published REST operation that declares one
//! answers with that document instead of the entity's stored attributes, so
//! the generated project keeps a mapping declaration per mapping the model
//! publishes and hands it to [`ExportMapping::apply`] at the boundary.
//!
//! ```
//! use mxrs::mapping::{ExportMapping, ObjectMapping};
//!
//! // `(Array)|(Object)` at the root, one key renamed, one associated object
//! // nested — exactly what the model's element tree declares.
//! let mapping = ExportMapping::new(
//!     ObjectMapping::array("Sales.Order")
//!         .attribute("OrderNumber")
//!         .value("placed_at", "OrderDate")
//!         .child(
//!             "customer",
//!             "Sales.Order_Customer",
//!             ObjectMapping::object("Sales.Customer").attribute("Name"),
//!         ),
//! );
//! assert!(mapping.root().is_multiple());
//! ```

pub use mxrs_runtime_flows::{ExportMapping, NullValues, ObjectMapping, ValueMapping};
