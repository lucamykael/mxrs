//! Schema-checked Rust representation of any Forms element. Values remain
//! typed; BSON documents are strictly [`crate::mpr_codec`]'s concern.
//!
//! Ports `Mxrb::Forms::{Assignment,Node}` from `lib/mxrb/forms/node.rb`.
//! Deliberately **not** ported: the `method_missing`/`respond_to_missing?`
//! Ruby-ergonomics layer and the pluggable-widget branches in
//! `normalize_node`/`nested_value` (mxrs has no pluggable-widget support
//! yet — see `mpr_codec.rs`).

use std::rc::Rc;

use crate::catalog::{Catalog, Property, Size, Type};
use crate::error::{FormsError, Result};
use crate::values::{
    AttributeReference, Condition, DataType, EntityReference, EnumValue, Expression, Reference,
    Text, TextTemplate, XPathConstraint,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    String(String),
    Integer(i64),
    Boolean(bool),
    Size(Size),
    Node(Node),
    Enum(EnumValue),
    Reference(Reference),
    Text(Text),
    Expression(Expression),
    TextTemplate(TextTemplate),
    AttributeReference(AttributeReference),
    EntityReference(EntityReference),
    DataType(DataType),
    Condition(Condition),
    XPathConstraint(XPathConstraint),
    Binary(crate::values::BinaryAsset),
    List(Vec<Value>),
    /// A decoded `CustomWidgets$CustomWidget` (Data Grid 2/Gallery/
    /// ComboBox and any third-party pluggable widget) — not a [`Node`]
    /// because pluggable widgets aren't part of this crate's fixed
    /// `Catalog` schema; see `mxrs-pluggable` for why. Its inline schema is
    /// retained, so this variant supports both decode and encode.
    Pluggable(Box<mxrs_pluggable::WidgetNode<Node>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub property: Property,
    pub value: Value,
}

#[derive(Clone)]
pub struct Node {
    catalog: Rc<Catalog>,
    type_name: String,
    assignments: Vec<Assignment>,
    /// The 11.12.1 `Placeholder: Texts$Text` storage projection of a
    /// typed ClientTemplate. Kept only at this documented codec seam so
    /// no-op writes retain translation identities and BSON field order.
    pub(crate) legacy_placeholder: Option<mxrs_bson::Document>,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("type_name", &self.type_name)
            .field("assignments", &self.assignments)
            .finish()
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.type_name == other.type_name && self.assignments == other.assignments
    }
}

impl Node {
    pub fn new(type_name: impl Into<String>, catalog: Rc<Catalog>) -> Result<Self> {
        let type_name = type_name.into();
        let schema_type = catalog.fetch_type(&type_name)?;
        if schema_type.is_enum() {
            return Err(FormsError::TypeIsEnum {
                type_name: schema_type.name.clone(),
            });
        }
        let resolved_name = schema_type.name.clone();
        Ok(Self {
            catalog,
            type_name: resolved_name,
            assignments: Vec::new(),
            legacy_placeholder: None,
        })
    }

    pub fn schema_type(&self) -> &Type {
        self.catalog
            .fetch_type(&self.type_name)
            .expect("type_name was validated in Node::new")
    }

