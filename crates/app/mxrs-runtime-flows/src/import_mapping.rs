//! Applying an import mapping: turning a JSON document into the objects
//! the model declares for it.
//!
//! An `ImportMappings$ImportMapping` is a tree of object elements, each an
//! entity the document's objects at its JSON path become, with the
//! attributes its value elements read and the objects nested below it
//! reached by association. A path is a document's own: `(Object)|payload`
//! is the key `payload` of the object at the root, `(Object)|value|(Object)`
//! each object of the array `value` holds, `(Wrapper)` each plain value of
//! an array — an object holding it as its `(Value)` — and `(Array)` each
//! array of an array.
//!
//! An object element either creates its objects or finds them by the
//! attributes its value elements mark as keys; one it does not find is
//! created, left out, or refused, as the element's backup says.

use mxrs_bson::{Bson, Document};
use serde_json::Value;

use crate::value::FlowValue;

/// What an object element does to have an object for a part of the
/// document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectHandling {
    /// Creates a new object.
    Create,
    /// Finds the object whose key attributes hold the part's values.
    Find,
}

/// What an object element that finds no object does instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingObject {
    Create,
    /// Leaves the part, and what is below it, out.
    Ignore,
    /// Refuses the whole document.
    Error,
}

/// What a value element's attribute holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportedType {
    String,
    Integer,
    Long,
    Decimal,
    Boolean,
    DateTime,
    /// A value of the enumeration, by its name.
    Enumeration,
}

/// One `ImportMappings$ValueMappingElement`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedValue {
    /// The attribute's own name.
    pub attribute: String,
    /// Where the value is, after its object's path.
    path: Vec<String>,
    pub ty: ImportedType,
    pub key: bool,
}

/// One `ImportMappings$ObjectMappingElement`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedObject {
    /// The entity, by its qualified name.
    pub entity: String,
    /// The association to the object above, by its qualified name; empty
    /// at the root.
    pub association: String,
    /// Where the objects are, after the path of the object above.
    path: Vec<String>,
    /// Whether the path holds many: an array's objects.
    pub multiple: bool,
    pub handling: ObjectHandling,
    pub missing: MissingObject,
    pub values: Vec<ImportedValue>,
    pub children: Vec<ImportedObject>,
}

/// One import mapping: its root element.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportMapping {
    pub root: ImportedObject,
}

impl ImportMapping {
    /// The mapping an `ImportMappings$ImportMapping` document states, when
    /// it is one a JSON document can be read with: one root element of an
    /// entity, no custom handler, no value converter, not excluded.
    pub fn from_document(document: &Document) -> Option<Self> {
        if document.get_str("$Type").ok() != Some("ImportMappings$ImportMapping")
            || document.get_bool("Excluded").unwrap_or(false)
        {
            return None;
        }
        let elements = documents(document.get("Elements"));
        let [root] = elements.as_slice() else {
            return None;
        };
        Some(Self {
            root: object_element(root, &[])?,
        })
    }
}

fn documents(value: Option<&Bson>) -> Vec<&Document> {
    match value {
        Some(Bson::Array(items)) => items.iter().filter_map(Bson::as_document).collect(),
        _ => Vec::new(),
    }
}

