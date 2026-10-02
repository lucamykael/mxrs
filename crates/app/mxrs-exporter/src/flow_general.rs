//! Turns any structured flow into the Rust that declares it.
//!
//! [`flow_export`](crate::flow_export) recovers the flows whose every
//! expression the typed builders can check. This is the rest: each activity
//! is read field by field and written as the builder call that states it,
//! with expressions kept the way Mendix writes them (`mx("...")`).
//!
//! Nothing is taken on trust. While the source text is written, the same
//! calls are made against the builders' own document constructors
//! ([`mxrs_dsl::flow_actions::actions`]), and the flow is only reported as
//! converted when the writer would rebuild its stored body unchanged from the
//! result. A field this reader does not know keeps the whole flow in the
//! imported model instead of being dropped.

use std::collections::{HashMap, HashSet};

use mxrs_bson::{Bson, Document};
use mxrs_dsl::flow_actions::{
    AggregateFunction, ChangeKind, ListChange, LogSeverity, MemberName, MessageKind, NativeEnum,
    OptionDefault, OptionKind, OptionSpec, SortOrder, actions,
};
use mxrs_ir::flow::{
    Activity, DataType, ErrorHandling, FlowParameterDecl, MicroflowDecl, NativeDocument, SwitchCase,
};
use mxrs_ir::{NativeValue, flow::FlowReturnType};
use mxrs_model::Microflow;
use mxrs_writer::flow_graph::{Node, structured_nodes};

use crate::{names, rust_string};

/// Why a flow could not be converted, for diagnostics.
pub(crate) type Outcome<T> = std::result::Result<T, String>;

/// What the model declares, as far as naming it from a flow goes.
pub(crate) struct Model<'a> {
    /// Qualified names of the entities a struct declares.
    pub(crate) entities: &'a HashSet<String>,
    /// `(entity, attribute)` of every attribute an accessor names.
    pub(crate) attributes: &'a HashSet<(String, String)>,
    /// Qualified names of the microflows a type names.
    pub(crate) microflows: &'a HashSet<String>,
    /// Qualified names of the nanoflows a type names.
    pub(crate) nanoflows: &'a HashSet<String>,
    /// `Module.Enumeration.Value`s an enum's variant names.
    pub(crate) enumeration_values: &'a HashSet<String>,
    /// Java actions a generated macro calls, by qualified name.
    pub(crate) java_actions: &'a HashSet<String>,
    /// Each association a struct declares a field for, and that struct's
    /// entity.
    pub(crate) association_owners: &'a HashMap<String, String>,
}

/// Reads one stored document, and notices the fields nobody asked for.
struct Fields<'a> {
    document: &'a Document,
    seen: HashSet<&'a str>,
}

impl<'a> Fields<'a> {
    fn new(document: &'a Document) -> Self {
        Self {
            document,
            seen: HashSet::from(["$ID", "$Type"]),
        }
    }

