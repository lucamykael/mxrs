//! Storage-independent declaration types shared by `mxrs-dsl` (which
//! constructs them from a user-authored Rust program) and `mxrs-writer`
//! (which compiles them into `.mpr` BSON). See `decisions/mxrs-rust-rewrite-plan.md`
//! in this project's ai-memory for the surrounding Phase 3 plan.
//!
//! This crate deliberately has no dependency on `mxrs-model`, `mxrs-bson`,
//! or the `.mpr` storage layer. Its declarations contain only concepts an
//! author can name. `mxrs-writer` owns the one-way lowering into the
//! storage-oriented model structs and assigns identities to new artifacts.

pub mod application;
pub mod declaration;
pub mod flow;
pub mod markers;
pub mod page;

pub use application::{
    ModuleRoleDecl, NavigationDecl, NavigationItemDecl, NavigationProfileDecl, PasswordPolicyDecl,
    ProjectSecurityDecl, RoleHomeDecl, SecurityLevel, UserRoleDecl,
};
pub use declaration::{
    AssociationDecl, AssociationOwner, AssociationStorage, AssociationType, AttributeDecl,
    AttributeType, EntityDecl, EnumerationDecl, EnumerationValueDecl, ModuleDecl, ProjectDecl,
};
pub use flow::{Activity, Member, MicroflowCallMapping, MicroflowDecl};
pub use markers::{
    AssociationMarker, AttributeMarker, EntityMarker, EnumerationMarker, MicroflowMarker,
    MicroflowRef, NanoflowMarker, NanoflowRef, Ref, Reference, ReferenceSet,
};
pub use page::{
    ButtonAction, DataSourceDecl, LayoutDecl, LayoutGridColumnDecl, LayoutGridRowDecl, LayoutKind,
    LayoutRef, PageDecl, PageParameterDecl, WidgetDecl,
};
