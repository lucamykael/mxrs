//! The relational schema a Mendix model deploys to, independent of any SQL
//! dialect.
//!
//! Ports the derivation half of `lib/mxrb/runtime/schema_migrator.rb`: one
//! table per persistable, non-view entity, one join table per association,
//! and physical names keyed by storage GUIDs so a rename never loses data.
//! The names are mxrb's, byte for byte, which is what lets a database created
//! by either tool be read by the other.
//!
//! Nothing here emits SQL. A backend maps [`AttributeType`] to its own column
//! types and writes its own DDL; what it must not do is invent its own names,
//! because the query side derives the same names from the same model and the
//! two only meet if they agree. That agreement used to be two identical
//! copies of [`physical_name`] in separate crates.
//!
//! Not to be confused with `mxrs-schema`, which is the Mendix *metamodel*.

use std::collections::BTreeMap;

use mxrs_model::{AttributeType, Module};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    #[error("ambiguous entity {0}")]
    AmbiguousEntity(String),
    #[error("unknown entity {0}")]
    UnknownEntity(String),
    #[error("ambiguous association {0}")]
    AmbiguousAssociation(String),
    #[error("unknown association {0}")]
    UnknownAssociation(String),
}

pub type Result<T> = std::result::Result<T, SchemaError>;

