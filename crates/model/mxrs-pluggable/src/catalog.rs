//! Schema value types for pluggable (`CustomWidgets`) widget packages, and
//! the [`Catalog`] registry that accumulates them across every widget
//! instance decoded from a project. Ports `Mxrb::Pluggable`'s `Data.define`
//! structs and the `Catalog` class from `lib/mxrb/pluggable/catalog.rb`.
//!
//! Deliberately **not** ported here: `Catalog#resolve`/`#compatible_object?`/
//! `#schema_excess` — these pick the best-matching schema *revision* for a
//! concrete widget *instance* (an assigned property/value tree), which
//! requires the instance model (`Mxrb::Pluggable::Node`/`ObjectNode`) this
//! crate doesn't build yet (see `mpr_codec` module doc). `fetch` (the
//! fallback `resolve` itself uses whenever revisions are absent or none
//! compatible) is ported and is the only lookup available until that
//! follow-up lands.

use std::collections::HashMap;

/// A single-locale caption/text pair. Ports `Pluggable::Translation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    pub language: String,
    pub text: String,
}

/// Ports `Pluggable::EnumerationValue`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumerationValue {
    pub key: String,
    pub caption: String,
}

/// Ports `Pluggable::ActionVariable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionVariable {
    pub key: String,
    pub kind: String,
    pub caption: String,
}

/// Ports `Pluggable::ReturnType`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnType {
    pub kind: String,
    pub list: bool,
    pub entity_property: String,
    pub assignable_to: String,
}

/// Ports `Pluggable::ValueType`. `object_type` is boxed to break the
/// recursive `ValueType -> ObjectType -> PropertyType -> ValueType` cycle
/// (mirrors a nested `Object`/`Widgets`-kind property).
#[derive(Debug, Clone, PartialEq)]
pub struct ValueType {
    pub kind: String,
    pub list: bool,
    pub linked: bool,
    pub metadata: bool,
    pub entity_property: String,
    pub allow_non_persistable_entities: bool,
    pub path_kind: String,
    pub path_type: String,
    pub parameter_list: bool,
    pub multiline: bool,
    pub default_value: String,
    pub required: bool,
    pub on_change_property: String,
    pub data_source_property: String,
    pub selectable_objects_property: String,
    pub attribute_types: Vec<String>,
    pub association_types: Vec<String>,
    pub selection_types: Vec<String>,
    pub enumeration_values: Vec<EnumerationValue>,
    pub action_variables: Vec<ActionVariable>,
    pub object_type: Option<Box<ObjectType>>,
    pub return_type: Option<ReturnType>,
    pub translations: Vec<Translation>,
    pub set_label: bool,
    pub default_type: String,
    pub allow_upload: bool,
}

impl ValueType {
    pub fn is_object(&self) -> bool {
        self.kind == "Object"
    }

    pub fn is_widgets(&self) -> bool {
        self.kind == "Widgets"
    }
}

/// Ports `Pluggable::PropertyType`. `ruby_name` (mxrb's `method_missing`
/// ergonomics helper) is deliberately not carried — `mxrs-dsl`'s job, same
/// boundary `mxrs-forms` already draws for its own `Node`.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyType {
    pub key: String,
    pub category: String,
    pub caption: String,
    pub description: String,
    pub prompt: String,
    pub default: bool,
    pub value_type: ValueType,
}

impl PropertyType {
    pub fn is_system(&self) -> bool {
        self.value_type.kind == "System"
    }
}

/// Ports `Pluggable::ObjectType`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjectType {
    pub properties: Vec<PropertyType>,
}

impl ObjectType {
    pub fn property(&self, key: &str) -> Option<&PropertyType> {
        self.properties.iter().find(|property| property.key == key)
    }

    pub fn fetch_property(&self, key: &str) -> crate::error::Result<&PropertyType> {
        self.property(key)
            .ok_or_else(|| crate::error::PluggableError::UnknownProperty(key.to_string()))
    }
}

/// Ports `Pluggable::WidgetType`.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetType {
    pub id: String,
    pub name: String,
    pub description: String,
    pub prompt: String,
    pub studio_pro_category: String,
    pub studio_category: String,
    pub platform: String,
    pub offline: bool,
    pub needs_context: bool,
    pub plugin: bool,
    pub help_url: String,
    pub object_type: ObjectType,
}

/// A semantic registry mapping a stable widget id (e.g.
/// `"com.mendix.widget.web.image.Image"`) to the widget-type schema(s) seen
/// for it across every instance decoded so far. Ports `Pluggable::Catalog`.
#[derive(Debug, Default)]
pub struct Catalog {
    types: HashMap<String, WidgetType>,
    revisions: HashMap<String, Vec<WidgetType>>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a decoded widget-type schema, merging it with any prior
    /// schema seen for the same widget id (a widget package can carry
    /// slightly different property sets across app versions/revisions —
    /// merging keeps every property any revision has declared). Returns the
    /// merged schema now stored for this id.
    pub fn register(&mut self, widget_type: WidgetType) -> WidgetType {
        let revisions = self.revisions.entry(widget_type.id.clone()).or_default();
        if !revisions.contains(&widget_type) {
            revisions.push(widget_type.clone());
        }
        let merged = match self.types.get(&widget_type.id) {
            Some(existing) => merge_widget_types(existing, &widget_type),
            None => widget_type,
        };
        self.types.insert(merged.id.clone(), merged.clone());
        merged
    }

    pub fn type_(&self, widget_id: &str) -> Option<&WidgetType> {
        self.types.get(widget_id)
    }

    pub fn fetch(&self, widget_id: &str) -> crate::error::Result<&WidgetType> {
        self.type_(widget_id)
            .ok_or_else(|| crate::error::PluggableError::UnknownWidget(widget_id.to_string()))
    }