    fn kind(&self) -> &'a str {
        self.document.get_str("$Type").unwrap_or_default()
    }

    fn value(&mut self, key: &'a str) -> Outcome<&'a Bson> {
        self.seen.insert(key);
        self.document
            .get(key)
            .ok_or_else(|| format!("{} has no {key}", self.kind()))
    }

    fn text(&mut self, key: &'a str) -> Outcome<&'a str> {
        match self.value(key)? {
            Bson::String(text) => Ok(text),
            other => Err(format!("{}.{key} is not text: {other:?}", self.kind())),
        }
    }

    fn boolean(&mut self, key: &'a str) -> Outcome<bool> {
        match self.value(key)? {
            Bson::Boolean(value) => Ok(*value),
            other => Err(format!("{}.{key} is not a boolean: {other:?}", self.kind())),
        }
    }

    fn document(&mut self, key: &'a str) -> Outcome<&'a Document> {
        match self.value(key)? {
            Bson::Document(document) => Ok(document),
            other => Err(format!(
                "{}.{key} is not a document: {other:?}",
                self.kind()
            )),
        }
    }

    /// The documents of a stored list, which must carry `marker`.
    fn items(&mut self, key: &'a str, marker: i32) -> Outcome<Vec<&'a Document>> {
        let Bson::Array(values) = self.value(key)? else {
            return Err(format!("{}.{key} is not a list", self.kind()));
        };
        match values.first() {
            Some(Bson::Int32(found)) if *found == marker => {}
            other => {
                return Err(format!(
                    "{}.{key} has list marker {other:?}, expected {marker}",
                    self.kind()
                ));
            }
        }
        values[1..]
            .iter()
            .map(|value| {
                value
                    .as_document()
                    .ok_or_else(|| format!("{}.{key} holds a non-document", self.kind()))
            })
            .collect()
    }

    /// Accepts a field that is absent or `null`: one the builders leave to
    /// the model.
    fn absent_or_null(&mut self, key: &'a str) -> Outcome<()> {
        self.seen.insert(key);
        match self.document.get(key) {
            None | Some(Bson::Null) => Ok(()),
            Some(other) => Err(format!("{}.{key} is set: {other:?}", self.kind())),
        }
    }

    /// Accepts a field only some model versions store, as long as it holds
    /// nothing: an empty `Expressions$NoExpression` beside a text expression.
    fn empty_expression_model(&mut self, key: &'a str) -> Outcome<()> {
        self.seen.insert(key);
        match self.document.get(key) {
            None | Some(Bson::Null) => Ok(()),
            Some(Bson::Document(model))
                if model.get_str("$Type").ok() == Some("Expressions$NoExpression")
                    && model
                        .keys()
                        .all(|key| matches!(key.as_str(), "$ID" | "$Type")) =>
            {
                Ok(())
            }
            Some(other) => Err(format!("{}.{key} is set: {other:?}", self.kind())),
        }
    }

    /// Fails when the document holds a field nothing read.
    fn finish(self) -> Outcome<()> {
        match self
            .document
            .keys()
            .find(|key| !self.seen.contains(key.as_str()))
        {
            Some(key) => Err(format!("{} has an unread field {key}", self.kind())),
            None => Ok(()),
        }
    }
}

/// One activity as the document that declares it and the statement that
/// builds that document.
struct Statement {
    document: NativeDocument,
    /// The call, starting at `flow.`; later lines belong to its closure.
    lines: Vec<String>,
    /// The variable the activity declares.
    declares: Option<String>,
}

struct Converter<'a> {
    model: &'a Model<'a>,
    default_handling: &'static str,
    /// The names given to the points the flow's paths converge on, by the
    /// identity of the node each one precedes.
    labels: HashMap<String, String>,
}

/// The Mendix variables in scope and the Rust bindings that hold them.
type Scope = HashMap<String, String>;

fn expression(text: &str) -> String {
    format!("mx({})", rust_string(text))
}

fn enumeration<E: NativeEnum>(value: &str, what: &str) -> Outcome<&'static str> {
    E::VARIANTS
        .iter()
        .find(|(native, _)| *native == value)
        .map(|(_, source)| *source)
        .ok_or_else(|| format!("unknown {what} {value:?}"))
}

/// A closure whose body is `lines`, or the empty one when there are none.
fn closure(receiver: &str, lines: Vec<String>) -> Vec<String> {
    if lines.is_empty() {
        return vec!["|_| {}".to_string()];
    }
    let mut out = vec![format!("|{receiver}| {{")];
    out.extend(indented(lines));
    out.push("}".to_string());
    out
}

/// One level deeper. rustfmt lays the generated source out, but leaves a
/// statement it cannot fit — one holding a long expression — exactly as it
/// was written, so what is written is already indented.
fn indented(lines: Vec<String>) -> impl Iterator<Item = String> {
    lines.into_iter().map(|line| format!("    {line}"))
}

/// `head` followed by a trailing closure and the closing of the call.
fn call_with_closure(head: String, closure_lines: Vec<String>) -> Vec<String> {
    let mut lines = closure_lines;
    let first = lines.remove(0);
    let mut out = vec![format!("{head}{first}")];
    out.extend(lines);
    let last = out.last_mut().expect("a call has at least one line");
    last.push_str(");");
    out
}

impl Converter<'_> {
    fn variable(scope: &Scope, name: &str) -> String {
        match scope.get(name) {
            Some(binding) => format!("&{binding}"),
            None => format!("&var({})", rust_string(name)),
        }
    }

    /// Brings a variable the flow declares into scope, under the Rust
    /// binding that holds it. A name already in scope, or one Rust cannot
    /// bind, is a flow the model itself would reject.
    fn declare(scope: &mut Scope, name: &str) -> Outcome<String> {
        let binding = crate::flow_export::binding(name)
            .ok_or_else(|| format!("variable {name:?} is not an identifier"))?;
        if scope.insert(name.to_string(), binding.clone()).is_some() {
            return Err(format!("variable {name} is declared twice in one scope"));
        }
        Ok(binding)
    }

    fn entity(&self, name: &str) -> String {
        match crate::flow_export::marker(name) {
            Some(marker) if self.model.entities.contains(name) => format!("Ref::<{marker}>::new()"),
            _ => rust_string(name),
        }
    }

    /// The accessor naming `Module.Entity.Attribute`, when an entity of this
    /// project declares it.
    fn accessor(&self, attribute: &str) -> Option<String> {
        let (entity, name) = attribute.rsplit_once('.')?;
        self.model
            .attributes
            .contains(&(entity.to_string(), name.to_string()))
            .then(|| names::attribute(entity, name))
    }

    fn attribute(&self, attribute: &str) -> String {
        self.accessor(attribute)
            .unwrap_or_else(|| rust_string(attribute))
    }

    fn member(&self, attribute: &str, association: &str) -> Outcome<(MemberName, String)> {
        match (attribute.is_empty(), association.is_empty()) {
            (false, true) => Ok((
                MemberName::attribute(attribute),
                self.accessor(attribute).unwrap_or_else(|| {
                    format!("MemberName::attribute({})", rust_string(attribute))
                }),
            )),
            (true, false) => Ok((
                MemberName::association(association),
                format!("MemberName::association({})", rust_string(association)),
            )),
            _ => Err(format!(
                "a member names exactly one of an attribute and an association: {attribute:?}/{association:?}"
            )),
        }
    }

    fn microflow(&self, name: &str) -> String {
        match crate::flow_export::flow_marker(name) {
            Some(marker) if self.model.microflows.contains(name) => {
                format!("MicroflowRef::<{marker}>::new()")
            }
            _ => rust_string(name),
        }
    }

    fn nanoflow(&self, name: &str) -> String {
        match crate::flow_export::nanoflow_marker(name) {
            Some(marker) if self.model.nanoflows.contains(name) => {
                format!("NanoflowRef::<{marker}>::new()")
            }
            _ => rust_string(name),
        }
    }

    fn data_type_source(&self, ty: &DataType) -> String {
        match ty {
            DataType::String => "DataType::String".into(),
            DataType::Integer => "DataType::Integer".into(),
            DataType::Long => "DataType::Long".into(),
            DataType::Float => "DataType::Float".into(),
            DataType::Decimal => "DataType::Decimal".into(),
            DataType::Boolean => "DataType::Boolean".into(),
            DataType::DateTime => "DataType::DateTime".into(),
            DataType::Binary => "DataType::Binary".into(),
            DataType::Object(entity) | DataType::List(entity) => {
                let constructor = if matches!(ty, DataType::Object(_)) {
                    ("object", "Object")
                } else {
                    ("list", "List")
                };
                match crate::flow_export::marker(entity) {
                    Some(marker) if self.model.entities.contains(entity) => {
                        format!("DataType::{}::<{marker}>()", constructor.0)
                    }
                    _ => format!(
                        "DataType::{}({}.into())",
                        constructor.1,
                        rust_string(entity)
                    ),
                }
            }
            DataType::Enumeration(name) => {
                format!("DataType::Enumeration({}.into())", rust_string(name))
            }
        }
    }

    /// States the options of `specs` that differ from their defaults, on the
    /// document and as setter calls on `receiver`.
    fn options(
        fields: &mut Fields<'_>,
        specs: &[OptionSpec],
        document: &mut NativeDocument,
        receiver: &str,
        lines: &mut Vec<String>,
    ) -> Outcome<()> {
        for spec in specs {
            // The options table is written against the action document; a
            // nested option is read by the activity that owns the nesting.
            if spec.path.contains('.') {
                continue;
            }
            let key: &'static str = spec.path;
            if !fields.document.contains_key(key) {
                fields.seen.insert(key);
                if spec.always {
                    return Err(format!("{} has no {key}", fields.kind()));
                }
                continue;
            }
            let (value, rendered) = match spec.kind {
                OptionKind::Bool => {
                    let value = fields.boolean(key)?;
                    (NativeValue::Bool(value), value.to_string())
                }
                OptionKind::Text => {
                    let value = fields.text(key)?;
                    (NativeValue::Text(value.to_string()), rust_string(value))
                }
                OptionKind::Expression => {
                    let value = fields.text(key)?;
                    (NativeValue::Text(value.to_string()), expression(value))
                }
                OptionKind::Enumeration(variants) => {
                    let value = fields.text(key)?;
                    let source = variants
                        .iter()
                        .find(|(native, _)| *native == value)
                        .map(|(_, source)| *source)
                        .ok_or_else(|| format!("unknown {} {value:?}", spec.setter))?;
                    (NativeValue::Text(value.to_string()), source.to_string())
                }
            };
            let default = match spec.default {
                OptionDefault::Bool(value) => NativeValue::Bool(value),
                OptionDefault::Text(value) => NativeValue::Text(value.to_string()),
            };
            if value != default {
                document.set_path(spec.path, value);
                lines.push(format!("{receiver}.{}({rendered});", spec.setter));
            }
        }
        Ok(())
    }

    fn data_type(document: &Document) -> Outcome<DataType> {
        let entity = || {
            document
                .get_str("Entity")
                .map(str::to_string)
                .map_err(|_| "a type without its entity".to_string())
        };
        Ok(match document.get_str("$Type").unwrap_or_default() {
            "DataTypes$StringType" => DataType::String,
            "DataTypes$IntegerType" => DataType::Long,
            "DataTypes$BooleanType" => DataType::Boolean,
            "DataTypes$FloatType" => DataType::Float,
            "DataTypes$DecimalType" => DataType::Decimal,
            "DataTypes$DateTimeType" => DataType::DateTime,
            "DataTypes$BinaryType" => DataType::Binary,
            "DataTypes$ObjectType" => DataType::Object(entity()?),
            "DataTypes$ListType" => DataType::List(entity()?),
            "DataTypes$EnumerationType" => DataType::Enumeration(
                document
                    .get_str("Enumeration")
                    .map_err(|_| "an enumeration type without its enumeration".to_string())?
                    .to_string(),
            ),
            other => return Err(format!("unsupported data type {other}")),
        })
    }

    fn change_items(
        &self,
        fields: &mut Fields<'_>,
        document: &mut NativeDocument,
        receiver: &str,
        lines: &mut Vec<String>,
    ) -> Outcome<()> {
        for item in fields.items("Items", 2)? {
            let mut item_fields = Fields::new(item);
            if item_fields.kind() != "Microflows$ChangeActionItem" {
                return Err(format!("unexpected change item {}", item_fields.kind()));
            }
            let attribute = item_fields.text("Attribute")?;
            let association = item_fields.text("Association")?;
            let kind = item_fields.text("Type")?;
            let value = item_fields.text("Value")?;
            item_fields.empty_expression_model("ValueModel")?;
            item_fields.finish()?;
            let (member, member_source) = self.member(attribute, association)?;
            let kind = ChangeKind::from_native(kind)
                .ok_or_else(|| format!("unknown change type {kind:?}"))?;
            let method = match kind {
                ChangeKind::Set => "set",
                ChangeKind::Add => "add",
                ChangeKind::Remove => "remove",
            };
            if let Some(items) = document.list_mut("Items") {
                items.push(NativeValue::Document(actions::change_item(
                    &member, kind, value,
                )));
            }
            lines.push(format!(
                "{receiver}.{method}({member_source}, {});",
                expression(value)
            ));
        }
        Ok(())
    }

    fn sortings(
        &self,
        list: &Document,
        receiver: &str,
        method: &str,
        lines: &mut Vec<String>,
    ) -> Outcome<Vec<NativeDocument>> {
        let mut list_fields = Fields::new(list);
        let mut documents = Vec::new();
        for sorting in list_fields.items("Sortings", 2)? {
            let mut sorting_fields = Fields::new(sorting);
            let reference = sorting_fields.document("AttributeRef")?;
            let order = sorting_fields.text("SortOrder")?;
            sorting_fields.finish()?;
            let mut reference_fields = Fields::new(reference);
            let attribute = reference_fields.text("Attribute")?;
            reference_fields.absent_or_null("EntityRef")?;
            reference_fields.finish()?;
            let order_value =
                SortOrder::from_native(order).ok_or_else(|| format!("unknown order {order:?}"))?;
            documents.push(actions::sorting(attribute, order_value));
            lines.push(format!(
                "{receiver}.{method}({}, {});",
                self.attribute(attribute),
                enumeration::<SortOrder>(order, "sort order")?
            ));
        }
        list_fields.finish()?;
        Ok(documents)
    }

    fn template_parameters(
        template: &mut Fields<'_>,
        receiver: &str,
        lines: &mut Vec<String>,
    ) -> Outcome<Vec<NativeDocument>> {
        let mut documents = Vec::new();
        for parameter in template.items("Parameters", 2)? {
            let mut parameter_fields = Fields::new(parameter);
            let value = parameter_fields.text("Expression")?;
            parameter_fields.finish()?;
            documents.push(actions::template_parameter(value));
            lines.push(format!("{receiver}.parameter({});", expression(value)));
        }
        Ok(documents)
    }

    /// One action activity.
    fn action(&self, action: &Document, scope: &Scope) -> Outcome<Statement> {
        let mut fields = Fields::new(action);
        let kind = fields.kind();
        // How the action answers its own failure is said around it, by the
        // caller that knows whether it has a handler.
        fields.text("ErrorHandlingType")?;
        let statement = match kind {
            "Microflows$CastAction" => {
                let name = fields.text("VariableName")?;
                Statement {
                    document: actions::cast(name),
                    lines: vec![format!("flow.cast({});", rust_string(name))],
                    declares: Some(name.to_string()),
                }
            }
            "Microflows$CreateVariableAction" => {
                let name = fields.text("VariableName")?;
                let value = fields.text("InitialValue")?;
                let ty = Self::data_type(fields.document("VariableType")?)?;
                fields.empty_expression_model("InitialValueModel")?;
                Statement {
                    document: actions::create_variable(name, &ty, value),
                    lines: vec![format!(
                        "flow.create_variable({}, {}, {});",
                        rust_string(name),
                        self.data_type_source(&ty),
                        expression(value)
                    )],
                    declares: Some(name.to_string()),
                }
            }
            "Microflows$ChangeVariableAction" => {
                let name = fields.text("ChangeVariableName")?;
                let value = fields.text("Value")?;
                fields.empty_expression_model("ValueModel")?;
                Statement {
                    document: actions::change_variable(name, value),
                    lines: vec![format!(
                        "flow.change_variable({}, {});",
                        Self::variable(scope, name),
                        expression(value)
                    )],
                    declares: None,
                }
            }
            "Microflows$CreateChangeAction" => {
                let name = fields.text("VariableName")?;
                let entity = fields.text("Entity")?;
                let mut document = actions::create(name, entity);
                let mut lines = Vec::new();
                self.change_items(&mut fields, &mut document, "create", &mut lines)?;
                Self::options(
                    &mut fields,
                    mxrs_dsl::flow_actions::CHANGE_OPTIONS,
                    &mut document,
                    "create",
                    &mut lines,
                )?;
                Statement {
                    document,
                    lines: call_with_closure(
                        format!(
                            "flow.create({}, {}, ",
                            rust_string(name),
                            self.entity(entity)
                        ),
                        closure("create", lines),
                    ),
                    declares: Some(name.to_string()),
                }
            }
            "Microflows$ChangeAction" => {
                let name = fields.text("ChangeVariableName")?;
                let mut document = actions::change(name);
                let mut lines = Vec::new();
                self.change_items(&mut fields, &mut document, "change", &mut lines)?;
                Self::options(
                    &mut fields,
                    mxrs_dsl::flow_actions::CHANGE_OPTIONS,
                    &mut document,
                    "change",
                    &mut lines,
                )?;
                Statement {
                    document,
                    lines: call_with_closure(
                        format!("flow.change({}, ", Self::variable(scope, name)),
                        closure("change", lines),
                    ),
                    declares: None,
                }
            }
            "Microflows$CommitAction" | "Microflows$DeleteAction" | "Microflows$RollbackAction" => {
                let (key, specs, plain, with, receiver): (_, &[OptionSpec], _, _, _) = match kind {
                    "Microflows$CommitAction" => (
                        "CommitVariableName",
                        mxrs_dsl::flow_actions::COMMIT_OPTIONS,
                        "commit",
                        "commit_with",
                        "commit",
                    ),
                    "Microflows$DeleteAction" => (
                        "DeleteVariableName",
                        mxrs_dsl::flow_actions::REFRESH_OPTIONS,
                        "delete_object",
                        "delete_with",
                        "delete",
                    ),
                    _ => (
                        "RollbackVariableName",
                        mxrs_dsl::flow_actions::REFRESH_OPTIONS,
                        "rollback",
                        "rollback_with",
                        "rollback",
                    ),
                };
                let name = fields.text(key)?;
                let mut document = match kind {
                    "Microflows$CommitAction" => actions::commit(name),
                    "Microflows$DeleteAction" => actions::delete(name),
                    _ => actions::rollback(name),
                };
                let mut lines = Vec::new();
                Self::options(&mut fields, specs, &mut document, receiver, &mut lines)?;
                let variable = Self::variable(scope, name);
                Statement {
                    document,
                    lines: if lines.is_empty() {
                        vec![format!("flow.{plain}({variable});")]
                    } else {
                        call_with_closure(
                            format!("flow.{with}({variable}, "),
                            closure(receiver, lines),
                        )
                    },
                    declares: None,
                }
            }
            "Microflows$RetrieveAction" => {
                let name = fields.text("ResultVariableName")?;
                let source = fields.document("RetrieveSource")?;
                let mut source_fields = Fields::new(source);
                match source_fields.kind() {
                    "Microflows$AssociationRetrieveSource" => {
                        let association = source_fields.text("AssociationId")?;
                        let start = source_fields.text("StartVariableName")?;
                        source_fields.finish()?;
                        Statement {
                            document: actions::retrieve_associated(name, start, association),
                            lines: vec![format!(
                                "flow.retrieve_associated({}, {}, {});",
                                rust_string(name),
                                Self::variable(scope, start),
                                rust_string(association)
                            )],
                            declares: Some(name.to_string()),
                        }
                    }
                    "Microflows$DatabaseRetrieveSource" => {
                        let entity = source_fields.text("Entity")?;
                        let xpath = source_fields.text("XpathConstraint")?;
                        let mut document = actions::retrieve(name, entity);
                        let mut lines = Vec::new();
                        if !xpath.is_empty() {
                            document.set_path("RetrieveSource.XpathConstraint", xpath);
                            lines.push(format!("retrieve.xpath({});", rust_string(xpath)));
                        }
                        let sortings = self.sortings(
                            source_fields.document("NewSortings")?,
                            "retrieve",
                            "sort_by",
                            &mut lines,
                        )?;
                        if let Some(list) = document
                            .document_mut("RetrieveSource")
                            .and_then(|source| source.document_mut("NewSortings"))
                            .and_then(|sortings| sortings.list_mut("Sortings"))
                        {
                            list.extend(sortings.into_iter().map(NativeValue::Document));
                        }
                        let mut range = Fields::new(source_fields.document("Range")?);
                        match range.kind() {
                            "Microflows$ConstantRange" => {
                                if range.boolean("SingleObject")? {
                                    document.set_path(
                                        "RetrieveSource.Range",
                                        NativeDocument::new("Microflows$ConstantRange")
                                            .with("SingleObject", true),
                                    );
                                    lines.push("retrieve.first();".to_string());
                                }
                            }
                            "Microflows$CustomRange" => {
                                let limit = range.text("LimitExpression")?;
                                let offset = range.text("OffsetExpression")?;
                                // Some models keep the flag of the range
                                // this one replaced; only unset, it says
                                // nothing the limit does not.
                                range.seen.insert("SingleObject");
                                if !matches!(
                                    range.document.get("SingleObject"),
                                    None | Some(Bson::Boolean(false))
                                ) {
                                    return Err(
                                        "a custom range that also asks for a single object"
                                            .to_string(),
                                    );
                                }
                                document.set_path(
                                    "RetrieveSource.Range",
                                    actions::custom_range(limit, offset),
                                );
                                lines.push(format!(
                                    "retrieve.range({}, {});",
                                    expression(limit),
                                    expression(offset)
                                ));
                            }
                            other => return Err(format!("unsupported retrieve range {other}")),
                        }
                        range.finish()?;
                        source_fields.finish()?;
                        Statement {
                            document,
                            lines: call_with_closure(
                                format!(
                                    "flow.retrieve({}, {}, ",
                                    rust_string(name),
                                    self.entity(entity)
                                ),
                                closure("retrieve", lines),
                            ),
                            declares: Some(name.to_string()),
                        }
                    }
                    other => return Err(format!("unsupported retrieve source {other}")),
                }
            }
            "Microflows$CreateListAction" => {
                let name = fields.text("VariableName")?;
                let entity = fields.text("Entity")?;
                Statement {
                    document: actions::create_list(name, entity),
                    lines: vec![format!(
                        "flow.create_list_of({}, {});",
                        rust_string(name),
                        self.entity(entity)
                    )],
                    declares: Some(name.to_string()),
                }
            }
            "Microflows$ChangeListAction" => {
                let name = fields.text("ChangeVariableName")?;
                let change = fields.text("Type")?;
                let value = fields.text("Value")?;
                let change_value = ListChange::from_native(change)
                    .ok_or_else(|| format!("unknown list change {change:?}"))?;
                Statement {
                    document: actions::change_list(name, change_value, value),
                    lines: vec![format!(
                        "flow.change_list({}, {}, {});",
                        Self::variable(scope, name),
                        enumeration::<ListChange>(change, "list change")?,
                        expression(value)
                    )],
                    declares: None,
                }
            }
            "Microflows$ListOperationsAction" => {
                let name = fields.text("ResultVariableName")?;
                let operation = fields.document("NewOperation")?;
                self.list_operation(name, operation, scope)?
            }
            "Microflows$AggregateAction" => {
                let name = fields.text("VariableName")?;
                let list = fields.text("AggregateVariableName")?;
                let function = fields.text("AggregateFunction")?;
                let attribute = fields.text("Attribute")?;
                let function_value = AggregateFunction::from_native(function)
                    .ok_or_else(|| format!("unknown aggregate {function:?}"))?;
                if function_value == AggregateFunction::Reduce {
                    return Err("a reduce aggregate has no builder yet".to_string());
                }
                let mut document = actions::aggregate(name, list, function_value);
                let mut lines = Vec::new();
                if !attribute.is_empty() {
                    document.set("Attribute", attribute);
                    lines.push(format!(
                        "aggregate.attribute({});",
                        self.attribute(attribute)
                    ));
                }
                // Fields only newer models store: an expression to aggregate
                // instead of an attribute, and what `reduce` starts from.
                for key in ["ReduceInitialValueExpression", "ReduceReturnDataType"] {
                    fields.seen.insert(key);
                }
                let uses_expression = if action.contains_key("UseExpression") {
                    fields.boolean("UseExpression")?
                } else {
                    false
                };
                if uses_expression {
                    let value = fields.text("Expression")?;
                    document.set("Expression", value).set("UseExpression", true);
                    lines.push(format!("aggregate.expression({});", expression(value)));
                } else {
                    fields.seen.insert("Expression");
                }
                Statement {
                    document,
                    lines: call_with_closure(
                        format!(
                            "flow.aggregate({}, {}, {}, ",
                            rust_string(name),
                            Self::variable(scope, list),
                            enumeration::<AggregateFunction>(function, "aggregate")?
                        ),
                        closure("aggregate", lines),
                    ),
                    declares: Some(name.to_string()),
                }
            }
            "Microflows$LogMessageAction" => {
                let level = fields.text("Level")?;
                let node = fields.text("Node")?;
                let mut template = Fields::new(fields.document("MessageTemplate")?);
                let text = template.text("Text")?;
                let level_value = LogSeverity::from_native(level)
                    .ok_or_else(|| format!("unknown log level {level:?}"))?;
                let mut document = actions::log(level_value, node, text);
                let mut lines = Vec::new();
                let parameters = Self::template_parameters(&mut template, "log", &mut lines)?;
                template.finish()?;
                if let Some(list) = document
                    .document_mut("MessageTemplate")
                    .and_then(|template| template.list_mut("Parameters"))
                {
                    list.extend(parameters.into_iter().map(NativeValue::Document));
                }
                Self::options(
                    &mut fields,
                    mxrs_dsl::flow_actions::LOG_OPTIONS,
                    &mut document,
                    "log",
                    &mut lines,
                )?;
                Statement {
                    document,
                    lines: call_with_closure(
                        format!(
                            "flow.log({}, {}, {}, ",
                            enumeration::<LogSeverity>(level, "log level")?,
                            expression(node),
                            rust_string(text)
                        ),
                        closure("log", lines),
                    ),
                    declares: None,
                }
            }
            "Microflows$MicroflowCallAction" => {
                let result = fields.text("ResultVariableName")?;
                let uses_result = fields.boolean("UseReturnVariable")?;
                let mut call = Fields::new(fields.document("MicroflowCall")?);
                let target = call.text("Microflow")?;
                let keeps = uses_result && !result.is_empty();
                let mut document = actions::call(target, keeps.then_some(result));
                let mut lines = Vec::new();
                if !uses_result {
                    document
                        .set("UseReturnVariable", false)
                        .set("ResultVariableName", result);
                    lines.push(format!("call.discard_result({});", rust_string(result)));
                }
                call.seen.insert("QueueSettings");
                if let Some(Bson::Document(queue)) = call.document.get("QueueSettings") {
                    let mut queue_fields = Fields::new(queue);
                    let name = queue_fields.text("Queue")?;
                    queue_fields.absent_or_null("Retry")?;
                    queue_fields.finish()?;
                    document.set_path("MicroflowCall.QueueSettings", actions::queue_settings(name));
                    lines.push(format!("call.queue({});", rust_string(name)));
                }
                for mapping in call.items("ParameterMappings", 2)? {
                    let mut mapping_fields = Fields::new(mapping);
                    let parameter = mapping_fields.text("Parameter")?;
                    let argument = mapping_fields.text("Argument")?;
                    mapping_fields.empty_expression_model("ArgumentModel")?;
                    mapping_fields.finish()?;
                    if let Some(list) = document
                        .document_mut("MicroflowCall")
                        .and_then(|call| call.list_mut("ParameterMappings"))
                    {
                        list.push(NativeValue::Document(
                            NativeDocument::new("Microflows$MicroflowCallParameterMapping")
                                .with("Parameter", parameter)
                                .with("Argument", argument),
                        ));
                    }
                    // The builder qualifies a bare name with the target, so a
                    // parameter of the target is written by its own name.
                    let short = parameter
                        .strip_prefix(target)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))
                        .unwrap_or(parameter);
                    lines.push(format!(
                        "call.argument({}, {});",
                        rust_string(short),
                        expression(argument)
                    ));
                }
                call.finish()?;
                let head = if keeps {
                    format!(
                        "flow.call_into({}, {}, ",
                        rust_string(result),
                        self.microflow(target)
                    )
                } else {
                    format!("flow.call({}, ", self.microflow(target))
                };
                Statement {
                    document,
                    lines: call_with_closure(head, closure("call", lines)),
                    declares: keeps.then(|| result.to_string()),
                }
            }
            "Microflows$JavaActionCallAction" => {
                let java_action = fields.text("JavaAction")?;
                let result = fields.text("ResultVariableName")?;
                let uses_result = fields.boolean("UseReturnVariable")?;
                fields.absent_or_null("QueueSettings")?;
                let keeps = uses_result && !result.is_empty();
                let mut document = actions::call_java(java_action, keeps.then_some(result));
                let mut lines = Vec::new();
                if !uses_result {
                    document
                        .set("UseReturnVariable", false)
                        .set("ResultVariableName", result);
                    lines.push(format!("call.discard_result({});", rust_string(result)));
                }
                for mapping in fields.items("ParameterMappings", 2)? {
                    let mut mapping_fields = Fields::new(mapping);
                    let parameter = mapping_fields.text("Parameter")?;
                    let mut value = Fields::new(mapping_fields.document("Value")?);
                    mapping_fields.finish()?;
                    let short = parameter
                        .strip_prefix(java_action)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))
                        .unwrap_or(parameter);
                    let (value_document, line) = match value.kind() {
                        "Microflows$BasicCodeActionParameterValue" => {
                            let argument = value.text("Argument")?;
                            (
                                NativeDocument::new("Microflows$BasicCodeActionParameterValue")
                                    .with("Argument", argument),
                                format!(
                                    "call.argument({}, {});",
                                    rust_string(short),
                                    expression(argument)
                                ),
                            )
                        }
                        "Microflows$EntityTypeCodeActionParameterValue" => {
                            let entity = value.text("Entity")?;
                            (
                                NativeDocument::new(
                                    "Microflows$EntityTypeCodeActionParameterValue",
                                )
                                .with("Entity", entity),
                                format!(
                                    "call.entity_argument({}, {});",
                                    rust_string(short),
                                    self.entity(entity)
                                ),
                            )
                        }
                        "Microflows$MicroflowParameterValue" => {
                            let microflow = value.text("Microflow")?;
                            (
                                NativeDocument::new("Microflows$MicroflowParameterValue")
                                    .with("Microflow", microflow),
                                format!(
                                    "call.microflow_argument({}, {});",
                                    rust_string(short),
                                    self.microflow(microflow)
                                ),
                            )
                        }
                        other => return Err(format!("unsupported Java argument {other}")),
                    };
                    value.finish()?;
                    if let Some(list) = document.list_mut("ParameterMappings") {
                        list.push(NativeValue::Document(
                            NativeDocument::new("Microflows$JavaActionParameterMapping")
                                .with("Parameter", parameter)
                                .with("Value", value_document),
                        ));
                    }
                    lines.push(line);
                }
                let head = if keeps {
                    format!(
                        "flow.call_java_into({}, {}, ",
                        rust_string(result),
                        rust_string(java_action)
                    )
                } else {
                    format!("flow.call_java({}, ", rust_string(java_action))
                };
                Statement {
                    document,
                    lines: call_with_closure(head, closure("call", lines)),
                    declares: keeps.then(|| result.to_string()),
                }
            }
            "Microflows$NanoflowCallAction" => {
                let result = fields.text("OutputVariableName")?;
                let uses_result = fields.boolean("UseReturnVariable")?;
                let mut call = Fields::new(fields.document("NanoflowCall")?);
                let target = call.text("Nanoflow")?;
                let keeps = uses_result && !result.is_empty();
                let mut document = actions::call_nanoflow(target, keeps.then_some(result));
                let mut lines = Vec::new();
                if !uses_result {
                    document
                        .set("UseReturnVariable", false)
                        .set("OutputVariableName", result);
                    lines.push(format!("call.discard_result({});", rust_string(result)));
                }
                for mapping in call.items("ParameterMappings", 2)? {
                    let mut mapping_fields = Fields::new(mapping);
                    let parameter = mapping_fields.text("Parameter")?;
                    let argument = mapping_fields.text("Argument")?;
                    mapping_fields.empty_expression_model("ArgumentModel")?;
                    mapping_fields.finish()?;
                    if let Some(list) = document
                        .document_mut("NanoflowCall")
                        .and_then(|call| call.list_mut("ParameterMappings"))
                    {
                        list.push(NativeValue::Document(
                            NativeDocument::new("Microflows$NanoflowCallParameterMapping")
                                .with("Parameter", parameter)
                                .with("Argument", argument),
                        ));
                    }
                    let short = parameter
                        .strip_prefix(target)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))
                        .unwrap_or(parameter);
                    lines.push(format!(
                        "call.argument({}, {});",
                        rust_string(short),
                        expression(argument)
                    ));
                }
                call.finish()?;
                let head = if keeps {
                    format!(
                        "flow.call_nanoflow_into({}, {}, ",
                        rust_string(result),
                        self.nanoflow(target)
                    )
                } else {
                    format!("flow.call_nanoflow({}, ", self.nanoflow(target))
                };
                Statement {
                    document,
                    lines: call_with_closure(head, closure("call", lines)),
                    declares: keeps.then(|| result.to_string()),
                }
            }
            "Microflows$JavaScriptActionCallAction" => {
                let javascript_action = fields.text("JavaScriptAction")?;
                let result = fields.text("OutputVariableName")?;
                let uses_result = fields.boolean("UseReturnVariable")?;
                let keeps = uses_result && !result.is_empty();
                let mut document =
                    actions::call_javascript(javascript_action, keeps.then_some(result));
                let mut lines = Vec::new();
                if !uses_result {
                    document
                        .set("UseReturnVariable", false)
                        .set("OutputVariableName", result);
                    lines.push(format!("call.discard_result({});", rust_string(result)));
                }
                for mapping in fields.items("ParameterMappings", 2)? {
                    let mut mapping_fields = Fields::new(mapping);
                    let parameter = mapping_fields.text("Parameter")?;
                    let mut value = Fields::new(mapping_fields.document("ParameterValue")?);
                    mapping_fields.finish()?;
                    let short = parameter
                        .strip_prefix(javascript_action)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))
                        .unwrap_or(parameter);
                    let (value_document, line) = match value.kind() {
                        "Microflows$BasicCodeActionParameterValue" => {
                            let argument = value.text("Argument")?;
                            (
                                NativeDocument::new("Microflows$BasicCodeActionParameterValue")
                                    .with("Argument", argument),
                                format!(
                                    "call.argument({}, {});",
                                    rust_string(short),
                                    expression(argument)
                                ),
                            )
                        }
                        "Microflows$EntityTypeCodeActionParameterValue" => {
                            let entity = value.text("Entity")?;
                            (
                                NativeDocument::new(
                                    "Microflows$EntityTypeCodeActionParameterValue",
                                )
                                .with("Entity", entity),
                                format!(
                                    "call.entity_argument({}, {});",
                                    rust_string(short),
                                    self.entity(entity)
                                ),
                            )
                        }
                        "Microflows$NanoflowParameterValue" => {
                            let nanoflow = value.text("Nanoflow")?;
                            (
                                NativeDocument::new("Microflows$NanoflowParameterValue")
                                    .with("Nanoflow", nanoflow),
                                format!(
                                    "call.nanoflow_argument({}, {});",
                                    rust_string(short),
                                    self.nanoflow(nanoflow)
                                ),
                            )
                        }
                        other => return Err(format!("unsupported JavaScript argument {other}")),
                    };
                    value.finish()?;
                    if let Some(list) = document.list_mut("ParameterMappings") {
                        list.push(NativeValue::Document(
                            NativeDocument::new("Microflows$JavaScriptActionParameterMapping")
                                .with("Parameter", parameter)
                                .with("ParameterValue", value_document),
                        ));
                    }
                    lines.push(line);
                }
                let head = if keeps {
                    format!(
                        "flow.call_javascript_into({}, {}, ",
                        rust_string(result),
                        rust_string(javascript_action)
                    )
                } else {
                    format!("flow.call_javascript({}, ", rust_string(javascript_action))
                };
                Statement {
                    document,
                    lines: call_with_closure(head, closure("call", lines)),
                    declares: keeps.then(|| result.to_string()),
                }
            }
            "Microflows$ShowMessageAction" => {
                let message_kind = fields.text("Type")?;
                let kind_value = MessageKind::from_native(message_kind)
                    .ok_or_else(|| format!("unknown message type {message_kind:?}"))?;
                let mut document = actions::show_message(kind_value);
                let mut lines = Vec::new();
                let mut template = Fields::new(fields.document("Template")?);
                let mut text = Fields::new(template.document("Text")?);
                for translation in text.items("Items", 3)? {
                    let mut translation_fields = Fields::new(translation);
                    let language = translation_fields.text("LanguageCode")?;
                    let content = translation_fields.text("Text")?;
                    translation_fields.finish()?;
                    if let Some(items) = document
                        .document_mut("Template")
                        .and_then(|template| template.document_mut("Text"))
                        .and_then(|text| text.list_mut("Items"))
                    {
                        items.push(NativeValue::Document(
                            NativeDocument::new("Texts$Translation")
                                .with("LanguageCode", language)
                                .with("Text", content),
                        ));
                    }
                    lines.push(format!(
                        "message.text({}, {});",
                        rust_string(language),
                        rust_string(content)
                    ));
                }
                text.finish()?;
                let parameters = Self::template_parameters(&mut template, "message", &mut lines)?;
                template.finish()?;
                if let Some(list) = document
                    .document_mut("Template")
                    .and_then(|template| template.list_mut("Parameters"))
                {
                    list.extend(parameters.into_iter().map(NativeValue::Document));
                }
                Self::options(
                    &mut fields,
                    mxrs_dsl::flow_actions::MESSAGE_OPTIONS,
                    &mut document,
                    "message",
                    &mut lines,
                )?;
                Statement {
                    document,
                    lines: call_with_closure(
                        format!(
                            "flow.show_message({}, ",
                            enumeration::<MessageKind>(message_kind, "message type")?
                        ),
                        closure("message", lines),
                    ),
                    declares: None,
                }
            }
            "Microflows$CloseFormAction" => {
                let pages = fields.text("NumberOfPagesToClose")?;
                Statement {
                    document: actions::close_page(pages),
                    lines: vec![if pages.is_empty() {
                        "flow.close_page();".to_string()
                    } else {
                        format!("flow.close_pages({});", expression(pages))
                    }],
                    declares: None,
                }
            }
            "Microflows$ShowFormAction" => {
                let mut settings = Fields::new(fields.document("FormSettings")?);
                let page = settings.text("Form")?;
                let mut document = actions::show_page(page);
                let mut lines = Vec::new();
                settings.seen.insert("TitleOverride");
                if let Some(Bson::Document(title)) = settings.document.get("TitleOverride") {
                    let mut title_fields = Fields::new(title);
                    if title_fields.kind() != "Microflows$TextTemplate" {
                        return Err(format!("{page} is opened under an unknown kind of title"));
                    }
                    let mut declared = actions::title_override();
                    let mut text = Fields::new(title_fields.document("Text")?);
                    for translation in text.items("Items", 3)? {
                        let mut translation_fields = Fields::new(translation);
                        let language = translation_fields.text("LanguageCode")?;
                        let content = translation_fields.text("Text")?;
                        translation_fields.finish()?;
                        if let Some(items) = declared
                            .document_mut("Text")
                            .and_then(|text| text.list_mut("Items"))
                        {
                            items.push(NativeValue::Document(
                                NativeDocument::new("Texts$Translation")
                                    .with("LanguageCode", language)
                                    .with("Text", content),
                            ));
                        }
                        lines.push(format!(
                            "page.title({}, {});",
                            rust_string(language),
                            rust_string(content)
                        ));
                    }
                    text.finish()?;
                    for parameter in title_fields.items("Parameters", 2)? {
                        let mut parameter_fields = Fields::new(parameter);
                        let value = parameter_fields.text("Expression")?;
                        parameter_fields.finish()?;
                        if let Some(parameters) = declared.list_mut("Parameters") {
                            parameters
                                .push(NativeValue::Document(actions::template_parameter(value)));
                        }
                        lines.push(format!("page.title_parameter({});", expression(value)));
                    }
                    title_fields.finish()?;
                    document.set_path("FormSettings.TitleOverride", declared);
                } else if !matches!(
                    settings.document.get("TitleOverride"),
                    None | Some(Bson::Null)
                ) {
                    return Err(format!("{page} is opened under an unknown kind of title"));
                }
                for mapping in settings.items("ParameterMappings", 2)? {
                    let mut mapping_fields = Fields::new(mapping);
                    let parameter = mapping_fields.text("Parameter")?;
                    let argument = mapping_fields.text("Argument")?;
                    // A page variable the argument could be bound to instead:
                    // only its empty form is something the builder leaves out.
                    if let Ok(variable) = mapping_fields.document("Variable") {
                        let unset = variable.iter().all(|(key, value)| match value {
                            _ if matches!(key.as_str(), "$ID" | "$Type") => true,
                            Bson::String(text) => text.is_empty(),
                            Bson::Boolean(flag) => !flag,
                            _ => false,
                        });
                        if !unset {
                            return Err(format!("{page} binds an argument to a page variable"));
                        }
                    }
                    mapping_fields.finish()?;
                    if let Some(list) = document
                        .document_mut("FormSettings")
                        .and_then(|settings| settings.list_mut("ParameterMappings"))
                    {
                        list.push(NativeValue::Document(
                            NativeDocument::new("Forms$PageParameterMapping")
                                .with("Argument", argument)
                                .with("Parameter", parameter),
                        ));
                    }
                    let short = parameter
                        .strip_prefix(page)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))
                        .unwrap_or(parameter);
                    lines.push(format!(
                        "page.argument({}, {});",
                        rust_string(short),
                        expression(argument)
                    ));
                }
                settings.finish()?;
                Self::options(
                    &mut fields,
                    mxrs_dsl::flow_actions::PAGE_OPTIONS,
                    &mut document,
                    "page",
                    &mut lines,
                )?;
                Statement {
                    document,
                    lines: call_with_closure(
                        format!("flow.show_page({}, ", rust_string(page)),
                        closure("page", lines),
                    ),
                    declares: None,
                }
            }
            // An activity no builder covers yet is still declared: as the
            // document the model stores for it, field by field.
            _ => {
                let document = native(action, true)?;
                for key in action.keys() {
                    fields.seen.insert(key);
                }
                let mut lines = native_source(&document);
                lines[0] = format!("flow.native_action({}", lines[0]);
                lines
                    .last_mut()
                    .expect("a document has a line")
                    .push_str(");");
                let mut document = document;
                document.set("ErrorHandlingType", "Rollback");
                Statement {
                    document,
                    lines,
                    declares: None,
                }
            }
        };
        fields.finish()?;
        let mut statement = statement;
        if let Some(lines) = self.activity_macro(kind, action, scope) {
            statement.lines = lines;
        }
        Ok(statement)
    }

    fn list_operation(
        &self,
        name: &str,
        operation: &Document,
        scope: &Scope,
    ) -> Outcome<Statement> {
        let mut fields = Fields::new(operation);
        let kind = fields
            .kind()
            .strip_prefix("Microflows$")
            .unwrap_or_default();
        let list = fields.text("ListName")?;
        let result = rust_string(name);
        let list_source = Self::variable(scope, list);
        let (operation_document, line) = match kind {
            "Head" | "Tail" => (
                actions::unary_operation(kind, list),
                format!(
                    "flow.list_{}({result}, {list_source});",
                    kind.to_ascii_lowercase()
                ),
            ),
            "Union" | "Intersect" | "Subtract" | "Contains" | "Equals" => {
                let second = fields.text("SecondListOrObjectName")?;
                (
                    actions::binary_operation(kind, list, second),
                    format!(
                        "flow.list_{}({result}, {list_source}, {});",
                        kind.to_ascii_lowercase(),
                        Self::variable(scope, second)
                    ),
                )
            }
            "Find" | "Filter" => {
                let attribute = fields.text("Attribute")?;
                let association = fields.text("Association")?;
                let value = fields.text("Expression")?;
                let (member, member_source) = self.member(attribute, association)?;
                (
                    actions::member_operation(kind, list, &member, value),
                    format!(
                        "flow.list_{}({result}, {list_source}, {member_source}, {});",
                        kind.to_ascii_lowercase(),
                        expression(value)
                    ),
                )
            }
            "FindByExpression" | "FilterByExpression" => {
                let value = fields.text("Expression")?;
                let method = if kind == "FindByExpression" {
                    "list_find_by"
                } else {
                    "list_filter_by"
                };
                (
                    actions::expression_operation(kind, list, value),
                    format!(
                        "flow.{method}({result}, {list_source}, {});",
                        expression(value)
                    ),
                )
            }
            "ListRange" => {
                let mut range = Fields::new(fields.document("CustomRange")?);
                let limit = range.text("LimitExpression")?;
                let offset = range.text("OffsetExpression")?;
                range.finish()?;
                (
                    actions::range_operation(list, limit, offset),
                    format!(
                        "flow.list_range({result}, {list_source}, {}, {});",
                        expression(limit),
                        expression(offset)
                    ),
                )
            }
            "Sort" => {
                let mut lines = Vec::new();
                let sortings =
                    self.sortings(fields.document("Sortings")?, "sort", "by", &mut lines)?;
                fields.finish()?;
                let mut operation_document = actions::sort_operation(list);
                if let Some(list) = operation_document
                    .document_mut("Sortings")
                    .and_then(|sortings| sortings.list_mut("Sortings"))
                {
                    list.extend(sortings.into_iter().map(NativeValue::Document));
                }
                return Ok(Statement {
                    document: actions::list_operation(name, operation_document),
                    lines: call_with_closure(
                        format!("flow.list_sort({result}, {list_source}, "),
                        closure("sort", lines),
                    ),
                    declares: Some(name.to_string()),
                });
            }
            other => return Err(format!("list operation {other} has no builder yet")),
        };
        fields.finish()?;
        Ok(Statement {
            document: actions::list_operation(name, operation_document),
            lines: vec![line],
            declares: Some(name.to_string()),
        })
    }

    /// A sequence of nodes: one block of the flow.
    fn block(
        &self,
        nodes: &[Node<'_>],
        scope: &mut Scope,
        lines: &mut Vec<String>,
        nested: bool,
    ) -> Outcome<Vec<Activity>> {
        let mut activities = Vec::new();
        for node in nodes {
            let (node, handler) = match node {
                Node::Handled { node, handler } => (node.as_ref(), Some(handler.as_slice())),
                node => (node, None),
            };
            // What the handler sees is what was there before the activity
            // it answers for.
            let before = scope.clone();
            let mut statement = Vec::new();
            let Some(activity) = self.statement(node, scope, &mut statement, nested)? else {
                lines.extend(statement);
                continue;
            };
            let handling = match node {
                Node::Simple(document) => document
                    .get_document("Action")
                    .ok()
                    .and_then(|action| action.get_str("ErrorHandlingType").ok()),
                Node::Loop { node, .. } => node.get_str("ErrorHandlingType").ok(),
                _ => None,
            }
            .unwrap_or(self.default_handling);
            let activity = match (ErrorHandling::from_native_name(handling), handler) {
                (None, None) if handling == self.default_handling => activity,
                (Some(ErrorHandling::Continue), None) => {
                    modify(&mut statement, vec!["continue_on_error()".to_string()])?;
                    Activity::OnError {
                        handling: ErrorHandling::Continue,
                        activity: Box::new(activity),
                        handler: Vec::new(),
                    }
                }
                (
                    Some(handling @ (ErrorHandling::Custom | ErrorHandling::CustomWithoutRollback)),
                    Some(handler),
                ) => {
                    let mut handler_lines = Vec::new();
                    let handler =
                        self.block(handler, &mut before.clone(), &mut handler_lines, true)?;
                    let method = if handling == ErrorHandling::Custom {
                        "on_error"
                    } else {
                        "on_error_without_rollback"
                    };
                    let mut call = closure("flow", handler_lines);
                    call[0] = format!("{method}({}", call[0]);
                    call.last_mut().expect("a closure has a line").push(')');
                    modify(&mut statement, call)?;
                    Activity::OnError {
                        handling,
                        activity: Box::new(activity),
                        handler,
                    }
                }
                (_, Some(_)) => {
                    return Err(format!(
                        "an activity with an error handler handles errors with {handling}"
                    ));
                }
                (_, None) => {
                    return Err(format!(
                        "an activity handles errors with {handling} but has no handler"
                    ));
                }
            };
            lines.extend(statement);
            activities.push(activity);
        }
        Ok(activities)
    }

    /// Converts one node into the builder call that declares it. `None`
    /// when the node needs no statement of its own: the flow's last end
    /// event, whose return the caller states.
    fn statement(
        &self,
        node: &Node<'_>,
        scope: &mut Scope,
        lines: &mut Vec<String>,
        nested: bool,
    ) -> Outcome<Option<Activity>> {
        let mut activities = Vec::new();
        {
            match node {
                Node::Handled { .. } => {
                    return Err("an error handler on an error handler".to_string());
                }
                Node::Label(target) => {
                    let name = self
                        .labels
                        .get(target)
                        .cloned()
                        .ok_or("a point the reading did not name")?;
                    lines.push(format!("flow.label({});", rust_string(&name)));
                    activities.push(Activity::Label(name));
                }
                Node::Jump(target) => {
                    let name = self
                        .labels
                        .get(target)
                        .cloned()
                        .ok_or("a jump to a point nothing names")?;
                    lines.push(format!("flow.jump({});", rust_string(&name)));
                    activities.push(Activity::Jump(name));
                }
                Node::Switch { split, cases } => {
                    let inheritance =
                        split.get_str("$Type").ok() == Some("Microflows$InheritanceSplit");
                    let mut case_lines = Vec::new();
                    let mut declared = Vec::new();
                    for case in cases {
                        let mut branch_lines = Vec::new();
                        let branch =
                            self.block(&case.body, &mut scope.clone(), &mut branch_lines, true)?;
                        let head = match case.values.as_slice() {
                            [value] if value.is_empty() && inheritance => "on.empty(".to_string(),
                            [value] if value == "(empty)" && !inheritance => {
                                "on.empty(".to_string()
                            }
                            [value] if inheritance => format!("on.case({}, ", self.entity(value)),
                            [value] => format!("on.case({}, ", rust_string(value)),
                            values if inheritance => {
                                return Err(format!(
                                    "one branch for {} entities has no builder yet",
                                    values.len()
                                ));
                            }
                            values => format!(
                                "on.cases([{}], ",
                                values
                                    .iter()
                                    .map(|value| rust_string(value))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        };
                        case_lines.extend(call_with_closure(head, closure("flow", branch_lines)));
                        declared.push(SwitchCase {
                            values: case.values.clone(),
                            activities: branch,
                        });
                    }
                    if inheritance {
                        let mut split_fields = node_fields(split, &["Caption", "Documentation"])?;
                        let variable = split_fields.text("SplitVariableName")?;
                        split_fields.finish()?;
                        lines.extend(call_with_closure(
                            format!("flow.switch_type({}, ", Self::variable(scope, variable)),
                            closure("on", case_lines),
                        ));
                        activities.push(Activity::TypeSwitch {
                            variable: variable.to_string(),
                            cases: declared,
                        });
                    } else {
                        match self.split_condition(split)? {
                            Condition::Expression(condition) => {
                                lines.extend(call_with_closure(
                                    format!("flow.switch({}, ", expression(condition)),
                                    closure("on", case_lines),
                                ));
                                activities.push(Activity::Switch {
                                    expression: condition.to_string(),
                                    cases: declared,
                                });
                            }
                            Condition::Rule { rule, arguments } => {
                                let mut rule_lines =
                                    closure("rule", rule_arguments(rule, &arguments));
                                rule_lines
                                    .last_mut()
                                    .expect("a closure has a line")
                                    .push(',');
                                let mut on_lines = closure("on", case_lines);
                                on_lines.last_mut().expect("a closure has a line").push(',');
                                lines.push("flow.switch_by_rule(".to_string());
                                lines.push(format!("    {},", rust_string(rule)));
                                lines.extend(indented(rule_lines));
                                lines.extend(indented(on_lines));
                                lines.push(");".to_string());
                                activities.push(Activity::RuleSwitch {
                                    rule: rule.to_string(),
                                    arguments: arguments
                                        .iter()
                                        .map(|(parameter, argument)| {
                                            (parameter.to_string(), argument.to_string())
                                        })
                                        .collect(),
                                    cases: declared,
                                });
                            }
                        }
                    }
                }
                Node::Simple(document) => match document.get_str("$Type").unwrap_or_default() {
                    "Microflows$ErrorEvent" => {
                        node_fields(document, &[])?.finish()?;
                        activities.push(Activity::RaiseError);
                        lines.push("flow.raise_error();".to_string());
                    }
                    "Microflows$EndEvent" => {
                        node_fields(document, &["Documentation", "ReturnValue"])?.finish()?;
                        let returned = document.get_str("ReturnValue").unwrap_or_default();
                        // The flow's own last end event is its return, which
                        // the caller states; one inside a block ends the flow
                        // from there.
                        if nested {
                            activities.push(Activity::ReturnValue {
                                expression: returned.to_string(),
                            });
                            lines.push(if returned.is_empty() {
                                "flow.end();".to_string()
                            } else {
                                format!("flow.return_with({});", expression(returned))
                            });
                        }
                    }
                    "Microflows$BreakEvent" => {
                        node_fields(document, &[])?.finish()?;
                        activities.push(Activity::BreakLoop);
                        lines.push("flow.break_loop();".to_string());
                    }
                    "Microflows$ContinueEvent" => {
                        node_fields(document, &[])?.finish()?;
                        activities.push(Activity::ContinueLoop);
                        lines.push("flow.continue_loop();".to_string());
                    }
                    "Microflows$ActionActivity" => {
                        node_fields(
                            document,
                            &[
                                "Action",
                                "AutoGenerateCaption",
                                "BackgroundColor",
                                "Caption",
                                "Disabled",
                                "Documentation",
                            ],
                        )?
                        .finish()?;
                        let disabled = document.get_bool("Disabled").unwrap_or(false);
                        let action = document
                            .get_document("Action")
                            .map_err(|_| "an activity without an action".to_string())?;
                        let statement = self.action(action, scope)?;

                        let mut statement_lines = statement.lines;
                        if let Some(name) = &statement.declares {
                            let binding = Self::declare(scope, name)?;
                            statement_lines[0] = format!("let {binding} = {}", statement_lines[0]);
                        }
                        let mut activity = Activity::Action(statement.document);
                        if disabled {
                            modify(&mut statement_lines, vec!["disabled()".to_string()])?;
                            activity = Activity::Disabled(Box::new(activity));
                        }
                        lines.extend(statement_lines);
                        activities.push(activity);
                    }
                    other => return Err(format!("unexpected node {other}")),
                },
                Node::Decision { split, yes, no } => {
                    let condition = match self.split_condition(split)? {
                        Condition::Expression(condition) => condition,
                        Condition::Rule { rule, arguments } => {
                            let branch =
                                |nodes: &[Node<'_>]| -> Outcome<(Vec<Activity>, Vec<String>)> {
                                    let mut branch_lines = Vec::new();
                                    let activities = self.block(
                                        nodes,
                                        &mut scope.clone(),
                                        &mut branch_lines,
                                        true,
                                    )?;
                                    Ok((activities, closure("flow", branch_lines)))
                                };
                            let (true_branch, mut yes_lines) = branch(yes)?;
                            let (false_branch, mut no_lines) = branch(no)?;
                            let mut rule_lines = closure("rule", rule_arguments(rule, &arguments));
                            for closure in [&mut rule_lines, &mut yes_lines, &mut no_lines] {
                                closure.last_mut().expect("a closure has a line").push(',');
                            }
                            lines.push("flow.decision_by_rule(".to_string());
                            lines.push(format!("    {},", rust_string(rule)));
                            lines.extend(indented(rule_lines));
                            lines.extend(indented(yes_lines));
                            lines.extend(indented(no_lines));
                            lines.push(");".to_string());
                            return Ok(Some(Activity::RuleDecision {
                                rule: rule.to_string(),
                                arguments: arguments
                                    .iter()
                                    .map(|(parameter, argument)| {
                                        (parameter.to_string(), argument.to_string())
                                    })
                                    .collect(),
                                true_branch,
                                false_branch,
                            }));
                        }
                    };
                    let branch = |nodes: &[Node<'_>]| -> Outcome<(Vec<Activity>, Vec<String>)> {
                        let mut branch_lines = Vec::new();
                        let activities =
                            self.block(nodes, &mut scope.clone(), &mut branch_lines, true)?;
                        Ok((activities, closure("flow", branch_lines)))
                    };
                    let (true_branch, mut yes_lines) = branch(yes)?;
                    let (false_branch, mut no_lines) = branch(no)?;
                    lines.push("flow.decision(".to_string());
                    lines.push(format!("    {},", expression(condition)));
                    yes_lines
                        .last_mut()
                        .expect("a closure has a line")
                        .push(',');
                    no_lines.last_mut().expect("a closure has a line").push(',');
                    lines.extend(indented(yes_lines));
                    lines.extend(indented(no_lines));
                    lines.push(");".to_string());
                    activities.push(Activity::Decision {
                        condition: condition.to_string(),
                        true_branch,
                        false_branch,
                    });
                }
                Node::Loop { node, body } => {
                    let mut loop_fields =
                        node_fields(node, &["Documentation", "LoopSource", "ObjectCollection"])?;
                    loop_fields.text("ErrorHandlingType")?;
                    loop_fields.finish()?;
                    let source = node
                        .get_document("LoopSource")
                        .map_err(|_| "a loop without a source".to_string())?;
                    let mut body_scope = scope.clone();
                    let mut body_lines = Vec::new();
                    match source.get_str("$Type").unwrap_or_default() {
                        "Microflows$IterableList" => {
                            let mut source_fields = Fields::new(source);
                            let list = source_fields.text("ListVariableName")?;
                            let iterator = source_fields.text("VariableName")?;
                            source_fields.finish()?;
                            let item = Self::declare(&mut body_scope, iterator)?;
                            let loop_activities =
                                self.block(body, &mut body_scope, &mut body_lines, true)?;
                            lines.push(format!(
                                "flow.for_each({}, {}, |flow, {item}| {{",
                                Self::variable(scope, list),
                                rust_string(iterator)
                            ));
                            lines.extend(indented(body_lines));
                            lines.push("});".to_string());
                            activities.push(Activity::LoopOver {
                                list_variable: list.to_string(),
                                iterator: iterator.to_string(),
                                activities: loop_activities,
                            });
                        }
                        "Microflows$WhileLoopCondition" => {
                            let mut source_fields = Fields::new(source);
                            let condition = source_fields.text("WhileExpression")?;
                            // The caption drawn on the loop: the model's.
                            source_fields.seen.insert("Caption");
                            source_fields.finish()?;
                            let loop_activities =
                                self.block(body, &mut body_scope, &mut body_lines, true)?;
                            lines.push(format!(
                                "flow.while_loop({}, |flow| {{",
                                expression(condition)
                            ));
                            lines.extend(indented(body_lines));
                            lines.push("});".to_string());
                            activities.push(Activity::WhileLoop {
                                condition: condition.to_string(),
                                activities: loop_activities,
                            });
                        }
                        other => return Err(format!("unsupported loop source {other}")),
                    }
                }
            }
        }
        Ok(activities.pop())
    }

    /// What an exclusive split decides on.
    fn split_condition<'a>(&self, split: &'a Document) -> Outcome<Condition<'a>> {
        let mut split_fields = node_fields(split, &["Caption", "Documentation", "SplitCondition"])?;
        if split_fields.text("ErrorHandlingType")? != self.default_handling {
            return Err("a split with its own error handling".to_string());
        }
        split_fields.finish()?;
        let condition = split
            .get_document("SplitCondition")
            .map_err(|_| "a split without a condition".to_string())?;
        let mut condition_fields = Fields::new(condition);
        match condition_fields.kind() {
            "Microflows$ExpressionSplitCondition" => {
                let expression = condition_fields.text("Expression")?;
                condition_fields.finish()?;
                Ok(Condition::Expression(expression))
            }
            "Microflows$RuleSplitCondition" => {
                let mut call = Fields::new(condition_fields.document("RuleCall")?);
                condition_fields.finish()?;
                let rule = call.text("Microflow")?;
                let mut arguments = Vec::new();
                for mapping in call.items("ParameterMappings", 2)? {
                    let mut mapping_fields = Fields::new(mapping);
                    arguments.push((
                        mapping_fields.text("Parameter")?,
                        mapping_fields.text("Argument")?,
                    ));
                    mapping_fields.finish()?;
                }
                call.finish()?;
                Ok(Condition::Rule { rule, arguments })
            }
            other => Err(format!("a split decided by {other} has no builder yet")),
        }
    }
}

