//! Public authoring facade for Cargo-native Mendix applications.
//!
//! Application crates depend on `mxrs`; the smaller implementation crates
//! remain available for framework development, but are not part of the
//! normal application-facing dependency surface.

pub use mxrs_dsl::{
    ButtonBuilder, CallArgument, ContainerBuilder, DataViewBuilder, EntityBuilder,
    EnumerationBuilder, FlowBuilder, LayoutGridBuilder, LayoutGridColumnBuilder,
    LayoutGridRowBuilder, ModuleBuilder, NavigationBuilder, NavigationItemBuilder,
    NavigationProfileBuilder, PageBuilder, PluggableWidgetBuilder, ProjectBuilder, SecurityBuilder,
    UserRoleBuilder,
};
pub use mxrs_expr::*;
pub use mxrs_ir::*;
pub use mxrs_macros::{MxEntity, MxEnumeration, application, project};
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
    ImportedDocumentRef, ImportedProjectManifest, ImportedUnit, PROJECT_ASSET_DIRECTORIES,
    ProjectError, capture_imported_project, capture_project_assets, materialize_project_assets,
    read_imported_manifest, rebuild_imported_project, replace_imported_project,
    restore_imported_project,
};
pub use mxrs_runtime::{
    Action, EntityAction, EntityRule, MemberRight, ObjectValue, Runtime, RuntimeError,
    SecurityContext, SecurityPolicy, Store, StoreSchema,
};
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
        ContainerBuilder, DataViewBuilder, EntityMarker, EnumerationBuilder, EnumerationMarker,
        Expr, FlowBuilder, LayoutGridBuilder, MendixType, MicroflowMarker, MicroflowRef,
        ModuleBuilder, MxBool, MxDecimal, MxEntity, MxEnumeration, MxFloat, MxInteger, MxLong,
        MxString, NanoflowMarker, NanoflowRef, NavigationBuilder, NavigationItemBuilder,
        NavigationProfileBuilder, PageBuilder, ProjectBuilder, Ref, Reference, ReferenceSet,
        RenderExpr, SecurityBuilder, SecurityLevel, TypedAttributeMarker, UserRoleBuilder, Var,
        application, boolean, decimal, float, integer, long, project, string,
    };
}
