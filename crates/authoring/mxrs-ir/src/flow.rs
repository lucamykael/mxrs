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
    /// A value of the qualified enumeration, e.g. `Sales.OrderStatus`.
    Enumeration(String),
}

/// The type of a flow variable. The same set a flow can return, under the
/// name a variable declaration reads better with.
pub type DataType = FlowReturnType;

impl FlowReturnType {
    /// An object of the entity `E` declares.
    pub fn object<E: crate::EntityMarker>() -> Self {
        Self::Object(E::qualified_name())
    }

    /// A list of the entity `E` declares.
    pub fn list<E: crate::EntityMarker>() -> Self {
        Self::List(E::qualified_name())
    }

    /// A value of the enumeration `E` declares.
    pub fn enumeration<E: crate::EnumerationMarker>() -> Self {
        Self::Enumeration(E::qualified_name())
    }
}

/// One value inside a [`NativeDocument`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeValue {
    Null,
    Bool(bool),
    Int32(i32),
    Int64(i64),
    Text(String),
    Document(NativeDocument),
    /// A stored list: the marker Mendix prefixes it with, then its items.
    List(i32, Vec<NativeValue>),
    /// The identity of another document of the same tree, named by where
    /// that document is from the root: `Type.ObjectType.PropertyTypes[3]`.
    /// A document says nothing of identities, so one part pointing at
    /// another says where the other is.
    Pointer(String),
    /// A stored identity that names nothing in the tree: a document outside
    /// it, or — all zeros — nothing at all.
    Identity(String),
    /// Bytes the model stores as data, not as an identity: a template's
    /// thumbnail.
    Binary(Vec<u8>),
}

impl NativeValue {
    /// The identity that points at nothing.
    pub const NOTHING: &'static str = "00000000-0000-0000-0000-000000000000";
}

impl From<bool> for NativeValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i32> for NativeValue {
    fn from(value: i32) -> Self {
        Self::Int32(value)
    }
}

impl From<i64> for NativeValue {
    fn from(value: i64) -> Self {
        Self::Int64(value)
    }
}

impl From<&str> for NativeValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for NativeValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<NativeDocument> for NativeValue {
    fn from(value: NativeDocument) -> Self {
        Self::Document(value)
    }
}

/// A model document as Mendix stores it, without identities: its type and
/// its fields, in order.
///
/// An activity whose every option the authoring surface states is carried
/// this way, so nothing the model can say about it is out of reach — and the
/// writer assigns identities when it lowers the document into the model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NativeDocument {
    pub ty: String,
    pub fields: Vec<(String, NativeValue)>,
}

impl NativeDocument {
    /// The document a pointer's `path` names from this one: this document
    /// itself for the empty path.
    pub fn at(&self, path: &str) -> Option<&NativeDocument> {
        let Some((head, rest)) = path
            .split_once('.')
            .or((!path.is_empty()).then_some((path, "")))
        else {
            return Some(self);
        };
        let (key, index) = match head.split_once('[') {
            Some((key, index)) => (key, Some(index.strip_suffix(']')?.parse::<usize>().ok()?)),
            None => (head, None),
        };
        let found = match (self.get(key)?, index) {
            (NativeValue::List(_, items), Some(index)) => items.get(index)?,
            (value, None) => value,
            _ => return None,
        };
        match found {
            NativeValue::Document(document) => document.at(rest),
            _ => None,
        }
    }

    pub fn new(ty: impl Into<String>) -> Self {
        Self {
            ty: ty.into(),
            fields: Vec::new(),
        }
    }

    /// Sets `key`, replacing its value in place when the document already
    /// has it.
    pub fn set(&mut self, key: &str, value: impl Into<NativeValue>) -> &mut Self {
        let value = value.into();
        match self.fields.iter_mut().find(|(name, _)| name == key) {
            Some((_, slot)) => *slot = value,
            None => self.fields.push((key.to_string(), value)),
        }
        self
    }

