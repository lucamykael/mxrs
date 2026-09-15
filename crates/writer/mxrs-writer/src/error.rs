use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriterError {
    #[error("flow {flow:?} has invalid parameter {parameter:?}: {reason}")]
    InvalidFlowParameter {
        flow: String,
        parameter: String,
        reason: String,
    },

    #[error("flow {flow:?} has invalid call to {target:?}: {reason}")]
    InvalidFlowCall {
        flow: String,
        target: String,
        reason: String,
    },

    #[error("unsupported Mendix version {0:?} (mxrs-schema has no embedded schema hash for it)")]
    UnsupportedVersion(String),

    #[error("newly created .mpr has no root unit")]
    MissingRootUnit,

    #[error("association target {0:?} does not match any entity declared in this module")]
    UnknownAssociationTarget(String),

    #[error(
        "cross-module association target {0:?} does not match any entity declared anywhere in the project (expected \"Module.Entity\")"
    )]
    UnknownCrossModuleAssociationTarget(String),

    #[error("module {0:?} has no DomainModel unit to synchronize")]
    MissingDomainModel(String),

    #[error("module unit {0:?} was not found")]
    MissingModuleUnit(String),

    #[error("module unit {0:?} has no string Name")]
    MissingModuleName(String),

    #[error("functional instrumentation module {0:?} already exists")]
    InstrumentationModuleExists(String),

    #[error("project settings unit not found")]
    MissingProjectSettings,

    #[error("model settings part not found")]
    MissingModelSettings,

    #[error("entities missing from domain model of module {module_name:?}: {missing:?}")]
    EntitiesMissingFromDomainModel {
        module_name: String,
        missing: Vec<String>,
    },

    #[error("duplicate association {module_name}.{name:?} declared while synchronizing")]
    DuplicateAssociation { module_name: String, name: String },

    #[error("duplicate entity {module_name}.{name:?} declared while synchronizing")]
    DuplicateEntity { module_name: String, name: String },

    #[error("duplicate {kind} {name:?} declared while synchronizing security")]
    DuplicateSecurityName { kind: String, name: String },

    #[error("security references undeclared user role {0:?}")]
    UnknownUserRole(String),

    #[error("security references unknown module role {0:?}")]
    UnknownModuleRole(String),

    #[error("duplicate navigation profile {0:?}")]
    DuplicateNavigationProfile(String),

    #[error("navigation target at {0} declares conflicting or missing page/microflow choices")]
    ConflictingNavigationTargets(String),

    #[error("navigation references unknown {kind} target {reference:?} at {path}")]
    UnknownNavigationTarget {
        kind: String,
        reference: String,
        path: String,
    },

    #[error(
        "page {0:?} declares widgets but no layout — a Forms$Page has no widgets of its own; every widget attaches to a LayoutCallArgument inside LayoutCall.arguments, so a layout (via PageBuilder::layout) is required whenever a page has widgets"
    )]
    PageWidgetsRequireLayout(String),

    #[error("layout placeholder {placeholder:?} cannot appear inside page {page:?}")]
    PlaceholderOutsideLayout { page: String, placeholder: String },

    #[error("layout {layout:?} contains invalid or duplicated placeholder {placeholder:?}")]
    InvalidLayoutPlaceholder { layout: String, placeholder: String },

    #[error("layout {0:?} declares non-positive canvas dimensions")]
    InvalidLayoutCanvas(String),

    #[error("duplicate layout {0:?} declared while synchronizing")]
    DuplicateLayout(String),

    #[error("layout reference {layout:?} has an invalid placeholder reference {parameter:?}")]
    InvalidLayoutParameterReference { layout: String, parameter: String },

    #[error("page {page:?} declares duplicate parameter {parameter:?}")]
    DuplicatePageParameter { page: String, parameter: String },

    #[error("page {page:?} data view references unknown parameter {parameter:?}")]
    UnknownPageParameter { page: String, parameter: String },

    #[error(
        "page {page:?} data view expects parameter {parameter:?} to have entity {expected:?}, but it declares {actual:?}"
    )]
    PageParameterEntityMismatch {
        page: String,
        parameter: String,
        expected: String,
        actual: String,
    },

    #[error(
        "entity {module_name}.{name} declares an access rule with no module roles; a rule that grants rights to nobody is not expressible"
    )]
    AccessRuleWithoutRoles { module_name: String, name: String },

    #[error("entity {entity:?} references unknown OQL view source {source_name:?}")]
    UnknownOqlViewSource { entity: String, source_name: String },

    #[error("module {module_name:?} declares duplicate OQL view source {name:?}")]
    DuplicateOqlViewSource { module_name: String, name: String },

    #[error("OQL view entity {0:?} cannot be persistable")]
    PersistableOqlView(String),

    #[error("entity {module_name}.{name} declares an empty index")]
    EmptyEntityIndex { module_name: String, name: String },

    #[error("entity {module_name}.{name} index references unknown attribute {attribute:?}")]
    UnknownIndexedAttribute {
        module_name: String,
        name: String,
        attribute: String,
    },

    #[error("entity {module_name}.{name} declares duplicate indexes with members {members:?}")]
    DuplicateEntityIndex {
        module_name: String,
        name: String,
        members: Vec<String>,
    },

    #[error("entity {module_name}.{name} declares duplicate lifecycle event {event:?}")]
    DuplicateLifecycleEvent {
        module_name: String,
        name: String,
        event: String,
    },

    #[error("entity {module_name}.{name} lifecycle event {event:?} has an empty handler")]
    EmptyLifecycleHandler {
        module_name: String,
        name: String,
        event: String,
    },

    #[error("entity {entity:?} lifecycle event {event:?} references unknown microflow {handler:?}")]
    UnknownLifecycleHandler {
        entity: String,
        event: String,
        handler: String,
    },

    #[error("entity {module_name}.{name} generalizes unknown entity {target:?}")]
    UnknownGeneralizationTarget {
        module_name: String,
        name: String,
        target: String,
    },

    #[error("duplicate constant {module_name}.{name:?} declared while synchronizing")]
    DuplicateConstant { module_name: String, name: String },

    #[error("duplicate task queue {module_name}.{name:?} declared while synchronizing")]
    DuplicateTaskQueue { module_name: String, name: String },

    #[error("invalid task queue {name:?}: {reason}")]
    InvalidTaskQueue { name: String, reason: String },

    #[error("duplicate regular expression {module_name}.{name:?} declared while synchronizing")]
    DuplicateRegularExpression { module_name: String, name: String },

    #[error("duplicate scheduled event {module_name}.{name:?} declared while synchronizing")]
    DuplicateScheduledEvent { module_name: String, name: String },

    #[error("duplicate menu {module_name}.{name:?} declared while synchronizing")]
    DuplicateMenu { module_name: String, name: String },

    #[error("scheduled event {name:?} declares a negative legacy interval ({interval})")]
    InvalidScheduleInterval { name: String, interval: i64 },

    #[error("scheduled event {name:?} has an invalid RFC 3339 start instant {value:?}")]
    InvalidScheduledEventStart { name: String, value: String },

    #[error("scheduled event {name:?} has invalid {field} value {value}")]
    InvalidScheduledEventScheduleValue {
        name: String,
        field: &'static str,
        value: i64,
    },

    #[error("scheduled event {0:?} declares no microflow to run")]
    ScheduledEventWithoutMicroflow(String),

    #[error("Forms metamodel error: {0}")]
    Forms(#[from] mxrs_forms::FormsError),

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("MPR I/O error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),

    #[error("model read error: {0}")]
    Model(#[from] mxrs_model::ModelError),

    #[error("project scaffolding error: {0}")]
    ProjectTemplate(#[from] mxrs_schema::ProjectTemplateError),

    #[error("identity error: {0}")]
    Identity(#[from] mxrs_identity::IdentityError),
}

pub type Result<T> = std::result::Result<T, WriterError>;
