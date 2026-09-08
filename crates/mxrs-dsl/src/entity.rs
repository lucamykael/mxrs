use mxrs_ir::declaration::{AssociationDecl, EntityDecl};
use mxrs_model::association::{AssociationType, Owner, StorageFormat};
use mxrs_model::attribute::AttributeType;
use mxrs_model::Attribute;

pub struct EntityBuilder {
    decl: EntityDecl,
}

impl EntityBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        EntityBuilder { decl: EntityDecl::new(name) }
    }

    pub(crate) fn into_decl(self) -> EntityDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    pub fn persistable(&mut self, value: bool) -> &mut Self {
        self.decl.persistable = value;
        self
    }

    pub fn string(&mut self, name: impl Into<String>) -> &mut Attribute {
        self.push_attribute(name, AttributeType::String)
    }

    pub fn integer(&mut self, name: impl Into<String>) -> &mut Attribute {
        self.push_attribute(name, AttributeType::Integer)
    }

    pub fn long(&mut self, name: impl Into<String>) -> &mut Attribute {
        self.push_attribute(name, AttributeType::Long)
    }

    pub fn decimal(&mut self, name: impl Into<String>) -> &mut Attribute {
        self.push_attribute(name, AttributeType::Decimal)
    }

    pub fn boolean(&mut self, name: impl Into<String>) -> &mut Attribute {
        self.push_attribute(name, AttributeType::Boolean)
    }

    pub fn datetime(&mut self, name: impl Into<String>) -> &mut Attribute {
        self.push_attribute(name, AttributeType::DateTime)
    }

    pub fn autonumber(&mut self, name: impl Into<String>) -> &mut Attribute {
        self.push_attribute(name, AttributeType::AutoNumber)
    }

    /// Declares an association from this entity to `target` (`"Entity"` for
    /// a same-module target, `"Module.Entity"` for cross-module) — resolved
    /// to a concrete `mxrs_model::Association` by `mxrs-writer` once every
    /// entity in the project has an assigned id. Defaults to `Owner::Default`
    /// / `StorageFormat::Column`; set the returned `AssociationDecl`'s public
    /// fields directly to override.
    pub fn association(
        &mut self,
        name: impl Into<String>,
        target: impl Into<String>,
        association_type: AssociationType,
    ) -> &mut AssociationDecl {
        self.decl.associations.push(AssociationDecl {
            name: name.into(),
            target: target.into(),
            association_type,
            owner: Owner::Default,
            storage_format: StorageFormat::Column,
            documentation: String::new(),
        });
        self.decl.associations.last_mut().expect("just pushed")
    }

    fn push_attribute(&mut self, name: impl Into<String>, attribute_type: AttributeType) -> &mut Attribute {
        self.decl.attributes.push(Attribute {
            id: None,
            name: Some(name.into()),
            documentation: String::new(),
            attribute_type,
            default_value: None,
            data_storage_guid: None,
            export_level: "Hidden".into(),
            raw_type_doc: None,
            raw_value_doc: None,
            length: None,
            localize_date: None,
            enumeration: None,
            required: false,
            unique: false,
        });
        self.decl.attributes.last_mut().expect("just pushed")
    }
}
