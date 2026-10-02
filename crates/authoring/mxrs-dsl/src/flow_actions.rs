//! Everything a flow can do, stated in full.
//!
//! The typed core of [`FlowBuilder`] (`create_object`, `decision`,
//! `loop_over`, ...) checks what it can: that an attribute belongs to the
//! entity being created, that an argument has the parameter's type. This
//! module is the rest of what a Mendix flow can say — every activity kind,
//! with every option the model stores for it — so that a flow imported from
//! a model is Rust a person can read and edit, whatever it does.
//!
//! Expressions are written the way Mendix writes them, with [`mx`]:
//!
//! ```text
//! let orders = flow.retrieve("Orders", Ref::<Order>::new(), |retrieve| {
//!     retrieve.xpath("[Status = 'Open']");
//!     retrieve.sort_by(Order::created(), SortOrder::Descending);
//! });
//! flow.decision(
//!     mx("$Orders = empty"),
//!     |flow| {
//!         flow.show_message(MessageKind::Warning, |message| {
//!             message.text("en_US", "Nothing to ship.");
//!         });
//!         flow.return_with(mx("false"));
//!     },
//!     |flow| {
//!         flow.for_each(&orders, "Order", |flow, order| {
//!             flow.change(&order, |change| {
//!                 change.set(Order::status(), mx("Sales.OrderStatus.Shipped"));
//!                 change.commit(Commit::Yes);
//!             });
//!         });
//!         flow.return_with(mx("true"));
//!     },
//! );
//! ```
//!
//! Each activity is declared as the action document the model stores (see
//! [`mxrs_ir::NativeDocument`]). The functions in [`actions`] build those
//! documents and the option tables beside them describe every option a
//! document carries, which is what lets an importer write the same calls a
//! person would.

use mxrs_expr::{ListVar, Mx, Var};
use mxrs_ir::flow::{Activity, DataType, NativeDocument, NativeValue, SwitchCase};
use mxrs_ir::{
    AssociationMarker, AssociationRef, AttributeMarker, AttributeRef, EntityMarker,
    MicroflowMarker, MicroflowRef, Ref,
};

use crate::flow::FlowBuilder;

/// Something a flow holds in a variable: a parameter, the result of an
/// activity, the item of a loop.
pub trait Variable {
    fn variable_name(&self) -> &str;
}

impl<M: EntityMarker> Variable for Var<M> {
    fn variable_name(&self) -> &str {
        self.name()
    }
}

impl<M: EntityMarker> Variable for ListVar<M> {
    fn variable_name(&self) -> &str {
        self.name()
    }
}

impl<T: Variable + ?Sized> Variable for &T {
    fn variable_name(&self) -> &str {
        (**self).variable_name()
    }
}

/// A flow variable of whatever type its activity gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowVar(String);

impl FlowVar {
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl Variable for FlowVar {
    fn variable_name(&self) -> &str {
        &self.0
    }
}

/// What a name stands for where an activity macro takes a value: a
/// variable is `$` and its name, text (a `&str` constant) is a Mendix
/// string, and an expression is itself.
pub trait ActivityValue {
    fn activity_value(&self) -> Mx;
}

impl ActivityValue for &str {
    fn activity_value(&self) -> Mx {
        Mx::from(mxrs_expr::string(*self))
    }
}

impl ActivityValue for String {
    fn activity_value(&self) -> Mx {
        Mx::from(mxrs_expr::string(self))
    }
}

impl ActivityValue for FlowVar {
    fn activity_value(&self) -> Mx {
        Mx::from(self)
    }
}

impl<M: EntityMarker> ActivityValue for Var<M> {
    fn activity_value(&self) -> Mx {
        Mx::from(self)
    }
}

impl<M: EntityMarker> ActivityValue for ListVar<M> {
    fn activity_value(&self) -> Mx {
        Mx::from(self)
    }
}

impl<T: mxrs_expr::MendixType> ActivityValue for mxrs_expr::Expr<T> {
    fn activity_value(&self) -> Mx {
        Mx::from(self)
    }
}

impl ActivityValue for Mx {
    fn activity_value(&self) -> Mx {
        self.clone()
    }
}

impl From<&FlowVar> for Mx {
    fn from(variable: &FlowVar) -> Self {
        mxrs_expr::mx(format!("${}", variable.0))
    }
}

/// Names a variable the flow already has — one the model provides
/// (`currentUser`, `latestError`) or one declared out of this builder's
/// sight.
pub fn var(name: impl Into<String>) -> FlowVar {
    FlowVar(name.into())
}

/// An entity, named by the struct that declares it or by its qualified name.
pub trait EntityName {
    fn entity_name(&self) -> String;
}

impl<M: EntityMarker> EntityName for Ref<M> {
    fn entity_name(&self) -> String {
        self.qualified_name()
    }
}

impl EntityName for &str {
    fn entity_name(&self) -> String {
        (*self).to_string()
    }
}

/// A microflow, named by the type its declaration generates or by its
/// qualified name.
pub trait MicroflowName {
    fn microflow_name(&self) -> String;
}

impl<M: MicroflowMarker> MicroflowName for MicroflowRef<M> {
    fn microflow_name(&self) -> String {
        self.qualified_name()
    }
}

impl MicroflowName for &str {
    fn microflow_name(&self) -> String {
        (*self).to_string()
    }
}

/// An attribute, named by its entity's accessor or as
/// `Module.Entity.Attribute`.
pub trait AttributeName {
    fn attribute_name(&self) -> String;
}

impl<A: AttributeMarker> AttributeName for AttributeRef<A> {
    fn attribute_name(&self) -> String {
        self.qualified_name()
    }
}

impl AttributeName for &str {
    fn attribute_name(&self) -> String {
        (*self).to_string()
    }
}

/// An association, named by its entity's accessor or as
/// `Module.Association`.
pub trait AssociationName {
    fn association_name(&self) -> String;
}

impl<A: AssociationMarker> AssociationName for AssociationRef<A> {
    fn association_name(&self) -> String {
        self.qualified_name()
    }
}

impl AssociationName for &str {
    fn association_name(&self) -> String {
        (*self).to_string()
    }
}

/// An attribute or an association of an entity: what a change sets, and what
/// a list is searched or filtered by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberName {
    Attribute(String),
    Association(String),
}

impl MemberName {
    /// `Module.Entity.Attribute`, for an attribute no accessor names.
    pub fn attribute(name: impl Into<String>) -> Self {
        Self::Attribute(name.into())
    }

    /// `Module.Association`, for an association no accessor names.
    pub fn association(name: impl Into<String>) -> Self {
        Self::Association(name.into())
    }

    fn attribute_text(&self) -> &str {
        match self {
            Self::Attribute(name) => name,
            Self::Association(_) => "",
        }
    }

    fn association_text(&self) -> &str {
        match self {
            Self::Association(name) => name,
            Self::Attribute(_) => "",
        }
    }
}

impl<A: AttributeMarker> From<AttributeRef<A>> for MemberName {
    fn from(attribute: AttributeRef<A>) -> Self {
        Self::Attribute(attribute.qualified_name())
    }
}

impl<A: AssociationMarker> From<AssociationRef<A>> for MemberName {
    fn from(association: AssociationRef<A>) -> Self {
        Self::Association(association.qualified_name())
    }
}

/// An option with a closed set of values, as the model spells them.
pub trait NativeEnum: Copy + 'static {
    /// `(the model's spelling, the Rust path that names it)` per value.
    const VARIANTS: &'static [(&'static str, &'static str)];

    fn native(self) -> &'static str;
}

macro_rules! native_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $native:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($(#[$variant_meta])* $variant),+
        }

        impl $name {
            /// The value as the model stores it.
            pub const fn native_name(self) -> &'static str {
                match self {
                    $(Self::$variant => $native),+
                }
            }

            /// The value the model's spelling names.
            pub fn from_native(value: &str) -> Option<Self> {
                match value {
                    $($native => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl NativeEnum for $name {
            const VARIANTS: &'static [(&'static str, &'static str)] = &[
                $(($native, concat!(stringify!($name), "::", stringify!($variant)))),+
            ];

            fn native(self) -> &'static str {
                self.native_name()
            }
        }
    };
}

native_enum! {
    /// Whether an activity commits the object it creates or changes.
    Commit {
        No => "No",
        Yes => "Yes",
        /// Commits without running the entity's event handlers.
        WithoutEvents => "YesWithoutEvents",
    }
}

native_enum! {
    SortOrder {
        Ascending => "Ascending",
        Descending => "Descending",
    }
}

