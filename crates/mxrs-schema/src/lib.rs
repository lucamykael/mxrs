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
pub use compatibility::{SCHEMA_HASH_11_12_1, apply_document, schema_hash};
pub use model_package::{ModelPackage, ModelPackageEntry, ModelPackageError, read_model_package};
pub use project_template::{ProjectTemplateError, TemplateUnit, project_template_units};
pub use runtime_model::{RuntimeModelError, RuntimeModelSchema};
pub use system_model::{SYSTEM_MODULE_ID, SystemModelError, system_model_documents};
pub use tables::{ATTRIBUTE_TYPES, TABLES, TableInfo, UNIT_TYPES, table_info};
