//! Public authoring facade for Cargo-native Mendix applications.
//!
//! Application crates depend on `mxrs`; the smaller implementation crates
//! remain available for framework development, but are not part of the
//! normal application-facing dependency surface.

pub use mxrs_dsl::{
    ButtonBuilder, CallArgument, ConstantBuilder, ContainerBuilder, DataViewBuilder, EntityBuilder,
    EnumerationBuilder, FlowBuilder, FlowParameterBuilder, LayoutBuilder, LayoutGridBuilder,
    LayoutGridColumnBuilder, LayoutGridRowBuilder, MenuBuilder, MenuItemBuilder,
    MicroflowModuleBuilder, ModuleBuilder, NanoflowModuleBuilder, NavigationBuilder,
    NavigationItemBuilder, NavigationProfileBuilder, PageBuilder, PluggableWidgetBuilder,
    ProjectBuilder, ScheduledEventBuilder, SecurityBuilder, TaskQueueBuilder, UserRoleBuilder,
};
pub use mxrs_expr::*;
pub use mxrs_ir::*;
#[doc(hidden)]
pub use mxrs_macros::project_facade as __project;
pub use mxrs_macros::{MxEntity, MxEnumeration, application};

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
    capture_imported_project, capture_project_assets, materialize_project_assets,
    read_imported_manifest, rebuild_imported_project, replace_imported_project,
    restore_imported_project,
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
pub use mxrs_writer::{synchronize_project, write_project};

/// Implemented by `#[mxrs::application]` for the root application type.
pub trait ApplicationDefinition {
    const MENDIX_VERSION: &'static str;

    fn declaration() -> ProjectDecl;
}

/// Imports commonly used authoring types and macros.
pub mod prelude {
    pub use crate::{
        ApplicationDefinition, AssociationMarker, AttributeMarker, ButtonBuilder, CallArgument,
        ConstantBuilder, ConstantType, ContainerBuilder, DataViewBuilder, EntityMarker,
        EnumerationBuilder, EnumerationMarker, Expr, FlowBuilder, LayoutGridBuilder, MendixType,
        MenuActionDecl, MenuBuilder, MenuIconDecl, MenuItemBuilder, MicroflowMarker,
        MicroflowModuleBuilder, MicroflowRef, ModuleBuilder, MxBool, MxDecimal, MxEntity,
        MxEnumeration, MxFloat, MxInteger, MxLong, MxString, NanoflowMarker, NanoflowModuleBuilder,
        NanoflowRef, NavigationBuilder, NavigationItemBuilder, NavigationProfileBuilder, OnOverlap,
        PageBuilder, ProjectBuilder, Ref, Reference, ReferenceSet, RenderExpr, ScheduleUnit,
        ScheduledEventBuilder, SecurityBuilder, SecurityLevel, TaskQueueBuilder, TaskQueueConfig,
        TaskQueueScope, TypedAttributeMarker, UserRoleBuilder, Var, application, boolean, decimal,
        float, integer, long, project, string,
    };
}
