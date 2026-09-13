//! Storage-independent project/module/entity declarations. Nothing in this
//! module knows about BSON field names, UUIDs, raw documents, export levels,
//! or other `.mpr` representation details. `mxrs-writer` lowers these values
//! into `mxrs-model` only at the persistence boundary.

use crate::flow::MicroflowDecl;
use crate::page::PageDecl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// An author-level attribute kind.
///
/// The storage model uses a separate enum and is lowered by `mxrs-writer`:
///
/// ```compile_fail
/// let _ = mxrs_ir::AttributeDecl::new("Name", "String");
/// ```
pub enum AttributeType {
    String,
    Integer,
    Long,
    Float,
    Decimal,
    Boolean,
    DateTime,
    AutoNumber,
    HashString,
    Binary,
    Enumeration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeDecl {
    pub name: String,
    pub documentation: String,
    pub attribute_type: AttributeType,
    pub default_value: Option<String>,
    pub length: Option<i32>,
    pub localize_date: Option<bool>,
    /// Qualified name of the backing enumeration for
    /// [`AttributeType::Enumeration`].
    pub enumeration: Option<String>,
    pub required: bool,
    pub unique: bool,
}

impl AttributeDecl {
    pub fn new(name: impl Into<String>, attribute_type: AttributeType) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            attribute_type,
            default_value: None,
            length: None,
            localize_date: None,
            enumeration: None,
            required: false,
            unique: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Association cardinality at the declaration boundary.
///
/// Raw strings cannot cross this boundary:
///
/// ```compile_fail
/// let _: mxrs_ir::AssociationType = "Reference";
/// ```
pub enum AssociationType {
    Reference,
    ReferenceSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssociationOwner {
    #[default]
    Default,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssociationStorage {
    #[default]
    Column,
    Table,
}

#[derive(Debug, Clone)]
pub struct AssociationDecl {
    pub name: String,
    /// The referenced entity, as `"Entity"` (same module) or
    /// `"Module.Entity"` (cross-module).
    pub target: String,
    pub association_type: AssociationType,
    pub owner: AssociationOwner,
    pub storage: AssociationStorage,
    pub documentation: String,
}

#[derive(Debug, Clone)]
pub struct EntityDecl {
    pub name: String,
    pub documentation: String,
    pub persistable: bool,
    pub attributes: Vec<AttributeDecl>,
    pub associations: Vec<AssociationDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumerationValueDecl {
    pub name: String,
    /// Localized captions as `(language_code, text)` pairs.
    pub captions: Vec<(String, String)>,
}

impl EnumerationValueDecl {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            captions: vec![("en_US".to_string(), name.clone())],
            name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumerationDecl {
    pub name: String,
    pub documentation: String,
    pub values: Vec<EnumerationValueDecl>,
}

impl EnumerationDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            values: vec![],
        }
    }
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
    pub enumerations: Vec<EnumerationDecl>,
    pub microflows: Vec<MicroflowDecl>,
    pub pages: Vec<PageDecl>,
}

#[derive(Debug, Clone)]
pub struct ProjectDecl {
    pub mendix_version: String,
    pub modules: Vec<ModuleDecl>,
}
