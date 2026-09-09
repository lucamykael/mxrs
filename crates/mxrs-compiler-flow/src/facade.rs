//! Ties the crate's pieces together into one project-scoped entry point —
//! no direct `mxrb` equivalent (the Ruby wires
//! `DatabaseConnectorActionCompiler`/`MicroflowNodeCompiler`/
//! `MicroflowDocumentCompiler` together ad hoc at each call site instead),
//! added here because a real caller (`mxrs-compiler-flow`'s own
//! integration tests, and eventually `mxrs-cli`/an acceptance pass against
//! a real project) needs exactly this: build the project-wide index once,
//! then compile any number of flows/code actions against it.

use mxrs_model::Project;
use mxrs_schema::RuntimeModelSchema;

use crate::code_action::CodeActionCompiler;
use crate::database_connector::DatabaseConnectorCompiler;
use crate::document::FlowDocumentCompiler;
use crate::node::FlowNodeCompiler;
use crate::support::ProjectFlowIndex;
use crate::CompilerError;

pub struct FlowCompiler {
    schema: RuntimeModelSchema,
    index: ProjectFlowIndex,
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
        document_compiler.compile(source, module_name)
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
