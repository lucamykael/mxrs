//! Compiles project shell and page infrastructure documents into Mendix
//! Runtime shape, sibling to `mxrs-compiler-domain`/`mxrs-compiler-flow`.
//! The ported surface covers:
//! - `lib/mxrb/compiler/navigation_document_compiler.rb` (93 lines) — `navigation`
//! - `lib/mxrb/compiler/settings_document_compiler.rb` (45 lines) — `settings`
//! - `lib/mxrb/compiler/artifact_document_compiler.rb` (102 lines) — `artifact`
//!   (plus `compiler/model_values.rb#image_format`/`#image_bytes` — `image_format`)
//! - `page_document_compiler.rb` — page Runtime metadata
//! - `web_operation_compiler.rb` — the project-wide operation catalog
//! - `web_list_data_source.rb` — shared modern-widget list-source resolver
//! - `gallery_bundle_compiler.rb` — official React Gallery properties
//! - `data_grid_bundle_compiler.rb` — Data Grid 2 properties and columns
//! - `combo_box_bundle_compiler.rb` — association/database/enumeration Combo Box
//! - `image_bundle_compiler.rb` — static and dynamically-bound Image properties
//! - `generic_widget_bundle_compiler.rb` — schema-driven pluggable fallback
//! - `page_bundle_compiler.rb` — page/layout module emitter and bundle dispatch
//!
//! The emitter also renders structural/static Forms widgets directly
//! (layout grids, containers, tables, tabs, snippets, labels and text) while
//! keeping an override hook for richer native-widget implementations.

mod artifact;
mod combo_box_bundle;
mod data_grid_bundle;
mod gallery_bundle;
mod generic_widget_bundle;
mod image_bundle;
mod image_format;
mod legacy_widget_bundle;
mod navigation;
mod page_bundle;
mod page_document;
mod settings;
mod web_list_data_source;
mod web_operation;

use mxrs_bson::Document;
use mxrs_schema::RuntimeModelSchema;

pub use artifact::{ArtifactCompiler, NAME_ONLY_TYPES, TYPES};
pub use combo_box_bundle::{COMBO_BOX_WIDGET_ID, ComboBoxBundleCompiler};
pub use data_grid_bundle::DataGridBundleCompiler;
pub use gallery_bundle::{GALLERY_WIDGET_ID, GalleryBundleCompiler};
pub use generic_widget_bundle::GenericWidgetBundleCompiler;
pub use image_bundle::{IMAGE_WIDGET_ID, ImageBundleCompiler, StaticImageOptions};
pub use legacy_widget_bundle::LegacyWidgetBundleCompiler;
pub use mxrs_compiler_support::{menu_operation_id, operation_id, widget_data_source_id};
pub use navigation::NavigationCompiler;
pub use page_bundle::{PageBundle, PageBundleCompiler, ProjectPageBundleCompiler};
pub use page_document::PageDocumentCompiler;
pub use settings::SettingsCompiler;
pub use web_list_data_source::WebListDataSource;
pub use web_operation::{DATA_GRID_WIDGET_ID, WebOperationCompiler};

#[derive(Debug, thiserror::Error)]
pub enum CompilerError {
    #[error(transparent)]
    Flow(#[from] mxrs_compiler_flow::CompilerError),
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
    #[error("unsupported page root {type_name:?}")]
    UnsupportedPageRoot { type_name: String },
    #[error("invalid page slot {name:?}")]
    InvalidPageSlot { name: String },
    #[error("duplicate page slot {name:?}")]
    DuplicatePageSlot { name: String },
    #[error("unsupported Runtime data type {type_name:?}")]
    UnsupportedDataType { type_name: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Facade for document compilation and the project-wide operation catalog.
/// Use [`WidgetsCompiler::for_project`] when page security or cross-document
/// web-operation resolution is required; [`WidgetsCompiler::new`] remains a
/// lightweight path for isolated Navigation/Settings/Artifact compilation.
pub struct WidgetsCompiler {
    schema: RuntimeModelSchema,
    page: PageDocumentCompiler,
    web_operations: Option<WebOperationCompiler>,
}

impl WidgetsCompiler {
    /// `existing_runtime_documents` seeds `RuntimeModelSchema`'s ID-matched
    /// counterpart/observed-field lookups, exactly like `FlowCompiler::new`
    /// — pass an empty slice for a fresh compile with nothing to reconcile
    /// against.
    pub fn new(existing_runtime_documents: &[Document]) -> Result<Self, CompilerError> {
        Ok(WidgetsCompiler {
            schema: RuntimeModelSchema::for_11(existing_runtime_documents)?,
            page: PageDocumentCompiler::without_security(),
            web_operations: None,
        })
    }

    pub fn for_project(
        project: &mxrs_model::Project,
        existing_runtime_documents: &[Document],
    ) -> Result<Self, CompilerError> {
        Ok(Self {
            schema: RuntimeModelSchema::for_11(existing_runtime_documents)?,
            page: PageDocumentCompiler::new(project)?,
            web_operations: Some(WebOperationCompiler::new(project)?),
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

    pub fn compile_page(
        &self,
        source: &Document,
        module_name: &str,
    ) -> Result<Document, CompilerError> {
        self.page.compile(source, module_name)
    }

    pub fn compile_web_operations(&self) -> Vec<Document> {
        self.web_operations
            .as_ref()
            .map(WebOperationCompiler::compile)
            .unwrap_or_default()
    }
}