    /// All merged widget-type schemas, sorted by id.
    pub fn types(&self) -> Vec<&WidgetType> {
        let mut types: Vec<&WidgetType> = self.types.values().collect();
        types.sort_by(|a, b| a.id.cmp(&b.id));
        types
    }

    /// Every schema revision seen for every widget id, in registration
    /// order (not merged) — the raw material `resolve` would pick among.
    pub fn schema_definitions(&self) -> Vec<&WidgetType> {
        self.revisions.values().flatten().collect()
    }

    pub fn property_count(&self) -> usize {
        self.types()
            .iter()
            .map(|t| count_properties(&t.object_type))
            .sum()
    }
}

fn merge_widget_types(existing: &WidgetType, incoming: &WidgetType) -> WidgetType {
    let mut merged = incoming.clone();
    merged.object_type = merge_object_types(&existing.object_type, &incoming.object_type);
    merged
}

fn merge_object_types(existing: &ObjectType, incoming: &ObjectType) -> ObjectType {
    let mut by_key: Vec<(String, PropertyType)> = existing
        .properties
        .iter()
        .map(|property| (property.key.clone(), property.clone()))
        .collect();
    for property in &incoming.properties {
        match by_key.iter_mut().find(|(key, _)| key == &property.key) {
            Some((_, previous)) => *previous = merge_property_types(previous, property),
            None => by_key.push((property.key.clone(), property.clone())),
        }
    }
    ObjectType {
        properties: by_key.into_iter().map(|(_, property)| property).collect(),
    }
}

fn merge_property_types(existing: &PropertyType, incoming: &PropertyType) -> PropertyType {
    let old_value = &existing.value_type;
    let new_value = &incoming.value_type;
    if old_value.kind != new_value.kind {
        return incoming.clone();
    }
    let (Some(old_object), Some(new_object)) = (&old_value.object_type, &new_value.object_type)
    else {
        return incoming.clone();
    };
    let mut merged = incoming.clone();
    merged.value_type.object_type = Some(Box::new(merge_object_types(old_object, new_object)));
    merged
}

fn count_properties(object_type: &ObjectType) -> usize {
    object_type
        .properties
        .iter()
        .map(|property| {
            1 + property
                .value_type
                .object_type
                .as_deref()
                .map(count_properties)
                .unwrap_or(0)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_type(kind: &str) -> ValueType {
        ValueType {
            kind: kind.to_string(),
            list: false,
            linked: false,
            metadata: false,
            entity_property: String::new(),
            allow_non_persistable_entities: false,
            path_kind: "No".to_string(),
            path_type: "None".to_string(),
            parameter_list: false,
            multiline: false,
            default_value: String::new(),
            required: false,
            on_change_property: String::new(),
            data_source_property: String::new(),
            selectable_objects_property: String::new(),
            attribute_types: vec![],
            association_types: vec![],
            selection_types: vec![],
            enumeration_values: vec![],
            action_variables: vec![],
            object_type: None,
            return_type: None,
            translations: vec![],
            set_label: false,
            default_type: "None".to_string(),
            allow_upload: false,
        }
    }

    fn property(key: &str, kind: &str) -> PropertyType {
        PropertyType {
            key: key.to_string(),
            category: String::new(),
            caption: String::new(),
            description: String::new(),
            prompt: String::new(),
            default: false,
            value_type: value_type(kind),
        }
    }

    fn widget_type(id: &str, properties: Vec<PropertyType>) -> WidgetType {
        WidgetType {
            id: id.to_string(),
            name: String::new(),
            description: String::new(),
            prompt: String::new(),
            studio_pro_category: String::new(),
            studio_category: String::new(),
            platform: "Web".to_string(),
            offline: false,
            needs_context: false,
            plugin: true,
            help_url: String::new(),
            object_type: ObjectType { properties },
        }
    }

    #[test]
    fn register_then_fetch_round_trips() {
        let mut catalog = Catalog::new();
        let widget = widget_type("com.example.Foo", vec![property("caption", "String")]);
        catalog.register(widget.clone());
        assert_eq!(catalog.fetch("com.example.Foo").unwrap(), &widget);
    }

    #[test]
    fn register_merges_new_properties_across_revisions() {
        let mut catalog = Catalog::new();
        catalog.register(widget_type(
            "com.example.Foo",
            vec![property("a", "String")],
        ));
        let merged = catalog.register(widget_type(
            "com.example.Foo",
            vec![property("b", "Boolean")],
        ));
        assert_eq!(merged.object_type.properties.len(), 2);
        assert!(merged.object_type.property("a").is_some());
        assert!(merged.object_type.property("b").is_some());
    }

    #[test]
    fn register_keeps_every_distinct_revision() {
        let mut catalog = Catalog::new();
        catalog.register(widget_type(
            "com.example.Foo",
            vec![property("a", "String")],
        ));
        catalog.register(widget_type(
            "com.example.Foo",
            vec![property("b", "Boolean")],
        ));
        assert_eq!(catalog.schema_definitions().len(), 2);
    }

    #[test]
    fn fetch_unknown_widget_errors() {
        let catalog = Catalog::new();
        assert!(catalog.fetch("nope").is_err());
    }

    #[test]
    fn property_count_includes_nested_object_type() {
        let mut catalog = Catalog::new();
        let mut nested = property("child", "Object");
        nested.value_type.object_type = Some(Box::new(ObjectType {
            properties: vec![property("grandchild", "String")],
        }));
        catalog.register(widget_type("com.example.Nested", vec![nested]));
        assert_eq!(catalog.property_count(), 2);
    }
}
