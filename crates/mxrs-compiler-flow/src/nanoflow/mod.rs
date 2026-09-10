//! Lowers a `Microflows$Nanoflow` into a client-side JS instruction
//! program — ports `lib/mxrb/compiler/nanoflow_program_compiler.rb` (483
//! lines) verbatim, including its allow-listed activity set, its
//! all-or-nothing-per-flow failure semantics, and the expression
//! mini-language in [`expression`].
//!
//! **This is not the BSON pipeline** — no [`crate::CompilerError`] variant
//! exists for anything in this module, on purpose: an unsupported
//! node/action anywhere is a documented, expected outcome
//! (`unsupported()`/`diagnostics()`), not a compiler failure the way a
//! missing Runtime field is for the node/document compilers.
//!
//! **Kept stateful and mutable, unlike [`crate::FlowNodeCompiler`]** (which
//! was deliberately refactored away from Ruby's mutable `prepare`/`compile`
//! split — see that module's doc comment). Here the asymmetry is
//! deliberate in the other direction: `programs`'s in-progress placeholder
//! entry (registered *before* recursing into a flow's graph) is what
//! correctly terminates recursive/mutual nanoflow-call cycles — a cycle's
//! second `reference()` call sees the in-progress entry and returns a
//! forward reference instead of infinitely re-entering. That's genuine
//! shared mutable state central to correctness, not an artifact of Ruby's
//! style, so it isn't a candidate for the same stateless refactor.
//!
//! **Documented simplifications versus a byte-for-byte port**:
//! - `compile_graph_node`'s "was the node's own compile result truly nil,
//!   or just an empty-but-defined instruction array" distinction
//!   (`nanoflow_program_compiler.rb:118-124`) collapses to "is the
//!   resulting instruction list empty" here. The one case this changes:
//!   a `ChangeAction` whose every `Items` entry is itself unsupported
//!   compiles to an empty instruction list in both versions, but the Ruby
//!   would still append a trailing `jump` instruction when the node has
//!   exactly one outgoing flow (since its `instruction` variable held `[]`,
//!   not `nil`) — this port skips that trailing jump in that one narrow
//!   edge case. Not expected to matter in practice (a `ChangeAction` with
//!   zero real member changes is itself a degenerate model).
//! - `WebOperationCompiler.operation_id`/`menu_operation_id`
//!   (`lib/mxrb/compiler/web_operation_compiler.rb:14-23`) is ported ahead
//!   of its own future crate (`mxrs-compiler-widgets`) purely because
//!   `compile_microflow_call`/`compile_commit` need byte-identical IDs —
//!   see [`operation_id`]. Move it there (not reimplement it again) once
//!   that crate exists.
//! - `compile_split`'s `targets` map doesn't dedupe a (invalid/degenerate)
//!   model with two flows sharing the same case value the way Ruby's
//!   `to_h` would (last one wins) — an edge case not worth the extra
//!   bookkeeping for a model shape Studio Pro shouldn't produce anyway.

pub mod expression;
pub mod js_value;

use std::collections::HashMap;
use std::path::Path;

use mxrs_bson::{Bson, Document};
use sha2::{Digest, Sha256};

use crate::support::{array_docs, get_any, get_doc_any, get_str_any, AssociationInfo, ProjectFlowIndex};
use expression::{leading_variable_name, parse_expression, Expression};
use js_value::JsValue;

pub struct UnsupportedNode {
    pub flow: String,
    pub node_type: String,
}

pub enum NanoflowDiagnostic {
    /// (a) A node no `StartEvent`-reachable flow edge leads to — compiled
    /// output is unaffected (it simply isn't compiled, same as `mxrb`),
    /// this is purely a visibility improvement over the silent vanish.
    UnreachableNode {
        flow: String,
        node_id: String,
        node_type: String,
    },
    /// (b) A binary expression whose left operand (as the lazy regex
    /// match resolved it) itself contains an `and`/`or` keyword — a
    /// best-effort signal for the cases most likely to have parsed
    /// differently than a human would expect. Compiled output is
    /// unaffected; the parse tree is unchanged.
    AmbiguousBinaryLeftOperand { flow: String, expression: String },
}

struct CompiledProgram {
    reference: String,
    declaration: Option<String>,
}

pub struct NanoflowCompiler<'a> {
    nanoflows: &'a HashMap<String, Document>,
    javascript_actions: &'a HashMap<String, Document>,
    associations: &'a HashMap<String, AssociationInfo>,
    project_root: Option<&'a Path>,
    programs: HashMap<String, Option<CompiledProgram>>,
    flow_stack: Vec<String>,
    variable_kind_stack: Vec<HashMap<String, String>>,
    unsupported: Vec<UnsupportedNode>,
    diagnostics: Vec<NanoflowDiagnostic>,
}