native_enum! {
    /// How a change touches a member: sets it, or adds to or removes from a
    /// reference set.
    ChangeKind {
        Set => "Set",
        Add => "Add",
        Remove => "Remove",
    }
}

native_enum! {
    /// What a change does to a list.
    ListChange {
        Set => "Set",
        Add => "Add",
        Remove => "Remove",
        Clear => "Clear",
    }
}

native_enum! {
    LogSeverity {
        Trace => "Trace",
        Debug => "Debug",
        Info => "Info",
        Warning => "Warning",
        Error => "Error",
        Critical => "Critical",
    }
}

native_enum! {
    MessageKind {
        Information => "Information",
        Warning => "Warning",
        Error => "Error",
    }
}

native_enum! {
    AggregateFunction {
        Sum => "Sum",
        Average => "Average",
        Count => "Count",
        Minimum => "Minimum",
        Maximum => "Maximum",
        All => "All",
        Any => "Any",
        Reduce => "Reduce",
    }
}

/// What kind of value an option takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    /// Plain text: a name, an XPath constraint.
    Text,
    /// A Mendix expression.
    Expression,
    /// One of a closed set: `(the model's spelling, the Rust path)`.
    Enumeration(&'static [(&'static str, &'static str)]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionDefault {
    Bool(bool),
    Text(&'static str),
}

/// One option of an activity: where the model stores it, the method that
/// states it, and the value it has when nothing does.
#[derive(Debug, Clone, Copy)]
pub struct OptionSpec {
    /// Dotted path of the field inside the action document.
    pub path: &'static str,
    /// The method of the options type that sets it.
    pub setter: &'static str,
    pub kind: OptionKind,
    pub default: OptionDefault,
    /// Whether every model version stores the field. A field only newer
    /// versions carry is written when stated and otherwise left as the
    /// model has it.
    pub always: bool,
}

impl OptionSpec {
    pub fn default_value(&self) -> NativeValue {
        match self.default {
            OptionDefault::Bool(value) => NativeValue::Bool(value),
            OptionDefault::Text(value) => NativeValue::Text(value.to_string()),
        }
    }
}

/// Writes the default of every option all model versions store.
fn apply_defaults(document: &mut NativeDocument, specs: &[OptionSpec]) {
    for spec in specs.iter().filter(|spec| spec.always) {
        document.set_path(spec.path, spec.default_value());
    }
}

macro_rules! option_setter {
    ($(#[$meta:meta])* $setter:ident, $path:literal, Bool) => {
        $(#[$meta])*
        pub fn $setter(&mut self, value: bool) -> &mut Self {
            self.document.set_path($path, value);
            self
        }
    };
    ($(#[$meta:meta])* $setter:ident, $path:literal, Text) => {
        $(#[$meta])*
        pub fn $setter(&mut self, value: impl Into<String>) -> &mut Self {
            self.document.set_path($path, value.into());
            self
        }
    };
    ($(#[$meta:meta])* $setter:ident, $path:literal, Expression) => {
        $(#[$meta])*
        pub fn $setter(&mut self, value: impl Into<Mx>) -> &mut Self {
            self.document.set_path($path, value.into().into_text());
            self
        }
    };
    ($(#[$meta:meta])* $setter:ident, $path:literal, Enumeration<$enumeration:ty>) => {
        $(#[$meta])*
        pub fn $setter(&mut self, value: $enumeration) -> &mut Self {
            self.document.set_path($path, value.native_name());
            self
        }
    };
}

macro_rules! option_kind {
    (Bool) => {
        OptionKind::Bool
    };
    (Text) => {
        OptionKind::Text
    };
    (Expression) => {
        OptionKind::Expression
    };
    (Enumeration<$enumeration:ty>) => {
        OptionKind::Enumeration(<$enumeration as NativeEnum>::VARIANTS)
    };
}

macro_rules! option_default {
    (Bool, $default:expr) => {
        OptionDefault::Bool($default)
    };
    (Text, $default:expr) => {
        OptionDefault::Text($default)
    };
    (Expression, $default:expr) => {
        OptionDefault::Text($default)
    };
    (Enumeration<$enumeration:ty>, $default:expr) => {
        OptionDefault::Text($default.native_name())
    };
}

macro_rules! option_always {
    (always) => {
        true
    };
    (newer) => {
        false
    };
}

/// Declares the options of one activity kind once: the type whose methods
/// state them, and the table that describes them.
///
/// `always` marks an option every model version stores; `newer` one that
/// only some do.
macro_rules! options {
    (
        $(#[$meta:meta])*
        $name:ident, $specs:ident {
            $(
                $(#[$option_meta:meta])*
                $setter:ident($path:literal): $kind:ident $(<$enumeration:ty>)? $mode:ident $default:expr;
            )*
        }
    ) => {
        $(#[$meta])*
        pub struct $name<'a> {
            document: &'a mut NativeDocument,
        }

        impl $name<'_> {
            $(
                option_setter!($(#[$option_meta])* $setter, $path, $kind $(<$enumeration>)?);
            )*
        }

        /// The options of this activity kind, as the model stores them.
        pub const $specs: &[OptionSpec] = &[
            $(
                OptionSpec {
                    path: $path,
                    setter: stringify!($setter),
                    kind: option_kind!($kind $(<$enumeration>)?),
                    default: option_default!($kind $(<$enumeration>)?, $default),
                    always: option_always!($mode),
                },
            )*
        ];
    };
}

options! {
    /// What a `create` or `change` sets and how it leaves the object.
    ChangeOptions, CHANGE_OPTIONS {
        /// Commits the object, with or without its event handlers.
        commit("Commit"): Enumeration<Commit> always Commit::No;
        /// Sends the changed object to the client.
        refresh_in_client("RefreshInClient"): Bool always false;
    }
}

impl ChangeOptions<'_> {
    fn item(&mut self, member: MemberName, kind: ChangeKind, value: Mx) -> &mut Self {
        let item = actions::change_item(&member, kind, value.as_str());
        if let Some(items) = self.document.list_mut("Items") {
            items.push(NativeValue::Document(item));
        }
        self
    }

    /// Sets an attribute or an association.
    pub fn set(&mut self, member: impl Into<MemberName>, value: impl Into<Mx>) -> &mut Self {
        self.item(member.into(), ChangeKind::Set, value.into())
    }

    /// Adds to a reference set.
    pub fn add(&mut self, member: impl Into<MemberName>, value: impl Into<Mx>) -> &mut Self {
        self.item(member.into(), ChangeKind::Add, value.into())
    }

    /// Removes from a reference set.
    pub fn remove(&mut self, member: impl Into<MemberName>, value: impl Into<Mx>) -> &mut Self {
        self.item(member.into(), ChangeKind::Remove, value.into())
    }
}

options! {
    CommitOptions, COMMIT_OPTIONS {
        /// Sends the committed object to the client.
        refresh_in_client("RefreshInClient"): Bool always false;
        /// Runs the entity's event handlers; `false` commits without them.
        with_events("WithEvents"): Bool always true;
    }
}

options! {
    /// Options of a delete or a rollback.
    RefreshOptions, REFRESH_OPTIONS {
        /// Tells the client the object is gone or restored.
        refresh_in_client("RefreshInClient"): Bool always false;
    }
}

options! {
    /// Which objects a database retrieve reads.
    RetrieveOptions, RETRIEVE_OPTIONS {
        /// The XPath constraint, brackets included: `[Status = 'Open']`.
        xpath("RetrieveSource.XpathConstraint"): Text always "";
    }
}

impl RetrieveOptions<'_> {
    /// Retrieves the first object instead of a list.
    pub fn first(&mut self) -> &mut Self {
        self.document.set_path(
            "RetrieveSource.Range",
            NativeDocument::new("Microflows$ConstantRange").with("SingleObject", true),
        );
        self
    }

    /// Retrieves at most `limit` objects, after skipping `offset`. An empty
    /// expression leaves that side open.
    pub fn range(&mut self, limit: impl Into<Mx>, offset: impl Into<Mx>) -> &mut Self {
        self.document.set_path(
            "RetrieveSource.Range",
            actions::custom_range(limit.into().as_str(), offset.into().as_str()),
        );
        self
    }

    /// Orders the result; later calls break ties of earlier ones.
    pub fn sort_by(&mut self, attribute: impl AttributeName, order: SortOrder) -> &mut Self {
        let sorting = actions::sorting(&attribute.attribute_name(), order);
        if let Some(sortings) = self
            .document
            .document_mut("RetrieveSource")
            .and_then(|source| source.document_mut("NewSortings"))
            .and_then(|sortings| sortings.list_mut("Sortings"))
        {
            sortings.push(NativeValue::Document(sorting));
        }
        self
    }
}

/// The attributes a list is sorted by.
pub struct SortOptions<'a> {
    document: &'a mut NativeDocument,
}

impl SortOptions<'_> {
    /// Orders by `attribute`; later calls break ties of earlier ones.
    pub fn by(&mut self, attribute: impl AttributeName, order: SortOrder) -> &mut Self {
        let sorting = actions::sorting(&attribute.attribute_name(), order);
        if let Some(sortings) = self
            .document
            .document_mut("NewOperation")
            .and_then(|operation| operation.document_mut("Sortings"))
            .and_then(|sortings| sortings.list_mut("Sortings"))
        {
            sortings.push(NativeValue::Document(sorting));
        }
        self
    }
}

/// What an aggregate computes over.
pub struct AggregateOptions<'a> {
    document: &'a mut NativeDocument,
}

impl AggregateOptions<'_> {
    /// Aggregates one attribute of the list's objects.
    pub fn attribute(&mut self, attribute: impl AttributeName) -> &mut Self {
        self.document.set("Attribute", attribute.attribute_name());
        self
    }

    /// Aggregates an expression evaluated for each object, which it names
    /// `$currentObject`.
    pub fn expression(&mut self, expression: impl Into<Mx>) -> &mut Self {
        self.document
            .set("Expression", expression.into().into_text())
            .set("UseExpression", true);
        self
    }
}

options! {
    LogOptions, LOG_OPTIONS {
        /// Appends the stack trace of the error being handled.
        include_stack_trace("IncludeLatestStackTrace"): Bool always false;
    }
}

impl LogOptions<'_> {
    /// Supplies the next `{n}` placeholder of the message.
    pub fn parameter(&mut self, value: impl Into<Mx>) -> &mut Self {
        let parameter = actions::template_parameter(value.into().as_str());
        if let Some(parameters) = self
            .document
            .document_mut("MessageTemplate")
            .and_then(|template| template.list_mut("Parameters"))
        {
            parameters.push(NativeValue::Document(parameter));
        }
        self
    }
}

/// The arguments a microflow is called with.
pub struct CallOptions<'a> {
    document: &'a mut NativeDocument,
}

impl CallOptions<'_> {
    /// Passes `value` for `parameter`, named as the called flow declares it.
    pub fn argument(&mut self, parameter: &str, value: impl Into<Mx>) -> &mut Self {
        let Some(call) = self.document.document_mut("MicroflowCall") else {
            return self;
        };
        let target = call.text("Microflow").unwrap_or_default().to_string();
        let parameter = if parameter.contains('.') {
            parameter.to_string()
        } else {
            format!("{target}.{parameter}")
        };
        let mapping = NativeDocument::new("Microflows$MicroflowCallParameterMapping")
            .with("Parameter", parameter)
            .with("Argument", value.into().into_text());
        if let Some(mappings) = call.list_mut("ParameterMappings") {
            mappings.push(NativeValue::Document(mapping));
        }
        self
    }
}

impl CallOptions<'_> {
    /// Says the call's result is not used. `name` is the variable name the
    /// model still keeps for it — empty when it keeps none.
    pub fn discard_result(&mut self, name: &str) -> &mut Self {
        self.document
            .set("UseReturnVariable", false)
            .set("ResultVariableName", name);
        self
    }

    /// Runs the call in the background, on the task queue `Module.Queue`.
    pub fn queue(&mut self, queue: &str) -> &mut Self {
        self.document.set_path(
            "MicroflowCall.QueueSettings",
            actions::queue_settings(queue),
        );
        self
    }
}

/// The arguments a Java action is called with.
pub struct JavaCallOptions<'a> {
    document: &'a mut NativeDocument,
}

impl JavaCallOptions<'_> {
    /// Says the call's result is not used. `name` is the variable name the
    /// model still keeps for it — empty when it keeps none.
    pub fn discard_result(&mut self, name: &str) -> &mut Self {
        self.document
            .set("UseReturnVariable", false)
            .set("ResultVariableName", name);
        self
    }

    fn mapping(&mut self, parameter: &str, value: NativeDocument) -> &mut Self {
        let action = self.document.text("JavaAction").unwrap_or_default();
        let parameter = if parameter.contains('.') {
            parameter.to_string()
        } else {
            format!("{action}.{parameter}")
        };
        let mapping = NativeDocument::new("Microflows$JavaActionParameterMapping")
            .with("Parameter", parameter)
            .with("Value", value);
        if let Some(mappings) = self.document.list_mut("ParameterMappings") {
            mappings.push(NativeValue::Document(mapping));
        }
        self
    }

    /// Passes the value of an expression.
    pub fn argument(&mut self, parameter: &str, value: impl Into<Mx>) -> &mut Self {
        self.mapping(
            parameter,
            NativeDocument::new("Microflows$BasicCodeActionParameterValue")
                .with("Argument", value.into().into_text()),
        )
    }

    /// Passes an entity, for a parameter that takes a type.
    pub fn entity_argument(&mut self, parameter: &str, entity: impl EntityName) -> &mut Self {
        self.mapping(
            parameter,
            NativeDocument::new("Microflows$EntityTypeCodeActionParameterValue")
                .with("Entity", entity.entity_name()),
        )
    }

    /// Passes a microflow, for a parameter that takes one to call back.
    pub fn microflow_argument(
        &mut self,
        parameter: &str,
        microflow: impl MicroflowName,
    ) -> &mut Self {
        self.mapping(
            parameter,
            NativeDocument::new("Microflows$MicroflowParameterValue")
                .with("Microflow", microflow.microflow_name()),
        )
    }
}

