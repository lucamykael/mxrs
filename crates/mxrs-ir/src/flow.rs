//! Microflow body IR — a small semantic activity list, analogous to
//! mxrb's `FlowBuilder`/`FlowBodyDsl` `@body` array (an array of
//! `{type:, ...}` hashes) rather than the low-level Mendix BSON activity
//! graph (`Microflows$ActionActivity`/`StartEvent`/`SequenceFlow`...) that
//! `mxrs_model::Microflow` stores — `mxrs-writer` is what compiles this IR
//! into that BSON graph, mirroring `Writer#build_microflow_graph`.
//!
//! Scoped activity set for this pass (mirrors mxrb's `activity_action_doc`
//! dispatch, narrowed to the plan's "minimal microflow" ask for Phase 3):
//! create/change/delete object, commit, call microflow, and a two-branch
//! decision. Not yet ported: retrieve (by source/association), create/change
//! variable, show message, loops, rescue blocks — same "widen incrementally"
//! precedent already used for `mxrs-forms`'s widget catalog and
//! `mxrs-model`'s page widgets.

/// A single attribute or association assignment inside a create/change
/// object activity. `value` is a raw Mendix expression string (e.g. `"'A-1'"`
/// for a string literal, `"$order/Number"` for a member access, or
/// `"$SomeVar"` for a variable reference) — mxrs-dsl is responsible for any
/// friendlier literal-to-expression coercion, mirroring how
/// `member_value_expr` lives in mxrb's writer rather than its DSL builder.
#[derive(Debug, Clone)]
pub struct Member {
    pub attribute: Option<String>,
    pub association: Option<String>,
    pub value: String,
}

impl Member {
    pub fn attribute(name: impl Into<String>, value: impl Into<String>) -> Self {
        Member {
            attribute: Some(name.into()),
            association: None,
            value: value.into(),
        }
    }

    pub fn association(name: impl Into<String>, value: impl Into<String>) -> Self {
        Member {
            attribute: None,
            association: Some(name.into()),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MicroflowCallMapping {
    pub parameter: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub enum Activity {
    CreateObject {
        variable: String,
        entity: String,
        members: Vec<Member>,
        commit: bool,
    },
    ChangeObject {
        variable: String,
        entity: String,
        members: Vec<Member>,
        commit: bool,
    },
    Commit {
        variable: String,
    },
    DeleteObject {
        variable: String,
    },
    CallMicroflow {
        name: String,
        result_variable: Option<String>,
        use_return: bool,
        mappings: Vec<MicroflowCallMapping>,
    },
    Decision {
        condition: String,
        true_branch: Vec<Activity>,
        false_branch: Vec<Activity>,
    },
}

#[derive(Debug, Clone)]
pub struct MicroflowDecl {
    pub name: String,
    pub documentation: String,
    pub activities: Vec<Activity>,
    /// The microflow's `return` expression (e.g. `"$order"`), or `None` for
    /// a void return.
    pub return_expression: Option<String>,
}

impl MicroflowDecl {
    pub fn new(name: impl Into<String>) -> Self {
        MicroflowDecl {
            name: name.into(),
            documentation: String::new(),
            activities: vec![],
            return_expression: None,
        }
    }
}
