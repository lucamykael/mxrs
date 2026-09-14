//! Recursively compiles ordered graph nodes nested inside a microflow/
//! nanoflow/rule — ports `lib/mxrb/compiler/microflow_node_compiler.rb`
//! (254 lines). `node_value`'s resolution order is the crux of this whole
//! crate; read directly from the Ruby source (not a summary) given the
//! stakes, and ported field-for-field below.
//!
//! **Deliberate structural divergence from the Ruby**: `MicroflowNodeCompiler`
//! is `prepare(flow)` then `compile(value)` — mutable `@variable_types`
//! rebuilt from scratch on every `prepare` call, with no cross-call
//! persistence requirement (unlike the nanoflow compiler's `@programs`
//! cache, which genuinely needs to persist across calls to break call
//! cycles — see `nanoflow/mod.rs`'s doc comment for that contrast).
//! `FlowNodeCompiler` here is `&self`-only, matching
//! `mxrs-compiler-domain::DomainCompiler`'s house style: [`variable_types_for`]
//! is a pure pre-pass, and its result is threaded explicitly through
//! [`FlowNodeCompiler::compile`] instead of being an implicit prior side
//! effect. This makes "compiled without preparing" impossible by
//! construction, rather than a runtime foot-gun only `mxrb` would catch.
//!
//! **Minor, documented divergence in array compilation**: Ruby's
//! `compile(array)` uses `filter_map`, which drops both `nil` *and*
//! `false` results. This crate only drops `Bson::Null` (the intended case
//! — an editorial/annotation node vanishing from its containing array) and
//! keeps `Bson::Boolean(false)`, since no realistic Mendix graph array
//! holds raw booleans as elements — reproducing the `filter_map` quirk
//! exactly would risk silently eating a genuine `false` value for zero
//! practical benefit.

use std::cell::RefCell;
use std::collections::HashMap;

use mxrs_bson::{Bson, Document, doc};
use mxrs_schema::RuntimeModelSchema;

use crate::CompilerError;
use crate::database_connector::DatabaseConnectorCompiler;
use crate::support::{derived_id, get_any, get_doc_any, get_str_any};
use crate::types::{data_type, is_data_type_document};

pub use crate::support::AssociationInfo;

/// Non-fatal, out-of-band findings surfaced during node compilation — never
/// changes the compiled output, purely an explicit diagnostic sink.
///
/// **Rust-does-it-better opportunity, not a divergence from `mxrb`**:
/// `database_connector_action_compiler.rb`'s own `unconfigured_write?` doc
/// comment already flags this exact case as high-risk, but Ruby only acts
/// on it silently — the write still gets swapped for a no-op
/// `LogMessageAction` (see [`noop_database_action`]), and the fact that it
/// happened is otherwise invisible until someone notices missing data in
/// production. This variant carries the same signal into something a
/// caller (`mxrs-cli`, an acceptance pass) can actually list before
/// deploy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowDiagnostic {
    UnconfiguredWrite {
        action_id: String,
        connection_name: String,
    },
}

/// A module-role-name -> [(entity qualified name)] map keyed by the
/// `Microflows$MicroflowParameter` nodes found anywhere in a flow's tree —
/// replaces `MicroflowNodeCompiler#prepare`'s mutable `@variable_types`.
pub type VariableTypes = HashMap<String, String>;

/// The 7-entry `[$Type, RuntimeField] -> EditorField` rename table
/// (`microflow_node_compiler.rb:14-22`).
fn field_source(type_name: &str, runtime_field: &str) -> Option<&'static str> {
    Some(match (type_name, runtime_field) {
        ("Microflows$BasicCodeActionParameterValue", "ValueExpression") => "Argument",
        ("Microflows$CreateVariableAction", "Type") => "VariableType",
        ("Microflows$MicroflowParameter", "Type") => "VariableType",
        ("Microflows$MappingRequestHandling", "ContentTypeRuntime") => "ContentType",
        ("Microflows$RetrieveSorting", "AttributePath") => "AttributeRef",
        ("Microflows$RestCallAction", "RequestTimeOutExpression") => "TimeOutExpression",
        ("Microflows$ResultHandling", "VariableDataType") => "VariableType",
        _ => return None,
    })
}

pub struct FlowNodeCompiler<'a> {
    schema: &'a RuntimeModelSchema,
    database_connector: Option<DatabaseConnectorCompiler<'a>>,
    associations: &'a HashMap<String, AssociationInfo>,
    attribute_types: Option<&'a HashMap<String, String>>,
    /// Interior mutability, not shared cross-call state like the nanoflow
    /// compiler's `@programs` cache: `compile`/`compile_hash`/`compile_node`
    /// stay `&self` (this crate's house style — see the module doc comment),
    /// and every `FlowNodeCompiler` instance is freshly constructed per
    /// `FlowCompiler::compile_flow` call anyway, so there is no
    /// cross-flow leakage to worry about — just a side channel for
    /// [`FlowDiagnostic`]s alongside the primary `Result` return value.
    diagnostics: RefCell<Vec<FlowDiagnostic>>,
}

