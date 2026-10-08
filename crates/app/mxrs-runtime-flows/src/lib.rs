//! Native microflow/nanoflow interpreter for the MXRS runtime.
//!
//! Ports `lib/mxrb/runtime/native.rb`: the Mendix expression engine, the
//! sequence-flow graph walker (events, exclusive/inheritance splits, error
//! handlers with rollback, while/iterable loops), and the data activities
//! that execute directly against [`mxrs_runtime::Store`]. Anything the
//! native engine cannot execute is a named error, never a silent skip.
//!
//! Deliberate divergences from the oracle:
//! - mxrb's interpreter runs only the first activity of a loop body with
//!   two or more nodes (a known, unreported bug — the loop body's edges live
//!   in the document container's `Flows`); this port executes the whole
//!   body and pins that with a regression test.
//! - `[%CurrentDateTime%]` formats in UTC; mxrb uses the process-local zone.
//! - Lists are values, not Ruby object references: an iterable loop walks a
//!   snapshot of the list, `ChangeList` rebinds the named variable only
//!   (aliases keep their copy), and `union`/`intersect` do not deduplicate
//!   duplicates already inside the first list.
//! - `retrieve_association` results come back in the store's id order, which
//!   can differ from mxrb's insertion order when a `Reference` collapse or a
//!   `head` picks "the first" of several rows.

mod datetime;
mod engine;
mod export_mapping;
mod expression;
mod http_objects;
mod import_mapping;
mod marketplace;
mod value;

pub use engine::{
    Adapter, AdapterKind, Effect, Execution, FlowEngine, HttpCall, HttpResponse, JavaAction,
    OperationAnswer, StoreMembers,
};
pub use export_mapping::{ExportMapping, NullValues, ObjectMapping, ValueMapping};
pub use expression::{Expression, MemberSource, NoObjects};
pub use http_objects::{HttpAnswer, HttpBinding, HttpObjects};
pub use import_mapping::{
    ImportMapping, ImportedObject, ImportedType, ImportedValue, MissingObject, ObjectHandling,
};
pub use value::{FlowValue, ObjectRef, RequestValue, Variables, member_to_json};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FlowError {
    /// mxrb's `NativeRuntimeError`.
    #[error("{0}")]
    Native(String),
    /// A store or authorization failure, preserved so `NotAuthorized`
    /// reaches HTTP boundaries typed.
    #[error(transparent)]
    Runtime(#[from] mxrs_runtime::RuntimeError),
}

impl FlowError {
    pub fn native(message: impl Into<String>) -> Self {
        FlowError::Native(message.into())
    }

    pub fn unsupported_expression(detail: impl std::fmt::Display) -> Self {
        FlowError::Native(format!("unsupported Mendix expression: {detail}"))
    }
}
