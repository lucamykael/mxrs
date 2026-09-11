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

    #[error("entities missing from domain model of module {module_name:?}: {missing:?}")]
    EntitiesMissingFromDomainModel {
        module_name: String,
        missing: Vec<String>,
    },

    #[error("duplicate association {module_name}.{name:?} declared while synchronizing")]
    DuplicateAssociation { module_name: String, name: String },

    #[error("duplicate entity {module_name}.{name:?} declared while synchronizing")]
    DuplicateEntity { module_name: String, name: String },

    #[error("BSON codec error: {0}")]
    Bson(#[from] mxrs_bson::BsonCodecError),

    #[error("MPR I/O error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),

    #[error("project scaffolding error: {0}")]
    ProjectTemplate(#[from] mxrs_schema::ProjectTemplateError),
}

pub type Result<T> = std::result::Result<T, WriterError>;