impl<'a> FlowNodeCompiler<'a> {
    pub fn new(
        schema: &'a RuntimeModelSchema,
        associations: &'a HashMap<String, AssociationInfo>,
        database_connector: Option<DatabaseConnectorCompiler<'a>>,
    ) -> Self {
        FlowNodeCompiler {
            schema,
            database_connector,
            associations,
            attribute_types: None,
            diagnostics: RefCell::new(Vec::new()),
        }
    }

    pub fn with_attribute_types(mut self, attribute_types: &'a HashMap<String, String>) -> Self {
        self.attribute_types = Some(attribute_types);
        self
    }

    pub fn diagnostics(&self) -> Vec<FlowDiagnostic> {
        self.diagnostics.borrow().clone()
    }

    /// Replaces `MicroflowNodeCompiler#prepare` — walks the whole flow
    /// tree collecting every `Microflows$MicroflowParameter`'s entity type,
    /// keyed by parameter name (`microflow_node_compiler.rb:42-50`).
    pub fn variable_types_for(flow_root: &Document) -> VariableTypes {
        let mut types = VariableTypes::new();
        visit(&Bson::Document(flow_root.clone()), &mut |node| {
            if get_str_any(node, &["$Type"]).as_deref() != Some("Microflows$MicroflowParameter") {
                return;
            }
            let entity = get_doc_any(node, &["VariableType"])
                .and_then(|t| get_str_any(&t, &["Entity"]))
                .unwrap_or_default();
            if !entity.is_empty()
                && let Some(name) = get_str_any(node, &["Name"])
            {
                types.insert(name, entity);
            }
        });
        types
    }

    /// A plain field-value compile — never drops, `None` results (an
    /// editorial node reached directly, not through a containing array)
    /// become `Bson::Null`.
    pub fn compile(&self, value: &Bson, vars: &VariableTypes) -> Result<Bson, CompilerError> {
        match value {
            Bson::Document(document) => Ok(self
                .compile_hash(document, vars)?
                .map(Bson::Document)
                .unwrap_or(Bson::Null)),
            Bson::Array(items) => {
                let mut out = Vec::new();
                for item in mxrs_bson::parse_array(Some(items)).items {
                    let compiled = self.compile(&item, vars)?;
                    if !matches!(compiled, Bson::Null) {
                        out.push(compiled);
                    }
                }
                Ok(Bson::Array(out))
            }
            other => Ok(other.clone()),
        }
    }

    fn compile_hash(
        &self,
        source: &Document,
        vars: &VariableTypes,
    ) -> Result<Option<Document>, CompilerError> {
        let Some(type_name) = get_str_any(source, &["$Type"]) else {
            let mut out = Document::new();
            for (key, value) in source {
                out.insert(key.clone(), self.compile(value, vars)?);
            }
            return Ok(Some(out));
        };
        if type_name == "Microflows$AnnotationFlow" {
            return Ok(None);
        }
        if type_name == "Microflows$CustomRange" {
            return Ok(Some(compile_custom_range(source)));
        }
        if type_name == "Microflows$LoopedActivity" {
            return Ok(Some(self.compile_looped_activity(source, vars)?));
        }
        if type_name == "DatabaseConnector$ExecuteDatabaseQueryAction" {
            let connector = self
                .database_connector
                .as_ref()
                .expect("a flow referencing a database action needs a DatabaseConnectorCompiler");
            let replaced = if connector.is_unconfigured_write(source)? {
                self.diagnostics
                    .borrow_mut()
                    .push(FlowDiagnostic::UnconfiguredWrite {
                        action_id: get_str_any(source, &["$ID"]).unwrap_or_default(),
                        connection_name: connector.connection_name_for(source),
                    });
                noop_database_action(source)
            } else {
                connector.compile(source)?
            };
            return self.compile_hash(&replaced, vars);
        }
        Ok(Some(self.compile_node(source, vars)?))
    }

