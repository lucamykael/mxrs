//! Mendix version/schema registry, scoped to 11.x per the locked MVP
//! decision — see `decisions/mxrs-rust-rewrite-plan.md` (milestone M1.6)
//! in this project's ai-memory. Widening to the full 5.21-11.12 matrix
//! mxrb supports means adding version entries to each module here, not
//! restructuring them.
//!
//! Ports `lib/mxrb/schema/tables.rb`, `lib/mxrb/studio_compatibility.rb`,
//! and `lib/mxrb/compiler/schemas/{runtime-11.json,system-model-11.12.1.b64}`
//! from mxrb.

pub mod assets;
pub mod compatibility;
pub mod model_package;
pub mod project_template;
pub mod runtime_model;
pub mod system_model;
pub mod tables;

pub use assets::{RUNTIME_SCHEMA_11, SYSTEM_MODEL_SEED_11_12_1};
pub use compatibility::{apply_document, schema_hash, SCHEMA_HASH_11_12_1};
pub use model_package::{read_model_package, ModelPackageError};
pub use project_template::{project_template_units, ProjectTemplateError, TemplateUnit};
pub use runtime_model::{RuntimeModelError, RuntimeModelSchema};
pub use system_model::{system_model_documents, SystemModelError, SYSTEM_MODULE_ID};
pub use tables::{table_info, TableInfo, ATTRIBUTE_TYPES, TABLES, UNIT_TYPES};
