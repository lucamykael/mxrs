//! An import or export mapping, as a module declares it: the entity each
//! object of its JSON structure is and the attribute each value fills, in
//! the shape of the structure itself.
//!
//! ```
//! # use mxrs_dsl::ModuleBuilder;
//! # let mut module = ModuleBuilder::new("Sales");
//! module.import_mapping("IMM_Order", "Sales.JSON_Order", |mapping| {
//!     mapping.object("(Object)", "Sales.Order", |order| {
//!         order.value("number").key();
//!         order.array("lines", |lines| {
//!             lines.object("(Object)", "Sales.Line", |line| {
//!                 line.value("qty").attribute("Quantity");
//!             });
//!         });
//!     });
//! });
//! ```
//!
//! What a mapping leaves unsaid is what Studio Pro makes: a value fills the
//! attribute named as its element is exposed, an object hangs off its
//! parent's through `<Entity>_<Parent>`, an import creates its objects and
//! an export finds them, given its root.

use mxrs_ir::{
    ExportLevel, MappingAssociation, MappingDecl, MappingDirection, MappingElement,
    MappingElementKind, MappingValueType, NullValueOption, ObjectHandling, ObjectMapping,
    ValueMapping,
};

pub struct MappingBuilder {
    decl: MappingDecl,
}

impl MappingBuilder {
    pub(crate) fn new(
        direction: MappingDirection,
        name: impl Into<String>,
        json_structure: impl Into<String>,
    ) -> Self {
        Self {
            decl: MappingDecl::new(direction, name, json_structure),
        }
    }

    /// The root of the structure — `(Object)` or `(Array)` — as an object
    /// of `entity`.
    pub fn object(
        &mut self,
        key: impl Into<String>,
        entity: impl Into<String>,
        configure: impl FnOnce(&mut ObjectMappingBuilder<'_>),
    ) -> &mut Self {
        self.decl
            .elements
            .push(object_element(key.into(), Some(entity.into()), configure));
        self
    }

    /// The root array of the structure, of which the mapping makes no
    /// object.
    pub fn array(
        &mut self,
        key: impl Into<String>,
        configure: impl FnOnce(&mut ObjectMappingBuilder<'_>),
    ) -> &mut Self {
        self.decl
            .elements
            .push(object_element(key.into(), None, configure));
        self
    }

    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.documentation = value.into();
        self
    }

    /// Whether each value keeps its structure's sample value; mappings
    /// Studio Pro made before it kept them have none.
    pub fn sample_values(&mut self, value: bool) -> &mut Self {
        self.decl.sample_values = value;
        self
    }

    /// What an export writes for an attribute without a value.
    pub fn null_value_option(&mut self, value: NullValueOption) -> &mut Self {
        self.decl.null_value_option = value;
        self
    }

    pub fn excluded(&mut self, value: bool) -> &mut Self {
        self.decl.excluded = value;
        self
    }

    pub fn export_level(&mut self, value: ExportLevel) -> &mut Self {
        self.decl.export_level = value;
        self
    }

    pub(crate) fn into_decl(self) -> MappingDecl {
        self.decl
    }
}

fn object_element(
    key: String,
    entity: Option<String>,
    configure: impl FnOnce(&mut ObjectMappingBuilder<'_>),
) -> MappingElement {
    let mut object = ObjectMapping::new(entity);
    configure(&mut ObjectMappingBuilder {
        object: &mut object,
    });
    MappingElement {
        key,
        kind: MappingElementKind::Object(object),
    }
}

/// An object, array or wrapper of the structure.
pub struct ObjectMappingBuilder<'a> {
    object: &'a mut ObjectMapping,
}

impl ObjectMappingBuilder<'_> {
    /// A member or item of this one — `lines`, `(Object)`, `(Wrapper)` — as
    /// an object of `entity`.
    pub fn object(
        &mut self,
        key: impl Into<String>,
        entity: impl Into<String>,
        configure: impl FnOnce(&mut ObjectMappingBuilder<'_>),
    ) -> &mut Self {
        self.object
            .children
            .push(object_element(key.into(), Some(entity.into()), configure));
        self
    }

    /// An array of which the mapping makes no object: what it holds hangs
    /// off this one's entity.
    pub fn array(
        &mut self,
        key: impl Into<String>,
        configure: impl FnOnce(&mut ObjectMappingBuilder<'_>),
    ) -> &mut Self {
        self.object
            .children
            .push(object_element(key.into(), None, configure));
        self
    }

    /// A value of this object, filling the attribute named as its element
    /// is exposed unless [`ValueMappingBuilder::attribute`] says another.
    pub fn value(&mut self, key: impl Into<String>) -> ValueMappingBuilder<'_> {
        self.object.children.push(MappingElement {
            key: key.into(),
            kind: MappingElementKind::Value(ValueMapping::default()),
        });
        let Some(MappingElement {
            kind: MappingElementKind::Value(value),
            ..
        }) = self.object.children.last_mut()
        else {
            unreachable!("a value was just added");
        };
        ValueMappingBuilder { value }
    }

    /// The association this object hangs off its parent's through, when it
    /// is not `<Entity>_<Parent>`.
    pub fn via(&mut self, association: impl Into<String>) -> &mut Self {
        self.object.association = MappingAssociation::Other(association.into());
        self
    }

    /// No association: the object hangs off nothing.
    pub fn unassociated(&mut self) -> &mut Self {
        self.object.association = MappingAssociation::None;
        self
    }

    /// How the mapping comes by the object.
    pub fn handling(&mut self, value: ObjectHandling) -> &mut Self {
        self.object.handling = Some(value);
        self
    }

    /// What the mapping does when [`Self::handling`] finds none.
    pub fn backup(&mut self, value: ObjectHandling) -> &mut Self {
        self.object.backup = Some(value);
        self
    }

    pub fn allow_override(&mut self, value: bool) -> &mut Self {
        self.object.allow_override = value;
        self
    }
}

/// A value of the structure.
pub struct ValueMappingBuilder<'a> {
    value: &'a mut ValueMapping,
}

impl ValueMappingBuilder<'_> {
    /// The attribute the value fills, by its name in the object's entity.
    pub fn attribute(&mut self, name: impl Into<String>) -> &mut Self {
        self.value.attribute = Some(name.into());
        self
    }

    /// The value finds its object.
    pub fn key(&mut self) -> &mut Self {
        self.value.is_key = true;
        self
    }

    /// The microflow that converts the value, by its qualified name.
    pub fn converter(&mut self, microflow: impl Into<String>) -> &mut Self {
        self.value.converter = microflow.into();
        self
    }

    /// The type of the attribute, when it is not the type the value holds.
    pub fn value_type(&mut self, value: MappingValueType) -> &mut Self {
        self.value.value_type = Some(value);
        self
    }
}
