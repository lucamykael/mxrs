//! The domain model graph: entities + associations, embedded inside a single
//! `DomainModels$DomainModel` Unit row. Ports `lib/mxrb/model/domain_model.rb`.

use mxrs_bson::Document;

use crate::association::Association;
use crate::entity::Entity;
use crate::support::{docs_any, get_str_any};

#[derive(Debug, Clone)]
pub struct DomainModel {
    pub documentation: String,
    pub entities: Vec<Entity>,
    pub associations: Vec<Association>,
    pub cross_associations: Vec<Association>,
}

impl DomainModel {
    /// `module_name` qualifies each decoded entity's `qualified_name` when
    /// the BSON doesn't already carry one — mirrors `qualify_entities`,
    /// which mxrb derives from the containing Module's `Name` via a
    /// separate query. Callers already have that name via `Module`, so it's
    /// threaded in rather than re-fetched here.
    pub fn from_bson(doc: &Document, module_name: Option<&str>) -> Self {
        let mut entities: Vec<Entity> = docs_any(doc, &["entities", "Entities"])
            .iter()
            .map(Entity::from_bson)
            .collect();
        if let Some(module_name) = module_name {
            for entity in &mut entities {
                if entity.qualified_name.is_none() {
                    if let Some(name) = &entity.name {
                        entity.qualified_name = Some(format!("{module_name}.{name}"));
                    }
                }
            }
        }

        let associations = docs_any(doc, &["associations", "Associations"])
            .iter()
            .map(Association::from_bson)
            .collect();
        let cross_associations = docs_any(doc, &["crossAssociations", "CrossAssociations"])
            .iter()
            .map(Association::from_bson)
            .collect();

        DomainModel {
            documentation: get_str_any(doc, &["documentation", "Documentation"])
                .unwrap_or_default(),
            entities,
            associations,
            cross_associations,
        }
    }

    pub fn all_associations(&self) -> impl Iterator<Item = &Association> {
        self.associations
            .iter()
            .chain(self.cross_associations.iter())
    }

    pub fn to_bson(&self, id: &str) -> Document {
        mxrs_bson::doc! {
            "$ID": id,
            "$Type": "DomainModels$DomainModel",
            "entities": mxrs_bson::build_array(self.entities.iter().map(|e| mxrs_bson::Bson::Document(e.to_bson())).collect(), 3),
            "associations": mxrs_bson::build_array(self.associations.iter().map(|a| mxrs_bson::Bson::Document(a.to_bson())).collect(), 3),
            "crossAssociations": mxrs_bson::build_array(self.cross_associations.iter().map(|a| mxrs_bson::Bson::Document(a.to_bson())).collect(), 3),
            "annotations": mxrs_bson::build_array(vec![], 3),
            "documentation": self.documentation.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::{doc, Bson};

    #[test]
    fn qualifies_entities_missing_a_qualified_name() {
        let entity = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "name": "Order" };
        let d = doc! { "entities": mxrs_bson::build_array(vec![Bson::Document(entity)], 3) };
        let dm = DomainModel::from_bson(&d, Some("Sales"));
        assert_eq!(
            dm.entities[0].qualified_name.as_deref(),
            Some("Sales.Order")
        );
    }

    #[test]
    fn associations_and_cross_associations_both_show_in_all_associations() {
        let a1 = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "Name": "A" };
        let a2 = doc! { "$ID": uuid::Uuid::new_v4().to_string(), "Name": "B" };
        let d = doc! {
            "associations": mxrs_bson::build_array(vec![Bson::Document(a1)], 3),
            "crossAssociations": mxrs_bson::build_array(vec![Bson::Document(a2)], 3),
        };
        let dm = DomainModel::from_bson(&d, None);
        assert_eq!(dm.all_associations().count(), 2);
    }
}
