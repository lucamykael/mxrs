//! Public authoring facade for Cargo-native Mendix applications.
//!
//! Application crates depend on `mxrs`; the smaller implementation crates
//! remain available for framework development, but are not part of the
//! normal application-facing dependency surface.

pub use mxrs_dsl::{
    ButtonBuilder, CallArgument, ContainerBuilder, EntityBuilder, EnumerationBuilder, FlowBuilder,
    LayoutGridBuilder, LayoutGridColumnBuilder, LayoutGridRowBuilder, ModuleBuilder, PageBuilder,
    ProjectBuilder,
};
pub use mxrs_expr::*;
pub use mxrs_ir::*;
pub use mxrs_macros::{MxEntity, MxEnumeration, application, project};
pub use mxrs_project::{
    ImportedDocumentRef, ImportedProjectManifest, ImportedUnit, PROJECT_ASSET_DIRECTORIES,
    ProjectError, capture_imported_project, capture_project_assets, materialize_project_assets,
    read_imported_manifest, rebuild_imported_project, replace_imported_project,
    restore_imported_project,
};

/// Implemented by `#[mxrs::application]` for the root application type.
pub trait ApplicationDefinition {
    const MENDIX_VERSION: &'static str;

    fn declaration() -> ProjectDecl;
}

/// Imports commonly used authoring types and macros.
pub mod prelude {
    pub use crate::{
        ApplicationDefinition, AssociationMarker, AttributeMarker, ButtonBuilder, CallArgument,
        ContainerBuilder, EntityMarker, EnumerationBuilder, EnumerationMarker, Expr, FlowBuilder,
        LayoutGridBuilder, MendixType, MicroflowMarker, MicroflowRef, ModuleBuilder, MxBool,
        MxDecimal, MxEntity, MxEnumeration, MxFloat, MxInteger, MxLong, MxString, PageBuilder,
        ProjectBuilder, Ref, Reference, ReferenceSet, RenderExpr, TypedAttributeMarker, Var,
        application, boolean, decimal, float, integer, long, project, string,
    };
}
