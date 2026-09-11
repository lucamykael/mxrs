//! Ties the crate's pieces together into one project-scoped entry point —
//! no direct `mxrb` equivalent (the Ruby wires
//! `DatabaseConnectorActionCompiler`/`MicroflowNodeCompiler`/
//! `MicroflowDocumentCompiler` together ad hoc at each call site instead),
//! added here because a real caller (`mxrs-compiler-flow`'s own
//! integration tests, and eventually `mxrs-cli`/an acceptance pass against
//! a real project) needs exactly this: build the project-wide index once,
//! then compile any number of flows/code actions against it.

use std::cell::RefCell;

use mxrs_model::Project;
use mxrs_schema::RuntimeModelSchema;

use crate::CompilerError;
use crate::code_action::CodeActionCompiler;
use crate::database_connector::DatabaseConnectorCompiler;
use crate::document::FlowDocumentCompiler;
use crate::node::{FlowDiagnostic, FlowNodeCompiler};
use crate::support::ProjectFlowIndex;

pub struct FlowCompiler {
    schema: RuntimeModelSchema,
    index: ProjectFlowIndex,
    /// Accumulates every [`FlowDiagnostic`] across every [`Self::compile_flow`]
    /// call made against this instance — unlike a per-call node/document
    /// compiler (freshly built inside `compile_flow` and discarded once it
    /// returns), `FlowCompiler` itself is the project-scoped, many-calls
    /// object (see the module doc comment), so this is where a caller
    /// compiling an entire project's worth of flows can collect the full
    /// diagnostic list in one place instead of threading it through every
    /// call site.
    diagnostics: RefCell<Vec<FlowDiagnostic>>,
}

impl FlowCompiler {
    /// `existing_runtime_documents` seeds `RuntimeModelSchema`'s ID-matched-
    /// counterpart/observed-field lookups — pass an empty slice for a
    /// fresh compile with nothing to reconcile against (the common case;
    /// see `mxrs-schema::RuntimeModelSchema` for when a non-empty slice
    /// matters).
    pub fn new(
        project: &Project,
        existing_runtime_documents: &[mxrs_bson::Document],
    ) -> Result<Self, CompilerError> {
        Ok(FlowCompiler {
            schema: RuntimeModelSchema::for_11(existing_runtime_documents)?,
            index: ProjectFlowIndex::build(project)?,
            diagnostics: RefCell::new(Vec::new()),
        })
    }

    pub fn compile_flow(
        &self,
        source: &mxrs_bson::Document,
        module_name: &str,
    ) -> Result<mxrs_bson::Document, CompilerError> {
        let connector =
            DatabaseConnectorCompiler::new(&self.index.database_connections, &self.index.constants);
        let nodes = FlowNodeCompiler::new(&self.schema, &self.index.associations, Some(connector));
        let document_compiler =
            FlowDocumentCompiler::new(&self.schema, nodes, &self.index.role_map);
        let result = document_compiler.compile(source, module_name);
        self.diagnostics
            .borrow_mut()
            .extend(document_compiler.diagnostics());
        result
    }

    /// Every [`FlowDiagnostic`] recorded across every [`Self::compile_flow`]
    /// call made so far against this instance.
    pub fn diagnostics(&self) -> Vec<FlowDiagnostic> {
        self.diagnostics.borrow().clone()
    }

    pub fn compile_code_action(
        &self,
        source: &mxrs_bson::Document,
        module_name: Option<&str>,
    ) -> Result<mxrs_bson::Document, CompilerError> {
        CodeActionCompiler::compile(source, module_name)
    }

    /// A `Microflows$Nanoflow`'s own root document is compiled through
    /// [`compile_flow`] like any other flow (nanoflows use the same
    /// Runtime graph shape as microflows/rules) — this is a separate,
    /// additional artifact: the client-side JS program an app's *callers*
    /// of that nanoflow need, built via [`crate::nanoflow::NanoflowCompiler`]
    /// directly (it needs its own mutable cross-call cache, so it isn't
    /// folded into this facade's `&self` methods — see that module's doc
    /// comment for why).
    pub fn index(&self) -> &ProjectFlowIndex {
        &self.index
    }
}
