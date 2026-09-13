//! `AttributeReference`/`EntityReference`/`EntityPathStep` and their BSON
//! codec, extracted out of `mxrs-forms` so `mxrs-pluggable` can decode
//! `CustomWidgets$WidgetValue` kinds `Attribute`/`Entity`/`Association`
//! without depending on `mxrs-forms` directly.
//!
//! In mxrb, `Pluggable::MprCodec` calls back into the *same*
//! `Forms::MprCodec` instance that constructed it (`forms_codec:` in
//! `Pluggable::MprCodec.new`) for exactly these two reference kinds —
//! composition, not a compile-time dependency, so Ruby's duck typing never
//! notices the cycle. Rust's crate graph can't express that: `mxrs-forms`
//! already needs to depend on `mxrs-pluggable` (to decode an embedded
//! `CustomWidgets$CustomWidget` node), so `mxrs-pluggable` depending back
//! on `mxrs-forms` would be a real dependency cycle, not just an awkward
//! one. This crate is the shared base both sides depend on instead —
//! `mxrs-forms` re-exports these same types from `mxrs_forms::values` so
//! nothing about its public API changes.
//!
//! Ports the `AttributeReference`/`EntityReference`/`EntityPathStep`
//! structs and the `decode_attribute_reference`/`decode_entity_reference`/
//! `encode_attribute_reference`/`encode_entity_reference` methods of
//! `Mxrb::Forms::MprCodec` (`lib/mxrb/forms/mpr_codec.rb`) — those four
//! methods never actually touched `self`'s catalog/reference-decoder state
//! in mxrb either, confirmed by reading the method bodies, so lifting them
//! to free functions changes nothing about their behavior.
//!
//! What's deliberately **not** here: the schema-driven `Node`/`decode_node`
//! machinery (needs `mxrs-forms`'s full `Catalog`), and `TextTemplate`/
//! `Action`/`Icon`/`DataSource`/`Widgets` pluggable-value decoding (each
//! needs that same catalog-driven embedded-node decode, so they stay
//! blocked in `mxrs-pluggable` behind `PluggableError::NeedsFormsIntegration`
//! until a similar extraction happens for `Node` itself — a larger,
//! separate piece of work, out of scope here).