    pub fn catalog(&self) -> &Rc<Catalog> {
        &self.catalog
    }

    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }

    pub fn fetch(&self, property_id: &str) -> Result<Option<&Value>> {
        let property = self.schema_type().fetch_property(property_id, true)?;
        Ok(self
            .assignments
            .iter()
            .find(|a| a.property.name == property.name)
            .map(|a| &a.value))
    }

    pub fn assigned(&self, property_id: &str) -> Result<bool> {
        Ok(self.fetch(property_id)?.is_some())
    }

    pub fn set(&mut self, property_id: &str, value: Value) -> Result<()> {
        let property = self
            .schema_type()
            .fetch_property(property_id, true)?
            .clone();
        let normalized = self.normalize(&property, value)?;
        // Replacing a value must not move its BSON field to the end or
        // make a semantic no-op compare unequal after decoding again.
        if let Some(assignment) = self
            .assignments
            .iter_mut()
            .find(|a| a.property.name == property.name)
        {
            assignment.value = normalized;
            return Ok(());
        }
        self.assignments.push(Assignment {
            property,
            value: normalized,
        });
        Ok(())
    }

    pub fn append(&mut self, property_id: &str, value: Value) -> Result<()> {
        let property = self
            .schema_type()
            .fetch_property(property_id, true)?
            .clone();
        if !property.many() {
            return Err(FormsError::NotACollection {
                type_name: self.type_name.clone(),
                property: property.name,
            });
        }
        let mut current = match self.fetch(&property.name)? {
            Some(Value::List(items)) => items.clone(),
            _ => Vec::new(),
        };
        current.push(value);
        self.set(&property.name, Value::List(current))
    }

    pub fn unset(&mut self, property_id: &str) -> Result<()> {
        let property = self
            .schema_type()
            .fetch_property(property_id, true)?
            .clone();
        self.assignments
            .retain(|a| a.property.name != property.name);
        Ok(())
    }

    fn normalize(&self, property: &Property, value: Value) -> Result<Value> {
        if matches!(value, Value::Null) {
            return if property.optional {
                Ok(Value::Null)
            } else {
                Err(FormsError::UnexpectedNil {
                    type_name: self.type_name.clone(),
                    property: property.name.clone(),
                })
            };
        }
        if property.many() {
            let Value::List(items) = value else {
                return Err(FormsError::ExpectedArray {
                    type_name: self.type_name.clone(),
                    property: property.name.clone(),
                });
            };
            let normalized = items
                .into_iter()
                .map(|item| self.normalize_one(property, item))
                .collect::<Result<Vec<_>>>()?;
            return Ok(Value::List(normalized));
        }
        self.normalize_one(property, value)
    }

    fn normalize_one(&self, property: &Property, value: Value) -> Result<Value> {
        if property.is_reference() {
            return self.normalize_reference(property, value);
        }
        // Custom widgets are storage-level implementations of the
        // abstract Forms `Widget` type, but they intentionally have no
        // entry in the fixed native Forms catalog.
        if property.type_name == "Widget" && matches!(value, Value::Pluggable(_)) {
            return Ok(value);
        }
        if let Some(target) = self.catalog.type_(&property.type_name) {
            if target.is_enum() {
                return self.normalize_enum(target, value);
            }
            if target.is_element() {
                return self.normalize_node(property, target, value);
            }
        }
        self.normalize_scalar_or_external(property, value)
    }

    fn normalize_scalar_or_external(&self, property: &Property, value: Value) -> Result<Value> {
        let ok = matches!(
            (property.type_name.as_str(), &value),
            ("string", Value::String(_))
                | ("integer", Value::Integer(_))
                | ("size", Value::Size(_))
                | ("boolean", Value::Boolean(_))
                | ("blob", Value::Binary(_) | Value::String(_))
                | ("Text", Value::Text(_))
                | ("Expression", Value::Expression(_))
                | ("AttributeReference", Value::AttributeReference(_))
                | ("EntityReference", Value::EntityReference(_))
                | ("DataType", Value::DataType(_))
                | ("Condition", Value::Condition(_))
                | ("TextTemplate", Value::TextTemplate(_))
                | ("XPathConstraint", Value::XPathConstraint(_))
        );
        if ok {
            Ok(value)
        } else {
            Err(FormsError::TypeMismatch {
                type_name: self.type_name.clone(),
                property: property.name.clone(),
                expected: property.type_name.clone(),
            })
        }
    }

    fn normalize_enum(&self, enum_type: &Type, value: Value) -> Result<Value> {
        let candidate = match &value {
            Value::Enum(ev) => ev.value.clone(),
            Value::String(s) => s.clone(),
            _ => {
                return Err(FormsError::TypeMismatch {
                    type_name: self.type_name.clone(),
                    property: enum_type.name.clone(),
                    expected: "an enum value".to_string(),
                });
            }
        };
        let exact = enum_type.values.iter().find(|allowed| {
            **allowed == candidate
                || crate::catalog::ruby_name(allowed) == crate::catalog::ruby_name(&candidate)
        });
        match exact {
            Some(exact) => Ok(Value::Enum(EnumValue {
                enum_type_name: enum_type.name.clone(),
                value: exact.clone(),
            })),
            None => Err(FormsError::InvalidEnumValue {
                enum_name: enum_type.name.clone(),
                value: candidate,
            }),
        }
    }

    fn normalize_node(&self, property: &Property, target: &Type, value: Value) -> Result<Value> {
        let Value::Node(node) = &value else {
            return Err(FormsError::IncompatibleElement {
                type_name: self.type_name.clone(),
                property: property.name.clone(),
                target: target.name.clone(),
            });
        };
        let mut allowed = vec![target.name.clone()];
        allowed.extend(property.targets.iter().cloned());
        let compatible = allowed.iter().any(|candidate| {
            node.type_name == *candidate
                || self
                    .catalog
                    .descendant(&node.type_name, candidate)
                    .unwrap_or(false)
        });
        if compatible {
            Ok(value)
        } else {
            Err(FormsError::IncompatibleElement {
                type_name: self.type_name.clone(),
                property: property.name.clone(),
                target: target.name.clone(),
            })
        }
    }

    fn normalize_reference(&self, property: &Property, value: Value) -> Result<Value> {
        let kind = property
            .reference
            .expect("is_reference() checked by caller");
        match value {
            Value::Reference(r) if r.kind == kind => Ok(Value::Reference(r)),
            Value::String(target) => Ok(Value::Reference(Reference { target, kind })),
            _ => Err(FormsError::TypeMismatch {
                type_name: self.type_name.clone(),
                property: property.name.clone(),
                expected: "a reference".to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Rc<Catalog> {
        Rc::new(Catalog::for_version("11.12.1").unwrap())
    }

    #[test]
    fn new_rejects_enum_types() {
        let result = Node::new("Autofocus", catalog());
        assert!(matches!(result, Err(FormsError::TypeIsEnum { .. })));
    }

    #[test]
    fn widget_property_accepts_a_pluggable_widget() {
        let widget_type = mxrs_pluggable::WidgetType {
            id: "test.empty".to_string(),
            name: "Empty".to_string(),
            description: String::new(),
            prompt: String::new(),
            studio_pro_category: String::new(),
            studio_category: String::new(),
            platform: "Web".to_string(),
            offline: false,
            needs_context: false,
            plugin: false,
            help_url: String::new(),
            object_type: mxrs_pluggable::ObjectType::default(),
        };
        let custom = Value::Pluggable(Box::new(mxrs_pluggable::WidgetNode::new(
            widget_type,
            mxrs_pluggable::ObjectNode::new(),
        )));
        let mut column = Node::new("LayoutGridColumn", catalog()).unwrap();

        column
            .set("widgets", Value::List(vec![custom]))
            .expect("abstract Widget property accepts CustomWidgets storage node");
    }

    #[test]
    fn set_and_fetch_a_scalar_property() {
        let mut node = Node::new("Page", catalog()).unwrap();
        node.set("url", Value::String("orders".into())).unwrap();
        assert_eq!(
            node.fetch("url").unwrap(),
            Some(&Value::String("orders".into()))
        );
    }

    #[test]
    fn set_rejects_unknown_property() {
        let mut node = Node::new("Page", catalog()).unwrap();
        assert!(node.set("notAProperty", Value::String("x".into())).is_err());
    }

    #[test]
    fn set_rejects_wrong_scalar_type() {
        let mut node = Node::new("Page", catalog()).unwrap();
        assert!(
            node.set("popupWidth", Value::String("not-an-int".into()))
                .is_err()
        );
        assert!(node.set("popupWidth", Value::Integer(400)).is_ok());
    }

    #[test]
    fn set_replaces_existing_assignment_rather_than_duplicating() {
        let mut node = Node::new("Page", catalog()).unwrap();
        node.set("url", Value::String("a".into())).unwrap();
        node.set("url", Value::String("b".into())).unwrap();
        assert_eq!(
            node.assignments()
                .iter()
                .filter(|a| a.property.name == "url")
                .count(),
            1
        );
        assert_eq!(node.fetch("url").unwrap(), Some(&Value::String("b".into())));
    }

    #[test]
    fn append_requires_a_collection_property() {
        let mut node = Node::new("Page", catalog()).unwrap();
        assert!(node.append("url", Value::String("x".into())).is_err());
        assert!(
            node.append(
                "allowedRoles",
                Value::Reference(Reference {
                    target: "Administrator".into(),
                    kind: crate::catalog::ReferenceKind::ByName
                })
            )
            .is_ok()
        );
    }

    #[test]
    fn required_property_rejects_null() {
        let mut node = Node::new("Page", catalog()).unwrap();
        // popupCloseAction is a required (non-optional) string per the schema.
        assert!(node.set("popupCloseAction", Value::Null).is_err());
    }

    #[test]
    fn normalize_enum_accepts_exact_or_ruby_named_value() {
        let mut node = Node::new("Page", catalog()).unwrap();
        node.set("autofocus", Value::String("Off".into())).unwrap();
        assert!(matches!(
            node.fetch("autofocus").unwrap(),
            Some(Value::Enum(_))
        ));
        assert!(
            node.set("autofocus", Value::String("NotAValue".into()))
                .is_err()
        );
    }

    #[test]
    fn normalize_node_accepts_a_declared_target_type() {
        // Page.appearance is an element-typed property (type "Appearance"),
        // unlike Page.title (type "Text", an *external* value handled via
        // Value::Text, never a catalog-constructible Node).
        let mut page = Node::new("Page", catalog()).unwrap();
        let appearance = Node::new("Appearance", catalog()).unwrap();
        assert!(page.set("appearance", Value::Node(appearance)).is_ok());

        let wrong = Node::new("LayoutCall", catalog()).unwrap();
        assert!(page.set("appearance", Value::Node(wrong)).is_err());
    }
}
