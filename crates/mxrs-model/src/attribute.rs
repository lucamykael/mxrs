//! Entity attributes. Embedded inside `Entity` BSON — not separate Unit rows.
//! Ports `lib/mxrb/model/attribute.rb` from mxrb.

use mxrs_bson::{Document, doc};

use crate::support::{get_bool_any, get_doc_any, get_i32_any, get_id_any, get_str_any};

pub const DEFAULT_STRING_LENGTH: i32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Enum,
}

impl AttributeType {
    pub fn storage_type(self) -> &'static str {
        match self {
            AttributeType::String => "DomainModels$StringAttributeType",
            AttributeType::Integer => "DomainModels$IntegerAttributeType",
            AttributeType::Long => "DomainModels$LongAttributeType",
            AttributeType::Float => "DomainModels$FloatAttributeType",
            AttributeType::Decimal => "DomainModels$DecimalAttributeType",
            AttributeType::Boolean => "DomainModels$BooleanAttributeType",
            AttributeType::DateTime => "DomainModels$DateTimeAttributeType",
            AttributeType::AutoNumber => "DomainModels$AutoNumberAttributeType",
            AttributeType::HashString => "DomainModels$HashedStringAttributeType",
            AttributeType::Binary => "DomainModels$BinaryAttributeType",
            AttributeType::Enum => "DomainModels$EnumerationAttributeType",
        }
    }

    pub fn from_storage_type(storage_type: &str) -> Option<Self> {
        Some(match storage_type {
            "DomainModels$StringAttributeType" => AttributeType::String,
            "DomainModels$IntegerAttributeType" => AttributeType::Integer,
            "DomainModels$LongAttributeType" => AttributeType::Long,
            "DomainModels$FloatAttributeType" => AttributeType::Float,
            "DomainModels$DecimalAttributeType" => AttributeType::Decimal,
            "DomainModels$BooleanAttributeType" => AttributeType::Boolean,
            "DomainModels$DateTimeAttributeType" => AttributeType::DateTime,
            "DomainModels$AutoNumberAttributeType" => AttributeType::AutoNumber,
            "DomainModels$HashedStringAttributeType" => AttributeType::HashString,
            "DomainModels$BinaryAttributeType" => AttributeType::Binary,
            "DomainModels$EnumerationAttributeType" => AttributeType::Enum,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub id: Option<String>,
    pub name: Option<String>,
    pub documentation: String,
    pub attribute_type: AttributeType,
    pub default_value: Option<String>,
    pub data_storage_guid: Option<String>,
    pub export_level: String,
    /// Raw `type`/`Type` sub-document, kept for lossless inspection
    /// (`compare.rb`'s `native_type` diff uses this directly).
    pub raw_type_doc: Option<Document>,
    /// Raw `value`/`Value` sub-document (`compare.rb`'s `native_value`).
    pub raw_value_doc: Option<Document>,
    pub length: Option<i32>,
    pub localize_date: Option<bool>,
    /// Reference to the backing enumeration, when `attribute_type` is `Enum`.
    pub enumeration: Option<String>,
    pub required: bool,
    pub unique: bool,
}

impl Attribute {
    pub fn from_bson(doc: &Document) -> Self {
        let type_doc = get_doc_any(doc, &["type", "Type", "newType", "NewType"]);
        let (attribute_type, length, localize_date, enumeration) = match &type_doc {
            Some(t) => {
                let storage_type = get_str_any(t, &["$Type"]).unwrap_or_default();
                let attribute_type = AttributeType::from_storage_type(&storage_type)
                    .unwrap_or(AttributeType::String);
                let length = get_i32_any(t, &["length", "Length"]);
                let localize_date = get_bool_any(t, &["localizeDate", "LocalizeDate"]);
                let enumeration = get_str_any(t, &["enumeration", "Enumeration"])
                    .or_else(|| get_id_any(t, &["enumeration", "Enumeration"]));
                (attribute_type, length, localize_date, enumeration)
            }
            None => (AttributeType::String, None, None, None),
        };

        let raw_value_doc = get_doc_any(doc, &["value", "Value"]);
        let default_value = raw_value_doc
            .as_ref()
            .and_then(|v| get_str_any(v, &["defaultValue", "DefaultValue"]));

        Attribute {
            id: get_id_any(doc, &["$ID"]),
            name: get_str_any(doc, &["name", "Name"]),
            documentation: get_str_any(doc, &["documentation", "Documentation"])
                .unwrap_or_default(),
            attribute_type,
            default_value,
            data_storage_guid: get_id_any(doc, &["dataStorageGuid", "DataStorageGuid"]),
            export_level: get_str_any(doc, &["exportLevel", "ExportLevel"])
                .unwrap_or_else(|| "Hidden".into()),
            raw_type_doc: type_doc,
            raw_value_doc,
            length,
            localize_date,
            enumeration,
            required: false,
            unique: false,
        }
    }

    pub fn to_bson(&self) -> Document {
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let storage_type = self.attribute_type.storage_type();
        let mut type_doc = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": storage_type };
        if matches!(self.attribute_type, AttributeType::String) {
            type_doc.insert("length", self.length.unwrap_or(DEFAULT_STRING_LENGTH));
        }
        if matches!(self.attribute_type, AttributeType::DateTime) {
            type_doc.insert("localizeDate", self.localize_date.unwrap_or(true));
        }
        if matches!(self.attribute_type, AttributeType::Enum) {
            type_doc.insert("enumeration", self.enumeration.clone().unwrap_or_default());
        }

        doc! {
            "$ID": id,
            "$Type": "DomainModels$Attribute",
            "name": self.name.clone(),
            "documentation": self.documentation.clone(),
            "dataStorageGuid": self.data_storage_guid.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            "exportLevel": self.export_level.clone(),
            "type": type_doc,
            "value": doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "DomainModels$StoredValue",
                "defaultValue": self.default_value.clone().unwrap_or_default(),
            },
        }
    }
}

/// Marks attributes `required`/`unique` from an entity's `validationRules`,
/// matching `Entity.apply_validation_rules` — the rule references its
/// attribute by (possibly qualified) name, so callers pass the already
/// name-indexed attribute list.
pub fn apply_validation_rules(attributes: &mut [Attribute], rules: &[Document]) {
    for rule in rules {
        let Some(attr_ref) = get_str_any(rule, &["Attribute"]) else {
            continue;
        };
        let short_name = attr_ref.rsplit('.').next().unwrap_or(&attr_ref);
        let Some(attribute) = attributes
            .iter_mut()
            .find(|a| a.name.as_deref() == Some(short_name))
        else {
            continue;
        };
        let kind = get_doc_any(rule, &["RuleInfo"])
            .and_then(|r| get_str_any(&r, &["$Type"]))
            .unwrap_or_default();
        if kind.ends_with("RequiredRuleInfo") {
            attribute.required = true;
        }
        if kind.ends_with("UniqueRuleInfo") {
            attribute.unique = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumeration_reference_survives_serialization() {
        let source = doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "name": "State",
            "type": {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "DomainModels$EnumerationAttributeType",
                "enumeration": "Demo.State",
            },
        };
        let attribute = Attribute::from_bson(&source);
        let output = attribute.to_bson();
        assert_eq!(
            output
                .get_document("type")
                .unwrap()
                .get_str("enumeration")
                .unwrap(),
            "Demo.State"
        );
    }
}