options! {
    MessageOptions, MESSAGE_OPTIONS {
        /// Whether the user has to dismiss the message before going on.
        blocking("Blocking"): Bool always true;
    }
}

impl MessageOptions<'_> {
    /// The message in one language, with `{n}` placeholders.
    pub fn text(&mut self, language: &str, text: impl Into<String>) -> &mut Self {
        let translation = NativeDocument::new("Texts$Translation")
            .with("LanguageCode", language)
            .with("Text", text.into());
        if let Some(items) = self
            .document
            .document_mut("Template")
            .and_then(|template| template.document_mut("Text"))
            .and_then(|text| text.list_mut("Items"))
        {
            items.push(NativeValue::Document(translation));
        }
        self
    }

    /// Supplies the next `{n}` placeholder of the message.
    pub fn parameter(&mut self, value: impl Into<Mx>) -> &mut Self {
        let parameter = actions::template_parameter(value.into().as_str());
        if let Some(parameters) = self
            .document
            .document_mut("Template")
            .and_then(|template| template.list_mut("Parameters"))
        {
            parameters.push(NativeValue::Document(parameter));
        }
        self
    }
}

options! {
    /// How a page is opened.
    PageOptions, PAGE_OPTIONS {
        /// Closes this many pages first.
        close_pages("NumberOfPagesToClose"): Expression always "";
    }
}

impl PageOptions<'_> {
    /// Passes `value` for a parameter of the page, named as the page
    /// declares it.
    pub fn argument(&mut self, parameter: &str, value: impl Into<Mx>) -> &mut Self {
        let Some(settings) = self.document.document_mut("FormSettings") else {
            return self;
        };
        let page = settings.text("Form").unwrap_or_default().to_string();
        let parameter = if parameter.contains('.') {
            parameter.to_string()
        } else {
            format!("{page}.{parameter}")
        };
        let mapping = NativeDocument::new("Forms$PageParameterMapping")
            .with("Argument", value.into().into_text())
            .with("Parameter", parameter);
        if let Some(mappings) = settings.list_mut("ParameterMappings") {
            mappings.push(NativeValue::Document(mapping));
        }
        self
    }

    /// Opens the page under a title of its own instead of the one it
    /// declares: the title in one language, with `{n}` placeholders.
    pub fn title(&mut self, language: &str, text: impl Into<String>) -> &mut Self {
        let translation = NativeDocument::new("Texts$Translation")
            .with("LanguageCode", language)
            .with("Text", text.into());
        if let Some(items) = self
            .title_template()
            .and_then(|template| template.document_mut("Text"))
            .and_then(|text| text.list_mut("Items"))
        {
            items.push(NativeValue::Document(translation));
        }
        self
    }

    /// Supplies the next `{n}` placeholder of the title.
    pub fn title_parameter(&mut self, value: impl Into<Mx>) -> &mut Self {
        let parameter = actions::template_parameter(value.into().as_str());
        if let Some(parameters) = self
            .title_template()
            .and_then(|template| template.list_mut("Parameters"))
        {
            parameters.push(NativeValue::Document(parameter));
        }
        self
    }

    fn title_template(&mut self) -> Option<&mut NativeDocument> {
        let settings = self.document.document_mut("FormSettings")?;
        if settings.document_mut("TitleOverride").is_none() {
            settings.set("TitleOverride", actions::title_override());
        }
        settings.document_mut("TitleOverride")
    }
}

