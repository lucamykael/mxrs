//! Microflow body IR — a small semantic activity list, analogous to
//! mxrb's `FlowBuilder`/`FlowBodyDsl` `@body` array (an array of
//! `{type:, ...}` hashes) rather than the low-level Mendix BSON activity
//! graph (`Microflows$ActionActivity`/`StartEvent`/`SequenceFlow`...) that
//! `mxrs_model::Microflow` stores — `mxrs-writer` is what compiles this IR
//! into that BSON graph, mirroring `Writer#build_microflow_graph`.
//!
//! The authoring builders keep expressions and variables typed; this IR is
//! their storage-independent lowered form and therefore stores only the
//! canonical Mendix source, variable names, and signature/call type metadata.

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
    /// Typed authoring always supplies this. `None` is reserved for the
    /// separate functional-test instrumentation path accepting native text;
    /// project authoring rejects unchecked call expressions.
    pub value_type: Option<FlowReturnType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Error,
}

impl LogLevel {
    pub fn native_name(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Error => "Error",
        }
    }
}

/// Explicit Mendix return type paired with a flow's return expression.
/// Keeping this in the semantic IR prevents a non-void end event from being
/// persisted with `DataTypes$VoidType`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowReturnType {
    String,
    Integer,
    /// Distinct authoring tag, persisted as native IntegerType in flows.
    Long,
    Float,
    Decimal,
    Boolean,
    DateTime,
    Binary,
    Object(String),
    List(String),
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
    CreateList {
        variable: String,
        entity: String,
    },
    CallMicroflow {
        name: String,
        result_variable: Option<String>,
        /// Expected type of a captured result. Typed authoring supplies this;
        /// unchecked functional instrumentation uses its separate writer path.
        result_type: Option<FlowReturnType>,
        use_return: bool,
        mappings: Vec<MicroflowCallMapping>,
    },
    RetrieveObjects {
        entity: String,
        variable: String,
        xpath: Option<String>,
    },
    AggregateCount {
        list_variable: String,
        output_variable: String,
    },
    LogMessage {
        message: String,
        level: LogLevel,
        node: String,
    },
    ReturnValue {
        expression: String,
    },
    Decision {
        condition: String,
        true_branch: Vec<Activity>,
        false_branch: Vec<Activity>,
    },
    LoopOver {
        list_variable: String,
        iterator: String,
        activities: Vec<Activity>,
    },
    WhileLoop {
        condition: String,
        activities: Vec<Activity>,
    },
    BreakLoop,
    ContinueLoop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowParameterDecl {
    pub name: String,
    pub value_type: FlowReturnType,
    pub documentation: String,
    pub required: bool,
    pub default_value: Option<String>,
}

impl FlowParameterDecl {
    pub fn new(name: impl Into<String>, value_type: FlowReturnType) -> Self {
        Self {
            name: name.into(),
            value_type,
            documentation: String::new(),
            required: false,
            default_value: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MicroflowDecl {
    pub name: String,
    pub parameters: Vec<FlowParameterDecl>,
    pub documentation: String,
    pub activities: Vec<Activity>,
    /// Activities reached by the custom error-handler edge from the last
    /// main activity. Empty means the microflow has no rescue branch.
    pub rescue_activities: Vec<Activity>,
    /// The microflow's `return` expression (e.g. `"$order"`), or `None` for
    /// a void return.
    pub return_expression: Option<String>,
    pub return_type: Option<FlowReturnType>,
}

impl MicroflowDecl {
    pub fn new(name: impl Into<String>) -> Self {
        MicroflowDecl {
            name: name.into(),
            parameters: Vec::new(),
            documentation: String::new(),
            activities: vec![],
            rescue_activities: vec![],
            return_expression: None,
            return_type: None,
        }
    }
}