/// The statements that pass a rule its arguments.
fn rule_arguments(rule: &str, arguments: &[(&str, &str)]) -> Vec<String> {
    arguments
        .iter()
        .map(|(parameter, argument)| {
            // The builder qualifies a bare name with the rule.
            let short = parameter
                .strip_prefix(rule)
                .and_then(|rest| rest.strip_prefix('.'))
                .filter(|name| !name.contains('.'))
                .unwrap_or(parameter);
            format!(
                "rule.argument({}, {});",
                rust_string(short),
                expression(argument)
            )
        })
        .collect()
}

/// What a split decides on: an expression, or a rule called with arguments
/// (its qualified parameter names and the expressions passed for them).
enum Condition<'a> {
    Expression(&'a str),
    Rule {
        rule: &'a str,
        arguments: Vec<(&'a str, &'a str)>,
    },
}

/// A stored document as the authoring surface carries one: its type and its
/// fields in order, without identities. `action` leaves out how the action
/// answers failing, which is said around it.
fn native(document: &Document, action: bool) -> Outcome<NativeDocument> {
    let ty = document
        .get_str("$Type")
        .map_err(|_| "a document without a type".to_string())?;
    let mut out = NativeDocument::new(ty);
    for (key, value) in document {
        if matches!(key.as_str(), "$ID" | "$Type") || (action && key == "ErrorHandlingType") {
            continue;
        }
        out.set(key, native_value(value, ty, key)?);
    }
    Ok(out)
}