    /// A loop is a nested object collection whose sequence flows stay on
    /// the containing flow document. Keeping this lowering explicit avoids
    /// accidentally looking for (or synthesizing) an
    /// `ObjectCollection.Flows` field that does not exist in Runtime shape.
    fn compile_looped_activity(
        &self,
        source: &Document,
        vars: &VariableTypes,
    ) -> Result<Document, CompilerError> {
        let mut result = Document::new();
        result.insert(
            "$ID",
            get_any(source, &["$ID"]).cloned().unwrap_or(Bson::Null),
        );
        result.insert(
            "$Type",
            get_any(source, &["$Type"])
                .cloned()
                .unwrap_or_else(|| Bson::String("Microflows$LoopedActivity".to_string())),
        );
        result.insert(
            "ObjectCollection",
            self.compile(
                get_any(source, &["ObjectCollection"]).unwrap_or(&Bson::Null),
                vars,
            )?,
        );
        result.insert(
            "LoopSource",
            self.compile(
                get_any(source, &["LoopSource"]).unwrap_or(&Bson::Null),
                vars,
            )?,
        );
        result.insert(
            "ErrorHandlingType",
            get_any(source, &["ErrorHandlingType"])
                .cloned()
                .unwrap_or_else(|| Bson::String("Rollback".to_string())),
        );
        Ok(result)
    }

    fn compile_node(
        &self,
        source: &Document,
        vars: &VariableTypes,
    ) -> Result<Document, CompilerError> {
        let fields = self.schema.fields_for(source)?;
        let mut result = Document::new();
        for field in fields {
            result.insert(field.clone(), self.node_value(source, &field, vars)?);
        }
        Ok(result)
    }

    fn node_value(
        &self,
        source: &Document,
        field: &str,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        if field == "$ID" || field == "$Type" {
            return Ok(get_any(source, &[field]).cloned().unwrap_or(Bson::Null));
        }
        let type_name = get_str_any(source, &["$Type"]).unwrap_or_default();
        if field == "Type"
            && matches!(
                type_name.as_str(),
                "Microflows$AssociationRetrieveSource" | "Microflows$DatabaseRetrieveSource"
            )
        {
            return self.retrieve_type(source, vars);
        }
        if field == "StringRepresentation" && type_name == "Microflows$InheritanceCase" {
            return self.derived_default(source, field, vars);
        }
        if field == "ValueExpression" && type_name == "Microflows$MicroflowParameterValue" {
            let microflow = get_str_any(source, &["Microflow"]).unwrap_or_default();
            return Ok(Bson::String(format!("'{microflow}'")));
        }
        if field == "ValueExpression"
            && type_name == "Microflows$EntityTypeCodeActionParameterValue"
            && !source.contains_key(field)
        {
            return self.derived_default(source, field, vars);
        }

        let existing = self.schema.counterpart(source);
        if runtime_derived_fields(&type_name).contains(&field)
            && let Some(existing) = existing
            && let Some(value) = existing.get(field)
        {
            return Ok(value.clone());
        }

        let source_field = if source.contains_key(field) {
            field
        } else {
            field_source(&type_name, field).unwrap_or(field)
        };
        if source.contains_key(source_field) {
            return self.converted_field(source, source_field, field, vars);
        }
        self.default_or_existing(source, field, vars)
    }

    fn converted_field(
        &self,
        source: &Document,
        source_field: &str,
        runtime_field: &str,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        let value = source.get(source_field).cloned().unwrap_or(Bson::Null);
        if runtime_field == "AttributePath" {
            return Ok(Bson::String(runtime_attribute_path(&value)));
        }
        if matches!(runtime_field, "Type" | "VariableDataType")
            && let Bson::Document(d) = &value
            && is_data_type_document(d)
        {
            return Ok(Bson::String(data_type(Some(d))?));
        }
        self.compile(&value, vars)
    }

    fn default_or_existing(
        &self,
        source: &Document,
        field: &str,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        let type_name = get_str_any(source, &["$Type"]).unwrap_or_default();
        if let Some(value) = type_default(&type_name, field) {
            return Ok(value);
        }
        if let Some(existing) = self.schema.counterpart(source)
            && let Some(value) = existing.get(field)
        {
            return Ok(value.clone());
        }
        self.derived_default(source, field, vars)
    }

