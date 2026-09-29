//! Applying an export mapping: turning a flow result into the JSON document
//! the model declares for it.
//!
//! An `ExportMappings$ExportMapping` is a tree. Its root object element names
//! the entity the mapping describes and whether the document is one object or
//! an array of them; every level exposes attributes under the JSON keys the
//! model chose, and reaches nested objects by traversing an association.
//! Applying it is therefore a walk of the store starting from the objects the
//! flow returned — no XSD, no JSON-structure file, no custom Java handler
//! takes part.
//!
//! Two deliberate choices:
//!
//! - **Objects are not filtered by entity.** Mendix generalization means a
//!   flow may legitimately answer with a specialization of the mapping's
//!   entity, under a different qualified name. The mapping reads the members
//!   it names and nothing else, so a specialization maps exactly like its
//!   generalization.
//! - **Only an absent or `null` member is an empty value.** An empty string
//!   is a value a Mendix string attribute genuinely holds, so
//!   [`NullValues::LeaveOut`] keeps it rather than dropping the key.

use serde_json::{Map, Value};

use mxrs_runtime::{ObjectValue, Store};

use crate::value::{FlowValue, ObjectRef, member_to_json};

/// What the mapping does with a member the object left empty —
/// `ExportMappings$ExportMapping`'s `NullValueOption`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NullValues {
    /// `LeaveOutElement`: the key is absent from the document.
    #[default]
    LeaveOut,
    /// `SendAsNil`: the key is present and `null`.
    SendAsNil,
}

/// One export mapping: its root element and how it treats empty values.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportMapping {
    root: ObjectMapping,
    nulls: NullValues,
}

impl ExportMapping {
    /// The mapping rooted at `root`, leaving empty members out of the
    /// document — Mendix's default.
    pub fn new(root: ObjectMapping) -> Self {
        Self {
            root,
            nulls: NullValues::LeaveOut,
        }
    }

    /// `NullValueOption: SendAsNil` — an empty member becomes a `null` key
    /// instead of an absent one.
    pub fn sending_nils(mut self) -> Self {
        self.nulls = NullValues::SendAsNil;
        self
    }

    pub fn root(&self) -> &ObjectMapping {
        &self.root
    }

    pub fn null_values(&self) -> NullValues {
        self.nulls
    }

    /// Shapes a flow result into the mapped document, reading members and
    /// associated objects from `store`.
    ///
    /// A root array always answers an array, even for a single object; a root
    /// object answers the first object the flow returned, or `null` when it
    /// returned none.
    pub fn apply(&self, store: &Store, value: &FlowValue) -> Value {
        let sources = sources(store, value);
        if self.root.multiple {
            Value::Array(
                sources
                    .iter()
                    .map(|source| self.document(store, &self.root, source))
                    .collect(),
            )
        } else {
            sources.first().map_or(Value::Null, |source| {
                self.document(store, &self.root, source)
            })
        }
    }

    fn document(&self, store: &Store, mapping: &ObjectMapping, object: &ObjectValue) -> Value {
        let mut fields = Map::new();
        for value in &mapping.values {
            match object.members.get(&value.attribute) {
                Some(Value::Null) | None => {
                    if self.nulls == NullValues::SendAsNil {
                        fields.insert(value.key.clone(), Value::Null);
                    }
                }
                Some(member) => {
                    fields.insert(value.key.clone(), member_to_json(member));
                }
            }
        }
        for child in &mapping.children {
            let related = store.retrieve_association(&child.association, object);
            let nested = if child.multiple {
                Value::Array(
                    related
                        .iter()
                        .map(|source| self.document(store, child, source))
                        .collect(),
                )
            } else {
                match related.first() {
                    Some(source) => self.document(store, child, source),
                    None if self.nulls == NullValues::SendAsNil => Value::Null,
                    None => continue,
                }
            };
            fields.insert(child.key.clone(), nested);
        }
        Value::Object(fields)
    }
}

/// One `ExportMappings$ObjectMappingElement`: the entity at this level of the
/// document, the attributes it exposes, and the associated objects nested
/// under it.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectMapping {
    entity: String,
    /// The association traversed from the parent object; empty at the root,
    /// which the flow result supplies directly.
    association: String,
    /// The JSON key this level is nested under; empty at the root, which
    /// *is* the document.
    key: String,
    multiple: bool,
    values: Vec<ValueMapping>,
    children: Vec<ObjectMapping>,
}

impl ObjectMapping {
    /// One object of `entity` — JSON path `(Object)`.
    pub fn object(entity: impl Into<String>) -> Self {
        Self::at(entity, false)
    }