    /// The document with every whole number that fits 32 bits stored as
    /// one: the same document whichever width a version stores numbers in.
    pub fn narrowed(&self) -> NativeDocument {
        fn narrow(value: &NativeValue) -> NativeValue {
            match value {
                NativeValue::Int64(number) => {
                    i32::try_from(*number).map_or(NativeValue::Int64(*number), NativeValue::Int32)
                }
                NativeValue::Document(document) => NativeValue::Document(document.narrowed()),
                NativeValue::List(marker, items) => {
                    NativeValue::List(*marker, items.iter().map(narrow).collect())
                }
                other => other.clone(),
            }
        }
        NativeDocument {
            ty: self.ty.clone(),
            fields: self
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), narrow(value)))
                .collect(),
        }
    }

    /// Whether `other` says what this document says, whatever order either
    /// stores its fields in and whichever width its numbers are: the order
    /// of a document's fields means nothing to the model.
    pub fn says_the_same(&self, other: &NativeDocument) -> bool {
        fn canonical(document: &NativeDocument) -> NativeDocument {
            fn canonical_value(value: &NativeValue) -> NativeValue {
                match value {
                    NativeValue::Document(document) => NativeValue::Document(canonical(document)),
                    NativeValue::List(marker, items) => {
                        NativeValue::List(*marker, items.iter().map(canonical_value).collect())
                    }
                    other => other.clone(),
                }
            }
            let mut fields: Vec<(String, NativeValue)> = document
                .narrowed()
                .fields
                .iter()
                .map(|(key, field)| (key.clone(), canonical_value(field)))
                .collect();
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            NativeDocument {
                ty: document.ty.clone(),
                fields,
            }
        }
        canonical(self) == canonical(other)
    }

    /// Builder-style [`NativeDocument::set`].
    pub fn with(mut self, key: &str, value: impl Into<NativeValue>) -> Self {
        self.set(key, value);
        self
    }

    pub fn get(&self, key: &str) -> Option<&NativeValue> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut NativeValue> {
        self.fields
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            NativeValue::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The nested document under `key`, when it holds one.
    pub fn document_mut(&mut self, key: &str) -> Option<&mut NativeDocument> {
        match self.get_mut(key)? {
            NativeValue::Document(document) => Some(document),
            _ => None,
        }
    }

    /// The items of the list under `key`, when it holds one.
    pub fn list_mut(&mut self, key: &str) -> Option<&mut Vec<NativeValue>> {
        match self.get_mut(key)? {
            NativeValue::List(_, items) => Some(items),
            _ => None,
        }
    }

    /// Sets the field a dotted `path` names, descending through the nested
    /// documents on the way. A path through something that is not a document
    /// changes nothing.
    pub fn set_path(&mut self, path: &str, value: impl Into<NativeValue>) -> &mut Self {
        match path.split_once('.') {
            None => {
                self.set(path, value);
            }
            Some((head, rest)) => {
                if let Some(nested) = self.document_mut(head) {
                    nested.set_path(rest, value);
                }
            }
        }
        self
    }

    /// The value a dotted `path` names.
    pub fn get_path(&self, path: &str) -> Option<&NativeValue> {
        match path.split_once('.') {
            None => self.get(path),
            Some((head, rest)) => match self.get(head)? {
                NativeValue::Document(nested) => nested.get_path(rest),
                _ => None,
            },
        }
    }
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
    /// One action activity, stated in full: the action document the model
    /// stores for it. Every activity kind the typed variants above do not
    /// cover is declared this way, as is every option they leave out.
    Action(NativeDocument),
    /// An activity the model keeps but does not run.
    Disabled(Box<Activity>),
    /// An activity with its own answer to failing. `handler` runs when it
    /// does; a handler that does not end the flow continues with whatever
    /// follows the activity.
    OnError {
        handling: ErrorHandling,
        activity: Box<Activity>,
        handler: Vec<Activity>,
    },
    /// Ends the flow by raising the error being handled to its caller.
    RaiseError,
    /// A decision a rule makes: `arguments` are the rule's parameters, by
    /// qualified name, and the expressions passed for them.
    RuleDecision {
        rule: String,
        arguments: Vec<(String, String)>,
        true_branch: Vec<Activity>,
        false_branch: Vec<Activity>,
    },
    /// A branch per value of an enumeration expression.
    Switch {
        expression: String,
        cases: Vec<SwitchCase>,
    },
    /// A branch per value of the enumeration a rule returns; `arguments` as
    /// for [`Activity::RuleDecision`].
    RuleSwitch {
        rule: String,
        arguments: Vec<(String, String)>,
        cases: Vec<SwitchCase>,
    },
    /// Names this point of the flow, for [`Activity::Jump`] to come back —
    /// or across — to.
    Label(String),
    /// Ends this path by carrying on at the [`Activity::Label`] of that
    /// name, in the same flow or loop body.
    Jump(String),
    /// A branch per entity the object in `variable` may be an instance of.
    TypeSwitch {
        variable: String,
        cases: Vec<SwitchCase>,
    },
}

/// What an activity does about its own failure, when it is not the default
/// of rolling back and failing the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorHandling {
    /// Rolls back, then runs the handler.
    Custom,
    /// Runs the handler, keeping what the flow changed so far.
    CustomWithoutRollback,
    /// Carries on as if the activity had succeeded.
    Continue,
}

impl ErrorHandling {
    /// The name the model stores.
    pub fn native_name(self) -> &'static str {
        match self {
            ErrorHandling::Custom => "Custom",
            ErrorHandling::CustomWithoutRollback => "CustomWithoutRollBack",
            ErrorHandling::Continue => "Continue",
        }
    }

    pub fn from_native_name(name: &str) -> Option<Self> {
        Some(match name {
            "Custom" => ErrorHandling::Custom,
            "CustomWithoutRollBack" => ErrorHandling::CustomWithoutRollback,
            "Continue" => ErrorHandling::Continue,
            _ => return None,
        })
    }
}

/// One branch of a [`Activity::Switch`] or [`Activity::TypeSwitch`]: the
/// values that select it — enumeration value names, or qualified entity
/// names — and what runs when one does. The model writes the absence of a
/// value as `(empty)` for an enumeration and as an empty name for an entity.
#[derive(Debug, Clone)]
pub struct SwitchCase {
    pub values: Vec<String>,
    pub activities: Vec<Activity>,
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
    /// The module roles that may run the flow, as `Module.Role`. `None`
    /// when the declaration does not say: the model keeps what it has.
    pub allowed_roles: Option<Vec<String>>,
    /// What the declaration says the flow is related to.
    pub relations: FlowRelations,
}

/// What a flow's declaration says about the rest of the model: the flows it
/// calls, the entities it works with, and what uses it — each by qualified
/// name. Nothing here is written to the model, which already holds all of
/// it in the flow's body and in the documents that refer to the flow; a
/// build compares the two and reports where they disagree. `None` is a
/// relation the declaration does not state, and nothing is compared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowRelations {
    pub calls: Option<Vec<String>>,
    pub uses: Option<Vec<String>>,
    pub used_by: Option<Vec<String>>,
}

impl FlowRelations {
    pub fn is_stated(&self) -> bool {
        self.calls.is_some() || self.uses.is_some() || self.used_by.is_some()
    }
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
            allowed_roles: None,
            relations: FlowRelations::default(),
        }
    }
}