/// The system members an entity can carry, as `(model flag, column name)`.
/// The column names are physical and shared by every backend; the column
/// *types* are the backend's business.
pub const SYSTEM_COLUMNS: [(&str, &str); 4] = [
    ("owner", "__owner_id"),
    ("created_date", "__created_at"),
    ("changed_date", "__changed_at"),
    ("changed_by", "__changed_by_id"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaColumn {
    pub name: String,
    pub storage_key: String,
    pub sql_name: String,
    pub kind: AttributeType,
    pub default: Option<String>,
    pub required: bool,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitySchema {
    pub name: String,
    pub storage_key: String,
    pub table: String,
    pub columns: Vec<SchemaColumn>,
    /// Physical column names of the enabled system members, in
    /// [`SYSTEM_COLUMNS`] order.
    pub system_members: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociationSchema {
    pub name: String,
    pub qualified_name: String,
    pub storage_key: String,
    pub table: String,
    pub from_entity: String,
    pub to_entity: String,
    /// `true` for `Reference` (each source at most one target).
    pub reference: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeSchema {
    pub entities: Vec<EntitySchema>,
    pub associations: Vec<AssociationSchema>,
}

impl RuntimeSchema {
    /// Resolves a stored entity name against the current model. `Ok(None)`
    /// means the model no longer declares it — a migration question the
    /// caller answers, not an error by itself. An unqualified name matching
    /// several modules stays an error: never a silent pick.
    ///
    /// # Errors
    ///
    /// [`SchemaError::AmbiguousEntity`] when the short name matches more than
    /// one module.
    pub fn lookup(&self, name: &str) -> Result<Option<&EntitySchema>> {
        if let Some(exact) = self.entities.iter().find(|entity| entity.name == name) {
            return Ok(Some(exact));
        }
        let matches: Vec<&EntitySchema> = self
            .entities
            .iter()
            .filter(|entity| entity.name.rsplit('.').next() == Some(name))
            .collect();
        match matches.as_slice() {
            [only] => Ok(Some(only)),
            [] => Ok(None),
            _ => Err(SchemaError::AmbiguousEntity(name.to_string())),
        }
    }

    /// # Errors
    ///
    /// [`SchemaError::UnknownEntity`] when the model does not declare it, or
    /// [`SchemaError::AmbiguousEntity`] when the short name is not unique.
    pub fn entity(&self, name: &str) -> Result<&EntitySchema> {
        self.lookup(name)?
            .ok_or_else(|| SchemaError::UnknownEntity(name.to_string()))
    }

    /// # Errors
    ///
    /// [`SchemaError::UnknownAssociation`] or
    /// [`SchemaError::AmbiguousAssociation`].
    pub fn association(&self, name: &str) -> Result<&AssociationSchema> {
        let matches: Vec<&AssociationSchema> = self
            .associations
            .iter()
            .filter(|candidate| {
                candidate.qualified_name == name
                    || candidate.name == name
                    || candidate.qualified_name.rsplit('.').next() == Some(name)
            })
            .collect();
        match matches.as_slice() {
            [only] => Ok(only),
            [] => Err(SchemaError::UnknownAssociation(name.to_string())),
            _ => Err(SchemaError::AmbiguousAssociation(name.to_string())),
        }
    }
}

/// What a migration created or changed. Every backend reports the same
/// vocabulary, so a caller can render one result without knowing the dialect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationResult {
    pub created_tables: Vec<String>,
    pub added_columns: Vec<String>,
    pub rebuilt_tables: Vec<String>,
}

/// One removal a migration would perform; carried by the destructive-refusal
/// error so callers can present exactly what would be lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedItem {
    pub kind: &'static str,
    pub name: String,
    pub table: String,
    pub storage_key: String,
    pub entity_key: Option<String>,
    pub column: Option<String>,
}

/// `mxrb_<kind>_<sha256(key)[0,20]>` — mxrb's physical naming, kept verbatim
/// for database compatibility.
#[must_use]
pub fn physical_name(kind: &str, key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("mxrb_{kind}_{}", &hex[..20])
}

/// The catalog's name for an attribute type. Stored in
/// `mxrb_schema_attributes.logical_type`, which is why it is the *logical*
/// type and not the column type of whatever database is in front of it.
#[must_use]
pub const fn logical_type(kind: AttributeType) -> &'static str {
    match kind {
        AttributeType::String => "string",
        AttributeType::Integer => "integer",
        AttributeType::Long => "long",
        AttributeType::Float => "float",
        AttributeType::Decimal => "decimal",
        AttributeType::Boolean => "boolean",
        AttributeType::DateTime => "datetime",
        AttributeType::AutoNumber => "autonumber",
        AttributeType::HashString => "hashstring",
        AttributeType::Binary => "binary",
        AttributeType::Enum => "enum",
    }
}

/// Ports `SchemaMigrator.derive`.
#[must_use]
pub fn derive(modules: &[Module]) -> RuntimeSchema {
    let mut entities = Vec::new();
    let mut associations = Vec::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        let mut by_id: BTreeMap<String, String> = BTreeMap::new();
        for entity in module.entities() {
            let qualified = qualified_entity_name(module_name, entity);
            if let Some(id) = &entity.id {
                by_id.insert(id.clone(), qualified.clone());
            }
            by_id.insert(qualified.clone(), qualified);
        }
        for entity in module.entities() {
            if !entity.persistable || entity.oql_view() {
                continue;
            }
            entities.push(entity_schema(module_name, entity));
        }
        for association in module.associations() {
            let Some(name) = association.name.as_deref() else {
                continue;
            };
            let from_id = association.from_entity_id.clone().unwrap_or_default();
            let from = by_id.get(&from_id).cloned().unwrap_or(from_id);
            let to = resolve_target(
                modules,
                association.to_entity_id.as_deref().unwrap_or_default(),
                &by_id,
            );
            if from.is_empty() || to.is_empty() {
                continue;
            }
            let qualified = format!("{module_name}.{name}");
            let key = association
                .id
                .clone()
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| qualified.clone());
            associations.push(AssociationSchema {
                name: name.to_string(),
                qualified_name: qualified,
                storage_key: key.clone(),
                table: physical_name("association", &key),
                from_entity: from,
                to_entity: to,
                reference: association.association_type
                    == mxrs_model::association::AssociationType::Reference,
            });
        }
    }
    RuntimeSchema {
        entities,
        associations,
    }
}