    /// An array of `entity` objects — JSON path `(Array)|(Object)`.
    pub fn array(entity: impl Into<String>) -> Self {
        Self::at(entity, true)
    }

    fn at(entity: impl Into<String>, multiple: bool) -> Self {
        Self {
            entity: entity.into(),
            association: String::new(),
            key: String::new(),
            multiple,
            values: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Exposes `attribute` under its own name.
    pub fn attribute(self, attribute: impl Into<String>) -> Self {
        let attribute = attribute.into();
        self.value(attribute.clone(), attribute)
    }

    /// Exposes `attribute` under the JSON key `key`, which the model renamed.
    pub fn value(mut self, key: impl Into<String>, attribute: impl Into<String>) -> Self {
        self.values.push(ValueMapping {
            key: key.into(),
            attribute: attribute.into(),
        });
        self
    }

    /// Nests `mapping` under the JSON key `key`, reaching its objects through
    /// `association`.
    pub fn child(
        mut self,
        key: impl Into<String>,
        association: impl Into<String>,
        mapping: ObjectMapping,
    ) -> Self {
        self.children.push(ObjectMapping {
            association: association.into(),
            key: key.into(),
            ..mapping
        });
        self
    }

    pub fn entity(&self) -> &str {
        &self.entity
    }

    pub fn association(&self) -> &str {
        &self.association
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn is_multiple(&self) -> bool {
        self.multiple
    }

    pub fn values(&self) -> &[ValueMapping] {
        &self.values
    }

    pub fn children(&self) -> &[ObjectMapping] {
        &self.children
    }
}

/// One `ExportMappings$ValueMappingElement`: an attribute and the JSON key it
/// answers under.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueMapping {
    key: String,
    attribute: String,
}

impl ValueMapping {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn attribute(&self) -> &str {
        &self.attribute
    }
}

/// The store objects a flow result points at, in the order the result holds
/// them. Anything that is not an object reference contributes nothing: an
/// export mapping maps objects.
fn sources(store: &Store, value: &FlowValue) -> Vec<ObjectValue> {
    match value {
        FlowValue::Object(reference) => object_value(store, reference).into_iter().collect(),
        FlowValue::List(values) => values
            .iter()
            .flat_map(|value| sources(store, value))
            .collect(),
        _ => Vec::new(),
    }
}

fn object_value(store: &Store, reference: &ObjectRef) -> Option<ObjectValue> {
    store.find(&reference.entity, &reference.id).ok().flatten()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use mxrs_runtime::StoreSchema;

    fn store() -> Store {
        let schema = StoreSchema::default()
            .entity(
                "Api.Item",
                BTreeMap::from([
                    ("ItemId".to_string(), json!(0)),
                    ("Name".to_string(), json!("")),
                    ("Note".to_string(), Value::Null),
                ]),
                false,
            )
            .entity(
                "Api.Detail",
                BTreeMap::from([("Component".to_string(), json!(""))]),
                false,
            );
        Store::new(schema)
    }

    fn item_list() -> ObjectMapping {
        ObjectMapping::array("Api.Item")
            .attribute("ItemId")
            .value("item_name", "Name")
    }

    #[test]
    fn a_root_array_answers_every_object_the_flow_returned() {
        let mut store = store();
        let first = store.create("Api.Item").unwrap();
        store
            .set_member("Api.Item", &first.id, "Name", json!("first"))
            .unwrap();
        let second = store.create("Api.Item").unwrap();
        store
            .set_member("Api.Item", &second.id, "Name", json!("second"))
            .unwrap();
        let value = FlowValue::List(vec![
            FlowValue::Object(ObjectRef {
                entity: "Api.Item".into(),
                id: first.id.clone(),
            }),
            FlowValue::Object(ObjectRef {
                entity: "Api.Item".into(),
                id: second.id.clone(),
            }),
        ]);

        assert_eq!(
            ExportMapping::new(item_list()).apply(&store, &value),
            json!([
                { "ItemId": 0, "item_name": "first" },
                { "ItemId": 0, "item_name": "second" },
            ])
        );
    }

    #[test]
    fn a_root_array_still_answers_an_array_for_a_single_object() {
        let mut store = store();
        let item = store.create("Api.Item").unwrap();
        let value = FlowValue::Object(ObjectRef {
            entity: "Api.Item".into(),
            id: item.id.clone(),
        });

        assert_eq!(
            ExportMapping::new(item_list()).apply(&store, &value),
            json!([{ "ItemId": 0, "item_name": "" }])
        );
    }

    #[test]
    fn a_root_object_answers_null_when_the_flow_returned_nothing() {
        let store = store();
        let mapping = ExportMapping::new(ObjectMapping::object("Api.Item").attribute("ItemId"));

        assert_eq!(mapping.apply(&store, &FlowValue::Empty), Value::Null);
        assert_eq!(
            mapping.apply(&store, &FlowValue::List(Vec::new())),
            Value::Null
        );
    }

    #[test]
    fn leave_out_drops_an_empty_member_and_send_as_nil_keeps_it() {
        let mut store = store();
        let item = store.create("Api.Item").unwrap();
        let value = FlowValue::Object(ObjectRef {
            entity: "Api.Item".into(),
            id: item.id.clone(),
        });
        let root = ObjectMapping::object("Api.Item")
            .attribute("Name")
            .attribute("Note");

        assert_eq!(
            ExportMapping::new(root.clone()).apply(&store, &value),
            json!({ "Name": "" }),
        );
        assert_eq!(
            ExportMapping::new(root)
                .sending_nils()
                .apply(&store, &value),
            json!({ "Name": "", "Note": null }),
        );
    }

    #[test]
    fn a_child_element_traverses_its_association() {
        let mut store = store();
        let item = store.create("Api.Item").unwrap();
        let detail = store.create("Api.Detail").unwrap();
        store
            .set_member("Api.Detail", &detail.id, "Component", json!("gasket"))
            .unwrap();
        store
            .set_member(
                "Api.Item",
                &item.id,
                "Api.Detail_Item",
                json!(detail.id.clone()),
            )
            .unwrap();
        let value = FlowValue::Object(ObjectRef {
            entity: "Api.Item".into(),
            id: item.id.clone(),
        });
        let mapping =
            ExportMapping::new(ObjectMapping::object("Api.Item").attribute("Name").child(
                "detail",
                "Api.Detail_Item",
                ObjectMapping::object("Api.Detail").value("component", "Component"),
            ));

        assert_eq!(
            mapping.apply(&store, &value),
            json!({ "Name": "", "detail": { "component": "gasket" } })
        );
    }

    #[test]
    fn an_unreachable_child_object_follows_the_null_option() {
        let mut store = store();
        let item = store.create("Api.Item").unwrap();
        let value = FlowValue::Object(ObjectRef {
            entity: "Api.Item".into(),
            id: item.id.clone(),
        });
        let root = ObjectMapping::object("Api.Item").child(
            "detail",
            "Api.Detail_Item",
            ObjectMapping::object("Api.Detail").attribute("Component"),
        );

        assert_eq!(
            ExportMapping::new(root.clone()).apply(&store, &value),
            json!({})
        );
        assert_eq!(
            ExportMapping::new(root)
                .sending_nils()
                .apply(&store, &value),
            json!({ "detail": null })
        );
    }

    #[test]
    fn a_child_array_answers_an_empty_array_rather_than_being_left_out() {
        let mut store = store();
        let item = store.create("Api.Item").unwrap();
        let value = FlowValue::Object(ObjectRef {
            entity: "Api.Item".into(),
            id: item.id.clone(),
        });
        let mapping = ExportMapping::new(ObjectMapping::object("Api.Item").child(
            "details",
            "Api.Detail_Item",
            ObjectMapping::array("Api.Detail").attribute("Component"),
        ));

        assert_eq!(mapping.apply(&store, &value), json!({ "details": [] }));
    }

    #[test]
    fn a_specialization_maps_like_its_generalization() {
        let mut store = store();
        let item = store.create("Api.Item").unwrap();
        store
            .set_member("Api.Item", &item.id, "Name", json!("special"))
            .unwrap();
        // The mapping names `Api.Item`; the flow answers with the object's
        // own entity, which for a specialization is a different name.
        let value = FlowValue::Object(ObjectRef {
            entity: "Api.Item".into(),
            id: item.id.clone(),
        });
        let mapping = ExportMapping::new(ObjectMapping::object("Api.Other").attribute("Name"));

        assert_eq!(mapping.apply(&store, &value), json!({ "Name": "special" }));
    }

    #[test]
    fn a_datetime_member_answers_in_its_rendered_form() {
        let mut store = store();
        let item = store.create("Api.Item").unwrap();
        store
            .set_member(
                "Api.Item",
                &item.id,
                "Note",
                json!(format!("{}0", mxrs_runtime::DATETIME_MEMBER_PREFIX)),
            )
            .unwrap();
        let value = FlowValue::Object(ObjectRef {
            entity: "Api.Item".into(),
            id: item.id.clone(),
        });
        let mapping = ExportMapping::new(ObjectMapping::object("Api.Item").attribute("Note"));

        assert_eq!(
            mapping.apply(&store, &value),
            json!({ "Note": "1970-01-01 00:00:00 UTC" })
        );
    }
}