impl<'a> NanoflowCompiler<'a> {
    /// `project_root` is the directory containing the `.mpr` file (needed
    /// to build a `JavaScriptActionCallAction`'s `require()` path); pass
    /// `None` when no real project path is available — any JavaScript
    /// action call then compiles as unsupported rather than emitting a
    /// broken path.
    pub fn new(index: &'a ProjectFlowIndex, project_root: Option<&'a Path>) -> Self {
        NanoflowCompiler {
            nanoflows: &index.nanoflows,
            javascript_actions: &index.javascript_actions,
            associations: &index.associations,
            project_root,
            programs: HashMap::new(),
            flow_stack: Vec::new(),
            variable_kind_stack: Vec::new(),
            unsupported: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn reference(&mut self, qualified_name: &str) -> Option<String> {
        if let Some(existing) = self.programs.get(qualified_name) {
            return existing.as_ref().map(|p| format!("() => {}", p.reference));
        }
        let document = self.nanoflows.get(qualified_name).cloned()?;
        self.compile(&document, qualified_name)
    }

    pub fn declarations(&self) -> String {
        self.programs
            .values()
            .filter_map(|p| p.as_ref().and_then(|p| p.declaration.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn unsupported(&self) -> &[UnsupportedNode] {
        &self.unsupported
    }

    pub fn diagnostics(&self) -> &[NanoflowDiagnostic] {
        &self.diagnostics
    }

    fn compile(&mut self, document: &Document, qualified_name: &str) -> Option<String> {
        let reference_name = format!("mxrbNanoflow_{}", short_hash(qualified_name));
        self.programs.insert(
            qualified_name.to_string(),
            Some(CompiledProgram {
                reference: reference_name.clone(),
                declaration: None,
            }),
        );
        self.variable_kind_stack.push(variable_kinds(document));
        self.flow_stack.push(qualified_name.to_string());
        let unsupported_before = self.unsupported.len();
        let instructions = self.compile_graph(document, qualified_name);
        self.flow_stack.pop();
        self.variable_kind_stack.pop();

        if self.unsupported.len() > unsupported_before {
            self.programs.insert(qualified_name.to_string(), None);
            return None;
        }
        let program = JsValue::object(vec![
            ("name", JsValue::Str(qualified_name.to_string())),
            ("useListParameterByReference", JsValue::Bool(true)),
            ("instructions", JsValue::Array(instructions)),
        ]);
        let declaration = format!("const {reference_name} = {};", program.render());
        self.programs.insert(
            qualified_name.to_string(),
            Some(CompiledProgram {
                reference: reference_name.clone(),
                declaration: Some(declaration),
            }),
        );
        Some(format!("() => {reference_name}"))
    }

    fn compile_graph(&mut self, document: &Document, flow_name: &str) -> Vec<JsValue> {
        let (objects, by_id, flows) = graph(document);
        let start = objects
            .iter()
            .find(|o| get_str_any(o, &["$Type"]).as_deref() == Some("Microflows$StartEvent"))
            .cloned();
        let ordered = reachable_nodes(start.as_ref(), &by_id, &flows);

        let reachable_ids: std::collections::HashSet<String> =
            ordered.iter().map(model_id_of).collect();
        let start_id = start.as_ref().map(model_id_of);
        for object in &objects {
            let id = model_id_of(object);
            let type_name = get_str_any(object, &["$Type"]).unwrap_or_default();
            if !reachable_ids.contains(&id)
                && Some(&id) != start_id.as_ref()
                && !type_name.contains("Annotation")
            {
                self.diagnostics.push(NanoflowDiagnostic::UnreachableNode {
                    flow: flow_name.to_string(),
                    node_id: id,
                    node_type: type_name,
                });
            }
        }

        let mut instructions = Vec::new();
        for node in &ordered {
            let id = model_id_of(node);
            let node_flows = flows.get(&id).cloned().unwrap_or_default();
            instructions.extend(self.compile_graph_node(node, &node_flows, flow_name));
        }
        instructions
    }

    fn compile_graph_node(
        &mut self,
        node: &Document,
        flows: &[Document],
        flow_name: &str,
    ) -> Vec<JsValue> {
        let error_flows: Vec<Document> = flows
            .iter()
            .filter(|f| is_error_handler(f))
            .cloned()
            .collect();
        let normal_flows: Vec<Document> = flows
            .iter()
            .filter(|f| !is_error_handler(f))
            .cloned()
            .collect();
        let mut instructions = if error_flows.is_empty() {
            self.compile_node(node, &normal_flows, flow_name)
        } else {
            self.compile_try_catch(node, &normal_flows, &error_flows, flow_name)
                .into_iter()
                .collect()
        };
        if is_terminal(node) || instructions.is_empty() || normal_flows.len() != 1 {
            return instructions;
        }
        let id = model_id_of(node);
        let target = get_any(&normal_flows[0], &["DestinationPointer"])
            .map(model_id)
            .unwrap_or_default();
        instructions.push(JsValue::object(vec![
            ("type", JsValue::Str("jump".to_string())),
            ("label", JsValue::Str(format!("{id}$next"))),
            ("target", JsValue::Str(target)),
        ]));
        instructions
    }

    fn compile_try_catch(
        &mut self,
        node: &Document,
        normal_flows: &[Document],
        error_flows: &[Document],
        flow_name: &str,
    ) -> Option<JsValue> {
        if normal_flows.len() != 1 || error_flows.len() != 1 {
            self.mark_unsupported(flow_name, "Microflows$ErrorHandler", node);
            return None;
        }
        let compiled = self.compile_node(node, normal_flows, flow_name);
        if compiled.is_empty() {
            return None;
        }
        let mut body: Vec<JsValue> = compiled.into_iter().map(strip_label).collect();
        body.push(JsValue::object(vec![
            ("type", JsValue::Str("return".to_string())),
            (
                "result",
                JsValue::object(vec![
                    ("type", JsValue::Str("literal".to_string())),
                    ("value", JsValue::Bool(true)),
                ]),
            ),
            ("resultKind", JsValue::Str("primitive".to_string())),
        ]));
        let catch_target = get_any(&error_flows[0], &["DestinationPointer"])
            .map(model_id)
            .unwrap_or_default();
        Some(JsValue::object(vec![
            ("type", JsValue::Str("tryCatch".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("catchTarget", JsValue::Str(catch_target)),
            ("body", JsValue::Array(body)),
        ]))
    }

    fn compile_node(
        &mut self,
        node: &Document,
        flows: &[Document],
        flow_name: &str,
    ) -> Vec<JsValue> {
        let type_name = get_str_any(node, &["$Type"]).unwrap_or_default();
        match type_name.as_str() {
            "Microflows$EndEvent" => vec![self.compile_end(node)],
            "Microflows$ExclusiveSplit" => self
                .compile_split(node, flows, flow_name)
                .into_iter()
                .collect(),
            "Microflows$ExclusiveMerge" => self
                .compile_merge(node, flows, flow_name)
                .into_iter()
                .collect(),
            "Microflows$ActionActivity" => {
                // A disabled activity is a real, valid model (Studio Pro
                // lets an activity sit disabled with no action configured
                // at all) — the Runtime skips it entirely rather than
                // executing whatever `Action` it might still carry, so
                // this doesn't dispatch on `action_type` at all. A `noop`
                // is emitted rather than an empty instruction list so
                // `compile_graph_node`'s trailing-jump logic still runs —
                // an empty list means "no instructions AND no jump" there.
                if get_any(node, &["Disabled"]) == Some(&Bson::Boolean(true)) {
                    return vec![JsValue::object(vec![
                        ("type", JsValue::Str("noop".to_string())),
                        ("label", JsValue::Str(model_id_of(node))),
                    ])];
                }
                let action = get_doc_any(node, &["Action"]).unwrap_or_default();
                let action_type = get_str_any(&action, &["$Type"]).unwrap_or_default();
                match action_type.as_str() {
                    "Microflows$CreateVariableAction" => vec![self.compile_variable(&action, node)],
                    "Microflows$ChangeVariableAction" => {
                        vec![self.compile_change_variable(&action, node)]
                    }
                    "Microflows$CreateObjectAction" | "Microflows$CreateChangeAction" => {
                        self.compile_create_object(&action, node, flow_name)
                    }
                    "Microflows$ChangeAction" => {
                        self.compile_change_object(&action, node, flow_name)
                    }
                    "Microflows$NanoflowCallAction" => self
                        .compile_nanoflow_call(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$MicroflowCallAction" => self
                        .compile_microflow_call(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$JavaScriptActionCallAction" => self
                        .compile_javascript_call(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$ShowFormAction" => self
                        .compile_show_form(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$CloseFormAction" => vec![self.compile_close_form(&action, node)],
                    "Microflows$ShowMessageAction" => {
                        vec![self.compile_show_message(&action, node, flow_name)]
                    }
                    "Microflows$ValidationFeedbackAction" => self
                        .compile_validation(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$LogMessageAction" => {
                        vec![self.compile_log(&action, node, flow_name)]
                    }
                    "Microflows$CommitAction" => self
                        .compile_commit(&action, node, true, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$RetrieveAction" => self
                        .compile_retrieve(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$DeleteAction" => self
                        .compile_delete(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$RollbackAction" => self
                        .compile_rollback(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$CreateListAction" => self
                        .compile_create_list(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$ChangeListAction" => self
                        .compile_change_list(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$ListOperationsAction" => self
                        .compile_list_operation(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    "Microflows$AggregateAction" => self
                        .compile_aggregate(&action, node, flow_name)
                        .into_iter()
                        .collect(),
                    other => {
                        self.mark_unsupported(flow_name, other, node);
                        vec![]
                    }
                }
            }
            other => {
                self.mark_unsupported(flow_name, other, node);
                vec![]
            }
        }
    }

    fn compile_end(&mut self, node: &Document) -> JsValue {
        let raw = get_str_any(node, &["ReturnValue"]).unwrap_or_default();
        let kind = self.expression_kind_for(&raw);
        let expr = self.expression_for(&raw);
        JsValue::object(vec![
            ("type", JsValue::Str("return".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("result", expr.into()),
            ("resultKind", JsValue::Str(kind)),
        ])
    }

    fn compile_variable(&mut self, action: &Document, node: &Document) -> JsValue {
        let output_var = get_str_any(action, &["VariableName"]).unwrap_or_default();
        let kind = variable_kind(get_doc_any(action, &["VariableType"]).as_ref());
        let raw = get_str_any(action, &["InitialValue"]).unwrap_or_default();
        let value = self.expression_for(&raw);
        JsValue::object(vec![
            ("type", JsValue::Str("setVariable".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("outputVar", JsValue::Str(output_var)),
            ("outputKind", JsValue::Str(kind)),
            ("value", value.into()),
        ])
    }

    fn compile_change_variable(&mut self, action: &Document, node: &Document) -> JsValue {
        let output_var = get_str_any(action, &["ChangeVariableName"]).unwrap_or_default();
        let raw = get_str_any(action, &["Value"]).unwrap_or_default();
        let value = self.expression_for(&raw);
        JsValue::object(vec![
            ("type", JsValue::Str("setVariable".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("outputVar", JsValue::Str(output_var)),
            ("outputKind", JsValue::Str("primitive".to_string())),
            ("value", value.into()),
        ])
    }

    fn compile_create_object(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Vec<JsValue> {
        let variable = get_str_any(action, &["OutputVariableName"])
            .or_else(|| get_str_any(action, &["VariableName"]))
            .unwrap_or_default();
        let id = model_id_of(node);
        let create = JsValue::object(vec![
            ("type", JsValue::Str("createObject".to_string())),
            ("label", JsValue::Str(id.clone())),
            (
                "objectType",
                JsValue::Str(get_str_any(action, &["Entity"]).unwrap_or_default()),
            ),
            ("outputVar", JsValue::Str(variable.clone())),
        ]);
        let mut instructions = vec![create];
        instructions.extend(self.change_instructions(
            action,
            &variable,
            &format!("{id}$change"),
            flow_name,
        ));
        if get_str_any(action, &["Commit"]).as_deref() == Some("Yes") {
            if let Some(commit) = self.compile_commit(action, node, false, flow_name) {
                instructions.push(commit);
            }
        }
        instructions
    }

    fn compile_change_object(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Vec<JsValue> {
        let variable = get_str_any(action, &["ChangeVariableName"]).unwrap_or_default();
        let id = model_id_of(node);
        let mut instructions = self.change_instructions(action, &variable, &id, flow_name);
        if get_str_any(action, &["Commit"]).as_deref() == Some("Yes") {
            if let Some(commit) = self.compile_commit(action, node, false, flow_name) {
                instructions.push(commit);
            }
        }
        instructions
    }

    fn change_instructions(
        &mut self,
        action: &Document,
        variable: &str,
        label: &str,
        flow_name: &str,
    ) -> Vec<JsValue> {
        let items = array_docs(action, &["Items"]);
        let mut out = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let item_type = get_str_any(item, &["Type"]);
            // Changing a plain attribute populates `Attribute`; changing a
            // `Reference`/`ReferenceSet` association instead leaves
            // `Attribute` empty and populates `Association` — true for
            // `Set` (a `Reference`'s single related object) just as much as
            // `Add`/`Remove` (a `ReferenceSet`'s members), so every `Type`
            // falls back to `Association` the same way.
            let qualified = get_str_any(item, &["Attribute"])
                .filter(|a| !a.is_empty())
                .or_else(|| get_str_any(item, &["Association"]))
                .unwrap_or_default();
            let member = qualified.rsplit('.').next().unwrap_or_default().to_string();
            let instruction_type = match item_type.as_deref() {
                Some("Set") => "changeObject",
                Some("Add") => "addReference",
                Some("Remove") => "removeReference",
                _ => "",
            };
            if member.is_empty() || instruction_type.is_empty() {
                let item_type_name = get_str_any(item, &["$Type"]).unwrap_or_default();
                self.mark_unsupported(flow_name, &item_type_name, item);
                continue;
            }
            let raw = get_str_any(item, &["Value"]).unwrap_or_default();
            let value = self.expression_for(&raw);
            let item_label = if index == 0 {
                label.to_string()
            } else {
                format!("{label}${index}")
            };
            out.push(JsValue::object(vec![
                ("type", JsValue::Str(instruction_type.to_string())),
                ("label", JsValue::Str(item_label)),
                ("inputVar", JsValue::Str(variable.to_string())),
                ("member", JsValue::Str(member)),
                ("value", value.into()),
            ]));
        }
        out
    }

    fn compile_nanoflow_call(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let call = get_doc_any(action, &["NanoflowCall"]).unwrap_or_default();
        let target = get_str_any(&call, &["Nanoflow"]).unwrap_or_default();
        let reference = self.reference(&target);
        let Some(reference) = reference else {
            self.mark_unsupported(
                flow_name,
                &get_str_any(action, &["$Type"]).unwrap_or_default(),
                action,
            );
            return None;
        };
        let mut entries = vec![
            ("type", JsValue::Str("nanoflowCall".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("flow", JsValue::Raw(reference)),
            ("parameters", JsValue::Array(self.call_parameters(&call))),
        ];
        let output = get_str_any(action, &["OutputVariableName"]).unwrap_or_default();
        if !output.is_empty() {
            entries.push(("outputVar", JsValue::Str(output)));
        }
        Some(JsValue::object(entries))
    }

    fn compile_microflow_call(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let call = get_doc_any(action, &["MicroflowCall"]).unwrap_or_default();
        let name = get_str_any(&call, &["Microflow"]).unwrap_or_default();
        if name.is_empty() {
            self.mark_unsupported(
                flow_name,
                &get_str_any(action, &["$Type"]).unwrap_or_default(),
                action,
            );
            return None;
        }
        let op_id = operation_id(flow_name, &model_id_of(node));
        let mut entries = vec![
            ("type", JsValue::Str("microflowCall".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("operationId", JsValue::Str(op_id)),
            ("parameters", JsValue::Array(self.call_parameters(&call))),
        ];
        let output = get_str_any(action, &["ResultVariableName"]).unwrap_or_default();
        let use_return = get_any(action, &["UseReturnVariable"]) == Some(&Bson::Boolean(true));
        if use_return && !output.is_empty() {
            entries.push(("outputVar", JsValue::Str(output)));
        }
        Some(JsValue::object(entries))
    }

    fn compile_javascript_call(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let qualified = get_str_any(action, &["JavaScriptAction"]).unwrap_or_default();
        let action_type = get_str_any(action, &["$Type"]).unwrap_or_default();
        let Some(js_action) = self.javascript_actions.get(&qualified).cloned() else {
            self.mark_unsupported(flow_name, &action_type, action);
            return None;
        };
        let Some(project_root) = self.project_root else {
            self.mark_unsupported(flow_name, &action_type, action);
            return None;
        };
        let Some((module_name, name)) = qualified.rsplit_once('.') else {
            self.mark_unsupported(flow_name, &action_type, action);
            return None;
        };
        let reference = javascript_reference(project_root, module_name, name);
        let parameters = javascript_parameters(&js_action, action, self);
        let mut entries = vec![
            ("type", JsValue::Str("javaScriptActionCall".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("action", JsValue::Raw(reference)),
            ("parameters", JsValue::Array(parameters)),
        ];
        let output = get_str_any(action, &["OutputVariableName"]).unwrap_or_default();
        let use_return = get_any(action, &["UseReturnVariable"]) == Some(&Bson::Boolean(true));
        if use_return && !output.is_empty() {
            entries.push(("outputVar", JsValue::Str(output)));
        }
        Some(JsValue::object(entries))
    }

    fn compile_show_form(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let settings = get_doc_any(action, &["FormSettings"]).unwrap_or_default();
        let form = get_str_any(&settings, &["Form"]).unwrap_or_default();
        if form.is_empty() {
            self.mark_unsupported(
                flow_name,
                &get_str_any(action, &["$Type"]).unwrap_or_default(),
                action,
            );
            return None;
        }
        let path = format!("{}.page.xml", form.replace('.', "/"));
        let params = JsValue::object(vec![
            ("name", JsValue::Str(path.clone())),
            ("location", JsValue::Str("modal".to_string())),
            ("resizable", JsValue::Bool(false)),
        ]);
        let mappings: Vec<(String, JsValue)> = array_docs(&settings, &["ParameterMappings"])
            .iter()
            .map(|mapping| {
                let parameter = get_str_any(mapping, &["Parameter"]).unwrap_or_default();
                let key = format!("${}", parameter.rsplit('.').next().unwrap_or_default());
                let raw = get_str_any(mapping, &["Argument"]).unwrap_or_default();
                (key, self.expression_for(&raw).into())
            })
            .collect();
        let mut entries = vec![
            ("type", JsValue::Str("openForm".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("path", JsValue::Str(path)),
            ("params", params),
        ];
        if !mappings.is_empty() {
            entries.push(("inputArgs", JsValue::Object(mappings)));
        }
        Some(JsValue::object(entries))
    }

    fn compile_close_form(&mut self, action: &Document, node: &Document) -> JsValue {
        let value = get_str_any(action, &["NumberOfPagesToClose"]).unwrap_or_default();
        let mut entries = vec![
            ("type", JsValue::Str("closeForm".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
        ];
        if !value.is_empty() {
            entries.push(("numberOfPagesToClose", self.expression_for(&value).into()));
        }
        JsValue::object(entries)
    }

    fn compile_show_message(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> JsValue {
        let message = self.text_template_expression(action, "Template");
        let _ = flow_name;
        JsValue::object(vec![
            ("type", JsValue::Str("showMessage".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("message", message),
            (
                "messageType",
                JsValue::Str(
                    get_str_any(action, &["Type"])
                        .unwrap_or_default()
                        .to_lowercase(),
                ),
            ),
            (
                "blocking",
                JsValue::Bool(get_any(action, &["Blocking"]) == Some(&Bson::Boolean(true))),
            ),
        ])
    }

    fn compile_validation(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let input = get_str_any(action, &["ValidationVariableName"]).unwrap_or_default();
        if input.is_empty() {
            self.mark_unsupported(
                flow_name,
                &get_str_any(action, &["$Type"]).unwrap_or_default(),
                action,
            );
            return None;
        }
        // An empty `Attribute` is a real, general (not field-specific)
        // validation message — not the "missing data" signal it is for
        // `change_instructions`'s attribute/association fields, so no
        // `member` entry rather than falling back to unsupported.
        let attribute = get_str_any(action, &["Attribute"]).unwrap_or_default();
        let member = attribute.rsplit('.').next().unwrap_or_default().to_string();
        let text = self.text_template_expression(action, "FeedbackTemplate");
        let mut entries = vec![
            ("type", JsValue::Str("showValidation".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("inputVar", JsValue::Str(input)),
        ];
        if !member.is_empty() {
            entries.push(("member", JsValue::Str(member)));
        }
        entries.push(("text", text));
        Some(JsValue::object(entries))
    }

    fn compile_log(&mut self, action: &Document, node: &Document, flow_name: &str) -> JsValue {
        let message = self.text_template_expression(action, "MessageTemplate");
        let _ = flow_name;
        JsValue::object(vec![
            ("type", JsValue::Str("writeLog".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            (
                "level",
                JsValue::Str(
                    get_str_any(action, &["Level"])
                        .unwrap_or_default()
                        .to_lowercase(),
                ),
            ),
            ("message", message),
        ])
    }

    fn compile_commit(
        &mut self,
        action: &Document,
        node: &Document,
        with_label: bool,
        flow_name: &str,
    ) -> Option<JsValue> {
        let input = [
            "CommitVariableName",
            "ChangeVariableName",
            "OutputVariableName",
            "VariableName",
        ]
        .iter()
        .find_map(|key| get_str_any(action, &[key]))
        .unwrap_or_default();
        if input.is_empty() {
            self.mark_unsupported(
                flow_name,
                &get_str_any(action, &["$Type"]).unwrap_or_default(),
                action,
            );
            return None;
        }
        let op_id = operation_id(flow_name, &model_id_of(node));
        let mut entries = vec![
            ("type", JsValue::Str("commitObjects".to_string())),
            ("inputVar", JsValue::Str(input)),
            ("operationId", JsValue::Str(op_id)),
        ];
        if with_label {
            entries.push(("label", JsValue::Str(model_id_of(node))));
        }
        Some(JsValue::object(entries))
    }

    /// `Microflows$RetrieveAction` isn't in mxrb's own allow-listed activity
    /// set — `nanoflow_program_compiler.rb`'s `compile_node` has no case for
    /// it at all, so this is an intentional addition beyond the verbatim
    /// port, not a gap the Ruby closes elsewhere. Two known simplifications:
    /// an `XpathConstraint` is carried through as an opaque string rather
    /// than translated to JS (it's a distinct grammar from this module's
    /// `expression` mini-language — a future runtime consuming this IR
    /// needs its own XPath evaluator); and a plain `Reference` association's
    /// list-vs-single cardinality isn't resolved by traversal direction the
    /// way [`crate::FlowNodeCompiler::association_retrieve_type`] does for
    /// the BSON pipeline, since that needs full variable-type tracking this
    /// module doesn't have — only `reference_set` associations (always a
    /// list from either side) get `"list": true` here.
    fn compile_retrieve(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let output = get_str_any(action, &["ResultVariableName"]).unwrap_or_default();
        let source = get_doc_any(action, &["RetrieveSource"]).unwrap_or_default();
        let source_type = get_str_any(&source, &["$Type"]).unwrap_or_default();
        if output.is_empty() {
            self.mark_unsupported(flow_name, &source_type, action);
            return None;
        }
        match source_type.as_str() {
            "Microflows$DatabaseRetrieveSource" => {
                let entity = get_str_any(&source, &["Entity"]).unwrap_or_default();
                if entity.is_empty() {
                    self.mark_unsupported(flow_name, &source_type, action);
                    return None;
                }
                let single_object = get_doc_any(&source, &["Range"])
                    .map(|r| get_any(&r, &["SingleObject"]) == Some(&Bson::Boolean(true)))
                    .unwrap_or(false);
                let constraint = get_str_any(&source, &["XpathConstraint"]).unwrap_or_default();
                let mut entries = vec![
                    ("type", JsValue::Str("retrieveByEntity".to_string())),
                    ("label", JsValue::Str(model_id_of(node))),
                    ("outputVar", JsValue::Str(output)),
                    ("objectType", JsValue::Str(entity)),
                    ("singleObject", JsValue::Bool(single_object)),
                ];
                if !constraint.is_empty() {
                    entries.push(("constraint", JsValue::Str(constraint)));
                }
                Some(JsValue::object(entries))
            }
            "Microflows$AssociationRetrieveSource" => {
                let association_id = get_str_any(&source, &["AssociationId"]).unwrap_or_default();
                let start = get_str_any(&source, &["StartVariableName"]).unwrap_or_default();
                if association_id.is_empty() || start.is_empty() {
                    self.mark_unsupported(flow_name, &source_type, action);
                    return None;
                }
                let list = self
                    .associations
                    .get(&association_id)
                    .map(|a| a.reference_set)
                    .unwrap_or(false);
                Some(JsValue::object(vec![
                    ("type", JsValue::Str("retrieveByAssociation".to_string())),
                    ("label", JsValue::Str(model_id_of(node))),
                    ("outputVar", JsValue::Str(output)),
                    ("inputVar", JsValue::Str(start)),
                    ("association", JsValue::Str(association_id)),
                    ("list", JsValue::Bool(list)),
                ]))
            }
            _ => {
                self.mark_unsupported(flow_name, &source_type, action);
                None
            }
        }
    }

    /// Not in mxrb's allow-listed activity set, same as [`Self::compile_retrieve`].
    fn compile_delete(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let input = get_str_any(action, &["DeleteVariableName"]).unwrap_or_default();
        if input.is_empty() {
            self.mark_unsupported(flow_name, "Microflows$DeleteAction", action);
            return None;
        }
        Some(JsValue::object(vec![
            ("type", JsValue::Str("deleteObject".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("inputVar", JsValue::Str(input)),
        ]))
    }

    /// Not in mxrb's allow-listed activity set, same as [`Self::compile_retrieve`].
    fn compile_rollback(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let input = get_str_any(action, &["RollbackVariableName"]).unwrap_or_default();
        if input.is_empty() {
            self.mark_unsupported(flow_name, "Microflows$RollbackAction", action);
            return None;
        }
        Some(JsValue::object(vec![
            ("type", JsValue::Str("rollbackObject".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("inputVar", JsValue::Str(input)),
        ]))
    }

    /// Not in mxrb's allow-listed activity set, same as [`Self::compile_retrieve`].
    fn compile_create_list(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let output = get_str_any(action, &["VariableName"]).unwrap_or_default();
        let entity = get_str_any(action, &["Entity"]).unwrap_or_default();
        if output.is_empty() || entity.is_empty() {
            self.mark_unsupported(flow_name, "Microflows$CreateListAction", action);
            return None;
        }
        Some(JsValue::object(vec![
            ("type", JsValue::Str("createList".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("outputVar", JsValue::Str(output)),
            ("objectType", JsValue::Str(entity)),
        ]))
    }

    /// Only the `Add`/`Remove` operations observed in real models are
    /// implemented — not in mxrb's allow-listed activity set, same as
    /// [`Self::compile_retrieve`].
    fn compile_change_list(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let input = get_str_any(action, &["ChangeVariableName"]).unwrap_or_default();
        let op_type = get_str_any(action, &["Type"]);
        let instruction_type = match op_type.as_deref() {
            Some("Add") => "addToList",
            Some("Remove") => "removeFromList",
            _ => "",
        };
        if input.is_empty() || instruction_type.is_empty() {
            self.mark_unsupported(flow_name, "Microflows$ChangeListAction", action);
            return None;
        }
        let raw = get_str_any(action, &["Value"]).unwrap_or_default();
        let value = self.expression_for(&raw);
        Some(JsValue::object(vec![
            ("type", JsValue::Str(instruction_type.to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("inputVar", JsValue::Str(input)),
            ("value", value.into()),
        ]))
    }

    /// Only `UseExpression: false` is implemented — `Count` needs no
    /// `Attribute`, `Sum`/`Average`/`Min`/`Max` need one. The custom-reduce
    /// path (`UseExpression: true`, folding `Expression` over the list
    /// starting from `ReduceInitialValueExpression`) stays unsupported: no
    /// real example appeared in either acceptance project to confirm the
    /// accumulator-variable convention against. Not in mxrb's allow-listed
    /// activity set, same as [`Self::compile_retrieve`].
    fn compile_aggregate(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let output = get_str_any(action, &["VariableName"]).unwrap_or_default();
        let list_name = get_str_any(action, &["AggregateVariableName"]).unwrap_or_default();
        let function = get_str_any(action, &["AggregateFunction"]).unwrap_or_default();
        let use_expression = get_any(action, &["UseExpression"]) == Some(&Bson::Boolean(true));
        if output.is_empty() || list_name.is_empty() || use_expression {
            self.mark_unsupported(flow_name, "Microflows$AggregateAction", action);
            return None;
        }
        let mut entries = vec![
            ("type", JsValue::Str("aggregateList".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("outputVar", JsValue::Str(output)),
            ("listVar", JsValue::Str(list_name)),
            ("function", JsValue::Str(function.to_lowercase())),
        ];
        if function != "Count" {
            let attribute = get_str_any(action, &["Attribute"]).unwrap_or_default();
            let member = attribute.rsplit('.').next().unwrap_or_default().to_string();
            if member.is_empty() {
                self.mark_unsupported(flow_name, "Microflows$AggregateAction", action);
                return None;
            }
            entries.push(("attribute", JsValue::Str(member)));
        }
        Some(JsValue::object(entries))
    }

    /// `Sort`, `Head`, `Find`, `Filter`, `FilterByExpression`, and
    /// `FindByExpression` are implemented — every `NewOperation` kind
    /// observed in real models. The remaining Runtime-known kinds
    /// (`Intersect`, `Union`, `Contains`, `ListRange`, `Equals`) stay
    /// unsupported rather than guessed at: none appeared in either
    /// acceptance project, so there's no real example to check a compiled
    /// shape against. Not in mxrb's allow-listed activity set, same as
    /// [`Self::compile_retrieve`].
    fn compile_list_operation(
        &mut self,
        action: &Document,
        node: &Document,
        flow_name: &str,
    ) -> Option<JsValue> {
        let output = get_str_any(action, &["ResultVariableName"]).unwrap_or_default();
        let operation = get_doc_any(action, &["NewOperation"]).unwrap_or_default();
        let operation_type = get_str_any(&operation, &["$Type"]).unwrap_or_default();
        if output.is_empty() {
            self.mark_unsupported(flow_name, &operation_type, action);
            return None;
        }
        match operation_type.as_str() {
            "Microflows$Sort" => {
                let list_name = get_str_any(&operation, &["ListName"]).unwrap_or_default();
                if list_name.is_empty() {
                    self.mark_unsupported(flow_name, &operation_type, action);
                    return None;
                }
                let sortings_list = get_doc_any(&operation, &["Sortings"]).unwrap_or_default();
                let sortings: Vec<JsValue> = array_docs(&sortings_list, &["Sortings"])
                    .iter()
                    .map(|sorting| {
                        let attribute = get_doc_any(sorting, &["AttributeRef"])
                            .and_then(|a| get_str_any(&a, &["Attribute"]))
                            .unwrap_or_default();
                        let ascending =
                            get_str_any(sorting, &["SortOrder"]).as_deref() != Some("Descending");
                        JsValue::object(vec![
                            ("attribute", JsValue::Str(attribute)),
                            ("ascending", JsValue::Bool(ascending)),
                        ])
                    })
                    .collect();
                Some(JsValue::object(vec![
                    ("type", JsValue::Str("sortList".to_string())),
                    ("label", JsValue::Str(model_id_of(node))),
                    ("outputVar", JsValue::Str(output)),
                    ("listVar", JsValue::Str(list_name)),
                    ("sortings", JsValue::Array(sortings)),
                ]))
            }
            "Microflows$Head" => {
                let list_name = get_str_any(&operation, &["ListName"]).unwrap_or_default();
                if list_name.is_empty() {
                    self.mark_unsupported(flow_name, &operation_type, action);
                    return None;
                }
                Some(JsValue::object(vec![
                    ("type", JsValue::Str("listHead".to_string())),
                    ("label", JsValue::Str(model_id_of(node))),
                    ("outputVar", JsValue::Str(output)),
                    ("listVar", JsValue::Str(list_name)),
                ]))
            }
            // `Find`/`Filter` compare a named attribute against a value
            // expression for every item; `Find` returns the first match,
            // `Filter` every match. Same qualified-name-to-member
            // convention as `change_instructions`.
            "Microflows$Find" | "Microflows$Filter" => {
                let list_name = get_str_any(&operation, &["ListName"]).unwrap_or_default();
                let attribute = get_str_any(&operation, &["Attribute"]).unwrap_or_default();
                let member = attribute.rsplit('.').next().unwrap_or_default().to_string();
                if list_name.is_empty() || member.is_empty() {
                    self.mark_unsupported(flow_name, &operation_type, action);
                    return None;
                }
                let raw = get_str_any(&operation, &["Expression"]).unwrap_or_default();
                let value = self.expression_for(&raw);
                let instruction_type = if operation_type == "Microflows$Find" {
                    "findInList"
                } else {
                    "filterList"
                };
                Some(JsValue::object(vec![
                    ("type", JsValue::Str(instruction_type.to_string())),
                    ("label", JsValue::Str(model_id_of(node))),
                    ("outputVar", JsValue::Str(output)),
                    ("listVar", JsValue::Str(list_name)),
                    ("attribute", JsValue::Str(member)),
                    ("value", value.into()),
                ]))
            }
            // `FilterByExpression`/`FindByExpression` evaluate a boolean
            // expression per item, bound to the implicit `$currentObject`
            // variable — the same `expression` mini-language every other
            // expression field in this module already uses, so no special
            // handling needed beyond routing it through `expression_for`.
            "Microflows$FilterByExpression" | "Microflows$FindByExpression" => {
                let list_name = get_str_any(&operation, &["ListName"]).unwrap_or_default();
                let raw = get_str_any(&operation, &["Expression"]).unwrap_or_default();
                if list_name.is_empty() || raw.is_empty() {
                    self.mark_unsupported(flow_name, &operation_type, action);
                    return None;
                }
                let condition = self.expression_for(&raw);
                let instruction_type = if operation_type == "Microflows$FindByExpression" {
                    "findInListByExpression"
                } else {
                    "filterListByExpression"
                };
                Some(JsValue::object(vec![
                    ("type", JsValue::Str(instruction_type.to_string())),
                    ("label", JsValue::Str(model_id_of(node))),
                    ("outputVar", JsValue::Str(output)),
                    ("listVar", JsValue::Str(list_name)),
                    ("condition", condition.into()),
                ]))
            }
            _ => {
                self.mark_unsupported(flow_name, &operation_type, action);
                None
            }
        }
    }

    fn compile_split(
        &mut self,
        node: &Document,
        flows: &[Document],
        flow_name: &str,
    ) -> Option<JsValue> {
        let mut targets: Vec<(String, String)> = Vec::new();
        for flow in flows {
            let case_values = array_docs(flow, &["CaseValues"]);
            let case_value = case_values.first();
            let mut key = case_value
                .and_then(|c| get_str_any(c, &["Value"]))
                .unwrap_or_default();
            if case_value
                .and_then(|c| get_str_any(c, &["$Type"]))
                .as_deref()
                == Some("Microflows$NoCase")
            {
                key = String::new();
            }
            let dest = get_any(flow, &["DestinationPointer"])
                .map(model_id)
                .unwrap_or_default();
            targets.push((key, dest));
        }
        let condition_raw = get_doc_any(node, &["SplitCondition"])
            .and_then(|c| get_str_any(&c, &["Expression"]))
            .unwrap_or_default();
        if targets.is_empty() || condition_raw.is_empty() {
            self.mark_unsupported(
                flow_name,
                &get_str_any(node, &["$Type"]).unwrap_or_default(),
                node,
            );
            return None;
        }
        let condition = self.expression_for(&condition_raw);
        Some(JsValue::object(vec![
            ("type", JsValue::Str("switch".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("condition", condition.into()),
            (
                "targets",
                JsValue::Object(
                    targets
                        .into_iter()
                        .map(|(k, v)| (k, JsValue::Str(v)))
                        .collect(),
                ),
            ),
        ]))
    }

    fn compile_merge(
        &mut self,
        node: &Document,
        flows: &[Document],
        flow_name: &str,
    ) -> Option<JsValue> {
        if flows.len() != 1 {
            self.mark_unsupported(
                flow_name,
                &get_str_any(node, &["$Type"]).unwrap_or_default(),
                node,
            );
            return None;
        }
        let target = get_any(&flows[0], &["DestinationPointer"])
            .map(model_id)
            .unwrap_or_default();
        Some(JsValue::object(vec![
            ("type", JsValue::Str("jump".to_string())),
            ("label", JsValue::Str(model_id_of(node))),
            ("target", JsValue::Str(target)),
        ]))
    }

    fn text_template_expression(&mut self, action: &Document, field: &str) -> JsValue {
        let Some(template) = get_doc_any(action, &[field]) else {
            return self.expression_for("").into();
        };
        let text = match get_any(&template, &["Text"]) {
            Some(Bson::Document(text_doc)) => {
                let items = array_docs(text_doc, &["Items"]);
                items
                    .iter()
                    .find(|i| get_str_any(i, &["LanguageCode"]).as_deref() == Some("en_US"))
                    .or_else(|| items.first())
                    .and_then(|i| get_str_any(i, &["Text"]))
                    .unwrap_or_default()
            }
            Some(Bson::String(s)) => s.clone(),
            _ => String::new(),
        };
        let parameters = array_docs(&template, &["Parameters"]);
        if parameters.is_empty() {
            return self.expression_for(&text).into();
        }
        let mut args = vec![JsValue::object(vec![
            ("type", JsValue::Str("literal".to_string())),
            ("value", JsValue::Str(text)),
        ])];
        for parameter in &parameters {
            let raw = get_str_any(parameter, &["Expression"])
                .or_else(|| get_str_any(parameter, &["Argument"]))
                .unwrap_or_default();
            args.push(self.expression_for(&raw).into());
        }
        JsValue::object(vec![
            ("type", JsValue::Str("function".to_string())),
            ("name", JsValue::Str("formatString".to_string())),
            ("parameters", JsValue::Array(args)),
        ])
    }

    fn call_parameters(&mut self, call: &Document) -> Vec<JsValue> {
        array_docs(call, &["ParameterMappings"])
            .iter()
            .map(|mapping| {
                let parameter = get_str_any(mapping, &["Parameter"]).unwrap_or_default();
                let name = parameter.rsplit('.').next().unwrap_or_default().to_string();
                let raw = get_str_any(mapping, &["Argument"]).unwrap_or_default();
                let kind = self.expression_kind_for(&raw);
                let value = self.expression_for(&raw);
                JsValue::object(vec![
                    ("name", JsValue::Str(name)),
                    ("value", value.into()),
                    ("kind", JsValue::Str(kind)),
                ])
            })
            .collect()
    }

    fn expression_for(&mut self, raw: &str) -> Expression {
        let (expression, diagnostics) = parse_expression(raw);
        if !diagnostics.is_empty() {
            let flow = self.flow_stack.last().cloned().unwrap_or_default();
            for diagnostic in diagnostics {
                self.diagnostics
                    .push(NanoflowDiagnostic::AmbiguousBinaryLeftOperand {
                        flow: flow.clone(),
                        expression: diagnostic.source,
                    });
            }
        }
        expression
    }

    fn expression_kind_for(&self, raw: &str) -> String {
        let Some(name) = leading_variable_name(raw) else {
            return "primitive".to_string();
        };
        self.variable_kind_stack
            .last()
            .and_then(|kinds| kinds.get(&name).cloned())
            .unwrap_or_else(|| "primitive".to_string())
    }

    fn mark_unsupported(&mut self, flow_name: &str, node_type: &str, node: &Document) {
        let label = if node_type.is_empty() {
            get_str_any(node, &["$Type"]).unwrap_or_default()
        } else {
            node_type.to_string()
        };
        self.unsupported.push(UnsupportedNode {
            flow: flow_name.to_string(),
            node_type: label,
        });
    }
}

fn javascript_parameters(
    js_action: &Document,
    action: &Document,
    compiler: &mut NanoflowCompiler,
) -> Vec<JsValue> {
    let mut parameter_kinds: HashMap<String, String> = HashMap::new();
    for parameter in array_docs(js_action, &["Parameters"]) {
        let name = get_str_any(&parameter, &["Name"]).unwrap_or_default();
        parameter_kinds.insert(name, javascript_parameter_kind(&parameter));
    }
    array_docs(action, &["ParameterMappings"])
        .iter()
        .map(|mapping| {
            let parameter = get_str_any(mapping, &["Parameter"]).unwrap_or_default();
            let name = parameter.rsplit('.').next().unwrap_or_default().to_string();
            let value_doc = get_doc_any(mapping, &["ParameterValue"]).unwrap_or_default();
            let argument = get_str_any(&value_doc, &["Argument"])
                .or_else(|| quoted_entity(get_str_any(&value_doc, &["Entity"])))
                .unwrap_or_default();
            let kind = parameter_kinds
                .get(&name)
                .cloned()
                .unwrap_or_else(|| "primitive".to_string());
            let value = compiler.expression_for(&argument);
            JsValue::object(vec![("kind", JsValue::Str(kind)), ("value", value.into())])
        })
        .collect()
}

fn quoted_entity(entity: Option<String>) -> Option<String> {
    let entity = entity.unwrap_or_default();
    (!entity.is_empty()).then(|| serde_json::to_string(&entity).unwrap())
}

fn javascript_parameter_kind(parameter: &Document) -> String {
    let type_name = get_doc_any(parameter, &["ParameterType"])
        .and_then(|t| get_doc_any(&t, &["Type"]))
        .and_then(|t| get_str_any(&t, &["$Type"]))
        .unwrap_or_default();
    if type_name.contains("EntityType") {
        "object".to_string()
    } else if type_name.contains("ListType") {
        "list".to_string()
    } else {
        "primitive".to_string()
    }
}

fn javascript_reference(project_root: &Path, module_name: &str, name: &str) -> String {
    let path = project_root
        .join("javascriptsource")
        .join(module_name.to_lowercase())
        .join("actions")
        .join(name);
    let path_json = serde_json::to_string(&path.to_string_lossy().replace('\\', "/")).unwrap();
    format!("() => require({path_json}).{name}")
}

fn variable_kind(type_doc: Option<&Document>) -> String {
    let type_name = type_doc
        .and_then(|d| get_str_any(d, &["$Type"]))
        .unwrap_or_default();
    if type_name.contains("Object") {
        "object".to_string()
    } else {
        "primitive".to_string()
    }
}

fn variable_kinds(document: &Document) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let object_collection = get_doc_any(document, &["ObjectCollection"]).unwrap_or_default();
    for object in array_docs(&object_collection, &["Objects"]) {
        match get_str_any(&object, &["$Type"]).as_deref() {
            Some("Microflows$MicroflowParameter") => {
                let name = get_str_any(&object, &["Name"]).unwrap_or_default();
                result.insert(
                    name,
                    variable_kind(get_doc_any(&object, &["VariableType"]).as_ref()),
                );
            }
            Some("Microflows$ActionActivity") => {
                let action = get_doc_any(&object, &["Action"]).unwrap_or_default();
                if let Some(name) = get_str_any(&action, &["VariableName"]) {
                    result.insert(
                        name.clone(),
                        variable_kind(get_doc_any(&action, &["VariableType"]).as_ref()),
                    );
                    let action_type = get_str_any(&action, &["$Type"]).unwrap_or_default();
                    if matches!(
                        action_type.as_str(),
                        "Microflows$CreateObjectAction" | "Microflows$CreateChangeAction"
                    ) {
                        result.insert(name, "object".to_string());
                    }
                }
                if let Some(name) = get_str_any(&action, &["OutputVariableName"]) {
                    result.insert(name, "object".to_string());
                }
                if let Some(name) = get_str_any(&action, &["ResultVariableName"]) {
                    result.insert(name, "primitive".to_string());
                }
            }
            _ => {}
        }
    }
    result
}

type FlowsByOrigin = HashMap<String, Vec<Document>>;

fn graph(document: &Document) -> (Vec<Document>, HashMap<String, Document>, FlowsByOrigin) {
    let object_collection = get_doc_any(document, &["ObjectCollection"]).unwrap_or_default();
    let objects = array_docs(&object_collection, &["Objects"]);
    let by_id: HashMap<String, Document> = objects
        .iter()
        .map(|o| (model_id_of(o), o.clone()))
        .collect();
    let mut flows: HashMap<String, Vec<Document>> = HashMap::new();
    for flow in array_docs(document, &["Flows"]) {
        let origin = get_any(&flow, &["OriginPointer"])
            .map(model_id)
            .unwrap_or_default();
        flows.entry(origin).or_default().push(flow);
    }
    (objects, by_id, flows)
}

fn reachable_nodes(
    start: Option<&Document>,
    by_id: &HashMap<String, Document>,
    flows: &HashMap<String, Vec<Document>>,
) -> Vec<Document> {
    let mut queue: std::collections::VecDeque<Document> = std::collections::VecDeque::new();
    if let Some(start) = start {
        for flow in flows.get(&model_id_of(start)).cloned().unwrap_or_default() {
            if let Some(node) = get_any(&flow, &["DestinationPointer"])
                .map(model_id)
                .and_then(|id| by_id.get(&id))
            {
                queue.push_back(node.clone());
            }
        }
    }
    let mut visited = std::collections::HashSet::new();
    let mut result = Vec::new();
    while let Some(current) = queue.pop_front() {
        let id = model_id_of(&current);
        if visited.contains(&id) {
            continue;
        }
        visited.insert(id.clone());
        result.push(current.clone());
        for flow in flows.get(&id).cloned().unwrap_or_default() {
            if let Some(node) = get_any(&flow, &["DestinationPointer"])
                .map(model_id)
                .and_then(|dest| by_id.get(&dest))
            {
                queue.push_back(node.clone());
            }
        }
    }
    result
}

fn is_error_handler(flow: &Document) -> bool {
    get_any(flow, &["IsErrorHandler"]) == Some(&Bson::Boolean(true))
}

fn is_terminal(node: &Document) -> bool {
    matches!(
        get_str_any(node, &["$Type"]).as_deref(),
        Some("Microflows$EndEvent" | "Microflows$ExclusiveSplit" | "Microflows$ExclusiveMerge")
    )
}

fn strip_label(value: JsValue) -> JsValue {
    match value {
        JsValue::Object(entries) => {
            JsValue::Object(entries.into_iter().filter(|(k, _)| k != "label").collect())
        }
        other => other,
    }
}

fn model_id_of(document: &Document) -> String {
    document
        .get("$ID")
        .and_then(mxrs_bson::extract_id)
        .unwrap_or_default()
}

fn model_id(value: &Bson) -> String {
    match value {
        Bson::Document(document) => model_id_of(document),
        other => mxrs_bson::extract_id(other).unwrap_or_default(),
    }
}

fn short_hash(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..12].to_string()
}

/// Ports `WebOperationCompiler.operation_id` (`web_operation_compiler.rb:
/// 14-17`) — see this module's doc comment for why it lives here for now.
/// `Base64.strict_encode64(SHA256.digest(seed).byteslice(0,16))
/// .delete_suffix('==')`: standard (not URL-safe) base64 of the digest's
/// first 16 raw bytes, then the always-exactly-2 padding characters a
/// 16-byte input produces are stripped.
fn operation_id(a: &str, b: &str) -> String {
    let seed = format!("{a}/{b}");
    let digest = Sha256::digest(seed.as_bytes());
    base64_standard(&digest[0..16])
        .trim_end_matches("==")
        .to_string()
}

fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;

    fn empty_index() -> ProjectFlowIndex {
        ProjectFlowIndex {
            database_connections: HashMap::new(),
            constants: HashMap::new(),
            javascript_actions: HashMap::new(),
            nanoflows: HashMap::new(),
            associations: HashMap::new(),
            role_map: HashMap::new(),
        }
    }

    fn start_end_flow(end_type: &str) -> Document {
        doc! {
            "$Type": "Microflows$Nanoflow",
            "Name": "ACT_Do",
            "ObjectCollection": {
                "Objects": mxrs_bson::build_array(vec![
                    Bson::Document(doc! { "$ID": "11111111-1111-1111-1111-111111111111", "$Type": "Microflows$StartEvent" }),
                    Bson::Document(doc! {
                        "$ID": "22222222-2222-2222-2222-222222222222", "$Type": "Microflows$EndEvent",
                        "ReturnValue": end_type,
                    }),
                ], 3),
            },
            "Flows": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "$ID": "33333333-3333-3333-3333-333333333333",
                "OriginPointer": "11111111-1111-1111-1111-111111111111",
                "DestinationPointer": "22222222-2222-2222-2222-222222222222",
            })], 3),
        }
    }

    #[test]
    fn compiles_a_trivial_flow_to_a_declaration() {
        let mut index = empty_index();
        index
            .nanoflows
            .insert("Sales.ACT_Do".to_string(), start_end_flow("true"));
        let mut compiler = NanoflowCompiler::new(&index, None);
        let reference = compiler.reference("Sales.ACT_Do").unwrap();
        assert!(reference.starts_with("() => mxrbNanoflow_"));
        assert!(compiler.declarations().contains("const mxrbNanoflow_"));
        assert!(compiler.unsupported().is_empty());
    }

    #[test]
    fn a_missing_flow_reference_is_none() {
        let index = empty_index();
        let mut compiler = NanoflowCompiler::new(&index, None);
        assert!(compiler.reference("Sales.Bogus").is_none());
    }

    #[test]
    fn an_unsupported_action_discards_the_whole_flow() {
        let mut flow = start_end_flow("true");
        // Add an unsupported activity between start and end.
        let unsupported_node = doc! {
            "$ID": "44444444-4444-4444-4444-444444444444",
            "$Type": "Microflows$ActionActivity",
            "Action": { "$Type": "Microflows$RetrieveAction" },
        };
        if let Some(Bson::Document(oc)) = flow.get_mut("ObjectCollection") {
            if let Some(Bson::Array(objects)) = oc.get_mut("Objects") {
                objects.push(Bson::Document(unsupported_node));
            }
        }
        if let Some(Bson::Array(flows)) = flow.get_mut("Flows") {
            // Rewire start -> unsupported -> end.
            flows.clear();
            flows.push(Bson::Document(doc! {
                "OriginPointer": "11111111-1111-1111-1111-111111111111",
                "DestinationPointer": "44444444-4444-4444-4444-444444444444",
            }));
            flows.push(Bson::Document(doc! {
                "OriginPointer": "44444444-4444-4444-4444-444444444444",
                "DestinationPointer": "22222222-2222-2222-2222-222222222222",
            }));
        }
        let mut index = empty_index();
        index.nanoflows.insert("Sales.ACT_Do".to_string(), flow);
        let mut compiler = NanoflowCompiler::new(&index, None);
        assert!(compiler.reference("Sales.ACT_Do").is_none());
        assert_eq!(compiler.unsupported().len(), 1);
        assert_eq!(
            compiler.unsupported()[0].node_type,
            "Microflows$RetrieveAction"
        );
    }

    #[test]
    fn a_second_reference_call_reuses_the_memoized_program() {
        let mut index = empty_index();
        index
            .nanoflows
            .insert("Sales.ACT_Do".to_string(), start_end_flow("true"));
        let mut compiler = NanoflowCompiler::new(&index, None);
        let first = compiler.reference("Sales.ACT_Do").unwrap();
        let second = compiler.reference("Sales.ACT_Do").unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn operation_id_matches_base64_of_the_sha256_prefix() {
        let id = operation_id("Sales.MyPage", "widget1");
        assert_eq!(
            id.len(),
            22,
            "16 bytes of base64 minus the always-present '==' padding is 22 chars"
        );
        assert!(!id.contains('='));
    }
}