/// The action documents the activities above declare, as the model stores
/// them. An importer builds the same documents from the model it reads, which
/// is how it knows the Rust it writes says exactly what the model does.
pub mod actions {
    use super::*;

    fn action(ty: &str) -> NativeDocument {
        NativeDocument::new(ty).with("ErrorHandlingType", "Rollback")
    }

    fn list(marker: i32) -> NativeValue {
        NativeValue::List(marker, Vec::new())
    }

    /// The model's document for a variable type.
    pub fn data_type(ty: &DataType) -> NativeDocument {
        match ty {
            DataType::String => NativeDocument::new("DataTypes$StringType"),
            DataType::Integer | DataType::Long => NativeDocument::new("DataTypes$IntegerType"),
            DataType::Float => NativeDocument::new("DataTypes$FloatType"),
            DataType::Decimal => NativeDocument::new("DataTypes$DecimalType"),
            DataType::Boolean => NativeDocument::new("DataTypes$BooleanType"),
            DataType::DateTime => NativeDocument::new("DataTypes$DateTimeType"),
            DataType::Binary => NativeDocument::new("DataTypes$BinaryType"),
            DataType::Object(entity) => {
                NativeDocument::new("DataTypes$ObjectType").with("Entity", entity.as_str())
            }
            DataType::List(entity) => {
                NativeDocument::new("DataTypes$ListType").with("Entity", entity.as_str())
            }
            DataType::Enumeration(enumeration) => NativeDocument::new("DataTypes$EnumerationType")
                .with("Enumeration", enumeration.as_str()),
        }
    }

    pub fn create_variable(name: &str, ty: &DataType, value: &str) -> NativeDocument {
        action("Microflows$CreateVariableAction")
            .with("InitialValue", value)
            .with("VariableName", name)
            .with("VariableType", data_type(ty))
    }

    pub fn change_variable(variable: &str, value: &str) -> NativeDocument {
        action("Microflows$ChangeVariableAction")
            .with("ChangeVariableName", variable)
            .with("Value", value)
    }

    pub fn create(variable: &str, entity: &str) -> NativeDocument {
        let mut document = action("Microflows$CreateChangeAction")
            .with("Entity", entity)
            .with("Items", list(2))
            .with("VariableName", variable);
        apply_defaults(&mut document, CHANGE_OPTIONS);
        document
    }

    pub fn change(variable: &str) -> NativeDocument {
        let mut document = action("Microflows$ChangeAction")
            .with("ChangeVariableName", variable)
            .with("Items", list(2));
        apply_defaults(&mut document, CHANGE_OPTIONS);
        document
    }

    pub fn change_item(member: &MemberName, kind: ChangeKind, value: &str) -> NativeDocument {
        NativeDocument::new("Microflows$ChangeActionItem")
            .with("Association", member.association_text())
            .with("Attribute", member.attribute_text())
            .with("Type", kind.native_name())
            .with("Value", value)
    }

    pub fn commit(variable: &str) -> NativeDocument {
        let mut document = action("Microflows$CommitAction").with("CommitVariableName", variable);
        apply_defaults(&mut document, COMMIT_OPTIONS);
        document
    }

    pub fn delete(variable: &str) -> NativeDocument {
        let mut document = action("Microflows$DeleteAction").with("DeleteVariableName", variable);
        apply_defaults(&mut document, REFRESH_OPTIONS);
        document
    }

    /// Narrows the object an inheritance split decided on to the entity of
    /// the branch it runs in, under a variable of its own.
    pub fn cast(variable: &str) -> NativeDocument {
        action("Microflows$CastAction").with("VariableName", variable)
    }

    pub fn rollback(variable: &str) -> NativeDocument {
        let mut document =
            action("Microflows$RollbackAction").with("RollbackVariableName", variable);
        apply_defaults(&mut document, REFRESH_OPTIONS);
        document
    }

    pub fn retrieve(variable: &str, entity: &str) -> NativeDocument {
        let mut document = action("Microflows$RetrieveAction")
            .with("ResultVariableName", variable)
            .with(
                "RetrieveSource",
                NativeDocument::new("Microflows$DatabaseRetrieveSource")
                    .with("Entity", entity)
                    .with(
                        "NewSortings",
                        NativeDocument::new("Microflows$SortingsList").with("Sortings", list(2)),
                    )
                    .with(
                        "Range",
                        NativeDocument::new("Microflows$ConstantRange").with("SingleObject", false),
                    ),
            );
        apply_defaults(&mut document, RETRIEVE_OPTIONS);
        document
    }

    pub fn custom_range(limit: &str, offset: &str) -> NativeDocument {
        NativeDocument::new("Microflows$CustomRange")
            .with("LimitExpression", limit)
            .with("OffsetExpression", offset)
    }

    pub fn sorting(attribute: &str, order: SortOrder) -> NativeDocument {
        NativeDocument::new("Microflows$RetrieveSorting")
            .with(
                "AttributeRef",
                NativeDocument::new("DomainModels$AttributeRef")
                    .with("Attribute", attribute)
                    .with("EntityRef", NativeValue::Null),
            )
            .with("SortOrder", order.native_name())
    }

    pub fn retrieve_associated(variable: &str, start: &str, association: &str) -> NativeDocument {
        action("Microflows$RetrieveAction")
            .with("ResultVariableName", variable)
            .with(
                "RetrieveSource",
                NativeDocument::new("Microflows$AssociationRetrieveSource")
                    .with("AssociationId", association)
                    .with("StartVariableName", start),
            )
    }

    pub fn create_list(variable: &str, entity: &str) -> NativeDocument {
        action("Microflows$CreateListAction")
            .with("Entity", entity)
            .with("VariableName", variable)
    }

    pub fn change_list(variable: &str, change: ListChange, value: &str) -> NativeDocument {
        action("Microflows$ChangeListAction")
            .with("ChangeVariableName", variable)
            .with("Type", change.native_name())
            .with("Value", value)
    }

    /// A list operation: the result variable around the operation itself.
    pub fn list_operation(variable: &str, operation: NativeDocument) -> NativeDocument {
        action("Microflows$ListOperationsAction")
            .with("NewOperation", operation)
            .with("ResultVariableName", variable)
    }

    /// `Head` or `Tail`: an operation over one list alone.
    pub fn unary_operation(kind: &str, list: &str) -> NativeDocument {
        NativeDocument::new(format!("Microflows${kind}")).with("ListName", list)
    }

    /// `Union`, `Intersect`, `Subtract`, `Contains` or `Equals`.
    pub fn binary_operation(kind: &str, list: &str, second: &str) -> NativeDocument {
        NativeDocument::new(format!("Microflows${kind}"))
            .with("ListName", list)
            .with("SecondListOrObjectName", second)
    }

    /// `Find` or `Filter`: by the value of one member.
    pub fn member_operation(
        kind: &str,
        list: &str,
        member: &MemberName,
        value: &str,
    ) -> NativeDocument {
        NativeDocument::new(format!("Microflows${kind}"))
            .with("Association", member.association_text())
            .with("Attribute", member.attribute_text())
            .with("Expression", value)
            .with("ListName", list)
    }

    /// `FindByExpression` or `FilterByExpression`.
    pub fn expression_operation(kind: &str, list: &str, expression: &str) -> NativeDocument {
        NativeDocument::new(format!("Microflows${kind}"))
            .with("Expression", expression)
            .with("ListName", list)
    }

    pub fn sort_operation(list: &str) -> NativeDocument {
        NativeDocument::new("Microflows$Sort")
            .with("ListName", list)
            .with(
                "Sortings",
                NativeDocument::new("Microflows$SortingsList").with("Sortings", list_value(2)),
            )
    }

    fn list_value(marker: i32) -> NativeValue {
        list(marker)
    }

    pub fn range_operation(list: &str, limit: &str, offset: &str) -> NativeDocument {
        NativeDocument::new("Microflows$ListRange")
            .with("CustomRange", custom_range(limit, offset))
            .with("ListName", list)
    }

