//! Public authoring facade for Cargo-native Mendix applications.
//!
//! Application crates depend on `mxrs`; the smaller implementation crates
//! remain available for framework development, but are not part of the
//! normal application-facing dependency surface.

pub use mxrs_dsl::flow_actions;
pub use mxrs_dsl::{
    ActivityValue, AggregateFunction, AssociationName, AttributeName, ChangeKind, Commit,
    EntityName, FlowVar, ListChange, LogSeverity, MemberName, MessageKind, MicroflowName,
    NanoflowName, SortOrder, Variable, var,
};
pub use mxrs_dsl::{
    ButtonBuilder, CallArgument, ConstantBuilder, ContainerBuilder, DataViewBuilder,
    DemoUserBuilder, EntityBuilder, EnumerationBuilder, FlowBuilder, FlowParameterBuilder,
    LayoutBuilder, LayoutGridBuilder, LayoutGridColumnBuilder, LayoutGridRowBuilder, MenuBuilder,
    MenuItemBuilder, MicroflowModuleBuilder, ModuleBuilder, NanoflowModuleBuilder,
    NavigationBuilder, NavigationItemBuilder, NavigationProfileBuilder, PageBuilder,
    PluggableWidgetBuilder, ProjectBuilder, ScheduledEventBuilder, SecurityBuilder,
    TaskQueueBuilder, UserRoleBuilder,
};
pub use mxrs_expr::*;
pub use mxrs_frontend::{FrontendDecl, FrontendError, read_frontend};
pub use mxrs_ir::*;
#[doc(hidden)]
pub use mxrs_macros::project_facade as __project;
pub use mxrs_macros::{
    MxEntity, MxEnumeration, aggregate_list, application, call_java_action, call_javascript_action,
    call_microflow, call_nanoflow, change_list, change_object, change_variable, commit_object,
    constant, create_list, create_object, create_variable, declaration, delete_object, demo_user,
    dto, entity, enumeration, layout, log, menu, microflow, module_roles, nanoflow, navigation,
    navigation_item, page, retrieve, rollback_object, route, security, service, view,
};

/// The collector declaration macros submit to. Re-exported so an application
/// crate depends on `mxrs` alone.
#[doc(hidden)]
pub use inventory;

/// Declares a project using the public `mxrs` authoring surface.
#[macro_export]
macro_rules! project {
    ($($input:tt)*) => {
        $crate::__project! { ($crate) $($input)* }
    };
}
pub use mxrs_materializers::{
    MaterializeError, MaterializeReport, application_manifest, embedded_asset_hashes,
    embedded_frontend_source_hash, materialize_frontend_sources, materialize_manifest,
    materialize_mpr,
};
pub use mxrs_oql::{
    Dialect as OqlDialect, Finding as OqlFinding, OqlError, Projection as OqlProjection,
    Query as OqlQuery, analyze as analyze_oql, catalog as oql_catalog,
    parameters as oql_parameters, translate as translate_oql,
};
pub use mxrs_packager::{
    Entrypoints as PackageEntrypoints, PackageError, PackageFile, PackageManifest, PackageOptions,
    PackageReport, package, verify_package,
};
pub use mxrs_project::{
    ImportedProjectManifest, ImportedUnit, PROJECT_ASSET_DIRECTORIES, ProjectError,
    capture_imported_project, capture_project_assets, materialize_java_sources,
    materialize_project_assets, read_imported_manifest, rebuild_imported_project,
    replace_imported_project, restore_imported_project,
};
pub use mxrs_runtime::{
    Action, EntityAction, EntityRule, MemberRight, ObjectValue, Runtime, RuntimeError,
    SecurityContext, SecurityPolicy, Store, StoreSchema,
};
pub use mxrs_runtime_http::{HttpError as RuntimeHttpError, RuntimeHttp};
pub use mxrs_runtime_sqlite::{SqliteRuntimeError, SqliteRuntimeStore};
pub use mxrs_scaffold::{
    MxrsDependency, ProjectScaffold, ScaffoldError, ScaffoldReport, generate_project,
};
pub use mxrs_semantic::{
    Artifact as SemanticArtifact, ArtifactKind as SemanticArtifactKind,
    Diagnostic as SemanticDiagnostic, Reference as SemanticReference,
    SearchHit as SemanticSearchHit, SemanticError, SemanticIndex,
};
pub use mxrs_writer::relations::relation_warnings as flow_relation_warnings;
pub use mxrs_writer::{synchronize_project, write_project};

pub mod mapping;
pub mod ports;
pub mod registry;

/// Registers a declaration assembled by hand.
///
/// The declaration attributes cover what a model usually holds; this is the
/// same registration for anything built directly against the project. The
/// first argument is the [`registry::Stage`] it belongs to.
///
/// ```
/// mxrs::register!(Document, |project| {
///     let mut module = mxrs::ModuleBuilder::new("Sales");
///     module.constant("Region", |constant| {
///         constant.value("EU");
///     });
///     project.merge_module(module.into_decl());
/// });
/// ```
#[macro_export]
macro_rules! register {
    ($stage:ident, $apply:expr $(,)?) => {
        $crate::inventory::submit! {
            $crate::registry::Declaration::new(
                $crate::registry::Stage::$stage,
                ::core::module_path!(),
                ::core::file!(),
                ::core::line!(),
                $apply,
            )
        }
    };
}

