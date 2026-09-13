use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriterError {
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

    #[error("Forms metamodel error: {0}")]
    Forms(#[from] mxrs_forms::FormsError),

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("MPR I/O error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),

    #[error("project scaffolding error: {0}")]
    ProjectTemplate(#[from] mxrs_schema::ProjectTemplateError),

    #[error("identity error: {0}")]
    Identity(#[from] mxrs_identity::IdentityError),
}

pub type Result<T> = std::result::Result<T, WriterError>;