fn segments(path: &str) -> Vec<String> {
    path.split('|')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// `path` after the `above` it starts with.
fn after(path: &[String], above: &[String]) -> Option<Vec<String>> {
    path.starts_with(above)
        .then(|| path[above.len()..].to_vec())
}

fn object_element(element: &Document, above: &[String]) -> Option<ImportedObject> {
    if element.get_str("$Type").ok() != Some("ImportMappings$ObjectMappingElement") {
        return None;
    }
    // A handler decides what the object is; nothing here can.
    if !matches!(element.get("CustomHandlerCall"), None | Some(Bson::Null)) {
        return None;
    }
    let entity = element
        .get_str("Entity")
        .ok()
        .filter(|entity| !entity.is_empty())?;
    let own = segments(element.get_str("JsonPath").ok()?);
    let path = after(&own, above)?;
    let occurs = match element.get("MaxOccurs")? {
        Bson::Int32(occurs) => i64::from(*occurs),
        Bson::Int64(occurs) => *occurs,
        _ => return None,
    };
    let handling = match element.get_str("ObjectHandling").ok()? {
        "Create" => ObjectHandling::Create,
        "Find" => ObjectHandling::Find,
        _ => return None,
    };
    let missing = match element.get_str("ObjectHandlingBackup").unwrap_or("Create") {
        "Create" => MissingObject::Create,
        "Ignore" => MissingObject::Ignore,
        "Error" => MissingObject::Error,
        _ => return None,
    };
    let mut values = Vec::new();
    let mut children = Vec::new();
    for child in documents(element.get("Children")) {
        match child.get_str("$Type").ok()? {
            "ImportMappings$ValueMappingElement" => values.push(value_element(child, &own)?),
            "ImportMappings$ObjectMappingElement" => children.push(object_element(child, &own)?),
            _ => return None,
        }
    }
    if handling == ObjectHandling::Find && !values.iter().any(|value| value.key) {
        return None;
    }
    Some(ImportedObject {
        entity: entity.to_string(),
        association: element
            .get_str("Association")
            .unwrap_or_default()
            .to_string(),
        path,
        multiple: occurs != 1,
        handling,
        missing,
        values,
        children,
    })
}

fn value_element(element: &Document, above: &[String]) -> Option<ImportedValue> {
    // A converter is a microflow the value passes through first.
    if !element.get_str("Converter").unwrap_or_default().is_empty() {
        return None;
    }
    let attribute = element.get_str("Attribute").ok()?;
    let ty = match element.get_document("Type").ok()?.get_str("$Type").ok()? {
        "DataTypes$StringType" => ImportedType::String,
        "DataTypes$IntegerType" => ImportedType::Integer,
        "DataTypes$LongType" => ImportedType::Long,
        "DataTypes$DecimalType" | "DataTypes$FloatType" => ImportedType::Decimal,
        "DataTypes$BooleanType" => ImportedType::Boolean,
        "DataTypes$DateTimeType" => ImportedType::DateTime,
        "DataTypes$EnumerationType" => ImportedType::Enumeration,
        _ => return None,
    };
    Some(ImportedValue {
        attribute: attribute
            .rsplit('.')
            .next()
            .unwrap_or(attribute)
            .to_string(),
        path: after(&segments(element.get_str("JsonPath").ok()?), above)?,
        ty,
        key: element.get_bool("IsKey").unwrap_or(false),
    })
}

impl ImportedObject {
    /// The parts of the document `node` holds at this element's path: each
    /// becomes one object.
    pub fn parts<'a>(&self, node: &'a Value) -> Vec<&'a Value> {
        let parts = descend(vec![node], &self.path);
        if self.multiple {
            parts
        } else {
            parts.into_iter().take(1).collect()
        }
    }
}

impl ImportedValue {
    /// The value this element reads of the part `node`: `Ok(None)` when the
    /// part holds none, and why when what it holds is no value of the
    /// attribute's type.
    pub fn read(&self, node: &Value) -> Result<Option<FlowValue>, String> {
        let Some(found) = descend(vec![node], &self.path).into_iter().next() else {
            return Ok(None);
        };
        if found.is_null() {
            return Ok(None);
        }
        let refused = || format!("{found} is not what {} holds", self.attribute);
        Ok(Some(match &self.ty {
            ImportedType::String | ImportedType::Enumeration => FlowValue::String(match found {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            }),
            ImportedType::Integer | ImportedType::Long => FlowValue::Int(match found {
                Value::Number(number) => number
                    .as_i64()
                    .or_else(|| number.as_f64().map(|value| value.trunc() as i64))
                    .ok_or_else(refused)?,
                Value::String(text) => text.trim().parse().map_err(|_| refused())?,
                _ => return Err(refused()),
            }),
            ImportedType::Decimal => FlowValue::Float(match found {
                Value::Number(number) => number.as_f64().ok_or_else(refused)?,
                Value::String(text) => text.trim().parse().map_err(|_| refused())?,
                _ => return Err(refused()),
            }),
            ImportedType::Boolean => FlowValue::Bool(match found {
                Value::Bool(flag) => *flag,
                Value::String(text) if text.eq_ignore_ascii_case("true") => true,
                Value::String(text) if text.eq_ignore_ascii_case("false") => false,
                _ => return Err(refused()),
            }),
            ImportedType::DateTime => FlowValue::DateTime(match found {
                Value::String(text) => crate::datetime::parse_iso(text).ok_or_else(refused)?,
                // Milliseconds since the epoch, as a number.
                Value::Number(number) => number.as_f64().ok_or_else(refused)? / 1000.0,
                _ => return Err(refused()),
            }),
        }))
    }
}

