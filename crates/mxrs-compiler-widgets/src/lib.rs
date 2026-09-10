//! Phase 5's third compiler crate — the "shell" sub-step of
//! `mxrs-compiler-widgets`'s four-part roadmap slice (see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory):
//! compiles Navigation/Settings/Artifact editor-shape documents into
//! Mendix Runtime shape, sibling to `mxrs-compiler-domain`/
//! `mxrs-compiler-flow`. Ports three `mxrb` files:
//!
//! - `lib/mxrb/compiler/navigation_document_compiler.rb` (93 lines) — `navigation`
//! - `lib/mxrb/compiler/settings_document_compiler.rb` (45 lines) — `settings`
//! - `lib/mxrb/compiler/artifact_document_compiler.rb` (102 lines) — `artifact`
//!   (plus `compiler/model_values.rb#image_format`/`#image_bytes` — `image_format`)
//!
//! Deliberately **not** in this slice (the next sub-step of the same
//! roadmap entry): `PageDocumentCompiler`, `WebOperationCompiler` (its
//! `operation_id`/`menu_operation_id` currently lives, temporarily, in
//! `mxrs-compiler-flow`'s `nanoflow` module — see that module's own doc
//! comment for why, and move it here once page compilation starts), and
//! every bundle compiler (`page_bundle_compiler.rb`/`legacy_page_builder.rb`
//! most of all — deliberately held to last in the roadmap, not this
//! crate's problem yet).

mod artifact;
mod image_format;
mod navigation;
mod settings;
mod support;

use mxrs_bson::Document;
use mxrs_schema::RuntimeModelSchema;

pub use artifact::{ArtifactCompiler, NAME_ONLY_TYPES, TYPES};
pub use navigation::NavigationCompiler;
pub use settings::SettingsCompiler;

#[derive(Debug, thiserror::Error)]
pub enum CompilerError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error(transparent)]
    Schema(#[from] mxrs_schema::RuntimeModelError),

    #[error("unsupported settings root {type_name:?}")]
    UnsupportedSettingsRoot { type_name: String },
    #[error("unsupported Runtime artifact {type_name:?}")]
    UnsupportedArtifactType { type_name: String },
    #[error("artifact {name:?} is outside a module")]
    ArtifactOutsideModule { name: String },
    #[error("cannot determine format for image {name:?}")]
    CannotDetermineImageFormat { name: String },
}

/// Project-scoped entry point, mirroring `mxrs_compiler_flow::FlowCompiler`'s
/// facade shape (build once, compile many). Unlike `FlowCompiler`, this
/// facade doesn't take a `&Project` — Navigation/Settings/Artifact each
/// compile a single already-located source document with no cross-flow
/// indexing needed; a project-wide accessor will be worth adding once page/
/// widget compilation (which *does* need cross-references, e.g. resolving
/// page/microflow targets) joins this crate.
pub struct WidgetsCompiler {
    schema: RuntimeModelSchema,
}

impl WidgetsCompiler {
    /// `existing_runtime_documents` seeds `RuntimeModelSchema`'s ID-matched
    /// counterpart/observed-field lookups, exactly like `FlowCompiler::new`
    /// — pass an empty slice for a fresh compile with nothing to reconcile
    /// against.
    pub fn new(existing_runtime_documents: &[Document]) -> Result<Self, CompilerError> {
        Ok(WidgetsCompiler {
            schema: RuntimeModelSchema::for_11(existing_runtime_documents)?,
        })
    }

    pub fn compile_navigation(&self, source: &Document) -> Result<Document, CompilerError> {
        NavigationCompiler::new(&self.schema).compile(source)
    }

    pub fn compile_settings(&self, source: &Document) -> Result<Document, CompilerError> {
        SettingsCompiler::new(&self.schema).compile(source)
    }

    pub fn compile_artifact(
        &self,
        source: &Document,
        module_name: Option<&str>,
    ) -> Result<Document, CompilerError> {
        ArtifactCompiler::compile(source, module_name)
    }
}
