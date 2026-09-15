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
pub mod task_queue;
pub use task_queue::{TaskQueueConfig, TaskQueueDecl, TaskQueueScope};

pub use application::{
    ModuleRoleDecl, NavigationDecl, NavigationIconDecl, NavigationItemDecl, NavigationProfileDecl,
    PasswordPolicyDecl, ProjectSecurityDecl, RoleHomeDecl, SecurityLevel, UserRoleDecl,
};
pub use declaration::{
    AccessMemberKind, AccessRuleDecl, AssociationDecl, AssociationOwner, AssociationStorage,
    AssociationType, AttributeDecl, AttributeType, ConstantDecl, ConstantType, EntityDecl,
    EntityImageDecl, EntityIndexDecl, EntityInheritanceDecl, EntitySourceDecl, EnumerationDecl,
    EnumerationValueDecl, ExportLevel, IndexMemberDecl, LifecycleDecl, LifecycleEvent,
    LocalizedText, MemberAccessDecl, MemberRights, MenuActionDecl, MenuDecl, MenuIconDecl,
    MenuItemDecl, ModuleDecl, OnOverlap, OqlViewSourceDecl, ProjectDecl, RegularExpressionDecl,
    ScheduleUnit, ScheduledEventDecl, ScheduledEventSchedule, SystemMember, SystemMembersDecl,
};
pub use flow::{Activity, FlowParameterDecl, Member, MicroflowCallMapping, MicroflowDecl};
pub use markers::system;
pub use markers::{
    AssociationMarker, AttributeMarker, EntityMarker, EnumerationMarker, MicroflowMarker,
    MicroflowRef, NanoflowMarker, NanoflowRef, Ref, Reference, ReferenceSet,
};
pub use page::{
    ButtonAction, DataSourceDecl, LayoutDecl, LayoutGridColumnDecl, LayoutGridRowDecl, LayoutKind,
    LayoutRef, PageDecl, PageParameterDecl, WidgetDecl,
};
