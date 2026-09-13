//! The typed, storage-independent value space `Node` assignments hold —
//! everything a Forms property can be set to besides a nested `Node` or
//! plain scalar.
//!
//! Ports `Mxrb::Forms::{Translation,BinaryAsset,Text,Expression,
//! EntityPathStep,EntityReference,AttributeReference,DataType,Condition,
//! TemplateParameter,TextTemplate,XPathConstraint,Reference,EnumValue}`
//! from `lib/mxrb/forms/values.rb`.

use mxrs_bson::BinarySubtype;

use crate::catalog::{ReferenceKind, ruby_name};

#[derive(Debug, Clone, PartialEq)]
pub struct Translation {
    pub language: Option<String>,
    pub text: String,
}

/// Binary form resources stay outside editable Ruby/Rust source in mxrb;
/// the storage codec materializes bytes only at the MPR boundary. mxrs
/// keeps the same shape even though it has no source-export story yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryAsset {
    data: Option<Vec<u8>>,
    pub subtype: BinarySubtype,
    pub source_path: Option<String>,
}

impl BinaryAsset {
    pub fn from_bytes(data: Vec<u8>, subtype: BinarySubtype) -> Self {
        Self {
            data: Some(data),
            subtype,
            source_path: None,
        }
    }

    pub fn read_path(path: impl Into<String>, subtype: BinarySubtype) -> Self {
        Self {
            data: None,
            subtype,
            source_path: Some(path.into()),
        }
    }

    pub fn empty(subtype: BinarySubtype) -> Self {
        Self::from_bytes(Vec::new(), subtype)
    }

    /// Resolves the actual bytes, reading from `source_path` if this asset
    /// was constructed via [`BinaryAsset::read_path`].
    pub fn bytes(&self) -> std::io::Result<Vec<u8>> {
        match (&self.data, &self.source_path) {
            (Some(data), _) => Ok(data.clone()),
            (None, Some(path)) => std::fs::read(path),
            (None, None) => Ok(Vec::new()),
        }
    }

    pub fn at(&self, path: impl Into<String>) -> Self {
        Self {
            data: None,
            subtype: self.subtype,
            source_path: Some(path.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub translations: Vec<Translation>,
}

impl Text {
    pub fn from_translations(translations: Vec<Translation>) -> Self {
        Self { translations }
    }

    pub fn from_plain(text: impl Into<String>) -> Self {
        Self {
            translations: vec![Translation {
                language: None,
                text: text.into(),
            }],
        }
    }

    pub fn to_display_string(&self) -> String {
        self.translations
            .first()
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expression {
    pub source: String,
}

impl Expression {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

/// `EntityPathStep`/`EntityReference`/`AttributeReference` now live in
/// `mxrs-forms-refs` — shared with `mxrs-pluggable`, which needs the same
/// two reference kinds to decode `CustomWidgets$WidgetValue`'s
/// `Attribute`/`Entity`/`Association` kinds without creating a dependency
/// cycle (`mxrs-forms` already depends on `mxrs-pluggable` to decode
/// embedded `CustomWidgets$CustomWidget` nodes). Re-exported here so
/// nothing about this crate's own public API changes.
pub use mxrs_forms_refs::{AttributeReference, EntityPathStep, EntityReference};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataType {
    pub name: String,
    pub target: Option<String>,
}

impl DataType {
    pub fn build(name: impl Into<String>, target: Option<String>) -> Self {
        Self {
            name: name.into(),
            target,
        }
    }
    pub fn object(entity: impl Into<String>) -> Self {
        Self::build("Object", Some(entity.into()))
    }
    pub fn list(entity: impl Into<String>) -> Self {
        Self::build("List", Some(entity.into()))
    }
    pub fn enumeration(enumeration: impl Into<String>) -> Self {
        Self::build("Enumeration", Some(enumeration.into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    pub attribute_value: String,
    pub editable_visible: bool,
}

impl Condition {
    pub fn when_value(attribute_value: impl Into<String>, visible: bool) -> Self {
        Self {
            attribute_value: attribute_value.into(),
            editable_visible: visible,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateParameter {
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextTemplate {
    pub text: Text,
    pub parameters: Vec<TemplateParameter>,
}

impl TextTemplate {
    pub fn build(text: Text, parameters: Vec<String>) -> Self {
        Self {
            text,
            parameters: parameters
                .into_iter()
                .map(|p| TemplateParameter {
                    expression: Expression::new(p),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XPathConstraint {
    pub clauses: Vec<String>,
}

impl XPathConstraint {
    pub fn from_clauses(clauses: Vec<String>) -> Self {
        Self { clauses }
    }
    pub fn source(&self) -> String {
        self.clauses.concat()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub target: String,
    pub kind: ReferenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumValue {
    pub enum_type_name: String,
    pub value: String,
}

impl EnumValue {
    pub fn to_ruby_name(&self) -> String {
        ruby_name(&self.value)
    }
}