fn native_value(value: &Bson, ty: &str, key: &str) -> Outcome<NativeValue> {
    Ok(match value {
        Bson::Null => NativeValue::Null,
        Bson::Boolean(value) => NativeValue::Bool(*value),
        Bson::Int32(value) => NativeValue::Int32(*value),
        Bson::Int64(value) => NativeValue::Int64(*value),
        Bson::String(value) => NativeValue::Text(value.clone()),
        Bson::Document(value) => NativeValue::Document(native(value, false)?),
        Bson::Array(values) => {
            let Some(Bson::Int32(marker)) = values.first() else {
                return Err(format!("{ty}.{key} is a list without a marker"));
            };
            NativeValue::List(
                *marker,
                values[1..]
                    .iter()
                    .map(|value| native_value(value, ty, key))
                    .collect::<Outcome<_>>()?,
            )
        }
        other => {
            return Err(format!(
                "{ty}.{key} holds a value nothing here can state: {other:?}"
            ));
        }
    })
}

/// The expression that builds `document`, one field to a line.
fn native_source(document: &NativeDocument) -> Vec<String> {
    let mut lines = vec![format!(
        "NativeDocument::new({})",
        rust_string(&document.ty)
    )];
    for (key, value) in &document.fields {
        let mut value_lines = native_value_source(value);
        value_lines[0] = format!(".with({}, {}", rust_string(key), value_lines[0]);
        value_lines
            .last_mut()
            .expect("a value has a line")
            .push(')');
        lines.extend(indented(value_lines));
    }
    lines
}

