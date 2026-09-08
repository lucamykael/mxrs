//! Project/Module/Entity declarations. Reuses `mxrs-model`'s `Attribute`
//! (and its `AttributeType`) as-is for entity attributes — those need no
//! name resolution — but associations need a name-based `target` (the
//! referenced entity doesn't have a UUID yet at declaration time), unlike
//! `mxrs_model::Association`, which is storage-shaped (`from_entity_id`/
//! `to_entity_id` as resolved UUID strings). `mxrs-writer` resolves
//! `AssociationDecl`s into `mxrs_model::Association`s once every entity in
//! the project has been assigned an id.

use mxrs_model::association::{AssociationType, Owner, StorageFormat};
use mxrs_model::Attribute;

use crate::flow::MicroflowDecl;

#[derive(Debug, Clone)]
pub struct AssociationDecl {
    pub name: String,
    /// The referenced entity, as `"Entity"` (same module) or
    /// `"Module.Entity"` (cross-module).
    pub target: String,
    pub association_type: AssociationType,
    pub owner: Owner,
    pub storage_format: StorageFormat,
    pub documentation: String,
}

#[derive(Debug, Clone)]
pub struct EntityDecl {
    pub name: String,
    pub documentation: String,
    pub persistable: bool,
    pub attributes: Vec<Attribute>,
    pub associations: Vec<AssociationDecl>,
}

impl EntityDecl {
    pub fn new(name: impl Into<String>) -> Self {
        EntityDecl {
            name: name.into(),
            documentation: String::new(),
            persistable: true,
            attributes: vec![],
            associations: vec![],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModuleDecl {
    pub name: String,
    pub entities: Vec<EntityDecl>,
    pub microflows: Vec<MicroflowDecl>,
}

#[derive(Debug, Clone)]
pub struct ProjectDecl {
    pub mendix_version: String,
    pub modules: Vec<ModuleDecl>,
}