    pub fn aggregate(variable: &str, list: &str, function: AggregateFunction) -> NativeDocument {
        action("Microflows$AggregateAction")
            .with("AggregateFunction", function.native_name())
            .with("AggregateVariableName", list)
            .with("Attribute", "")
            .with("VariableName", variable)
    }

    pub fn log(level: LogSeverity, node: &str, text: &str) -> NativeDocument {
        let mut document = action("Microflows$LogMessageAction")
            .with("Level", level.native_name())
            .with(
                "MessageTemplate",
                NativeDocument::new("Microflows$StringTemplate")
                    .with("Parameters", list(2))
                    .with("Text", text),
            )
            .with("Node", node);
        apply_defaults(&mut document, LOG_OPTIONS);
        document
    }

    pub fn template_parameter(expression: &str) -> NativeDocument {
        NativeDocument::new("Microflows$TemplateParameter").with("Expression", expression)
    }

    /// What a call on a task queue stores about the queue.
    pub fn queue_settings(queue: &str) -> NativeDocument {
        NativeDocument::new("Queues$QueueSettings")
            .with("Queue", queue)
            .with("Retry", NativeValue::Null)
    }

    /// A microflow call; `result` names the variable its return value goes
    /// into, when it has one worth naming.
    pub fn call(microflow: &str, result: Option<&str>) -> NativeDocument {
        action("Microflows$MicroflowCallAction")
            .with(
                "MicroflowCall",
                NativeDocument::new("Microflows$MicroflowCall")
                    .with("Microflow", microflow)
                    .with("ParameterMappings", list(2)),
            )
            .with("ResultVariableName", result.unwrap_or_default())
            .with("UseReturnVariable", true)
    }

    /// A Java action call; `result` as for [`call`].
    pub fn call_java(java_action: &str, result: Option<&str>) -> NativeDocument {
        action("Microflows$JavaActionCallAction")
            .with("JavaAction", java_action)
            .with("ParameterMappings", list(2))
            .with("ResultVariableName", result.unwrap_or_default())
            .with("UseReturnVariable", true)
    }

    pub fn show_message(kind: MessageKind) -> NativeDocument {
        let mut document = action("Microflows$ShowMessageAction")
            .with(
                "Template",
                NativeDocument::new("Microflows$TextTemplate")
                    .with("Parameters", list(2))
                    .with(
                        "Text",
                        NativeDocument::new("Texts$Text").with("Items", list(3)),
                    ),
            )
            .with("Type", kind.native_name());
        apply_defaults(&mut document, MESSAGE_OPTIONS);
        document
    }

    pub fn close_page(pages: &str) -> NativeDocument {
        action("Microflows$CloseFormAction").with("NumberOfPagesToClose", pages)
    }

    /// The title a page is opened under in place of its own, before any
    /// translation or placeholder is given.
    pub fn title_override() -> NativeDocument {
        NativeDocument::new("Microflows$TextTemplate")
            .with("Parameters", list(2))
            .with(
                "Text",
                NativeDocument::new("Texts$Text").with("Items", list(3)),
            )
    }

    pub fn show_page(page: &str) -> NativeDocument {
        let mut document = action("Microflows$ShowFormAction").with(
            "FormSettings",
            NativeDocument::new("Forms$FormSettings")
                .with("Form", page)
                .with("ParameterMappings", list(2)),
        );
        apply_defaults(&mut document, PAGE_OPTIONS);
        document
    }
}

/// What a parameter says about itself beyond its name and type.
pub struct ParameterOptions<'a> {
    declaration: &'a mut mxrs_ir::FlowParameterDecl,
}

impl ParameterOptions<'_> {
    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.declaration.documentation = value.into();
        self
    }

    pub fn required(&mut self, value: bool) -> &mut Self {
        self.declaration.required = value;
        self
    }

    pub fn default_value(&mut self, value: impl Into<Mx>) -> &mut Self {
        self.declaration.default_value = Some(value.into().into_text());
        self
    }
}

impl FlowBuilder {
    fn action(&mut self, document: NativeDocument) -> &mut Self {
        self.push(Activity::Action(document))
    }