fn entity_schema(module_name: &str, entity: &mxrs_model::Entity) -> EntitySchema {
    let qualified = qualified_entity_name(module_name, entity);
    let mut key = entity.data_storage_guid.clone().unwrap_or_default();
    if key.is_empty() {
        key = entity.id.clone().unwrap_or_default();
    }
    if key.is_empty() {
        key = qualified.clone();
    }
    let columns = entity
        .attributes
        .iter()
        .map(|attribute| {
            let mut attribute_key = attribute.data_storage_guid.clone().unwrap_or_default();
            if attribute_key.is_empty() {
                attribute_key = attribute.id.clone().unwrap_or_default();
            }
            if attribute_key.is_empty() {
                attribute_key = format!("{key}:{}", attribute.name.as_deref().unwrap_or_default());
            }
            let kind = attribute.attribute_type;
            SchemaColumn {
                name: attribute.name.clone().unwrap_or_default(),
                sql_name: physical_name("attribute", &attribute_key),
                storage_key: attribute_key,
                kind,
                default: attribute.default_value.clone(),
                required: attribute.required,
                unique: attribute.unique || kind == AttributeType::AutoNumber,
            }
        })
        .collect();
    let system = &entity.system_members;
    let system_members = SYSTEM_COLUMNS
        .iter()
        .filter(|(flag, _)| match *flag {
            "owner" => system.owner,
            "created_date" => system.created_date,
            "changed_date" => system.changed_date,
            "changed_by" => system.changed_by,
            _ => false,
        })
        .map(|(_, name)| *name)
        .collect();
    EntitySchema {
        name: qualified,
        table: physical_name("entity", &key),
        storage_key: key,
        columns,
        system_members,
    }
}

fn qualified_entity_name(module_name: &str, entity: &mxrs_model::Entity) -> String {
    entity.qualified_name.clone().unwrap_or_else(|| {
        format!(
            "{module_name}.{}",
            entity.name.as_deref().unwrap_or_default()
        )
    })
}

fn resolve_target(modules: &[Module], pointer: &str, local: &BTreeMap<String, String>) -> String {
    if let Some(target) = local.get(pointer) {
        return target.clone();
    }
    if pointer.contains('.') {
        return pointer.to_string();
    }
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        if let Some(entity) = module
            .entities()
            .iter()
            .find(|entity| entity.id.as_deref() == Some(pointer))
        {
            return qualified_entity_name(module_name, entity);
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The naming is a cross-tool contract, not an implementation detail: a
    /// database written by mxrb is read by mxrs and the other way round, and
    /// the only thing holding that together is this function.
    #[test]
    fn physical_names_are_mxrbs_and_depend_only_on_the_storage_key() {
        // The first 20 hex characters of sha256("abc-123").
        assert_eq!(
            physical_name("entity", "abc-123"),
            "mxrb_entity_5942d94f524882e0f29b"
        );
        assert_eq!(
            physical_name("attribute", "abc-123"),
            "mxrb_attribute_5942d94f524882e0f29b"
        );
        assert_eq!(
            physical_name("association", "abc-123"),
            "mxrb_association_5942d94f524882e0f29b"
        );
        // Only the key feeds the digest, and any difference in it shows.
        assert_ne!(physical_name("entity", "a"), physical_name("entity", "b"));
    }

    #[test]
    fn a_short_entity_name_resolves_only_while_it_is_unambiguous() {
        let entity = |name: &str| EntitySchema {
            name: name.to_string(),
            storage_key: name.to_string(),
            table: physical_name("entity", name),
            columns: Vec::new(),
            system_members: Vec::new(),
        };
        let schema = RuntimeSchema {
            entities: vec![entity("Sales.Order"), entity("Archive.Order")],
            associations: Vec::new(),
        };
        assert_eq!(
            schema
                .lookup("Sales.Order")
                .unwrap()
                .map(|e| e.name.as_str()),
            Some("Sales.Order")
        );
        assert_eq!(
            schema.lookup("Order"),
            Err(SchemaError::AmbiguousEntity("Order".to_string()))
        );
        assert_eq!(schema.lookup("Missing").unwrap(), None);
        assert_eq!(
            schema.entity("Missing"),
            Err(SchemaError::UnknownEntity("Missing".to_string()))
        );
    }

    #[test]
    fn logical_types_are_the_catalog_names_not_column_types() {
        assert_eq!(logical_type(AttributeType::AutoNumber), "autonumber");
        assert_eq!(logical_type(AttributeType::HashString), "hashstring");
        assert_eq!(logical_type(AttributeType::DateTime), "datetime");
    }
}