use mxrs_bson::{Bson, parse_array};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RefError {
    #[error("invalid {shape} at {path}")]
    InvalidShape { shape: &'static str, path: String },
}

pub type Result<T> = std::result::Result<T, RefError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityPathStep {
    pub association: String,
    pub destination_entity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityReference {
    pub entity: String,
    pub steps: Vec<EntityPathStep>,
    pub indirect: bool,
}

impl EntityReference {
    pub fn direct(entity: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            steps: Vec::new(),
            indirect: false,
        }
    }

    pub fn through(steps: Vec<EntityPathStep>) -> Self {
        let entity = steps
            .last()
            .map(|s| s.destination_entity.clone())
            .unwrap_or_default();
        Self {
            entity,
            steps,
            indirect: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeReference {
    pub attribute: String,
    pub entity_reference: Option<EntityReference>,
}

fn expect_string(raw: &Bson, path: &str) -> Result<String> {
    match raw {
        Bson::String(s) => Ok(s.clone()),
        _ => Err(RefError::InvalidShape {
            shape: "string",
            path: path.to_string(),
        }),
    }
}

/// Ports `Mxrb::Forms::MprCodec#decode_attribute_reference`.
pub fn decode_attribute_reference(raw: &Bson, path: &str) -> Result<AttributeReference> {
    let Bson::Document(doc) = raw else {
        let attribute = expect_string(raw, path)?;
        return Ok(AttributeReference {
            attribute,
            entity_reference: None,
        });
    };
    if !doc
        .get_str("$Type")
        .map(|t| t.ends_with("$AttributeRef"))
        .unwrap_or(false)
    {
        return Err(RefError::InvalidShape {
            shape: "AttributeReference",
            path: path.to_string(),
        });
    }
    // Ports Ruby's `doc['EntityRef'] && ...` — a present-but-`null`
    // `EntityRef` (an attribute directly on the widget's own context
    // entity, no indirection) must be treated the same as an absent key,
    // not passed to `decode_entity_reference` as if it were a real value.
    // Confirmed a real, previously-unexercised bug via `mxrs-pluggable`'s
    // `instance_oracle.rs` against real QRQC/SPC `AttributeRef` values —
    // this code just never saw one before this crate was extracted.
    let entity_reference = match doc.get("EntityRef") {
        Some(Bson::Null) | None => None,
        Some(v) => Some(decode_entity_reference(v, &format!("{path}.EntityRef"))?),
    };
    let attribute = doc.get_str("Attribute").unwrap_or("").to_string();
    Ok(AttributeReference {
        attribute,
        entity_reference,
    })
}

/// Ports `Mxrb::Forms::MprCodec#decode_entity_reference`.
pub fn decode_entity_reference(raw: &Bson, path: &str) -> Result<EntityReference> {
    let Bson::Document(doc) = raw else {
        let entity = expect_string(raw, path)?;
        return Ok(EntityReference::direct(entity));
    };
    let type_name = doc.get_str("$Type").unwrap_or("");
    if type_name.ends_with("$DirectEntityRef") {
        return Ok(EntityReference::direct(doc.get_str("Entity").unwrap_or("")));
    }
    if type_name.ends_with("$IndirectEntityRef") {
        let steps = match doc.get("Steps") {
            Some(Bson::Array(a)) => parse_array(Some(a))
                .items
                .iter()
                .filter_map(|step| {
                    let Bson::Document(step_doc) = step else {
                        return None;
                    };
                    Some(EntityPathStep {
                        association: step_doc.get_str("Association").unwrap_or("").to_string(),
                        destination_entity: step_doc
                            .get_str("DestinationEntity")
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        return Ok(EntityReference::through(steps));
    }
    Err(RefError::InvalidShape {
        shape: "EntityReference",
        path: path.to_string(),
    })
}

/// Ports `Mxrb::Forms::MprCodec#encode_attribute_reference`.
pub fn encode_attribute_reference(reference: &AttributeReference) -> mxrs_bson::Document {
    let mut document = mxrs_bson::Document::new();
    document.insert("$ID", uuid::Uuid::new_v4().to_string());
    document.insert("$Type", "DomainModels$AttributeRef");
    document.insert("Attribute", reference.attribute.clone());
    document.insert(
        "EntityRef",
        reference
            .entity_reference
            .as_ref()
            .map(|e| Bson::Document(encode_entity_reference(e)))
            .unwrap_or(Bson::Null),
    );
    document
}

/// Ports `Mxrb::Forms::MprCodec#encode_entity_reference`.
pub fn encode_entity_reference(reference: &EntityReference) -> mxrs_bson::Document {
    if reference.indirect {
        let steps: Vec<Bson> = reference
            .steps
            .iter()
            .map(|step| {
                let mut d = mxrs_bson::Document::new();
                d.insert("$ID", uuid::Uuid::new_v4().to_string());
                d.insert("$Type", "DomainModels$EntityRefStep");
                d.insert("Association", step.association.clone());
                d.insert("DestinationEntity", step.destination_entity.clone());
                Bson::Document(d)
            })
            .collect();
        let mut document = mxrs_bson::Document::new();
        document.insert("$ID", uuid::Uuid::new_v4().to_string());
        document.insert("$Type", "DomainModels$IndirectEntityRef");
        document.insert("Steps", mxrs_bson::build_array(steps, 2));
        return document;
    }
    let mut document = mxrs_bson::Document::new();
    document.insert("$ID", uuid::Uuid::new_v4().to_string());
    document.insert("$Type", "DomainModels$DirectEntityRef");
    document.insert("Entity", reference.entity.clone());
    document
}