    fn derived_default(
        &self,
        source: &Document,
        field: &str,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        let type_name = get_str_any(source, &["$Type"]).unwrap_or_default();
        Ok(match field {
            // Official compiler output uses this literal shape, and mxrb's
            // database connector emits it too. Unlike mxrb's generic node
            // pass, deriving it does not require an old MDP.
            "ValueExpression" if type_name == "Microflows$EntityTypeCodeActionParameterValue" => {
                let entity = get_str_any(source, &["Entity"])
                    .filter(|entity| !entity.trim().is_empty())
                    .ok_or_else(|| CompilerError::CannotDeriveRuntimeField {
                        type_name: type_name.clone(),
                        field: field.to_string(),
                    })?;
                Bson::String(crate::database_connector::expression_literal(&entity))
            }
            "Argument" => Bson::String(
                get_doc_any(source, &["Value"])
                    .and_then(|v| get_str_any(&v, &["Argument"]))
                    .unwrap_or_default(),
            ),
            "QueueSettings" | "NewCaseValue" => Bson::Null,
            "Location" => Bson::String("Content".to_string()),
            "ParameterMappings" => Bson::Array(vec![]),
            "StringRepresentation" => {
                let value = get_str_any(source, &["Value"]).unwrap_or_default();
                if type_name == "Microflows$InheritanceCase" && value.is_empty() {
                    Bson::String("(empty)".to_string())
                } else {
                    Bson::String(value)
                }
            }
            "Type" => return self.derived_runtime_type(source, vars),
            "IsRequired" | "Disabled" => Bson::Boolean(matches!(
                get_any(source, &[field]),
                Some(Bson::Boolean(true))
            )),
            "DefaultValue" | "Caption" | "ResultVariableName" | "OutputVariableName"
            | "FormObjectVariable" | "Queue" => Bson::String(String::new()),
            other => {
                return Err(CompilerError::CannotDeriveRuntimeField {
                    type_name,
                    field: other.to_string(),
                });
            }
        })
    }

    fn derived_runtime_type(
        &self,
        source: &Document,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        if get_str_any(source, &["$Type"]).as_deref() == Some("Microflows$AggregateAction") {
            let aggregate_function =
                get_str_any(source, &["AggregateFunction"]).unwrap_or_default();
            match aggregate_function.as_str() {
                "Count" => return Ok(Bson::String("Integer".to_string())),
                "All" | "Any" => return Ok(Bson::String("Boolean".to_string())),
                _ => {}
            }
            if let Some(attribute_type) = get_str_any(source, &["Attribute"]).and_then(|name| {
                self.attribute_types
                    .and_then(|attribute_types| attribute_types.get(&name))
            }) {
                return Ok(Bson::String(attribute_type.clone()));
            }
            return Err(CompilerError::CannotDeriveAggregateType { aggregate_function });
        }
        self.retrieve_type(source, vars)
    }

    fn retrieve_type(
        &self,
        source: &Document,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        let type_name = get_str_any(source, &["$Type"]).unwrap_or_default();
        if type_name == "Microflows$AssociationRetrieveSource" {
            return self.association_retrieve_type(source, vars);
        }
        if type_name != "Microflows$DatabaseRetrieveSource" {
            return Err(CompilerError::CannotDeriveRetrieveType { type_name });
        }
        let single = get_doc_any(source, &["Range"])
            .and_then(|r| get_any(&r, &["SingleObject"]).cloned())
            == Some(Bson::Boolean(true));
        let entity = get_str_any(source, &["Entity"]).unwrap_or_default();
        Ok(Bson::String(if single {
            entity
        } else {
            format!("[{entity}]")
        }))
    }

    fn association_retrieve_type(
        &self,
        source: &Document,
        vars: &VariableTypes,
    ) -> Result<Bson, CompilerError> {
        let association_id = get_str_any(source, &["AssociationId"]).unwrap_or_default();
        let association = self
            .associations
            .get(&association_id)
            .cloned()
            .or_else(|| self.runtime_association(&association_id));
        let Some(association) = association else {
            return Err(CompilerError::UnknownAssociation { association_id });
        };
        let start =
            get_str_any(source, &["StartVariableName"]).and_then(|name| vars.get(&name).cloned());
        let target = if start.is_some() && start == association.child {
            association.parent
        } else {
            association.child.clone()
        };
        let Some(target) = target.filter(|t| !t.is_empty()) else {
            return Ok(Bson::Null);
        };
        let list = association.reference_set || (start.is_some() && start == association.child);
        Ok(Bson::String(if list {
            format!("[{target}]")
        } else {
            target
        }))
    }

    /// Fallback for an association not declared in this project's own
    /// `mxrs-model`-derived index — resolved instead off an existing
    /// Runtime document via the schema, mirroring
    /// `MicroflowNodeCompiler#runtime_association`.
    fn runtime_association(&self, name: &str) -> Option<AssociationInfo> {
        let association = self.schema.named(name)?;
        let parent = get_any(association, &["ParentPointer"])
            .and_then(|id| self.schema.counterpart_id(id))
            .and_then(|d| get_str_any(d, &["QualifiedName"]));
        let child = get_any(association, &["ChildPointer"])
            .and_then(|id| self.schema.counterpart_id(id))
            .and_then(|d| get_str_any(d, &["QualifiedName"]));
        let reference_set = get_str_any(association, &["Type"]).as_deref() == Some("ReferenceSet");
        Some(AssociationInfo {
            parent,
            child,
            reference_set,
        })
    }
}

