//! Public authoring facade for Cargo-native Mendix applications.
//!
//! Application crates depend on `mxrs`; the smaller implementation crates
//! remain available for framework development, but are not part of the
//! normal application-facing dependency surface.

pub use mxrs_dsl::{CallArgument, EntityBuilder, FlowBuilder, ModuleBuilder, ProjectBuilder};
pub use mxrs_expr::*;
pub use mxrs_ir::*;
pub use mxrs_macros::{MxEntity, project};
pub use mxrs_project::{
    ImportedProjectManifest, ImportedUnit, ProjectError, capture_imported_project,
    read_imported_manifest, rebuild_imported_project,
};

/// Imports commonly used authoring types and macros.
pub mod prelude {
    pub use crate::{
        AssociationMarker, AttributeMarker, CallArgument, EntityMarker, Expr, FlowBuilder,
        MendixType, MicroflowMarker, MicroflowRef, ModuleBuilder, MxBool, MxDecimal, MxEntity,
        MxFloat, MxInteger, MxLong, MxString, ProjectBuilder, Ref, RenderExpr,
        TypedAttributeMarker, Var, boolean, decimal, float, integer, long, project, string,
    };
}