/// Names flows the project keeps in its imported model without declaring
/// them in Rust.
///
/// Each line declares a unit type carrying the flow's Mendix name, so the
/// flow can be called, bound to a button or attached to an entity event like
/// any flow written with `#[mxrs::microflow]`. A name Rust cannot spell
/// states the Mendix name after `=`.
///
/// ```
/// mxrs::imported! {
///     module = "Sales";
///     microflow ACT_ApproveOrder;
///     microflow type_ = "type";
///     nanoflow NAN_OpenOrder;
/// }
/// # use mxrs::MicroflowMarker;
/// assert_eq!(ACT_ApproveOrder::qualified_name(), "Sales.ACT_ApproveOrder");
/// assert_eq!(type_::qualified_name(), "Sales.type");
/// ```
#[macro_export]
macro_rules! imported {
    (module = $module:literal; $($kind:ident $marker:ident $(= $name:literal)?;)*) => {
        $($crate::__imported_flow!($kind, $module, $marker $(, $name)?);)*
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __imported_flow {
    ($kind:ident, $module:literal, $marker:ident) => {
        $crate::__imported_flow!($kind, $module, $marker, ::core::stringify!($marker));
    };
    (microflow, $module:literal, $marker:ident, $name:expr) => {
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, Copy)]
        pub struct $marker;
        impl $crate::MicroflowMarker for $marker {
            const MODULE: &'static str = $module;
            const NAME: &'static str = $name;
        }
        impl $crate::FlowName for $marker {
            fn flow_name() -> ::std::string::String {
                <Self as $crate::MicroflowMarker>::qualified_name()
            }
        }
    };
    (nanoflow, $module:literal, $marker:ident, $name:expr) => {
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, Copy)]
        pub struct $marker;
        impl $crate::NanoflowMarker for $marker {
            const MODULE: &'static str = $module;
            const NAME: &'static str = $name;
        }
        impl $crate::FlowName for $marker {
            fn flow_name() -> ::std::string::String {
                <Self as $crate::NanoflowMarker>::qualified_name()
            }
        }
    };
}

/// Implemented by `#[mxrs::application]` for the root application type.
pub trait ApplicationDefinition {
    const MENDIX_VERSION: &'static str;

    fn declaration() -> ProjectDecl;
}

/// Imports commonly used authoring types and macros.
pub mod prelude {
    pub use crate::{
        AggregateFunction, ChangeKind, Commit, DataType, FlowVar, ListChange, LogSeverity,
        MemberName, MessageKind, Mx, NativeDocument, NativeValue, SortOrder, Variable, mx, var,
    };
    pub use crate::{
        ApplicationDefinition, AssociationMarker, AttributeMarker, ButtonBuilder, CallArgument,
        ConstantBuilder, ConstantType, ContainerBuilder, DataViewBuilder, DemoUserBuilder,
        EntityMarker, EnumerationBuilder, EnumerationMarker, ExportLevel, Expr, FlowBuilder,
        LayoutGridBuilder, MendixType, MenuActionDecl, MenuBuilder, MenuIconDecl, MenuItemBuilder,
        MicroflowMarker, MicroflowModuleBuilder, MicroflowRef, ModuleBuilder, ModuleDecl, MxBool,
        MxDateTime, MxDecimal, MxEntity, MxEnumeration, MxFloat, MxInteger, MxLong, MxString,
        NanoflowMarker, NanoflowModuleBuilder, NanoflowRef, NavigationBuilder,
        NavigationItemBuilder, NavigationProfileBuilder, OnOverlap, PageBuilder, ProjectBuilder,
        Ref, Reference, ReferenceSet, RenderExpr, ScheduleUnit, ScheduledEventBuilder,
        ScheduledEventSchedule, SecurityBuilder, SecurityLevel, TaskQueueBuilder, TaskQueueConfig,
        TaskQueueScope, TypedAttributeMarker, UserRoleBuilder, Var, application, boolean, decimal,
        float, integer, long, project, string,
    };
    pub use crate::{
        AssignAssociation, AssignAttribute, AssociationRef, AttributeRef, LayoutBuilder,
        LifecycleEvent, MemberRights, MxBinary, MxList, MxObject, SystemMember, aggregate_list,
        call_java_action, call_javascript_action, call_microflow, call_nanoflow, change_list,
        change_object, change_variable, commit_object, constant, create_list, create_object,
        create_variable, declaration, delete_object, demo_user, dto, entity, enumeration, layout,
        log, menu, microflow, module_roles, nanoflow, navigation, navigation_item, page, retrieve,
        rollback_object, route, security, service, view,
    };
}