    /// Declares a parameter of any type a flow can take, including an
    /// enumeration or an entity no struct of this project declares.
    ///
    /// # Panics
    /// Panics inside a branch or a loop body: parameters belong to the
    /// flow's signature.
    pub fn parameter_of(
        &mut self,
        name: impl Into<String>,
        ty: DataType,
        configure: impl FnOnce(&mut ParameterOptions<'_>),
    ) -> FlowVar {
        assert!(
            !self.is_nested(),
            "flow parameters must be declared on the outer flow builder"
        );
        let name = name.into();
        let mut declaration = mxrs_ir::FlowParameterDecl::new(&name, ty);
        configure(&mut ParameterOptions {
            declaration: &mut declaration,
        });
        self.declaration_mut().parameters.push(declaration);
        FlowVar(name)
    }

    /// Declares the flow's return type, for a flow whose returns are all
    /// inside its branches.
    pub fn returns(&mut self, ty: DataType) -> &mut Self {
        self.declaration_mut().return_type = Some(ty);
        self
    }

    /// Returns the value of `expression` from the flow. At the end of the
    /// flow's own body this is its result; inside a branch or a loop it ends
    /// the flow there.
    pub fn return_with(&mut self, expression: impl Into<Mx>) -> &mut Self {
        let expression = expression.into().into_text();
        if self.is_nested() {
            return self.push(Activity::ReturnValue { expression });
        }
        self.declaration_mut().return_expression = Some(expression);
        self
    }

    /// Ends a flow that returns nothing, from inside a branch or a loop.
    pub fn end(&mut self) -> &mut Self {
        if self.is_nested() {
            self.push(Activity::ReturnValue {
                expression: String::new(),
            });
        }
        self
    }

    /// Declares a variable of a primitive or enumeration type.
    pub fn create_variable(
        &mut self,
        name: impl Into<String>,
        ty: DataType,
        value: impl Into<Mx>,
    ) -> FlowVar {
        let name = name.into();
        self.action(actions::create_variable(&name, &ty, value.into().as_str()));
        FlowVar(name)
    }

    pub fn change_variable(&mut self, variable: &impl Variable, value: impl Into<Mx>) -> &mut Self {
        self.action(actions::change_variable(
            variable.variable_name(),
            value.into().as_str(),
        ))
    }

    /// Creates an object; `configure` sets its members and says whether it
    /// is committed.
    pub fn create(
        &mut self,
        name: impl Into<String>,
        entity: impl EntityName,
        configure: impl FnOnce(&mut ChangeOptions<'_>),
    ) -> FlowVar {
        let name = name.into();
        let mut document = actions::create(&name, &entity.entity_name());
        configure(&mut ChangeOptions {
            document: &mut document,
        });
        self.action(document);
        FlowVar(name)
    }

    /// Changes an object; `configure` sets its members and says whether it
    /// is committed.
    pub fn change(
        &mut self,
        variable: &impl Variable,
        configure: impl FnOnce(&mut ChangeOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::change(variable.variable_name());
        configure(&mut ChangeOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Commits an object or a list, with options: without events, or
    /// refreshing the client.
    pub fn commit_with(
        &mut self,
        variable: &impl Variable,
        configure: impl FnOnce(&mut CommitOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::commit(variable.variable_name());
        configure(&mut CommitOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Deletes an object or a list, with options.
    pub fn delete_with(
        &mut self,
        variable: &impl Variable,
        configure: impl FnOnce(&mut RefreshOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::delete(variable.variable_name());
        configure(&mut RefreshOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Undoes the uncommitted changes of an object.
    pub fn rollback(&mut self, variable: &impl Variable) -> &mut Self {
        self.rollback_with(variable, |_| {})
    }

    pub fn rollback_with(
        &mut self,
        variable: &impl Variable,
        configure: impl FnOnce(&mut RefreshOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::rollback(variable.variable_name());
        configure(&mut RefreshOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Retrieves objects of `entity` from the database: all that match,
    /// unless `configure` asks for the first one or a range.
    pub fn retrieve(
        &mut self,
        name: impl Into<String>,
        entity: impl EntityName,
        configure: impl FnOnce(&mut RetrieveOptions<'_>),
    ) -> FlowVar {
        let name = name.into();
        let mut document = actions::retrieve(&name, &entity.entity_name());
        configure(&mut RetrieveOptions {
            document: &mut document,
        });
        self.action(document);
        FlowVar(name)
    }

    /// Retrieves what `start` refers to over `association`.
    pub fn retrieve_associated(
        &mut self,
        name: impl Into<String>,
        start: &impl Variable,
        association: impl AssociationName,
    ) -> FlowVar {
        let name = name.into();
        self.action(actions::retrieve_associated(
            &name,
            start.variable_name(),
            &association.association_name(),
        ));
        FlowVar(name)
    }

    /// Creates an empty list of `entity`, named by its qualified name or by
    /// the struct that declares it.
    pub fn create_list_of(&mut self, name: impl Into<String>, entity: impl EntityName) -> FlowVar {
        let name = name.into();
        self.action(actions::create_list(&name, &entity.entity_name()));
        FlowVar(name)
    }

    pub fn change_list(
        &mut self,
        list: &impl Variable,
        change: ListChange,
        value: impl Into<Mx>,
    ) -> &mut Self {
        self.action(actions::change_list(
            list.variable_name(),
            change,
            value.into().as_str(),
        ))
    }

    fn list_operation(&mut self, name: impl Into<String>, operation: NativeDocument) -> FlowVar {
        let name = name.into();
        self.action(actions::list_operation(&name, operation));
        FlowVar(name)
    }

    /// The first object of a list.
    pub fn list_head(&mut self, name: impl Into<String>, list: &impl Variable) -> FlowVar {
        self.list_operation(name, actions::unary_operation("Head", list.variable_name()))
    }

    /// The list without its first object.
    pub fn list_tail(&mut self, name: impl Into<String>, list: &impl Variable) -> FlowVar {
        self.list_operation(name, actions::unary_operation("Tail", list.variable_name()))
    }

    /// The first object whose `member` equals `value`.
    pub fn list_find(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        member: impl Into<MemberName>,
        value: impl Into<Mx>,
    ) -> FlowVar {
        self.list_operation(
            name,
            actions::member_operation(
                "Find",
                list.variable_name(),
                &member.into(),
                value.into().as_str(),
            ),
        )
    }

    /// The first object `expression` holds for; it names each candidate
    /// `$currentObject`.
    pub fn list_find_by(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        expression: impl Into<Mx>,
    ) -> FlowVar {
        self.list_operation(
            name,
            actions::expression_operation(
                "FindByExpression",
                list.variable_name(),
                expression.into().as_str(),
            ),
        )
    }

    /// The objects whose `member` equals `value`.
    pub fn list_filter(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        member: impl Into<MemberName>,
        value: impl Into<Mx>,
    ) -> FlowVar {
        self.list_operation(
            name,
            actions::member_operation(
                "Filter",
                list.variable_name(),
                &member.into(),
                value.into().as_str(),
            ),
        )
    }

    /// The objects `expression` holds for; it names each candidate
    /// `$currentObject`.
    pub fn list_filter_by(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        expression: impl Into<Mx>,
    ) -> FlowVar {
        self.list_operation(
            name,
            actions::expression_operation(
                "FilterByExpression",
                list.variable_name(),
                expression.into().as_str(),
            ),
        )
    }

    /// The list in the order `configure` states.
    pub fn list_sort(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        configure: impl FnOnce(&mut SortOptions<'_>),
    ) -> FlowVar {
        let name = name.into();
        let mut document =
            actions::list_operation(&name, actions::sort_operation(list.variable_name()));
        configure(&mut SortOptions {
            document: &mut document,
        });
        self.action(document);
        FlowVar(name)
    }

    /// At most `limit` objects of the list, after skipping `offset`.
    pub fn list_range(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        limit: impl Into<Mx>,
        offset: impl Into<Mx>,
    ) -> FlowVar {
        self.list_operation(
            name,
            actions::range_operation(
                list.variable_name(),
                limit.into().as_str(),
                offset.into().as_str(),
            ),
        )
    }

    fn list_binary(
        &mut self,
        kind: &str,
        name: impl Into<String>,
        list: &impl Variable,
        second: &impl Variable,
    ) -> FlowVar {
        self.list_operation(
            name,
            actions::binary_operation(kind, list.variable_name(), second.variable_name()),
        )
    }

    /// The objects in either list.
    pub fn list_union(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        second: &impl Variable,
    ) -> FlowVar {
        self.list_binary("Union", name, list, second)
    }

    /// The objects in both lists.
    pub fn list_intersect(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        second: &impl Variable,
    ) -> FlowVar {
        self.list_binary("Intersect", name, list, second)
    }

    /// The objects of `list` that `second` does not hold.
    pub fn list_subtract(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        second: &impl Variable,
    ) -> FlowVar {
        self.list_binary("Subtract", name, list, second)
    }

    /// Whether `list` holds `item`, or every object of a second list.
    pub fn list_contains(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        item: &impl Variable,
    ) -> FlowVar {
        self.list_binary("Contains", name, list, item)
    }

    /// Whether two lists hold the same objects.
    pub fn list_equals(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        second: &impl Variable,
    ) -> FlowVar {
        self.list_binary("Equals", name, list, second)
    }

    /// Counts a list, or reduces it with another function `configure` gives
    /// an attribute or an expression to.
    pub fn aggregate(
        &mut self,
        name: impl Into<String>,
        list: &impl Variable,
        function: AggregateFunction,
        configure: impl FnOnce(&mut AggregateOptions<'_>),
    ) -> FlowVar {
        let name = name.into();
        let mut document = actions::aggregate(&name, list.variable_name(), function);
        configure(&mut AggregateOptions {
            document: &mut document,
        });
        self.action(document);
        FlowVar(name)
    }

    /// Writes a log line. `node` is an expression — usually a quoted name —
    /// and `text` a template whose `{n}` placeholders `configure` supplies.
    pub fn log(
        &mut self,
        level: LogSeverity,
        node: impl Into<Mx>,
        text: impl Into<String>,
        configure: impl FnOnce(&mut LogOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::log(level, node.into().as_str(), &text.into());
        configure(&mut LogOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Calls a microflow and discards what it returns.
    pub fn call(
        &mut self,
        microflow: impl MicroflowName,
        configure: impl FnOnce(&mut CallOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::call(&microflow.microflow_name(), None);
        configure(&mut CallOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Calls a microflow and keeps what it returns in `name`.
    pub fn call_into(
        &mut self,
        name: impl Into<String>,
        microflow: impl MicroflowName,
        configure: impl FnOnce(&mut CallOptions<'_>),
    ) -> FlowVar {
        let name = name.into();
        let mut document = actions::call(&microflow.microflow_name(), Some(&name));
        configure(&mut CallOptions {
            document: &mut document,
        });
        self.action(document);
        FlowVar(name)
    }

    /// Calls a Java action, named `Module.Action`, and discards its result.
    pub fn call_java(
        &mut self,
        java_action: &str,
        configure: impl FnOnce(&mut JavaCallOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::call_java(java_action, None);
        configure(&mut JavaCallOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Calls a Java action and keeps its result in `name`.
    pub fn call_java_into(
        &mut self,
        name: impl Into<String>,
        java_action: &str,
        configure: impl FnOnce(&mut JavaCallOptions<'_>),
    ) -> FlowVar {
        let name = name.into();
        let mut document = actions::call_java(java_action, Some(&name));
        configure(&mut JavaCallOptions {
            document: &mut document,
        });
        self.action(document);
        FlowVar(name)
    }

    /// Shows the user a message; `configure` gives its text per language.
    pub fn show_message(
        &mut self,
        kind: MessageKind,
        configure: impl FnOnce(&mut MessageOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::show_message(kind);
        configure(&mut MessageOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Closes the page the flow was called from.
    pub fn close_page(&mut self) -> &mut Self {
        self.action(actions::close_page(""))
    }

    /// Closes as many pages as `count` evaluates to.
    pub fn close_pages(&mut self, count: impl Into<Mx>) -> &mut Self {
        self.action(actions::close_page(count.into().as_str()))
    }

    /// Opens a page, named `Module.Page`; `configure` passes its arguments.
    pub fn show_page(
        &mut self,
        page: &str,
        configure: impl FnOnce(&mut PageOptions<'_>),
    ) -> &mut Self {
        let mut document = actions::show_page(page);
        configure(&mut PageOptions {
            document: &mut document,
        });
        self.action(document)
    }

    /// Declares an activity as the action document the model stores for it,
    /// field by field. This is how an activity no builder here covers yet is
    /// still stated — and edited — in Rust; what it declares is reached
    /// afterwards with [`var`].
    pub fn native_action(&mut self, mut document: NativeDocument) -> &mut Self {
        if document.get("ErrorHandlingType").is_none() {
            document.set("ErrorHandlingType", "Rollback");
        }
        self.action(document)
    }

    /// A decision a rule makes rather than an expression.
    ///
    /// ```ignore
    /// flow.decision_by_rule(
    ///     "Sales.RULE_CanShip",
    ///     |rule| {
    ///         rule.argument("Order", mx("$Order"));
    ///     },
    ///     |flow| { /* it can */ },
    ///     |flow| { /* it cannot */ },
    /// );
    /// ```
    pub fn decision_by_rule(
        &mut self,
        rule: impl MicroflowName,
        arguments: impl FnOnce(&mut RuleArguments),
        then: impl FnOnce(&mut FlowBuilder),
        otherwise: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        let mut rule = RuleArguments {
            rule: rule.microflow_name(),
            arguments: Vec::new(),
        };
        arguments(&mut rule);
        self.push(Activity::RuleDecision {
            rule: rule.rule,
            arguments: rule.arguments,
            true_branch: FlowBuilder::nested(then),
            false_branch: FlowBuilder::nested(otherwise),
        })
    }

    /// A switch a rule decides: a branch per value of the enumeration the
    /// rule returns.
    pub fn switch_by_rule(
        &mut self,
        rule: impl MicroflowName,
        arguments: impl FnOnce(&mut RuleArguments),
        cases: impl FnOnce(&mut Cases),
    ) -> &mut Self {
        let mut rule = RuleArguments {
            rule: rule.microflow_name(),
            arguments: Vec::new(),
        };
        arguments(&mut rule);
        let mut on = Cases { cases: Vec::new() };
        cases(&mut on);
        self.push(Activity::RuleSwitch {
            rule: rule.rule,
            arguments: rule.arguments,
            cases: on.cases,
        })
    }

    /// Names this point of the flow so that [`FlowBuilder::jump`] can carry
    /// on from it: what a flow that tries again goes back to.
    ///
    /// ```ignore
    /// flow.label("again");
    /// let response = flow.call_into("Response", MicroflowRef::<SUB_Fetch>::new(), |_| {});
    /// flow.decision(
    ///     mx("$Response = empty"),
    ///     |flow| {
    ///         flow.jump("again");
    ///     },
    ///     |_| {},
    /// );
    /// ```
    pub fn label(&mut self, name: impl Into<String>) -> &mut Self {
        self.push(Activity::Label(name.into()))
    }

    /// Ends this path by carrying on at the [`FlowBuilder::label`] of that
    /// name, which the same flow — or the same loop body — declares.
    pub fn jump(&mut self, name: impl Into<String>) -> &mut Self {
        self.push(Activity::Jump(name.into()))
    }

    /// Inside a branch of [`FlowBuilder::switch_type`]: the object the
    /// switch decided on, as the entity of that branch.
    pub fn cast(&mut self, name: impl Into<String>) -> FlowVar {
        let name = name.into();
        self.action(actions::cast(&name));
        FlowVar(name)
    }

    /// Runs the branch that the value of an enumeration expression selects.
    ///
    /// ```ignore
    /// flow.switch(mx("$Order/Status"), |on| {
    ///     on.case("Open", |flow| { /* ... */ });
    ///     on.cases(["Shipped", "Closed"], |flow| { /* ... */ });
    ///     on.empty(|flow| { /* no status at all */ });
    /// });
    /// ```
    pub fn switch(
        &mut self,
        expression: impl Into<Mx>,
        cases: impl FnOnce(&mut Cases),
    ) -> &mut Self {
        let mut on = Cases { cases: Vec::new() };
        cases(&mut on);
        self.push(Activity::Switch {
            expression: expression.into().into_text(),
            cases: on.cases,
        })
    }

    /// Runs the branch for the entity the object in `variable` is an
    /// instance of. A branch reaches the object as that entity with
    /// [`FlowBuilder::cast`].
    pub fn switch_type(
        &mut self,
        variable: &impl Variable,
        cases: impl FnOnce(&mut TypeCases),
    ) -> &mut Self {
        let mut on = TypeCases { cases: Vec::new() };
        cases(&mut on);
        self.push(Activity::TypeSwitch {
            variable: variable.variable_name().to_string(),
            cases: on.cases,
        })
    }

    /// Runs `body` for each object of `list`, which it receives under the
    /// name `iterator`.
    pub fn for_each(
        &mut self,
        list: &impl Variable,
        iterator: impl Into<String>,
        body: impl FnOnce(&mut FlowBuilder, FlowVar),
    ) -> &mut Self {
        let iterator = iterator.into();
        let item = FlowVar(iterator.clone());
        let activities = FlowBuilder::nested(|flow| body(flow, item));
        self.push(Activity::LoopOver {
            list_variable: list.variable_name().to_string(),
            iterator,
            activities,
        })
    }
}

/// What a rule is called with in [`FlowBuilder::decision_by_rule`].
pub struct RuleArguments {
    rule: String,
    arguments: Vec<(String, String)>,
}

impl RuleArguments {
    /// Passes `value` for the rule's parameter `parameter`: its own name,
    /// or the qualified `Module.Rule.Parameter`.
    pub fn argument(&mut self, parameter: &str, value: impl Into<Mx>) -> &mut Self {
        let parameter = if parameter.contains('.') {
            parameter.to_string()
        } else {
            format!("{}.{parameter}", self.rule)
        };
        self.arguments.push((parameter, value.into().into_text()));
        self
    }
}

/// The branches of a [`FlowBuilder::switch`], one per value.
pub struct Cases {
    cases: Vec<SwitchCase>,
}

impl Cases {
    /// What runs when the expression is the enumeration value `value`.
    pub fn case(
        &mut self,
        value: impl Into<String>,
        body: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        self.cases([value], body)
    }

    /// One branch for several values.
    pub fn cases<S: Into<String>>(
        &mut self,
        values: impl IntoIterator<Item = S>,
        body: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        self.cases.push(SwitchCase {
            values: values.into_iter().map(Into::into).collect(),
            activities: FlowBuilder::nested(body),
        });
        self
    }

    /// What runs when the expression has no value.
    pub fn empty(&mut self, body: impl FnOnce(&mut FlowBuilder)) -> &mut Self {
        self.case("(empty)", body)
    }
}

/// The branches of a [`FlowBuilder::switch_type`], one per entity.
pub struct TypeCases {
    cases: Vec<SwitchCase>,
}

impl TypeCases {
    /// What runs when the object is an instance of `entity`.
    pub fn case(
        &mut self,
        entity: impl EntityName,
        body: impl FnOnce(&mut FlowBuilder),
    ) -> &mut Self {
        self.cases.push(SwitchCase {
            values: vec![entity.entity_name()],
            activities: FlowBuilder::nested(body),
        });
        self
    }

    /// What runs when there is no object.
    pub fn empty(&mut self, body: impl FnOnce(&mut FlowBuilder)) -> &mut Self {
        self.cases.push(SwitchCase {
            values: vec![String::new()],
            activities: FlowBuilder::nested(body),
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(document: &NativeDocument, path: &str) -> String {
        match document.get_path(path) {
            Some(NativeValue::Text(text)) => text.clone(),
            other => panic!("{path}: {other:?}"),
        }
    }

    #[test]
    fn an_option_left_unsaid_has_the_models_default() {
        let commit = actions::commit("Order");
        assert_eq!(
            commit.get("RefreshInClient"),
            Some(&NativeValue::Bool(false))
        );
        assert_eq!(commit.get("WithEvents"), Some(&NativeValue::Bool(true)));
        assert_eq!(text(&commit, "ErrorHandlingType"), "Rollback");

        let mut create = actions::create("NewOrder", "Sales.Order");
        assert_eq!(text(&create, "Commit"), "No");
        ChangeOptions {
            document: &mut create,
        }
        .set(
            MemberName::attribute("Sales.Order.Number"),
            mxrs_expr::mx("'A-1'"),
        )
        .commit(Commit::WithoutEvents);
        assert_eq!(text(&create, "Commit"), "YesWithoutEvents");
        let Some(NativeValue::List(2, items)) = create.get("Items") else {
            panic!("items")
        };
        let NativeValue::Document(item) = &items[0] else {
            panic!("item")
        };
        assert_eq!(text(item, "Attribute"), "Sales.Order.Number");
        assert_eq!(text(item, "Association"), "");
        assert_eq!(text(item, "Type"), "Set");
        assert_eq!(text(item, "Value"), "'A-1'");
    }

    #[test]
    fn a_retrieve_states_its_constraint_order_and_range() {
        let mut retrieve = actions::retrieve("Orders", "Sales.Order");
        assert_eq!(text(&retrieve, "RetrieveSource.XpathConstraint"), "");
        RetrieveOptions {
            document: &mut retrieve,
        }
        .xpath("[Total > 10]")
        .sort_by("Sales.Order.Total", SortOrder::Descending)
        .range(mxrs_expr::mx("10"), mxrs_expr::mx(""));
        assert_eq!(
            text(&retrieve, "RetrieveSource.XpathConstraint"),
            "[Total > 10]"
        );
        assert_eq!(
            text(&retrieve, "RetrieveSource.Range.LimitExpression"),
            "10"
        );
        let Some(NativeValue::List(2, sortings)) =
            retrieve.get_path("RetrieveSource.NewSortings.Sortings")
        else {
            panic!("sortings")
        };
        let NativeValue::Document(sorting) = &sortings[0] else {
            panic!("sorting")
        };
        assert_eq!(text(sorting, "AttributeRef.Attribute"), "Sales.Order.Total");
        assert_eq!(text(sorting, "SortOrder"), "Descending");
    }

    #[test]
    fn a_call_names_parameters_the_way_the_called_flow_declares_them() {
        let mut call = actions::call("Sales.SUB_Ship", Some("Shipped"));
        CallOptions {
            document: &mut call,
        }
        .argument("Order", mxrs_expr::mx("$Order"));
        assert_eq!(
            call.get("UseReturnVariable"),
            Some(&NativeValue::Bool(true))
        );
        let Some(NativeValue::List(2, mappings)) = call.get_path("MicroflowCall.ParameterMappings")
        else {
            panic!("mappings")
        };
        let NativeValue::Document(mapping) = &mappings[0] else {
            panic!("mapping")
        };
        assert_eq!(text(mapping, "Parameter"), "Sales.SUB_Ship.Order");
    }

    fn built(body: impl FnOnce(&mut FlowBuilder)) -> Vec<Activity> {
        FlowBuilder::nested(body)
    }

    #[test]
    fn a_modifier_says_how_the_next_activity_runs_and_no_other() {
        let order = var("Order");
        let activities = built(|flow| {
            flow.on_error(|flow| {
                flow.raise_error();
            })
            .commit_with(&order, |_| {});
            flow.continue_on_error().delete_with(&order, |_| {});
            flow.disabled().rollback(&order);
            flow.rollback(&order);
        });
        let [handled, continued, disabled, plain] = activities.as_slice() else {
            panic!("{activities:?}")
        };
        let Activity::OnError {
            handling: mxrs_ir::ErrorHandling::Custom,
            activity,
            handler,
        } = handled
        else {
            panic!("{handled:?}")
        };
        assert!(matches!(activity.as_ref(), Activity::Action(_)));
        assert!(matches!(handler.as_slice(), [Activity::RaiseError]));
        assert!(matches!(
            continued,
            Activity::OnError {
                handling: mxrs_ir::ErrorHandling::Continue,
                handler,
                ..
            } if handler.is_empty()
        ));
        assert!(
            matches!(disabled, Activity::Disabled(inner) if matches!(inner.as_ref(), Activity::Action(_)))
        );
        assert!(matches!(plain, Activity::Action(_)));
    }

    #[test]
    fn a_switch_has_a_branch_per_case_and_a_rule_its_arguments() {
        let animal = var("Animal");
        let activities = built(|flow| {
            flow.switch(mxrs_expr::mx("$Order/Status"), |on| {
                on.case("Open", |flow| {
                    flow.end();
                });
                on.cases(["Shipped", "Closed"], |_| {});
                on.empty(|_| {});
            });
            flow.switch_type(&animal, |on| {
                on.case("Zoo.Dog", |flow| {
                    flow.cast("Dog");
                });
                on.empty(|_| {});
            });
            flow.decision_by_rule(
                "Sales.RULE_CanShip",
                |rule| {
                    rule.argument("Order", mxrs_expr::mx("$Order"));
                },
                |_| {},
                |flow| {
                    flow.jump("again");
                },
            );
            flow.label("again");
        });
        let [switch, by_type, by_rule, label] = activities.as_slice() else {
            panic!("{activities:?}")
        };
        let values = |cases: &[SwitchCase]| -> Vec<Vec<String>> {
            cases.iter().map(|case| case.values.clone()).collect()
        };
        let Activity::Switch { expression, cases } = switch else {
            panic!("{switch:?}")
        };
        assert_eq!(expression, "$Order/Status");
        assert_eq!(
            values(cases),
            [
                vec!["Open".to_string()],
                vec!["Shipped".to_string(), "Closed".to_string()],
                vec!["(empty)".to_string()],
            ]
        );
        assert!(matches!(
            cases[0].activities.as_slice(),
            [Activity::ReturnValue { .. }]
        ));
        let Activity::TypeSwitch { variable, cases } = by_type else {
            panic!("{by_type:?}")
        };
        assert_eq!(variable, "Animal");
        assert_eq!(
            values(cases),
            [vec!["Zoo.Dog".to_string()], vec![String::new()]]
        );
        let Activity::RuleDecision {
            rule,
            arguments,
            false_branch,
            ..
        } = by_rule
        else {
            panic!("{by_rule:?}")
        };
        assert_eq!(rule, "Sales.RULE_CanShip");
        assert_eq!(
            arguments,
            &[("Sales.RULE_CanShip.Order".to_string(), "$Order".to_string())]
        );
        assert!(matches!(false_branch.as_slice(), [Activity::Jump(name)] if name == "again"));
        assert!(matches!(label, Activity::Label(name) if name == "again"));
    }

    #[test]
    fn a_call_can_discard_its_result_or_run_on_a_queue() {
        let mut call = actions::call("Sales.SUB_Ship", None);
        CallOptions {
            document: &mut call,
        }
        .discard_result("Shipped")
        .queue("Sales.Shipping");
        assert_eq!(
            call.get("UseReturnVariable"),
            Some(&NativeValue::Bool(false))
        );
        assert_eq!(text(&call, "ResultVariableName"), "Shipped");
        assert_eq!(
            text(&call, "MicroflowCall.QueueSettings.Queue"),
            "Sales.Shipping"
        );
    }

    #[test]
    fn a_page_can_be_opened_under_a_title_of_its_own() {
        let mut page = actions::show_page("Sales.Order_Edit");
        assert_eq!(page.get_path("FormSettings.TitleOverride"), None);
        PageOptions {
            document: &mut page,
        }
        .title("en_US", "Edit {1}")
        .title_parameter(mxrs_expr::mx("$Order/Number"));
        let Some(NativeValue::List(3, translations)) =
            page.get_path("FormSettings.TitleOverride.Text.Items")
        else {
            panic!("translations")
        };
        let NativeValue::Document(translation) = &translations[0] else {
            panic!("translation")
        };
        assert_eq!(text(translation, "LanguageCode"), "en_US");
        assert_eq!(text(translation, "Text"), "Edit {1}");
        assert!(matches!(
            page.get_path("FormSettings.TitleOverride.Parameters"),
            Some(NativeValue::List(2, parameters)) if parameters.len() == 1
        ));
    }

    #[test]
    fn an_activity_no_builder_covers_is_stated_as_its_document() {
        let activities = built(|flow| {
            flow.native_action(
                NativeDocument::new("Microflows$DownloadFileAction")
                    .with("FileDocumentVariableName", "Report")
                    .with("ShowFileInBrowser", false),
            );
        });
        let [Activity::Action(document)] = activities.as_slice() else {
            panic!("{activities:?}")
        };
        assert_eq!(document.ty, "Microflows$DownloadFileAction");
        // How it fails is the default unless something says otherwise.
        assert_eq!(text(document, "ErrorHandlingType"), "Rollback");
    }

    #[test]
    fn every_option_table_names_a_default_of_its_kind() {
        for specs in [
            CHANGE_OPTIONS,
            COMMIT_OPTIONS,
            REFRESH_OPTIONS,
            RETRIEVE_OPTIONS,
            LOG_OPTIONS,
            MESSAGE_OPTIONS,
            PAGE_OPTIONS,
        ] {
            for spec in specs {
                match (spec.kind, spec.default) {
                    (OptionKind::Bool, OptionDefault::Bool(_)) => {}
                    (OptionKind::Text | OptionKind::Expression, OptionDefault::Text(_)) => {}
                    (OptionKind::Enumeration(variants), OptionDefault::Text(default)) => {
                        assert!(
                            variants.iter().any(|(native, _)| *native == default),
                            "{}: {default}",
                            spec.setter
                        );
                    }
                    other => panic!("{}: {other:?}", spec.setter),
                }
            }
        }
    }
}
