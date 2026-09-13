//! Typed Rust builder API (v1) for declaring a Mendix project — domain
//! model (modules/entities/attributes/associations) plus a minimal
//! microflow-body subset — producing an `mxrs_ir::ProjectDecl` for
//! `mxrs-writer` to persist into a `.mpr`.
//!
//! This is a from-scratch design, not a port of `dsl/builder.rb`'s Ruby
//! `instance_eval` block syntax: per the plan's locked Phase 3 decision
//! (`decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory),
//! nested builders are configured via a closure taking `&mut Builder`
//! explicitly (`module.entity("Order", |e| { e.string("Number"); })`), never
//! ambient `self` — the one deliberate deviation from Ruby's ergonomics,
//! locked here before Phase 4 (compile-time reference checking) and Phase 8
//! (macro sugar) build on top of it.
//!
//! ```
//! use mxrs_dsl::{ProjectBuilder, string};
//! use mxrs_expr::{MxString, TypedAttributeMarker, attribute};
//! use mxrs_ir::{AttributeMarker, EntityMarker, Ref};
//!
//! struct Order;
//! impl EntityMarker for Order {
//!     const MODULE: &'static str = "Sales";
//!     const NAME: &'static str = "Order";
//! }
//! struct OrderNumber;
//! impl AttributeMarker for OrderNumber {
//!     type Entity = Order;
//!     const NAME: &'static str = "Number";
//! }
//! impl TypedAttributeMarker for OrderNumber { type Value = MxString; }
//!
//! let mut project = ProjectBuilder::new("11.12.1");
//! project.module("Sales", |m| {
//!     m.entity("Order", |e| {
//!         e.string("Number").default_value = Some("A-0000".into());
//!         e.decimal("Total");
//!     });
//!     m.microflow("ACT_CreateOrder", |f| {
//!         let order = f.create_object(
//!             "order",
//!             Ref::<Order>::new(),
//!             vec![attribute::<OrderNumber>(string("A-1"))],
//!             true,
//!         );
//!         f.return_value(order);
//!     });
//! });
//! let definition = project.build();
//! assert_eq!(definition.modules[0].entities[0].name, "Order");
//! ```

mod entity;
mod enumeration;
mod flow;
mod module;
pub mod page;
mod project;

pub use entity::EntityBuilder;
pub use enumeration::EnumerationBuilder;
pub use flow::{CallArgument, FlowBuilder};
pub use module::ModuleBuilder;
pub use mxrs_expr::{
    Expr, ListVar, MxBool, MxDecimal, MxFloat, MxInteger, MxLong, MxString, Var, boolean, decimal,
    float, integer, long, string,
};
pub use page::{
    ButtonBuilder, ContainerBuilder, LayoutGridBuilder, LayoutGridColumnBuilder,
    LayoutGridRowBuilder, PageBuilder,
};
pub use project::ProjectBuilder;
