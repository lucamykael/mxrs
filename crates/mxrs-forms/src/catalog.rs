//! Typed, immutable view of the canonical Mendix Forms metamodel, loaded
//! from the versioned schema JSON (`compiler/schemas/forms-11.12.1.json` in
//! mxrb) embedded as a compile-time asset.
//!
//! Ports `Mxrb::Forms::{Naming,Size,DefaultValue,Property,Type,Catalog}`
//! from `lib/mxrb/forms/catalog.rb`. Only 11.12.1 is embedded per the
//! locked MVP scope; widening means adding more embedded schema files and
//! entries to `FILES`.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value as Json;

use crate::error::{FormsError, Result};

const FORMS_SCHEMA_11_12_1: &str = include_str!("../assets/forms-11.12.1.json");

/// Converts an exact metamodel identifier into a conventional Rust-ish
/// (snake_case) name, mirroring `Mxrb::Forms::Naming.ruby_name`.
pub fn ruby_name(name: &str) -> String {
    static ACRONYM_BOUNDARY: OnceLock<Regex> = OnceLock::new();
    static WORD_BOUNDARY: OnceLock<Regex> = OnceLock::new();
    let acronym = ACRONYM_BOUNDARY.get_or_init(|| Regex::new(r"([A-Z]+)([A-Z][a-z])").unwrap());
    let word = WORD_BOUNDARY.get_or_init(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap());

    let step1 = acronym.replace_all(name, "${1}_${2}");
    let step2 = word.replace_all(&step1, "${1}_${2}");
    let normalized = step2.replace('-', "_").to_lowercase();
    if normalized == "class" {
        "css_class".to_string()
    } else {
        normalized
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    Literal(Json),
    Size(Size),
    Factory(String),
    /// The schema documents a `kind` this port doesn't special-case yet
    /// (currently just `unknown_data_type`). Mirrors mxrb's own
    /// `build_default`, which likewise falls through to a `nil` value for
    /// any `kind` its `case` doesn't match — informational only; nothing
    /// here reads default values to fill in unset properties.
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    ByName,
    ById,
    Reference,
}

impl ReferenceKind {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "by_name" => Some(Self::ByName),
            "by_id" => Some(Self::ById),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    One,
    Many,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub ruby_name: String,
    pub declared_by: String,
    pub type_name: String,
    pub targets: Vec<String>,
    pub cardinality: Cardinality,
    pub optional: bool,
    pub default_value: Option<DefaultValue>,
    pub reference: Option<ReferenceKind>,
}

impl Property {
    pub fn many(&self) -> bool {
        self.cardinality == Cardinality::Many
    }
    pub fn one(&self) -> bool {
        !self.many()
    }
    pub fn is_reference(&self) -> bool {
        self.reference.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Enum,
    Element,
    Extend,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Type {
    pub name: String,
    pub ruby_name: String,
    pub kind: TypeKind,
    pub abstract_: bool,
    pub base_name: Option<String>,
    pub properties: Vec<Property>,
    pub all_properties: Vec<Property>,
    pub values: Vec<String>,
    pub widget: bool,
}

impl Type {
    pub fn is_enum(&self) -> bool {
        self.kind == TypeKind::Enum
    }
    pub fn is_element(&self) -> bool {
        !self.is_enum()
    }
    pub fn is_concrete(&self) -> bool {
        self.is_element() && !self.abstract_
    }

    pub fn property(&self, identifier: &str, inherited: bool) -> Option<&Property> {
        let candidates = if inherited {
            &self.all_properties
        } else {
            &self.properties
        };
        let ruby_identifier = ruby_name(identifier);
        candidates
            .iter()
            .find(|p| p.name == identifier || p.ruby_name == ruby_identifier)
    }

    pub fn fetch_property(&self, identifier: &str, inherited: bool) -> Result<&Property> {
        self.property(identifier, inherited)
            .ok_or_else(|| FormsError::UnknownProperty {
                type_name: self.name.clone(),
                property: identifier.to_string(),
            })
    }
}

pub struct Catalog {
    pub version: String,
    types: Vec<Type>,
    by_name: HashMap<String, usize>,
    by_ruby_name: HashMap<String, usize>,
}

impl Catalog {
    pub fn for_version(version: &str) -> Result<Self> {
        let source = match version {
            "11.12.1" => FORMS_SCHEMA_11_12_1,
            other => return Err(FormsError::UnsupportedSchemaVersion(other.to_string())),
        };
        Self::parse(source, Some(version))
    }

    fn parse(source: &str, expected_version: Option<&str>) -> Result<Self> {
        let payload: Json = serde_json::from_str(source)?;
        let version = payload
            .get("mendix_version")
            .and_then(Json::as_str)
            .ok_or(FormsError::MalformedSchema("mendix_version"))?
            .to_string();
        if let Some(expected) = expected_version {
            if version != expected {
                return Err(FormsError::SchemaVersionMismatch {
                    expected: expected.to_string(),
                    actual: version,
                });
            }
        }

        let widget_names: std::collections::HashSet<&str> = payload
            .get("widget_types")
            .and_then(Json::as_array)
            .ok_or(FormsError::MalformedSchema("widget_types"))?
            .iter()
            .filter_map(|w| w.get("name").and_then(Json::as_str))
            .collect();

        let types_obj = payload
            .get("types")
            .and_then(Json::as_object)
            .ok_or(FormsError::MalformedSchema("types"))?;
        let mut types: Vec<Type> = types_obj
            .iter()
            .map(|(name, definition)| {
                build_type(name, definition, widget_names.contains(name.as_str()))
            })
            .collect::<Result<_>>()?;
        types.sort_by(|a, b| a.name.cmp(&b.name));

        let by_name = types
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.clone(), i))
            .collect();
        let by_ruby_name = types
            .iter()
            .enumerate()
            .map(|(i, t)| (t.ruby_name.clone(), i))
            .collect();

        Ok(Self {
            version,
            types,
            by_name,
            by_ruby_name,
        })
    }

    pub fn types(&self) -> &[Type] {
        &self.types
    }

    pub fn type_(&self, identifier: &str) -> Option<&Type> {
        self.by_name
            .get(identifier)
            .or_else(|| self.by_ruby_name.get(&ruby_name(identifier)))
            .map(|&i| &self.types[i])
    }

    pub fn fetch_type(&self, identifier: &str) -> Result<&Type> {
        self.type_(identifier)
            .ok_or_else(|| FormsError::UnknownType {
                version: self.version.clone(),
                type_name: identifier.to_string(),
            })
    }

    pub fn descendant(&self, candidate: &str, ancestor: &str) -> Result<bool> {
        let mut current = self.fetch_type(candidate)?;
        let ancestor = self.fetch_type(ancestor)?;
        while let Some(base_name) = &current.base_name {
            if base_name == &ancestor.name {
                return Ok(true);
            }
            current = self.fetch_type(base_name)?;
        }
        Ok(false)
    }
}

fn build_type(name: &str, definition: &Json, widget: bool) -> Result<Type> {
    let kind_str = definition
        .get("kind")
        .and_then(Json::as_str)
        .ok_or(FormsError::MalformedSchema("kind"))?;
    let kind = match kind_str {
        "enum" => TypeKind::Enum,
        "extend" => TypeKind::Extend,
        _ => TypeKind::Element,
    };
    let abstract_ = definition
        .get("abstract")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let base_name = definition
        .get("base")
        .and_then(Json::as_str)
        .map(str::to_string);
    let properties = build_properties(definition.get("properties"))?;
    let all_properties = build_properties(definition.get("all_properties"))?;
    let values = definition
        .get("values")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    Ok(Type {
        name: name.to_string(),
        ruby_name: ruby_name(name),
        kind,
        abstract_,
        base_name,
        properties,
        all_properties,
        values,
        widget,
    })
}

fn build_properties(definitions: Option<&Json>) -> Result<Vec<Property>> {
    let Some(array) = definitions.and_then(Json::as_array) else {
        return Ok(Vec::new());
    };
    array.iter().map(build_property).collect()
}

fn build_property(definition: &Json) -> Result<Property> {
    let name = definition
        .get("name")
        .and_then(Json::as_str)
        .ok_or(FormsError::MalformedSchema("property.name"))?
        .to_string();
    let declared_by = definition
        .get("declared_by")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let type_name = definition
        .get("type")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let targets = definition
        .get("targets")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let cardinality = match definition.get("cardinality").and_then(Json::as_str) {
        Some("many") => Cardinality::Many,
        _ => Cardinality::One,
    };
    let optional = definition
        .get("optional")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let default_value = definition
        .get("default_value")
        .map(build_default)
        .transpose()?;
    let reference = definition
        .get("reference")
        .and_then(Json::as_str)
        .and_then(ReferenceKind::parse);

    Ok(Property {
        ruby_name: ruby_name(&name),
        name,
        declared_by,
        type_name,
        targets,
        cardinality,
        optional,
        default_value,
        reference,
    })
}

fn build_default(definition: &Json) -> Result<DefaultValue> {
    let kind = definition
        .get("kind")
        .and_then(Json::as_str)
        .ok_or(FormsError::MalformedSchema("default_value.kind"))?;
    match kind {
        "literal" => Ok(DefaultValue::Literal(
            definition.get("value").cloned().unwrap_or(Json::Null),
        )),
        "size" => {
            let width = definition
                .get("width")
                .and_then(Json::as_i64)
                .ok_or(FormsError::MalformedSchema("default_value.width"))?;
            let height = definition
                .get("height")
                .and_then(Json::as_i64)
                .ok_or(FormsError::MalformedSchema("default_value.height"))?;
            Ok(DefaultValue::Size(Size { width, height }))
        }
        "factory" => Ok(DefaultValue::Factory(
            definition
                .get("type")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
        )),
        other => Ok(DefaultValue::Unknown(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_name_matches_settings_field_method_conventions() {
        assert_eq!(ruby_name("HttpPortNumber"), "http_port_number");
        assert_eq!(ruby_name("caption"), "caption");
        assert_eq!(ruby_name("class"), "css_class");
    }

    #[test]
    fn loads_the_embedded_11_12_1_schema() {
        let catalog = Catalog::for_version("11.12.1").unwrap();
        assert_eq!(catalog.version, "11.12.1");
        assert!(catalog.types().len() > 100);
    }

    #[test]
    fn rejects_unsupported_version() {
        assert!(Catalog::for_version("9.6.1.29396").is_err());
    }

    #[test]
    fn fetch_type_resolves_page() {
        let catalog = Catalog::for_version("11.12.1").unwrap();
        let page = catalog.fetch_type("Page").unwrap();
        assert!(page.property("title", true).is_some());
        assert!(page.property("layoutCall", true).is_some());
    }

    #[test]
    fn fetch_type_by_ruby_name_also_resolves() {
        let catalog = Catalog::for_version("11.12.1").unwrap();
        assert_eq!(
            catalog.type_("data_grid").map(|t| t.name.as_str()),
            catalog.type_("DataGrid").map(|t| t.name.as_str())
        );
    }

    #[test]
    fn descendant_walks_the_base_chain() {
        let catalog = Catalog::for_version("11.12.1").unwrap();
        // Page extends FormBase (per the schema dump inspected while porting).
        assert!(catalog.descendant("Page", "FormBase").unwrap());
        assert!(!catalog.descendant("FormBase", "Page").unwrap());
    }
}