/// What `nodes` hold at `path`: a key of an object, each item of an array
/// for `(Object)`, `(Array)` and `(Wrapper)`, the node itself for `(Value)`
/// and for `(Object)` of an object.
fn descend<'a>(mut nodes: Vec<&'a Value>, path: &[String]) -> Vec<&'a Value> {
    for segment in path {
        let mut next = Vec::new();
        for node in nodes {
            match (segment.as_str(), node) {
                ("(Value)", value) => next.push(value),
                ("(Object)", Value::Object(_)) => next.push(node),
                ("(Object)" | "(Array)" | "(Wrapper)", Value::Array(items)) => {
                    next.extend(items.iter().filter(|item| !item.is_null()));
                }
                (key, Value::Object(members)) => {
                    if let Some(found) = members.get(key)
                        && !found.is_null()
                    {
                        next.push(found);
                    }
                }
                _ => {}
            }
        }
        nodes = next;
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::doc;
    use serde_json::json;

    fn list(items: Vec<Document>) -> Bson {
        let mut values = vec![Bson::Int32(2)];
        values.extend(items.into_iter().map(Bson::Document));
        Bson::Array(values)
    }

    fn value(attribute: &str, path: &str, ty: &str, key: bool) -> Document {
        doc! {
            "$Type": "ImportMappings$ValueMappingElement",
            "Attribute": attribute,
            "Converter": "",
            "IsKey": key,
            "JsonPath": path,
            "Type": doc! { "$Type": ty },
        }
    }

    fn object(
        entity: &str,
        path: &str,
        occurs: i32,
        association: &str,
        children: Vec<Document>,
    ) -> Document {
        doc! {
            "$Type": "ImportMappings$ObjectMappingElement",
            "Association": association,
            "Children": list(children),
            "CustomHandlerCall": Bson::Null,
            "Entity": entity,
            "JsonPath": path,
            "MaxOccurs": occurs,
            "ObjectHandling": "Create",
            "ObjectHandlingBackup": "Create",
        }
    }

    /// Keys, arrays of objects and arrays of plain values are read where
    /// the paths say, each value as its attribute's type.
    #[test]
    fn a_mapping_reads_each_part_where_its_path_says() {
        let mapping = ImportMapping::from_document(&doc! {
            "$Type": "ImportMappings$ImportMapping",
            "Elements": list(vec![object(
                "Sales.Root",
                "(Object)",
                1,
                "",
                vec![
                    value("Sales.Root.Count", "(Object)|count", "DataTypes$IntegerType", false),
                    object(
                        "Sales.Line",
                        "(Object)|lines|(Object)",
                        -1,
                        "Sales.Line_Root",
                        vec![
                            value("Sales.Line.Price", "(Object)|lines|(Object)|price", "DataTypes$DecimalType", false),
                            object(
                                "Sales.Tag",
                                "(Object)|lines|(Object)|tags|(Wrapper)",
                                -1,
                                "Sales.Tag_Line",
                                vec![value("Sales.Tag.Value", "(Object)|lines|(Object)|tags|(Wrapper)|(Value)", "DataTypes$StringType", false)],
                            ),
                        ],
                    ),
                ],
            )]),
        })
        .unwrap();
        let document = json!({
            "count": "2",
            "lines": [{ "price": 2.5, "tags": ["a", "b"] }, { "price": 4 }]
        });
        let root = &mapping.root;
        let parts = root.parts(&document);
        assert_eq!(parts.len(), 1);
        assert_eq!(root.values[0].read(parts[0]), Ok(Some(FlowValue::Int(2))));
        let lines = root.children[0].parts(parts[0]);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            root.children[0].values[0].read(lines[1]),
            Ok(Some(FlowValue::Float(4.0)))
        );
        let tags = root.children[0].children[0].parts(lines[0]);
        assert_eq!(
            tags.iter()
                .map(|tag| root.children[0].children[0].values[0].read(tag).unwrap())
                .collect::<Vec<_>>(),
            [
                Some(FlowValue::String("a".into())),
                Some(FlowValue::String("b".into()))
            ]
        );
        assert!(root.values[0].read(&json!({ "count": "many" })).is_err());
    }
}
