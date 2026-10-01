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
use mxrs_ir::flow::{Activity, DataType, FlowParameterDecl, MicroflowDecl, NativeDocument};
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
    out.extend(lines);
    out.push("}".to_string());
    out
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
        let handling = fields.text("ErrorHandlingType")?;
        if handling != "Rollback" {
            return Err(format!("{kind} handles errors with {handling}"));
        }
        let statement = match kind {
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
                call.absent_or_null("QueueSettings")?;
                if !uses_result && !result.is_empty() {
                    return Err(format!(
                        "a discarded call to {target} still names a result variable"
                    ));
                }
                let mut document = actions::call(target, uses_result.then_some(result));
                let mut lines = Vec::new();
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
                let head = if uses_result {
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
                    declares: uses_result.then(|| result.to_string()),
                }
            }
            "Microflows$JavaActionCallAction" => {
                let java_action = fields.text("JavaAction")?;
                let result = fields.text("ResultVariableName")?;
                let uses_result = fields.boolean("UseReturnVariable")?;
                fields.absent_or_null("QueueSettings")?;
                if !uses_result && !result.is_empty() {
                    return Err(format!(
                        "a discarded call to {java_action} still names a result variable"
                    ));
                }
                let mut document = actions::call_java(java_action, uses_result.then_some(result));
                let mut lines = Vec::new();
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
                let head = if uses_result {
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
                    declares: uses_result.then(|| result.to_string()),
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
                settings.absent_or_null("TitleOverride")?;
                let mut document = actions::show_page(page);
                let mut lines = Vec::new();
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
            other => return Err(format!("{other} has no builder yet")),
        };
        fields.finish()?;
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
            match node {
                Node::Simple(document) => match document.get_str("$Type").unwrap_or_default() {
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
                        // A disabled activity does not run, and no builder
                        // says so yet.
                        if document.get_bool("Disabled").unwrap_or(false) {
                            return Err("it has a disabled activity".to_string());
                        }
                        let action = document
                            .get_document("Action")
                            .map_err(|_| "an activity without an action".to_string())?;
                        let statement = self.action(action, scope)?;
                        let mut statement_lines = statement.lines;
                        if let Some(name) = &statement.declares {
                            let binding = Self::declare(scope, name)?;
                            statement_lines[0] = format!("let {binding} = {}", statement_lines[0]);
                        }
                        lines.extend(statement_lines);
                        activities.push(Activity::Action(statement.document));
                    }
                    other => return Err(format!("unexpected node {other}")),
                },
                Node::Decision {
                    split,
                    yes,
                    no,
                    merge,
                } => {
                    let mut split_fields =
                        node_fields(split, &["Caption", "Documentation", "SplitCondition"])?;
                    if split_fields.text("ErrorHandlingType")? != "Rollback" {
                        return Err("a split with its own error handling".to_string());
                    }
                    split_fields.finish()?;
                    if let Some(merge) = merge {
                        node_fields(merge, &[])?.finish()?;
                    }
                    let condition = split
                        .get_document("SplitCondition")
                        .map_err(|_| "a split without a condition".to_string())?;
                    if condition.get_str("$Type").ok()
                        != Some("Microflows$ExpressionSplitCondition")
                    {
                        return Err("a rule-based split has no builder yet".to_string());
                    }
                    let mut condition_fields = Fields::new(condition);
                    let condition = condition_fields.text("Expression")?;
                    condition_fields.finish()?;
                    let branch = |nodes: &[Node<'_>]| -> Outcome<(Vec<Activity>, Vec<String>)> {
                        let mut branch_lines = Vec::new();
                        let activities =
                            self.block(nodes, &mut scope.clone(), &mut branch_lines, true)?;
                        Ok((activities, closure("flow", branch_lines)))
                    };
                    let (true_branch, mut yes_lines) = branch(yes)?;
                    let (false_branch, mut no_lines) = branch(no)?;
                    lines.push("flow.decision(".to_string());
                    lines.push(format!("{},", expression(condition)));
                    yes_lines
                        .last_mut()
                        .expect("a closure has a line")
                        .push(',');
                    no_lines.last_mut().expect("a closure has a line").push(',');
                    lines.extend(yes_lines);
                    lines.extend(no_lines);
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
                    let handling = loop_fields.text("ErrorHandlingType")?;
                    if handling != "Rollback" {
                        return Err(format!("a loop handles errors with {handling}"));
                    }
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
                            lines.extend(body_lines);
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
                            lines.extend(body_lines);
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
        Ok(activities)
    }
}

fn returns_a_value(declaration: &MicroflowDecl) -> bool {
    fn any(activities: &[Activity]) -> bool {
        activities.iter().any(|activity| match activity {
            Activity::ReturnValue { expression } => !expression.is_empty(),
            Activity::Decision {
                true_branch,
                false_branch,
                ..
            } => any(true_branch) || any(false_branch),
            Activity::LoopOver { activities, .. } | Activity::WhileLoop { activities, .. } => {
                any(activities)
            }
            _ => false,
        })
    }
    declaration.return_expression.is_some() || any(&declaration.activities)
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
    let converter = Converter { model };
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
    // A flow that returns nothing has nothing to return anywhere: a value on
    // one of its end events is a model the writer would have to guess about.
    if declaration.return_type.is_none() && returns_a_value(&declaration) {
        return Err("it returns a value but declares no return type".to_string());
    }
    if !mxrs_writer::flow_graph::preserves_body(document, &declaration) {
        return Err("the writer would not rebuild its stored body unchanged".to_string());
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
        if let Some(rest) = lines[index].strip_prefix(&declaration) {
            lines[index] = rest.to_string();
        } else if let Some(position) = crate::flow_export::find_identifier(&lines[index], &name) {
            lines[index].insert(position, '_');
        }
    }
    lines
}