fn native_value_source(value: &NativeValue) -> Vec<String> {
    match value {
        NativeValue::Null => vec!["NativeValue::Null".to_string()],
        NativeValue::Bool(value) => vec![value.to_string()],
        NativeValue::Int32(value) => vec![format!("{value}_i32")],
        NativeValue::Int64(value) => vec![format!("{value}_i64")],
        NativeValue::Text(value) => vec![rust_string(value)],
        NativeValue::Document(document) => native_source(document),
        NativeValue::List(marker, items) if items.is_empty() => {
            vec![format!("NativeValue::List({marker}, Vec::new())")]
        }
        NativeValue::List(marker, items) => {
            let mut lines = vec![
                "NativeValue::List(".to_string(),
                format!("    {marker},"),
                "    vec![".to_string(),
            ];
            for item in items {
                let mut item_lines = native_value_source(item);
                let last = item_lines.last_mut().expect("a value has a line");
                if !matches!(item, NativeValue::Null) {
                    last.push_str(".into()");
                }
                last.push(',');
                lines.extend(indented(indented(item_lines).collect()));
            }
            lines.push("    ],".to_string());
            lines.push(")".to_string());
            lines
        }
    }
}

/// Whether a statement's first line is an activity macro (`let order =
/// create_object!(...`), not a builder call.
fn is_activity_macro(line: &str) -> bool {
    let call = line
        .trim_start()
        .strip_prefix("let ")
        .and_then(|rest| rest.split_once(" = "))
        .map_or(line.trim_start(), |(_, call)| call);
    call.find("!(").is_some_and(|bang| {
        call[..bang]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '_')
    })
}

