//! Compiles editor-shape flow/action documents into Mendix Runtime shape —
//! second crate of Phase 5's *compiler* pipeline (see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory),
//! sibling to `mxrs-compiler-domain`. Ports six `mxrb` files:
//!
//! - `lib/mxrb/compiler/runtime_data_types.rb` (31 lines) — `types::data_type`
//! - `lib/mxrb/compiler/code_action_type_compiler.rb` (32 lines) — `types::code_action_type`
//! - `lib/mxrb/compiler/code_action_document_compiler.rb` (64 lines) — `code_action`
//! - `lib/mxrb/compiler/database_connector_action_compiler.rb` (280 lines) — `database_connector`
//! - `lib/mxrb/compiler/microflow_node_compiler.rb` (254 lines) — `node`
//! - `lib/mxrb/compiler/microflow_document_compiler.rb` (85 lines) — `document`
//! - `lib/mxrb/compiler/nanoflow_program_compiler.rb` (483 lines) — `nanoflow` (a
//!   distinct output target: client-side JS source, not BSON — see that
//!   module's own doc comment for why it has no `CompilerError` variant of
//!   its own)

mod code_action;
mod database_connector;
mod document;
mod facade;
pub mod nanoflow;
mod node;
mod support;
mod types;

pub use code_action::CodeActionCompiler;
pub use database_connector::DatabaseConnectorCompiler;
pub use document::FlowDocumentCompiler;
pub use facade::FlowCompiler;
pub use node::{FlowDiagnostic, FlowNodeCompiler};
pub use support::ProjectFlowIndex;
pub use types::{code_action_type, data_type};

#[derive(Debug, thiserror::Error)]
pub enum CompilerError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error(transparent)]
    Schema(#[from] mxrs_schema::RuntimeModelError),
    #[error(transparent)]
    SystemModel(#[from] mxrs_schema::SystemModelError),

    // runtime_data_types.rb
    #[error("unsupported Runtime data type {type_name:?}")]
    UnsupportedDataType { type_name: String },

    // code_action_type_compiler.rb
    #[error("unsupported code-action type {type_name:?}")]
    UnsupportedCodeActionType { type_name: String },

    // code_action_document_compiler.rb
    #[error("unsupported code action {type_name:?}")]
    UnsupportedCodeAction { type_name: String },
    #[error("code action {name:?} is outside a module")]
    CodeActionOutsideModule { name: String },

    // database_connector_action_compiler.rb
    #[error("database connection {connection_name:?} not found")]
    UnknownDatabaseConnection { connection_name: String },
    #[error("database query {qualified_name:?} not found")]
    UnknownDatabaseQuery { qualified_name: String },
    #[error("database query builder has no executable table and column mapping")]
    UnbuildableDatabaseQuery,
    #[error("unsafe database identifier {identifier:?}")]
    UnsafeDatabaseIdentifier { identifier: String },
    #[error("unsupported database parameter type {type_name:?}")]
    UnsupportedDatabaseParameterType { type_name: String },

    // microflow_node_compiler.rb
    #[error("cannot derive Runtime field {type_name}.{field}")]
    CannotDeriveRuntimeField { type_name: String, field: String },
    #[error("cannot derive {aggregate_function} aggregate type without an audited attribute type")]
    CannotDeriveAggregateType { aggregate_function: String },
    #[error("cannot derive Runtime retrieve type for {type_name:?}")]
    CannotDeriveRetrieveType { type_name: String },
    #[error("unknown association {association_id:?}")]
    UnknownAssociation { association_id: String },

    // microflow_document_compiler.rb
    #[error("unsupported flow root {type_name:?}")]
    UnsupportedFlowRoot { type_name: String },
    #[error("cannot derive Runtime root field {type_name}.{field}")]
    CannotDeriveRuntimeRootField { type_name: String, field: String },
}