fn runtime_derived_fields(type_name: &str) -> &'static [&'static str] {
    if type_name == "Microflows$ActionActivity" {
        &["Caption"]
    } else {
        &[]
    }
}

/// The 2-entry `TYPE_DEFAULTS` table (`microflow_node_compiler.rb:23-29`).
fn type_default(type_name: &str, field: &str) -> Option<Bson> {
    match (type_name, field) {
        ("Microflows$NoCase", "StringRepresentation") => Some(Bson::String(String::new())),
        ("Microflows$AggregateAction", "Expression") => Some(Bson::String(String::new())),
        ("Microflows$AggregateAction", "UseExpression") => Some(Bson::Boolean(false)),
        ("Microflows$AggregateAction", "ReduceInitialValueExpression") => {
            Some(Bson::String(String::new()))
        }
        _ => None,
    }
}

/// Runtime has no `CustomRange` concept — always folded into a
/// `ConstantRange` (`microflow_node_compiler.rb:76-81`).
fn compile_custom_range(source: &Document) -> Document {
    let id = get_str_any(source, &["$ID"]).unwrap_or_default();
    doc! {
        "$ID": derived_id(&id, "constant-range"),
        "$Type": "Microflows$ConstantRange",
        "SingleObject": get_any(source, &["SingleObject"]) == Some(&Bson::Boolean(true)),
    }
}

/// The concrete "connection isn't configured, no-op instead of failing"
/// mechanism (`microflow_node_compiler.rb:83-93`) — the resulting document
/// is fed back through `compile_hash` by the caller (its `$Type` no longer
/// matches any special case, so it takes the generic schema-driven path,
/// filling in any further Runtime fields `LogMessageAction` needs beyond
/// these seven).
fn noop_database_action(source: &Document) -> Document {
    let id = get_str_any(source, &["$ID"]).unwrap_or_default();
    doc! {
        "$ID": id.clone(),
        "$Type": "Microflows$LogMessageAction",
        "MessageTemplate": doc! {
            "$ID": derived_id(&id, "local-fallback-message"),
            "$Type": "Microflows$StringTemplate",
            "Parameters": Vec::<Bson>::new(),
            "Text": "",
        },
        "ErrorHandlingType": get_str_any(source, &["ErrorHandlingType"]).unwrap_or_else(|| "Rollback".to_string()),
        "Level": "Trace",
        "Node": "'Mxrb'",
        "IncludeLatestStackTrace": false,
    }
}

/// `Attribute` on the reference (or the reference itself when it's a
/// plain string) rewritten to `"Entity/Attribute"` shorthand — reproduces
/// `runtime_attribute_path` exactly, including its non-obvious behavior
/// when the reference already contains no dot (the common case: `entity`
/// comes out empty and the reference passes through unchanged).
fn runtime_attribute_path(reference: &Bson) -> String {
    let attribute = match reference {
        Bson::Document(d) => get_str_any(d, &["Attribute"]).unwrap_or_default(),
        Bson::String(s) => s.clone(),
        other => other.to_string(),
    };
    match attribute.rfind('.') {
        Some(index) => format!("{}/{attribute}", &attribute[..index]),
        None => attribute,
    }
}