/// Puts a modifier between `flow` and the builder call a statement makes:
/// `flow.call(..)` becomes `flow.<modifier>.call(..)`. The modifier may span
/// lines; the statement keeps whatever it binds.
fn modify(statement: &mut Vec<String>, modifier: Vec<String>) -> Outcome<()> {
    let first = statement
        .first()
        .ok_or("an activity without a statement")?
        .clone();
    // An activity written as its macro is told about itself by the
    // statement before it: the builder applies the modifier to the next
    // activity it declares.
    if is_activity_macro(&first) {
        let mut lines = modifier;
        lines[0] = format!("flow.{}", lines[0]);
        lines.last_mut().expect("a modifier has a line").push(';');
        statement.splice(0..0, lines);
        return Ok(());
    }
    let at = first
        .find("flow.")
        .ok_or("a statement that is not a builder call")?
        + "flow.".len();
    let (head, rest) = first.split_at(at);
    let mut lines = modifier;
    lines[0] = format!("{head}{}", lines[0]);
    let last = lines.last_mut().expect("a modifier has a line");
    last.push('.');
    last.push_str(rest);
    statement.splice(0..1, lines);
    Ok(())
}

/// Reads the fields a graph node carries around what it does: where it is
/// drawn and what it is captioned. They stay the model's when the node is
/// rebuilt in place; a field outside this set is one nothing here knows.
fn node_fields<'a>(node: &'a Document, own: &[&'a str]) -> Outcome<Fields<'a>> {
    let mut fields = Fields::new(node);
    for key in ["RelativeMiddlePoint", "Size"] {
        fields.value(key)?;
    }
    for key in own {
        fields.seen.insert(key);
    }
    Ok(fields)
}

/// The last end event of the flow's own body, when the body closes with one
/// rather than with branches that each end on their own.
fn final_return<'a>(nodes: &'a [Node<'a>]) -> Option<&'a Document> {
    match nodes.last()? {
        Node::Simple(end) if end.get_str("$Type").ok() == Some("Microflows$EndEvent") => Some(end),
        _ => None,
    }
}

/// Names every point a jump goes to before anything is written, in the
/// order the points are read: a jump may come before the point it goes to,
/// as when one branch runs into the middle of the next.
fn label_names(nodes: &[Node<'_>]) -> HashMap<String, String> {
    fn walk(nodes: &[Node<'_>], names: &mut HashMap<String, String>) {
        for node in nodes {
            match node {
                Node::Label(target) => {
                    let name = format!("point_{}", names.len() + 1);
                    names.insert(target.clone(), name);
                }
                Node::Decision { yes, no, .. } => {
                    walk(yes, names);
                    walk(no, names);
                }
                Node::Switch { cases, .. } => {
                    for case in cases {
                        walk(&case.body, names);
                    }
                }
                Node::Loop { body, .. } => walk(body, names),
                Node::Handled { node, handler } => {
                    walk(std::slice::from_ref(node.as_ref()), names);
                    walk(handler, names);
                }
                Node::Simple(_) | Node::Jump(_) => {}
            }
        }
    }
    let mut names = HashMap::new();
    walk(nodes, &mut names);
    names
}

/// Converts one flow document. On success: its declaration, and the lines of
/// the function body that builds it.
pub(crate) fn convert(
    module: &str,
    document: &Document,
    model: &Model<'_>,
) -> Outcome<(MicroflowDecl, Vec<String>)> {
    let nodes = structured_nodes(document).ok_or("its graph is not structured")?;
    let flow = Microflow::from_bson(document);
    let name = flow.name.as_ref().ok_or("it has no name")?;
    let converter = Converter {
        model,
        // What an activity does about failing when nothing says otherwise.
        default_handling: if document.get_str("$Type").ok() == Some("Microflows$Nanoflow") {
            "Abort"
        } else {
            "Rollback"
        },
        labels: label_names(&nodes),
    };
    let mut declaration = MicroflowDecl::new(name);
    declaration.documentation = flow.documentation.clone();
    let mut lines = Vec::new();
    let mut scope = Scope::new();
    let mut seen = HashSet::new();

    for parameter in &flow.parameters {
        if parameter.get_str("$Type").ok() != Some("Microflows$MicroflowParameter") {
            return Err("an unexpected parameter document".to_string());
        }
        let name = parameter
            .get_str("Name")
            .map_err(|_| "a parameter without a name".to_string())?;
        if !seen.insert(name.to_string()) {
            return Err(format!("two parameters named {name}"));
        }
        let ty = Converter::data_type(
            parameter
                .get_document("VariableType")
                .map_err(|_| format!("parameter {name} has no type"))?,
        )?;
        let mut declared = FlowParameterDecl::new(name, ty.clone());
        declared.documentation = parameter
            .get_str("Documentation")
            .unwrap_or_default()
            .to_string();
        declared.required = parameter.get_bool("IsRequired").unwrap_or(false);
        let mut options = Vec::new();
        if !declared.documentation.is_empty() {
            options.push(format!(
                "parameter.documentation({});",
                rust_string(&declared.documentation)
            ));
        }
        if declared.required {
            options.push("parameter.required(true);".to_string());
        }
        if let Ok(default) = parameter.get_str("DefaultValue")
            && !default.is_empty()
        {
            declared.default_value = Some(default.to_string());
            options.push(format!("parameter.default_value({});", expression(default)));
        }
        let mut call = call_with_closure(
            format!(
                "flow.parameter_of({}, {}, ",
                rust_string(name),
                converter.data_type_source(&ty)
            ),
            closure("parameter", options),
        );
        let binding = Converter::declare(&mut scope, name)?;
        call[0] = format!("let {binding} = {}", call[0]);
        lines.extend(call);
        declaration.parameters.push(declared);
    }

    let return_type = flow
        .return_type_document
        .as_ref()
        .ok_or("it has no return type")?;
    if return_type.get_str("$Type").ok() != Some("DataTypes$VoidType") {
        let ty = Converter::data_type(return_type)?;
        lines.push(format!(
            "flow.returns({});",
            converter.data_type_source(&ty)
        ));
        declaration.return_type = Some(ty);
    }

    let body = nodes
        .get(1..)
        .ok_or_else(|| "it has no start event".to_string())?;
    let (body, last) = match final_return(body) {
        Some(end) => (&body[..body.len() - 1], Some(end)),
        None => (body, None),
    };
    declaration.activities = converter.block(body, &mut scope, &mut lines, false)?;
    // A body that closes with returning branches has no end event of its
    // own: each of those branches was written with its own return above.
    if let Some(end) = last {
        let returned = end.get_str("ReturnValue").unwrap_or_default();
        if !returned.is_empty() {
            lines.push(format!("flow.return_with({});", expression(returned)));
            declaration.return_expression = Some(returned.to_string());
        }
    }
    if matches!(declaration.return_type, Some(FlowReturnType::Binary)) {
        return Err("a binary return type has no builder".to_string());
    }
    if let Some(difference) = mxrs_writer::flow_graph::rebuild_difference(document, &declaration) {
        return Err(format!(
            "the writer would not rebuild its stored body unchanged: {difference}"
        ));
    }
    // What is offered as Rust has to be something a build accepts.
    mxrs_writer::validate_flow_declaration(
        module,
        &declaration,
        document.get_str("$Type").ok() == Some("Microflows$Nanoflow"),
        model.entities,
    )
    .map_err(|error| format!("the writer would refuse it: {error}"))?;
    Ok((declaration, lines))
}

/// Drops the `let` of a binding nothing uses, and prefixes a loop item
/// nothing uses with `_`: the two things Rust would otherwise warn about.
pub(crate) fn polish(mut lines: Vec<String>) -> Vec<String> {
    for index in 0..lines.len() {
        let Some(name) = crate::flow_export::binding_name(&lines[index]) else {
            continue;
        };
        let declaration = format!("let {name} = ");
        let mut used = false;
        for line in &lines[index + 1..] {
            // A later declaration of the same name is another variable: a
            // flow reuses a name once the block that held it has closed.
            // Only its right-hand side can still read this one.
            if let Some(rest) = line.trim_start().strip_prefix(&declaration) {
                used = crate::flow_export::find_identifier(rest, &name).is_some();
                break;
            }
            if crate::flow_export::find_identifier(line, &name).is_some() {
                used = true;
                break;
            }
        }
        if used {
            continue;
        }
        let indent = lines[index].len() - lines[index].trim_start().len();
        if let Some(rest) = lines[index][indent..].strip_prefix(&declaration) {
            lines[index] = format!("{}{rest}", &lines[index][..indent]);
        } else if let Some(position) = crate::flow_export::find_identifier(&lines[index], &name) {
            lines[index].insert(position, '_');
        }
    }
    lines
}

/// Whether the struct declaring `Module.Entity` has a lower-case letter in
/// its name, which is how an activity macro reads it as a type.
fn entity_type_has_lowercase(qualified: &str) -> bool {
    let name = qualified.rsplit('.').next().unwrap_or(qualified);
    crate::entity_export::entity_type_name(name)
        .chars()
        .any(|c| c.is_ascii_lowercase())
}

/// A Mendix number literal Rust writes the same way.
fn is_number_literal(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    !whole.is_empty()
        && whole.chars().all(|c| c.is_ascii_digit())
        && (whole == "0" || !whole.starts_with('0'))
        && (digits.len() == whole.len()
            || (!fraction.is_empty() && fraction.chars().all(|c| c.is_ascii_digit())))
}

/// An activity written as the macro that declares it, when every part of it
/// reads back exactly; the builder call stands otherwise.
impl Converter<'_> {
    /// What a macro writes for Mendix expression `text`: a Rust literal or a
    /// variable's binding when that reads back as exactly `text`, and
    /// `mx("...")` otherwise.
    fn macro_value(&self, text: &str, scope: &Scope) -> String {
        if self.model.enumeration_values.contains(text) {
            return names::enumeration_value(text);
        }
        if let Some(inner) = text
            .strip_prefix('\'')
            .and_then(|rest| rest.strip_suffix('\''))
            && text.len() >= 2
            && !inner.replace("''", "").contains('\'')
        {
            return rust_string(&inner.replace("''", "'"));
        }
        if is_number_literal(text) || text == "true" || text == "false" {
            return text.to_string();
        }
        if let Some(binding) = text.strip_prefix('$').and_then(|name| scope.get(name)) {
            return binding.clone();
        }
        expression(text)
    }

    /// `Entity { field: value, ... }` for the items of a create or change,
    /// when every one sets a member `entity`'s struct declares a field for.
    fn macro_members(&self, action: &Document, entity: &str, scope: &Scope) -> Option<String> {
        let items = action
            .get("Items")
            .and_then(mxrs_writer::flow_graph::documents)?;
        let mut fields = Vec::new();
        for item in items {
            if item.get_str("Type").ok()? != "Set" {
                return None;
            }
            // An attribute by its name on its entity; an association by its
            // qualified name, on the entity whose struct declares it.
            let association = item.get_str("Association").ok()?;
            let (owner, member) = if association.is_empty() {
                let (owner, name) = item.get_str("Attribute").ok()?.rsplit_once('.')?;
                (owner.to_string(), name.to_string())
            } else {
                (entity.to_string(), association.to_string())
            };
            if owner != entity || !self.model.attributes.contains(&(owner, member.clone())) {
                return None;
            }
            let value = self.macro_value(item.get_str("Value").ok()?, scope);
            fields.push(format!("{}: {value}", names::field(entity, &member)));
        }
        let marker = self.typed_entity(entity)?;
        Some(if fields.is_empty() {
            format!("{marker} {{}}")
        } else {
            format!("{marker} {{ {} }}", fields.join(", "))
        })
    }

    /// The `commit`/`refresh` words of a create or change.
    fn macro_change_options(action: &Document) -> Option<String> {
        let mut words = String::new();
        match action.get_str("Commit").ok()? {
            "No" => {}
            "Yes" => words.push_str(", commit"),
            "YesWithoutEvents" => words.push_str(", commit_without_events"),
            _ => return None,
        }
        if action.get_bool("RefreshInClient").ok()? {
            words.push_str(", refresh");
        }
        Some(words)
    }

    fn typed_entity(&self, entity: &str) -> Option<String> {
        self.model
            .entities
            .contains(entity)
            .then(|| crate::flow_export::marker(entity))
            .flatten()
    }

    /// `, name = "..."` unless `name` is the one Studio Pro gives by default.
    fn macro_name(name: &str, default: &str) -> String {
        if name == default {
            String::new()
        } else {
            format!(", name = {}", rust_string(name))
        }
    }

    /// `[(member, Ascending), ...]` of a list of sortings.
    fn macro_sortings(&self, sortings: &Document) -> Option<String> {
        let mut pairs = Vec::new();
        for sorting in sortings
            .get("Sortings")
            .and_then(mxrs_writer::flow_graph::documents)?
        {
            let attribute = sorting
                .get_document("AttributeRef")
                .ok()?
                .get_str("Attribute")
                .ok()?;
            let order = match sorting.get_str("SortOrder").ok()? {
                "Ascending" => "Ascending",
                "Descending" => "Descending",
                _ => return None,
            };
            pairs.push(format!("({}, {order})", self.attribute(attribute)));
        }
        Some(format!("[{}]", pairs.join(", ")))
    }

    /// The lines of `kind`'s macro, for an action the builder has already
    /// read and checked.
    fn activity_macro(&self, kind: &str, action: &Document, scope: &Scope) -> Option<Vec<String>> {
        let text = |key: &str| action.get_str(key).ok();
        let short = |qualified: &str| {
            qualified
                .rsplit('.')
                .next()
                .unwrap_or(qualified)
                .to_string()
        };
        let line = match kind {
            "Microflows$CreateChangeAction" => {
                let entity = text("Entity")?;
                let name = text("VariableName")?;
                format!(
                    "create_object!(flow, {}{}{});",
                    self.macro_members(action, entity, scope)?,
                    Self::macro_name(name, &format!("New{}", short(entity))),
                    Self::macro_change_options(action)?
                )
            }
            "Microflows$ChangeAction" => {
                let items = action
                    .get("Items")
                    .and_then(mxrs_writer::flow_graph::documents)?;
                // The entity changed: the owner of an attribute it sets, or
                // the entity declaring an association it sets.
                let first = items.first()?;
                let entity = match first.get_str("Attribute").ok()?.rsplit_once('.') {
                    Some((owner, _)) => owner.to_string(),
                    None => {
                        let association = first.get_str("Association").ok()?;
                        self.model.association_owners.get(association)?.clone()
                    }
                };
                format!(
                    "change_object!(flow, {}, {}{});",
                    Self::variable(scope, text("ChangeVariableName")?),
                    self.macro_members(action, &entity, scope)?,
                    Self::macro_change_options(action)?
                )
            }
            "Microflows$CommitAction" => {
                let mut words = String::new();
                if !action.get_bool("WithEvents").unwrap_or(true) {
                    words.push_str(", without_events");
                }
                if action.get_bool("RefreshInClient").unwrap_or(false) {
                    words.push_str(", refresh");
                }
                format!(
                    "commit_object!(flow, {}{words});",
                    Self::variable(scope, text("CommitVariableName")?)
                )
            }
            "Microflows$DeleteAction" | "Microflows$RollbackAction" => {
                let (macro_name, key) = if kind == "Microflows$DeleteAction" {
                    ("delete_object", "DeleteVariableName")
                } else {
                    ("rollback_object", "RollbackVariableName")
                };
                let refresh = if action.get_bool("RefreshInClient").unwrap_or(false) {
                    ", refresh"
                } else {
                    ""
                };
                format!(
                    "{macro_name}!(flow, {}{refresh});",
                    Self::variable(scope, text(key)?)
                )
            }
            "Microflows$RetrieveAction" => {
                let name = text("ResultVariableName")?;
                let source = action.get_document("RetrieveSource").ok()?;
                if source.get_str("$Type").ok()? == "Microflows$AssociationRetrieveSource" {
                    // The association by the field that declares it, where an
                    // entity's struct does.
                    let association = source.get_str("AssociationId").ok()?;
                    let by = self.model.association_owners.get(association).map_or_else(
                        || rust_string(association),
                        |owner| names::attribute(owner, association),
                    );
                    return Some(vec![format!(
                        "retrieve!(flow, {}, by = {by}, name = {});",
                        Self::variable(scope, source.get_str("StartVariableName").ok()?),
                        rust_string(name)
                    )]);
                }
                if source.get_str("$Type").ok()? != "Microflows$DatabaseRetrieveSource" {
                    return None;
                }
                let entity = source.get_str("Entity").ok()?;
                let marker = self.typed_entity(entity)?;
                let mut options = String::new();
                let xpath = source.get_str("XpathConstraint").ok()?;
                if !xpath.is_empty() {
                    options.push_str(&format!(", xpath = {}", rust_string(xpath)));
                }
                let sortings = source.get_document("NewSortings").ok()?;
                if sortings
                    .get("Sortings")
                    .and_then(mxrs_writer::flow_graph::documents)
                    .is_some_and(|list| !list.is_empty())
                {
                    options.push_str(&format!(", sort = {}", self.macro_sortings(sortings)?));
                }
                let range = source.get_document("Range").ok()?;
                let first = match range.get_str("$Type").ok()? {
                    "Microflows$ConstantRange" => range.get_bool("SingleObject").ok()?,
                    "Microflows$CustomRange" => {
                        options.push_str(&format!(
                            ", range = ({}, {})",
                            self.macro_value(range.get_str("LimitExpression").ok()?, scope),
                            self.macro_value(range.get_str("OffsetExpression").ok()?, scope)
                        ));
                        false
                    }
                    _ => return None,
                };
                if first {
                    options.push_str(", first");
                }
                let default = if first {
                    short(entity)
                } else {
                    format!("{}List", short(entity))
                };
                format!(
                    "retrieve!(flow, {marker}{options}{});",
                    Self::macro_name(name, &default)
                )
            }
            "Microflows$CreateListAction" => {
                let entity = text("Entity")?;
                format!(
                    "create_list!(flow, {}{});",
                    self.typed_entity(entity)?,
                    Self::macro_name(text("VariableName")?, &format!("{}List", short(entity)))
                )
            }
            "Microflows$ChangeListAction" => {
                let list = Self::variable(scope, text("ChangeVariableName")?);
                let value = text("Value")?;
                match text("Type")? {
                    "Clear" if value.is_empty() => format!("change_list!(flow, {list}, clear);"),
                    "Add" => format!(
                        "change_list!(flow, {list}, add = {});",
                        self.macro_value(value, scope)
                    ),
                    "Remove" => {
                        format!(
                            "change_list!(flow, {list}, remove = {});",
                            self.macro_value(value, scope)
                        )
                    }
                    "Set" => format!(
                        "change_list!(flow, {list}, replace = {});",
                        self.macro_value(value, scope)
                    ),
                    _ => return None,
                }
            }
            "Microflows$ListOperationsAction" => {
                let name = rust_string(text("ResultVariableName")?);
                let operation = action.get_document("NewOperation").ok()?;
                let list = Self::variable(scope, operation.get_str("ListName").ok()?);
                let operation_text = |key: &str| operation.get_str(key).ok();
                let argument = match operation
                    .get_str("$Type")
                    .ok()?
                    .strip_prefix("Microflows$")?
                {
                    "Head" => "head".to_string(),
                    "Tail" => "tail".to_string(),
                    kind @ ("Union" | "Intersect" | "Subtract" | "Contains" | "Equals") => format!(
                        "{} = {}",
                        kind.to_ascii_lowercase(),
                        Self::variable(scope, operation_text("SecondListOrObjectName")?)
                    ),
                    kind @ ("Find" | "Filter") => {
                        if !operation_text("Association")?.is_empty() {
                            return None;
                        }
                        format!(
                            "{} = ({}, {})",
                            kind.to_ascii_lowercase(),
                            self.attribute(operation_text("Attribute")?),
                            self.macro_value(operation_text("Expression")?, scope)
                        )
                    }
                    kind @ ("FindByExpression" | "FilterByExpression") => format!(
                        "{} = {}",
                        if kind == "FindByExpression" {
                            "find_by"
                        } else {
                            "filter_by"
                        },
                        self.macro_value(operation_text("Expression")?, scope)
                    ),
                    "ListRange" => {
                        let range = operation.get_document("CustomRange").ok()?;
                        format!(
                            "range = ({}, {})",
                            self.macro_value(range.get_str("LimitExpression").ok()?, scope),
                            self.macro_value(range.get_str("OffsetExpression").ok()?, scope)
                        )
                    }
                    "Sort" => format!(
                        "sort = {}",
                        self.macro_sortings(operation.get_document("Sortings").ok()?)?
                    ),
                    _ => return None,
                };
                format!("change_list!(flow, {list}, {argument}, name = {name});")
            }
            "Microflows$AggregateAction" => {
                let name = rust_string(text("VariableName")?);
                let list = Self::variable(scope, text("AggregateVariableName")?);
                let function = text("AggregateFunction")?;
                let attribute = text("Attribute")?;
                let expression = action
                    .get_bool("UseExpression")
                    .unwrap_or(false)
                    .then(|| text("Expression"))
                    .flatten();
                let over = match (function, attribute.is_empty(), expression) {
                    ("Count", true, None) => "count".to_string(),
                    ("Reduce" | "Count", _, _) => return None,
                    (_, false, None) => format!(
                        "{} = {}",
                        function.to_ascii_lowercase(),
                        self.attribute(attribute)
                    ),
                    (_, true, Some(expression)) => format!(
                        "{}_of = {}",
                        function.to_ascii_lowercase(),
                        self.macro_value(expression, scope)
                    ),
                    _ => return None,
                };
                format!("aggregate_list!(flow, {list}, {over}, name = {name});")
            }
            "Microflows$CreateVariableAction" => {
                let ty = Self::data_type(action.get_document("VariableType").ok()?).ok()?;
                format!(
                    "create_variable!(flow, {}, {}, name = {});",
                    self.data_type_source(&ty),
                    self.macro_value(text("InitialValue")?, scope),
                    rust_string(text("VariableName")?)
                )
            }
            "Microflows$ChangeVariableAction" => format!(
                "change_variable!(flow, {}, {});",
                Self::variable(scope, text("ChangeVariableName")?),
                self.macro_value(text("Value")?, scope)
            ),
            "Microflows$MicroflowCallAction" => {
                let result = text("ResultVariableName")?;
                // A call whose result the model says it does not use stays a
                // builder call: the macro always keeps its result.
                let uses_result = action.get_bool("UseReturnVariable").ok()?;
                if !uses_result {
                    return None;
                }
                let call = action.get_document("MicroflowCall").ok()?;
                if matches!(call.get("QueueSettings"), Some(Bson::Document(_))) {
                    return None;
                }
                let target = call.get_str("Microflow").ok()?;
                if !self.model.microflows.contains(target) {
                    return None;
                }
                let marker = crate::flow_export::flow_marker(target)?;
                let mut arguments = Vec::new();
                for mapping in call
                    .get("ParameterMappings")
                    .and_then(mxrs_writer::flow_graph::documents)?
                {
                    let parameter = mapping.get_str("Parameter").ok()?;
                    let short = parameter
                        .strip_prefix(target)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))?;
                    if !mxrs_typegen::is_rust_identifier(short) || crate::rust_keyword(short) {
                        return None;
                    }
                    arguments.push(format!(
                        "{short}: {}",
                        self.macro_value(mapping.get_str("Argument").ok()?, scope)
                    ));
                }
                let call = if arguments.is_empty() {
                    marker
                } else {
                    format!("{marker} {{ {} }}", arguments.join(", "))
                };
                let name = if uses_result && !result.is_empty() {
                    format!(", name = {}", rust_string(result))
                } else {
                    String::new()
                };
                format!("call_microflow!(flow, {call}{name});")
            }
            "Microflows$NanoflowCallAction" => {
                let result = text("OutputVariableName")?;
                // A call whose result the model says it does not use stays a
                // builder call: the macro always keeps its result.
                if !action.get_bool("UseReturnVariable").ok()? {
                    return None;
                }
                let call = action.get_document("NanoflowCall").ok()?;
                let target = call.get_str("Nanoflow").ok()?;
                if !self.model.nanoflows.contains(target) {
                    return None;
                }
                let marker = crate::flow_export::nanoflow_marker(target)?;
                let mut arguments = Vec::new();
                for mapping in call
                    .get("ParameterMappings")
                    .and_then(mxrs_writer::flow_graph::documents)?
                {
                    let parameter = mapping.get_str("Parameter").ok()?;
                    let short = parameter
                        .strip_prefix(target)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))?;
                    if !mxrs_typegen::is_rust_identifier(short) || crate::rust_keyword(short) {
                        return None;
                    }
                    arguments.push(format!(
                        "{short}: {}",
                        self.macro_value(mapping.get_str("Argument").ok()?, scope)
                    ));
                }
                let call = if arguments.is_empty() {
                    marker
                } else {
                    format!("{marker} {{ {} }}", arguments.join(", "))
                };
                let name = if result.is_empty() {
                    String::new()
                } else {
                    format!(", name = {}", rust_string(result))
                };
                format!("call_nanoflow!(flow, {call}{name});")
            }
            "Microflows$JavaScriptActionCallAction" => {
                let action_name = text("JavaScriptAction")?;
                let result = text("OutputVariableName")?;
                if !action.get_bool("UseReturnVariable").ok()? {
                    return None;
                }
                let mut arguments = vec![rust_string(action_name)];
                if !result.is_empty() {
                    arguments.push(rust_string(result));
                }
                for mapping in action
                    .get("ParameterMappings")
                    .and_then(mxrs_writer::flow_graph::documents)?
                {
                    let parameter = mapping.get_str("Parameter").ok()?;
                    let short = parameter
                        .strip_prefix(action_name)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))?;
                    if !mxrs_typegen::is_rust_identifier(short) || crate::rust_keyword(short) {
                        return None;
                    }
                    let value = mapping.get_document("ParameterValue").ok()?;
                    let argument = match value.get_str("$Type").ok()? {
                        "Microflows$BasicCodeActionParameterValue" => {
                            self.macro_value(value.get_str("Argument").ok()?, scope)
                        }
                        "Microflows$EntityTypeCodeActionParameterValue" => {
                            let entity = value.get_str("Entity").ok()?;
                            let marker = self.typed_entity(entity)?;
                            if entity_type_has_lowercase(entity) {
                                marker
                            } else {
                                format!("{marker} {{}}")
                            }
                        }
                        _ => return None,
                    };
                    arguments.push(format!("{short} = {argument}"));
                }
                format!("call_javascript_action!(flow, {});", arguments.join(", "))
            }
            "Microflows$JavaActionCallAction" => {
                let action_name = text("JavaAction")?;
                if !self.model.java_actions.contains(action_name)
                    || matches!(action.get("QueueSettings"), Some(Bson::Document(_)))
                {
                    return None;
                }
                let result = text("ResultVariableName")?;
                let uses_result = action.get_bool("UseReturnVariable").ok()?;
                if !uses_result {
                    return None;
                }
                let mut arguments = Vec::new();
                if uses_result && !result.is_empty() {
                    arguments.push(rust_string(result));
                }
                for mapping in action
                    .get("ParameterMappings")
                    .and_then(mxrs_writer::flow_graph::documents)?
                {
                    let parameter = mapping.get_str("Parameter").ok()?;
                    let short = parameter
                        .strip_prefix(action_name)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .filter(|name| !name.contains('.'))?;
                    if !mxrs_typegen::is_rust_identifier(short) || crate::rust_keyword(short) {
                        return None;
                    }
                    let value = mapping.get_document("Value").ok()?;
                    let argument = match value.get_str("$Type").ok()? {
                        "Microflows$BasicCodeActionParameterValue" => {
                            self.macro_value(value.get_str("Argument").ok()?, scope)
                        }
                        "Microflows$EntityTypeCodeActionParameterValue" => {
                            let entity = value.get_str("Entity").ok()?;
                            let marker = self.typed_entity(entity)?;
                            // A struct is read as an entity by its lower-case
                            // letters; one in capitals says so with `{}`.
                            if entity_type_has_lowercase(entity) {
                                marker
                            } else {
                                format!("{marker} {{}}")
                            }
                        }
                        _ => return None,
                    };
                    arguments.push(format!("{short} = {argument}"));
                }
                let mut line = format!("{}!(flow", names::java_action(action_name));
                for argument in &arguments {
                    line.push_str(", ");
                    line.push_str(argument);
                }
                line.push_str(");");
                line
            }
            "Microflows$LogMessageAction" => {
                let level = LogSeverity::from_native(text("Level")?)?;
                let level = format!("{level:?}");
                let template = action.get_document("MessageTemplate").ok()?;
                let parameters = template
                    .get("Parameters")
                    .and_then(mxrs_writer::flow_graph::documents)?
                    .into_iter()
                    .map(|parameter| {
                        parameter
                            .get_str("Expression")
                            .ok()
                            .map(|value| self.macro_value(value, scope))
                    })
                    .collect::<Option<Vec<_>>>()?;
                let mut options = String::new();
                if !parameters.is_empty() {
                    options.push_str(&format!(", parameters = [{}]", parameters.join(", ")));
                }
                if action.get_bool("IncludeLatestStackTrace").unwrap_or(false) {
                    options.push_str(", stack_trace");
                }
                format!(
                    "log!(flow, {level}, {}, {}{options});",
                    self.macro_value(text("Node")?, scope),
                    rust_string(template.get_str("Text").ok()?)
                )
            }
            _ => return None,
        };
        Some(vec![line])
    }
}

