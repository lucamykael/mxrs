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
pub mod data_set;
pub mod declaration;
pub mod flow;
pub mod image;
pub mod javascript_action;
pub mod json_structure;
pub mod mapping;
pub mod markers;
pub mod page;
pub mod task_queue;
pub use task_queue::{TaskQueueConfig, TaskQueueDecl, TaskQueueScope};

pub use application::{
    DemoUserDecl, ModuleRoleDecl, NavigationDecl, NavigationIconDecl, NavigationItemDecl,
    NavigationProfileDecl, PasswordPolicyDecl, ProjectSecurityDecl, RoleHomeDecl, SecurityLevel,
    UserRoleDecl,
};
pub use data_set::{DataSetDecl, DataSetParameter, DataSetParameterType};
pub use declaration::{
    AccessMemberKind, AccessRuleDecl, AssociationDecl, AssociationOwner, AssociationStorage,
    AssociationType, AttributeDecl, AttributeType, ConstantDecl, ConstantType, EntityDecl,
    EntityImageDecl, EntityIndexDecl, EntityInheritanceDecl, EntitySourceDecl, EnumerationDecl,
    EnumerationValueDecl, ExportLevel, IndexMemberDecl, LifecycleDecl, LifecycleEvent,
    LocalizedText, MemberAccessDecl, MemberRights, MenuActionDecl, MenuDecl, MenuIconDecl,
    MenuItemDecl, ModuleDecl, OnOverlap, OqlViewSourceDecl, ProjectDecl, RegularExpressionDecl,
    ScheduleUnit, ScheduledEventDecl, ScheduledEventSchedule, SystemMember, SystemMembersDecl,
};
pub use flow::{
    Activity, DataType, ErrorHandling, FlowParameterDecl, FlowRelations, Member,
    MicroflowCallMapping, MicroflowDecl, NativeDocument, NativeValue, SwitchCase,
};
pub use image::{ImageCollectionDecl, ImageDecl, ImageFormat};
pub use javascript_action::{
    CodeActionParameter, CodeActionType, JavaActionDecl, JavaScriptActionDecl, JavaScriptPlatform,
};
pub use json_structure::{JsonElement, JsonElementType, JsonPrimitiveType, JsonStructureDecl};
pub use mapping::{
    MappingAssociation, MappingDecl, MappingDirection, MappingElement, MappingElementKind,
    MappingValueType, NullValueOption, ObjectHandling, ObjectMapping, ValueMapping,
};
pub use markers::system;
pub use markers::{
    AssociationMarker, AssociationRef, AttributeMarker, AttributeRef, EntityMarker,
    EnumerationMarker, FlowName, MicroflowMarker, MicroflowRef, NanoflowMarker, NanoflowRef, Ref,
    Reference, ReferenceSet,
};
pub use page::{
    ButtonAction, DataSourceDecl, FormDecl, LayoutDecl, LayoutGridColumnDecl, LayoutGridRowDecl,
    LayoutKind, LayoutRef, PageDecl, PageParameterDecl, WidgetDecl,
};