/// Deep-walks a `Bson` tree yielding every document with a `$Type` key —
/// mirrors `visit` (`microflow_node_compiler.rb:236-243`).
fn visit(value: &Bson, callback: &mut impl FnMut(&Document)) {
    match value {
        Bson::Document(document) => {
            if document.contains_key("$Type") {
                callback(document);
            }
            for (_, v) in document {
                visit(v, callback);
            }
        }
        Bson::Array(items) => {
            for item in mxrs_bson::parse_array(Some(items)).items {
                visit(&item, callback);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    fn schema() -> RuntimeModelSchema {
        RuntimeModelSchema::for_11(&[]).unwrap()
    }

    fn compiler<'a>(
        schema: &'a RuntimeModelSchema,
        associations: &'a HashMap<String, AssociationInfo>,
    ) -> FlowNodeCompiler<'a> {
        FlowNodeCompiler::new(schema, associations, None)
    }

    #[test]
    fn an_entity_type_argument_derives_the_official_literal_without_a_compiled_counterpart() {
        let schema = schema();
        let associations = HashMap::new();
        let compiler = compiler(&schema, &associations);
        let source = doc! { "$ID": "11111111-1111-1111-1111-111111111111", "$Type": "Microflows$EntityTypeCodeActionParameterValue", "Entity": "Sales.Order" };
        let result = compiler
            .compile(&Bson::Document(source.clone()), &VariableTypes::new())
            .unwrap();
        assert_eq!(
            result,
            Bson::Document(
                doc! { "$ID": "11111111-1111-1111-1111-111111111111", "$Type": "Microflows$EntityTypeCodeActionParameterValue", "Entity": "Sales.Order", "ValueExpression": "'Sales.Order'" }
            )
        );
        for entity in [None, Some(""), Some("  ")] {
            let mut invalid = source.clone();
            invalid.remove("Entity");
            if let Some(entity) = entity {
                invalid.insert("Entity", entity);
            }
            assert!(matches!(
                compiler.compile(&Bson::Document(invalid), &VariableTypes::new()),
                Err(CompilerError::CannotDeriveRuntimeField { .. })
            ));
        }
    }

    #[test]
    fn entity_edits_do_not_reuse_stale_compiled_literals_and_explicit_expressions_are_preserved() {
        let existing = doc! { "$ID": "11111111-1111-1111-1111-111111111111", "$Type": "Microflows$EntityTypeCodeActionParameterValue", "Entity": "Sales.Order", "ValueExpression": "'Sales.Previous'" };
        let schema = RuntimeModelSchema::for_11(std::slice::from_ref(&existing)).unwrap();
        let associations = HashMap::new();
        let compiler = compiler(&schema, &associations);
        let mut source = existing.clone();
        source.remove("ValueExpression");
        let mut expected = existing;
        expected.insert("ValueExpression", "'Sales.Order'");
        assert_eq!(
            compiler
                .compile(&Bson::Document(source.clone()), &VariableTypes::new())
                .unwrap(),
            Bson::Document(expected)
        );
        source.insert("ValueExpression", "'Sales.Explicit'");
        assert_eq!(
            compiler
                .compile(&Bson::Document(source.clone()), &VariableTypes::new())
                .unwrap(),
            Bson::Document(source)
        );
    }

    #[test]
    fn an_unconfigured_write_still_noops_but_also_records_a_diagnostic() {
        let schema = schema();
        let associations = HashMap::new();
        let query = doc! {
            "$ID": "q1", "Name": "DoInsert", "Query": "insert into orders values (1)",
        };
        let connection = doc! {
            "ConnectionString": "Sales.DbConnectionString",
            "Queries": mxrs_bson::build_array(vec![Bson::Document(query)], 3),
        };
        let mut connections = HashMap::new();
        connections.insert("Sales.MainDb".to_string(), connection);
        let mut constants = HashMap::new();
        constants.insert(
            "Sales.DbConnectionString".to_string(),
            doc! { "Name": "DbConnectionString", "DefaultValue": "" },
        );
        let db_compiler = DatabaseConnectorCompiler::new(&connections, &constants);
        let c = FlowNodeCompiler::new(&schema, &associations, Some(db_compiler));
        let action = Bson::Document(doc! {
            "$ID": "action-1",
            "$Type": "DatabaseConnector$ExecuteDatabaseQueryAction",
            "Query": "Sales.MainDb.DoInsert",
        });

        let compiled = c.compile(&action, &VariableTypes::new()).unwrap();

        let Bson::Document(compiled) = compiled else {
            panic!("expected a document")
        };
        assert_eq!(
            compiled.get_str("$Type").unwrap(),
            "Microflows$LogMessageAction"
        );
        assert_eq!(
            c.diagnostics(),
            vec![FlowDiagnostic::UnconfiguredWrite {
                action_id: "action-1".to_string(),
                connection_name: "Sales.MainDb".to_string(),
            }]
        );
    }

    #[test]
    fn variable_types_for_collects_parameter_entities_anywhere_in_the_tree() {
        let flow = doc! {
            "$Type": "Microflows$Microflow",
            "ObjectCollection": {
                "Objects": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "$Type": "Microflows$MicroflowParameter",
                    "Name": "Order",
                    "VariableType": { "Entity": "Sales.Order" },
                })], 3),
            },
        };
        let vars = FlowNodeCompiler::variable_types_for(&flow);
        assert_eq!(vars.get("Order").map(String::as_str), Some("Sales.Order"));
    }

    #[test]
    fn an_annotation_flow_is_dropped_from_its_containing_array() {
        let schema = schema();
        let associations = HashMap::new();
        let c = compiler(&schema, &associations);
        let array = Bson::Array(mxrs_bson::build_array(
            vec![Bson::Document(
                doc! { "$Type": "Microflows$AnnotationFlow" },
            )],
            3,
        ));
        let compiled = c.compile(&array, &VariableTypes::new()).unwrap();
        let Bson::Array(items) = compiled else {
            panic!("expected an array")
        };
        assert!(items.is_empty());
    }

    #[test]
    fn a_custom_range_folds_into_a_constant_range_with_a_stable_id() {
        let schema = schema();
        let associations = HashMap::new();
        let c = compiler(&schema, &associations);
        let source = doc! {
            "$ID": "11111111-1111-1111-1111-111111111111",
            "$Type": "Microflows$CustomRange",
            "SingleObject": true,
        };
        let compiled = c
            .compile(&Bson::Document(source.clone()), &VariableTypes::new())
            .unwrap();
        let Bson::Document(compiled) = compiled else {
            panic!("expected a document")
        };
        assert_eq!(
            compiled.get_str("$Type").unwrap(),
            "Microflows$ConstantRange"
        );
        assert!(compiled.get_bool("SingleObject").unwrap());
        // Recompiling the same source must produce the same id.
        let compiled_again = c
            .compile(&Bson::Document(source), &VariableTypes::new())
            .unwrap();
        let Bson::Document(compiled_again) = compiled_again else {
            panic!("expected a document")
        };
        assert_eq!(
            compiled.get_str("$ID").unwrap(),
            compiled_again.get_str("$ID").unwrap()
        );
    }

    #[test]
    fn association_retrieve_type_from_the_parent_side_is_a_list_only_for_reference_sets() {
        let schema = schema();
        let mut associations = HashMap::new();
        associations.insert(
            "Sales.Order_Customer".to_string(),
            AssociationInfo {
                parent: Some("Sales.Order".to_string()),
                child: Some("Sales.Customer".to_string()),
                reference_set: false,
            },
        );
        let c = compiler(&schema, &associations);
        let mut vars = VariableTypes::new();
        vars.insert("StartObject".to_string(), "Sales.Order".to_string());
        let source = doc! {
            "$Type": "Microflows$AssociationRetrieveSource",
            "AssociationId": "Sales.Order_Customer",
            "StartVariableName": "StartObject",
        };
        let value = c.retrieve_type(&source, &vars).unwrap();
        assert_eq!(value, Bson::String("Sales.Customer".to_string()));
    }

    #[test]
    fn association_retrieve_type_from_the_child_side_of_a_reference_is_a_list() {
        let schema = schema();
        let mut associations = HashMap::new();
        associations.insert(
            "Sales.Order_Customer".to_string(),
            AssociationInfo {
                parent: Some("Sales.Order".to_string()),
                child: Some("Sales.Customer".to_string()),
                reference_set: false,
            },
        );
        let c = compiler(&schema, &associations);
        let mut vars = VariableTypes::new();
        vars.insert("StartObject".to_string(), "Sales.Customer".to_string());
        let source = doc! {
            "$Type": "Microflows$AssociationRetrieveSource",
            "AssociationId": "Sales.Order_Customer",
            "StartVariableName": "StartObject",
        };
        let value = c.retrieve_type(&source, &vars).unwrap();
        assert_eq!(value, Bson::String("[Sales.Order]".to_string()));
    }

    #[test]
    fn an_unknown_association_is_a_loud_error() {
        let schema = schema();
        let associations = HashMap::new();
        let c = compiler(&schema, &associations);
        let source = doc! {
            "$Type": "Microflows$AssociationRetrieveSource",
            "AssociationId": "Sales.Bogus",
            "StartVariableName": "x",
        };
        let err = c.retrieve_type(&source, &VariableTypes::new()).unwrap_err();
        assert!(matches!(err, CompilerError::UnknownAssociation { .. }));
    }

    #[test]
    fn aggregate_types_follow_the_function_and_domain_attribute() {
        let schema = schema();
        let associations = HashMap::new();
        let attribute_types = HashMap::from([
            ("Sales.Order.Amount".to_string(), "Decimal".to_string()),
            ("Sales.Order.Number".to_string(), "Integer".to_string()),
        ]);
        let c = compiler(&schema, &associations).with_attribute_types(&attribute_types);
        let count = doc! { "$Type": "Microflows$AggregateAction", "AggregateFunction": "Count" };
        assert_eq!(
            c.derived_runtime_type(&count, &VariableTypes::new())
                .unwrap(),
            Bson::String("Integer".to_string())
        );
        let all = doc! { "$Type": "Microflows$AggregateAction", "AggregateFunction": "All" };
        assert_eq!(
            c.derived_runtime_type(&all, &VariableTypes::new()).unwrap(),
            Bson::String("Boolean".to_string())
        );
        let average = doc! {
            "$Type": "Microflows$AggregateAction",
            "AggregateFunction": "Average",
            "Attribute": "Sales.Order.Amount",
        };
        assert_eq!(
            c.derived_runtime_type(&average, &VariableTypes::new())
                .unwrap(),
            Bson::String("Decimal".to_string())
        );
        let sum = doc! {
            "$Type": "Microflows$AggregateAction",
            "AggregateFunction": "Sum",
            "Attribute": "Sales.Order.Missing",
        };
        let err = c
            .derived_runtime_type(&sum, &VariableTypes::new())
            .unwrap_err();
        assert!(matches!(
            err,
            CompilerError::CannotDeriveAggregateType { .. }
        ));
    }

    #[test]
    fn an_attribute_path_without_a_dot_passes_through_unchanged() {
        // The common case: a reference already in "Entity/Attribute"
        // shorthand has no dot, so `rfind('.')` finds nothing and the
        // value passes through as-is.
        let value = Bson::Document(doc! { "Attribute": "Order/Number" });
        assert_eq!(runtime_attribute_path(&value), "Order/Number");
    }

    #[test]
    fn an_attribute_path_with_a_dot_gets_the_literal_ruby_prefix_behavior() {
        // Reproduces `runtime_attribute_path` exactly, including its
        // non-obvious result when the reference itself contains a dot:
        // `entity` is everything before the *last* dot of the whole
        // string, then the result is `"{entity}/{attribute}"` with the
        // *original* (unsplit) attribute appended — not a "fixed" version.
        let value = Bson::Document(doc! { "Attribute": "Sales.Order/Number" });
        assert_eq!(runtime_attribute_path(&value), "Sales/Sales.Order/Number");
    }

    #[test]
    fn an_unrecognized_field_with_no_default_is_a_loud_error() {
        let schema = schema();
        let associations = HashMap::new();
        let c = compiler(&schema, &associations);
        let source = doc! { "$Type": "Microflows$SomeAction" };
        let err = c
            .derived_default(&source, "TotallyUnknownField", &VariableTypes::new())
            .unwrap_err();
        assert!(matches!(
            err,
            CompilerError::CannotDeriveRuntimeField { .. }
        ));
    }

    #[test]
    fn a_while_loop_lowers_to_the_five_field_runtime_shape() {
        let schema = schema();
        let associations = HashMap::new();
        let c = compiler(&schema, &associations);
        let source = Bson::Document(doc! {
            "$ID": "11111111-1111-1111-1111-111111111111",
            "$Type": "Microflows$LoopedActivity",
            "Documentation": "editor-only",
            "RelativeMiddlePoint": { "X": 10, "Y": 20 },
            "ObjectCollection": {
                "$ID": "22222222-2222-2222-2222-222222222222",
                "$Type": "Microflows$MicroflowObjectCollection",
                "Objects": mxrs_bson::build_array(Vec::<Bson>::new(), 3),
            },
            "LoopSource": {
                "$ID": "33333333-3333-3333-3333-333333333333",
                "$Type": "Microflows$WhileLoopCondition",
                "Caption": "editor-only",
                "WhileExpression": "$Continue",
            },
            "ErrorHandlingType": "CustomWithoutRollBack",
        });

        let Bson::Document(compiled) = c.compile(&source, &VariableTypes::new()).unwrap() else {
            panic!("expected a document")
        };
        assert_eq!(
            compiled.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "$ID",
                "$Type",
                "ObjectCollection",
                "LoopSource",
                "ErrorHandlingType"
            ]
        );
        let loop_source = compiled.get_document("LoopSource").unwrap();
        assert_eq!(
            loop_source.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["$ID", "$Type", "WhileExpression"]
        );
        assert_eq!(loop_source.get_str("WhileExpression").unwrap(), "$Continue");
    }

    #[test]
    fn an_iterable_loop_preserves_both_variable_names() {
        let schema = schema();
        let associations = HashMap::new();
        let c = compiler(&schema, &associations);
        let source = Bson::Document(doc! {
            "$ID": "11111111-1111-1111-1111-111111111111",
            "$Type": "Microflows$LoopedActivity",
            "ObjectCollection": {
                "$ID": "22222222-2222-2222-2222-222222222222",
                "$Type": "Microflows$MicroflowObjectCollection",
                "Objects": mxrs_bson::build_array(Vec::<Bson>::new(), 3),
            },
            "LoopSource": {
                "$ID": "33333333-3333-3333-3333-333333333333",
                "$Type": "Microflows$IterableList",
                "ListVariableName": "Orders",
                "VariableName": "Order",
            },
        });

        let Bson::Document(compiled) = c.compile(&source, &VariableTypes::new()).unwrap() else {
            panic!("expected a document")
        };
        let loop_source = compiled.get_document("LoopSource").unwrap();
        assert_eq!(loop_source.get_str("ListVariableName").unwrap(), "Orders");
        assert_eq!(loop_source.get_str("VariableName").unwrap(), "Order");
        assert_eq!(compiled.get_str("ErrorHandlingType").unwrap(), "Rollback");
    }
}