#[cfg(test)]
mod tests {
    use mxrs_bson::Bson;
    use mxrs_ir::Activity;
    use mxrs_writer::flow_graph::{Node, structured_nodes};

    #[test]
    fn a_jump_read_before_the_point_it_goes_to_is_already_named() {
        let commit = |variable: &str| Activity::Commit {
            variable: variable.into(),
        };
        // One branch runs into the middle of the next: its jump is read
        // before the point it goes to.
        let activities = vec![Activity::Decision {
            condition: "$A".into(),
            true_branch: vec![commit("First"), Activity::Jump("shared".into())],
            false_branch: vec![Activity::Decision {
                condition: "$B".into(),
                true_branch: vec![commit("Second")],
                false_branch: vec![Activity::Label("shared".into()), commit("Third")],
            }],
        }];
        let (objects, flows) =
            mxrs_writer::flow_compiler::build_microflow_graph(&activities, &[], None);
        let document = mxrs_bson::doc! {
            "ObjectCollection": {
                "Objects": mxrs_bson::build_array(objects.into_iter().map(Bson::Document).collect(), 3),
            },
            "Flows": mxrs_bson::build_array(flows.into_iter().map(Bson::Document).collect(), 3),
        };
        let nodes = structured_nodes(&document).expect("the graph is read");
        fn first_jump(nodes: &[Node<'_>]) -> Option<String> {
            nodes.iter().find_map(|node| match node {
                Node::Jump(target) => Some(target.clone()),
                Node::Decision { yes, no, .. } => first_jump(yes).or_else(|| first_jump(no)),
                _ => None,
            })
        }
        let target = first_jump(&nodes).expect("the reading has a jump");
        assert!(super::label_names(&nodes).contains_key(&target));
    }
}
