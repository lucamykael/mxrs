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

mod access;
mod constant;
mod data_set;
mod entity;
mod enumeration;
mod flow;
pub mod flow_actions;
mod image;
mod javascript_action;
mod json_structure;
mod mapping;
mod menu;
mod module;
mod navigation;
pub mod page;
mod project;
mod regular_expression;
mod rest;
mod scheduled_event;
mod security;
mod task_queue;
pub use mxrs_ir::{TaskQueueConfig, TaskQueueScope};
pub use task_queue::TaskQueueBuilder;

pub use access::AccessRuleBuilder;
pub use constant::ConstantBuilder;
pub use data_set::DataSetBuilder;
pub use entity::{EntityBuilder, EntityIndexBuilder, LifecycleBuilder, SystemMembersBuilder};
pub use enumeration::EnumerationBuilder;
pub use flow::{
    CallArgument, FlowBuilder, FlowParameterBuilder, MicroflowModuleBuilder, NanoflowModuleBuilder,
    RuleModuleBuilder,
};
pub use flow_actions::{
    ActivityValue, AggregateFunction, AssociationName, AttributeName, ChangeKind, Commit,
    EntityName, FlowVar, ListChange, LogSeverity, MemberName, MessageKind, MicroflowName,
    NanoflowName, SortOrder, Variable, var,
};
pub use image::ImageCollectionBuilder;
pub use javascript_action::{CodeActionValue, JavaActionBuilder, JavaScriptActionBuilder};
pub use json_structure::{JsonElementBuilder, JsonStructureBuilder};
pub use mapping::{MappingBuilder, ObjectMappingBuilder, ValueMappingBuilder};
pub use menu::{MenuBuilder, MenuItemBuilder};
pub use module::{ModuleBuilder, OqlViewSourceBuilder};
pub use mxrs_expr::{
    Expr, ListVar, MxBool, MxDecimal, MxFloat, MxInteger, MxLong, MxString, Var, boolean, decimal,
    float, integer, long, string,
};
pub use mxrs_expr::{Mx, mx};
pub use mxrs_ir::DataType;
pub use mxrs_ir::{
    ConstantType, ExportLevel, MemberRights, MenuActionDecl, MenuIconDecl, OnOverlap, ScheduleUnit,
    ScheduledEventSchedule, SecurityLevel,
};
pub use navigation::{NavigationBuilder, NavigationItemBuilder, NavigationProfileBuilder};
pub use page::{
    ButtonBuilder, ContainerBuilder, DataViewBuilder, LayoutBuilder, LayoutGridBuilder,
    LayoutGridColumnBuilder, LayoutGridRowBuilder, PageBuilder, PluggableWidgetBuilder,
};
pub use project::ProjectBuilder;
pub use regular_expression::RegularExpressionBuilder;
pub use rest::{PublishedRestServiceBuilder, RestOperationBuilder, RestResourceBuilder};
pub use scheduled_event::ScheduledEventBuilder;
pub use security::{DemoUserBuilder, SecurityBuilder, UserRoleBuilder};
