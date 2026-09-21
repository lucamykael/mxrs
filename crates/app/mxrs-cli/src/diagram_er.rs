//! Domain ER-diagram data and layout — ports the framework-neutral core of
//! `Mxrb::DomainDiagram` (`lib/mxrb/domain_diagram.rb`): the JSON
//! projection mxrb serves to its browser UI (`Document#to_h`) and the
//! audited visual-field writer (`LayoutWriter#apply!`, entity locations
//! plus association connection anchors only). The browser server and its
//! db-style lifecycle (`up|down|status|destroy|__serve`, port 4568) are
//! deliberately not ported — the CLI refuses those actions explicitly.
//! Cross-module anchor overrides interoperate with mxrb through the same
//! `_MxrbDomainDiagramAssociation` sidecar table
//! (`MprFile::domain_diagram_anchors`).

use std::collections::BTreeMap;
use std::path::Path;

use mxrs_bson::Document;
use serde_json::{Value, json};

/// `Mxrb::DomainDiagram::ANCHOR_VALUES` — anchor name to the native
/// connection-point value stored on an association.
const ANCHOR_VALUES: [(&str, &str); 4] = [
    ("north", "50;0"),
    ("east", "100;50"),
    ("south", "50;100"),
    ("west", "0;50"),
];

fn anchor_value(name: &str) -> Option<&'static str> {
    ANCHOR_VALUES
        .iter()
        .find(|(anchor, _)| *anchor == name)
        .map(|(_, value)| *value)
}

fn value_anchor(value: &str, fallback: &'static str) -> &'static str {
    ANCHOR_VALUES
        .iter()
        .find(|(_, native)| *native == value)
        .map(|(anchor, _)| *anchor)
        .unwrap_or(fallback)
}

/// Builds the diagram projection for a project — ports
/// `Mxrb::DomainDiagram::Document#to_h`.
pub fn document(path: &Path, selected_modules: &[String]) -> Result<Value, String> {
    let project = mxrs_model::Project::open(path, true).map_err(|error| error.to_string())?;
    let modules = project.modules().map_err(|error| error.to_string())?;
    let names: Vec<String> = modules
        .iter()
        .filter_map(|module| module.name.clone())
        .collect();
    let unknown: Vec<&String> = selected_modules
        .iter()
        .filter(|name| !names.contains(name))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown modules: {}",
            unknown
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let anchor_overrides = project
        .mpr()
        .domain_diagram_anchors()
        .map_err(|error| error.to_string())?;
    // Entity id AND qualified name both resolve to the qualified name, the
    // way mxrb's `known` map lets associations point either way.
    let mut known: BTreeMap<String, String> = BTreeMap::new();
    for module in &modules {
        let module_name = module.name.clone().unwrap_or_default();
        for entity in module.entities() {
            let qualified = entity.qualified_name.clone().unwrap_or_else(|| {
                format!("{module_name}.{}", entity.name.clone().unwrap_or_default())
            });
            if let Some(id) = &entity.id {
                known.insert(id.clone(), qualified.clone());
            }
            known.insert(qualified.clone(), qualified);
        }
    }
    let mut payloads = Vec::new();
    for module in &modules {
        let module_name = module.name.clone().unwrap_or_default();
        if !selected_modules.is_empty() && !selected_modules.contains(&module_name) {
            continue;
        }
        let Some(domain) = &module.domain_model else {
            continue;
        };
        let raw = domain_unit_doc(project.mpr(), &module.id)?;
        let connection_docs = association_docs(&raw);
        let entities: Vec<Value> = module
            .entities()
            .iter()
            .map(|entity| {
                let kind = if entity.oql_view() {
                    "oql_view"
                } else if entity.persistable {
                    "entity"
                } else {
                    "dto"
                };
                json!({
                    "id": entity.id,
                    "name": entity.name,
                    "qualified_name": entity.qualified_name.clone().unwrap_or_else(|| {
                        format!("{module_name}.{}", entity.name.clone().unwrap_or_default())
                    }),
                    "module": module_name,
                    "kind": kind,
                    "persistent": entity.persistable,
                    "x": entity.location.x,
                    "y": entity.location.y,
                    "attributes": entity.attributes.iter().map(|attribute| json!({
                        "name": attribute.name,
                        "type": attribute_type_name(attribute.attribute_type),
                        "required": attribute.required,
                        "key": attribute.unique,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let associations: Vec<Value> = domain
            .all_associations()
            .filter_map(|association| {
                let id = association.id.clone()?;
                let from = known.get(association.from_entity_id.as_deref()?)?;
                let to = known.get(association.to_entity_id.as_deref()?)?;
                let raw = connection_docs.get(&id);
                let cross_module = raw.and_then(|doc| doc.get_str("$Type").ok())
                    == Some("DomainModels$CrossAssociation");
                let override_anchors = anchor_overrides.get(&id);
                let stored_anchor = |field: &str, fallback: &'static str| {
                    raw.and_then(|doc| doc.get_str(field).ok())
                        .map(|value| value_anchor(value, fallback))
                        .unwrap_or(fallback)
                };
                Some(json!({
                    "id": id,
                    "name": association.name,
                    "from": from,
                    "to": to,
                    "type": format!("{:?}", association.association_type),
                    "owner": format!("{:?}", association.owner),
                    "source_anchor": override_anchors
                        .map(|(source, _)| source.as_str())
                        .unwrap_or_else(|| stored_anchor("ParentConnection", "east")),
                    "target_anchor": override_anchors
                        .map(|(_, target)| target.as_str())
                        .unwrap_or_else(|| stored_anchor("ChildConnection", "west")),
                    "cross_module": cross_module,
                    "editable_anchors": true,
                    "anchor_storage": if cross_module { "mxrb" } else { "native" },
                }))
            })
            .collect();
        payloads.push(json!({
            "id": module.id,
            "name": module_name,
            "entities": entities,
            "associations": associations,
        }));
    }
    Ok(json!({
        "project": {
            "name": project.name().map_err(|error| error.to_string())?,
            "mendix_version": project.mendix_version().map_err(|error| error.to_string())?,
        },
        "module_filter_applied": !selected_modules.is_empty(),
        "modules": payloads,
    }))
}

fn attribute_type_name(attribute_type: mxrs_model::attribute::AttributeType) -> &'static str {
    use mxrs_model::attribute::AttributeType;
    match attribute_type {
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

fn domain_unit_doc(mpr: &mxrs_mpr::MprFile, module_id: &str) -> Result<Document, String> {
    let unit = mpr
        .children_of(module_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|unit| unit.containment_name == "DomainModel")
        .ok_or_else(|| format!("module unit {module_id} has no DomainModel child"))?;
    mpr.parse_contents(&unit).map_err(|error| error.to_string())
}

/// `association id -> raw association document` across both the local and
/// cross-module arrays, mirroring mxrb's `association_by_id`.
fn association_docs(raw: &Document) -> BTreeMap<String, Document> {
    let mut docs = BTreeMap::new();
    for field in [
        "associations",
        "Associations",
        "crossAssociations",
        "CrossAssociations",
    ] {
        let Ok(array) = raw.get_array(field) else {
            continue;
        };
        for item in array {
            let Some(doc) = item.as_document() else {
                continue;
            };
            if let Some(id) = doc.get("$ID").and_then(mxrs_bson::extract_id) {
                docs.insert(id, doc.clone());
            }
        }
    }
    docs
}

/// What applying a layout would change — ports `LayoutWriter#apply!`'s
/// counting, split so the CLI can preview before mutating.
#[derive(Debug)]
pub struct LayoutPlan {
    pub unit_changes: Vec<(String, Document, usize)>,
    pub metadata_anchors: Vec<AnchorRow>,
    /// How many sidecar anchor rows would actually change — precomputed so
    /// a preview reports the same total an apply would.
    pub anchor_changes: usize,
}

impl LayoutPlan {
    pub fn changes(&self) -> usize {
        let unit_changes: usize = self.unit_changes.iter().map(|(_, _, count)| count).sum();
        unit_changes + self.anchor_changes
    }
}

/// Validates a layout payload against the project and computes the audited
/// visual changes (entity `Location`s, association connection anchors,
/// cross-module anchor metadata) without writing anything.
pub fn plan_layout(path: &Path, payload: &Value) -> Result<LayoutPlan, String> {
    let mpr = mxrs_mpr::MprFile::open(path, true).map_err(|error| error.to_string())?;
    let modules = payload
        .get("modules")
        .and_then(Value::as_array)
        .ok_or("layout payload must carry a modules array")?;
    let mut module_units: BTreeMap<String, String> = BTreeMap::new();
    for unit in mpr
        .units_by_containment("Modules")
        .map_err(|error| error.to_string())?
    {
        let doc = mpr
            .parse_contents(&unit)
            .map_err(|error| error.to_string())?;
        if let Ok(name) = doc.get_str("Name") {
            module_units.insert(name.to_string(), unit.unit_id.clone());
        }
    }
    let mut unit_changes = Vec::new();
    let mut metadata_anchors = Vec::new();
    for layout in modules {
        let name = layout
            .get("name")
            .and_then(Value::as_str)
            .ok_or("each module layout needs a name")?;
        let module_id = module_units
            .get(name)
            .ok_or_else(|| format!("domain module {name} not found"))?;
        let mut doc = domain_unit_doc(&mpr, module_id)
            .map_err(|_| format!("domain model {name} not found"))?;
        let unit_id = mpr
            .children_of(module_id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|unit| unit.containment_name == "DomainModel")
            .map(|unit| unit.unit_id)
            .ok_or_else(|| format!("domain model {name} not found"))?;
        let mut changed = update_entities(&mut doc, layout)?;
        let (association_changes, module_metadata) = update_associations(&mut doc, layout)?;
        changed += association_changes;
        metadata_anchors.extend(module_metadata);
        if changed > 0 {
            unit_changes.push((unit_id, doc, changed));
        }
    }
    let current = mpr
        .domain_diagram_anchors()
        .map_err(|error| error.to_string())?;
    let anchor_changes = metadata_anchors
        .iter()
        .filter(|(id, source, target)| current.get(id) != Some(&(source.clone(), target.clone())))
        .count();
    Ok(LayoutPlan {
        unit_changes,
        metadata_anchors,
        anchor_changes,
    })
}

/// Applies a validated plan inside one transaction. Returns the total
/// number of changed fields, sidecar anchor rows included — the same count
/// mxrb's `apply!` reports.
pub fn apply_layout(path: &Path, plan: LayoutPlan) -> Result<usize, String> {
    let mut mpr = mxrs_mpr::MprFile::open(path, false).map_err(|error| error.to_string())?;
    let unit_changes: usize = plan.unit_changes.iter().map(|(_, _, count)| count).sum();
    // One transaction covers both the unit documents and the sidecar anchor
    // rows — mxrb's `LayoutWriter#apply!` wraps both the same way, so a
    // failed sidecar write can never leave a half-applied layout behind.
    let anchor_changes = mpr
        .transaction(|mpr| {
            for (unit_id, doc, _) in plan.unit_changes {
                mpr.update_unit(&unit_id, doc)?;
            }
            mpr.write_domain_diagram_anchors(&plan.metadata_anchors)
        })
        .map_err(|error| error.to_string())?;
    Ok(unit_changes + anchor_changes)
}

fn update_entities(doc: &mut Document, layout: &Value) -> Result<usize, String> {
    let layouts = layout
        .get("entities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut by_id: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for entity in &layouts {
        let id = entity
            .get("id")
            .and_then(Value::as_str)
            .ok_or("each entity layout needs an id")?;
        by_id.insert(
            id.to_string(),
            (
                bounded_integer(entity.get("x"), "x")?,
                bounded_integer(entity.get("y"), "y")?,
            ),
        );
    }
    let field = if doc.contains_key("Entities") {
        "Entities"
    } else {
        "entities"
    };
    let mut entities: Vec<mxrs_bson::Bson> = doc
        .get_array(field)
        .map(|array| array.to_vec())
        .unwrap_or_default();
    let native_ids: Vec<String> = entities
        .iter()
        .filter_map(|item| item.as_document())
        .filter_map(|entity| entity.get("$ID").and_then(mxrs_bson::extract_id))
        .collect();
    let unknown: Vec<&String> = by_id.keys().filter(|id| !native_ids.contains(id)).collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown domain entities: {}",
            unknown
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut changed = 0;
    for item in &mut entities {
        let Some(entity) = item.as_document_mut() else {
            continue;
        };
        let Some(id) = entity.get("$ID").and_then(mxrs_bson::extract_id) else {
            continue;
        };
        let Some((x, y)) = by_id.get(&id) else {
            continue;
        };
        let key = if entity.contains_key("Location") {
            "Location"
        } else {
            "location"
        };
        // Match the stored shape: a `{x, y}` sub-document stays a
        // sub-document, everything else is the native `"x;y"` string —
        // mxrb's `entity[key].is_a?(Hash)` branch.
        let stored_as_document = matches!(entity.get(key), Some(mxrs_bson::Bson::Document(_)));
        if stored_as_document {
            let value = mxrs_bson::doc! { "x": *x, "y": *y };
            if entity.get_document(key).ok() != Some(&value) {
                changed += 1;
            }
            entity.insert(key, value);
        } else {
            let value = format!("{x};{y}");
            if entity.get_str(key).ok() != Some(value.as_str()) {
                changed += 1;
            }
            entity.insert(key, value);
        }
    }
    doc.insert(field, entities);
    Ok(changed)
}

/// `(association id, source anchor, target anchor)` for the sidecar table.
type AnchorRow = (String, String, String);

fn update_associations(
    doc: &mut Document,
    layout: &Value,
) -> Result<(usize, Vec<AnchorRow>), String> {
    let layouts = layout
        .get("associations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut by_id: BTreeMap<String, (String, String)> = BTreeMap::new();
    for association in &layouts {
        let id = association
            .get("id")
            .and_then(Value::as_str)
            .ok_or("each association layout needs an id")?;
        by_id.insert(
            id.to_string(),
            (
                anchor_name(association.get("source_anchor"))?,
                anchor_name(association.get("target_anchor"))?,
            ),
        );
    }
    let field = if doc.contains_key("Associations") {
        "Associations"
    } else {
        "associations"
    };
    let cross_field = if doc.contains_key("CrossAssociations") {
        "CrossAssociations"
    } else {
        "crossAssociations"
    };
    let mut associations: Vec<mxrs_bson::Bson> = doc
        .get_array(field)
        .map(|array| array.to_vec())
        .unwrap_or_default();
    let native_ids: Vec<String> = associations
        .iter()
        .filter_map(|item| item.as_document())
        .filter_map(|a| a.get("$ID").and_then(mxrs_bson::extract_id))
        .collect();
    let cross_ids: Vec<String> = doc
        .get_array(cross_field)
        .map(|array| array.to_vec())
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item.as_document())
        .filter_map(|a| a.get("$ID").and_then(mxrs_bson::extract_id))
        .collect();
    let unknown: Vec<&String> = by_id
        .keys()
        .filter(|id| !native_ids.contains(id) && !cross_ids.contains(id))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown domain associations: {}",
            unknown
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut changed = 0;
    for item in &mut associations {
        let Some(association) = item.as_document_mut() else {
            continue;
        };
        let Some(id) = association.get("$ID").and_then(mxrs_bson::extract_id) else {
            continue;
        };
        let Some((source, target)) = by_id.get(&id) else {
            continue;
        };
        let source_value = anchor_value(source).expect("validated anchor");
        let target_value = anchor_value(target).expect("validated anchor");
        if association.get_str("ParentConnection").ok() != Some(source_value)
            || association.get_str("ChildConnection").ok() != Some(target_value)
        {
            changed += 1;
        }
        association.insert("ParentConnection", source_value);
        association.insert("ChildConnection", target_value);
    }
    doc.insert(field, associations);
    let metadata = cross_ids
        .iter()
        .filter_map(|id| {
            let (source, target) = by_id.get(id)?;
            Some((id.clone(), source.clone(), target.clone()))
        })
        .collect();
    Ok((changed, metadata))
}

fn bounded_integer(value: Option<&Value>, name: &str) -> Result<i64, String> {
    let number = value
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{name} must be an integer"))?;
    if !(-100_000..=100_000).contains(&number) {
        return Err(format!("{name} is outside the diagram canvas"));
    }
    Ok(number)
}

fn anchor_name(value: Option<&Value>) -> Result<String, String> {
    let name = value.and_then(Value::as_str).unwrap_or_default();
    if anchor_value(name).is_none() {
        return Err(format!("unknown association anchor {name:?}"));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code, non_snake_case, non_camel_case_types)]
    mod markers {
        pub mod Sales {
            pub struct Customer;
            impl mxrs_ir::EntityMarker for Customer {
                const MODULE: &'static str = "Sales";
                const NAME: &'static str = "Customer";
            }
            pub struct Order;
            impl mxrs_ir::EntityMarker for Order {
                const MODULE: &'static str = "Sales";
                const NAME: &'static str = "Order";
            }
            pub struct Order_Order_Customer;
            impl mxrs_ir::AssociationMarker for Order_Order_Customer {
                type From = Order;
                type To = Customer;
                const NAME: &'static str = "Order_Customer";
                const ASSOCIATION_TYPE: mxrs_ir::AssociationType =
                    mxrs_ir::AssociationType::Reference;
            }
            pub struct Order_Order_Account;
            impl mxrs_ir::AssociationMarker for Order_Order_Account {
                type From = Order;
                type To = super::CRM::Account;
                const NAME: &'static str = "Order_Account";
                const ASSOCIATION_TYPE: mxrs_ir::AssociationType =
                    mxrs_ir::AssociationType::Reference;
            }
        }
        pub mod CRM {
            pub struct Account;
            impl mxrs_ir::EntityMarker for Account {
                const MODULE: &'static str = "CRM";
                const NAME: &'static str = "Account";
            }
        }
    }

    fn fixture(path: &Path) {
        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.entity("Customer", |entity| {
                entity.string("Name");
            });
            module.entity("Order", |entity| {
                entity.string("Number");
                entity.association::<markers::Sales::Order_Order_Customer>();
                entity.association::<markers::Sales::Order_Order_Account>();
            });
        });
        project.module("CRM", |module| {
            module.entity("Account", |entity| {
                entity.string("Code");
            });
        });
        mxrs_writer::write_project(path, &project.build()).unwrap();
    }

    fn sales_module(payload: &Value) -> &Value {
        payload["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|module| module["name"] == "Sales")
            .unwrap()
    }

    #[test]
    fn the_projection_carries_entities_associations_and_default_anchors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Project.mpr");
        fixture(&path);
        let payload = document(&path, &[]).unwrap();
        assert_eq!(payload["module_filter_applied"], Value::Bool(false));
        let sales = sales_module(&payload);
        let entities = sales["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 2);
        let order = entities
            .iter()
            .find(|entity| entity["name"] == "Order")
            .unwrap();
        assert_eq!(order["qualified_name"], "Sales.Order");
        assert_eq!(order["kind"], "entity");
        assert_eq!(order["attributes"][0]["name"], "Number");
        assert_eq!(order["attributes"][0]["type"], "string");

        let associations = sales["associations"].as_array().unwrap();
        assert_eq!(associations.len(), 2);
        let local = associations
            .iter()
            .find(|association| association["name"] == "Order_Customer")
            .unwrap();
        assert_eq!(local["from"], "Sales.Order");
        assert_eq!(local["to"], "Sales.Customer");
        assert_eq!(local["cross_module"], Value::Bool(false));
        assert_eq!(local["anchor_storage"], "native");
        assert_eq!(local["source_anchor"], "east");
        assert_eq!(local["target_anchor"], "west");
        let cross = associations
            .iter()
            .find(|association| association["name"] == "Order_Account")
            .unwrap();
        assert_eq!(cross["to"], "CRM.Account");
        assert_eq!(cross["cross_module"], Value::Bool(true));
        assert_eq!(cross["anchor_storage"], "mxrb");

        // Module filter selects and unknown module names are refused.
        let filtered = document(&path, &["CRM".to_string()]).unwrap();
        assert_eq!(filtered["modules"].as_array().unwrap().len(), 1);
        let error = document(&path, &["Ghost".to_string()]).unwrap_err();
        assert!(error.contains("unknown modules: Ghost"), "{error}");
    }

    #[test]
    fn layout_previews_then_applies_locations_and_anchors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Project.mpr");
        fixture(&path);
        let before = document(&path, &[]).unwrap();
        let sales = sales_module(&before);
        let order_id = sales["entities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entity| entity["name"] == "Order")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let associations = sales["associations"].as_array().unwrap();
        let local_id = associations
            .iter()
            .find(|association| association["name"] == "Order_Customer")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let cross_id = associations
            .iter()
            .find(|association| association["name"] == "Order_Account")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let layout = json!({ "modules": [{
            "name": "Sales",
            "entities": [{ "id": order_id, "x": 120, "y": 240 }],
            "associations": [
                { "id": local_id, "source_anchor": "north", "target_anchor": "south" },
                { "id": cross_id, "source_anchor": "south", "target_anchor": "north" },
            ],
        }]});

        // Preview counts without writing.
        let plan = plan_layout(&path, &layout).unwrap();
        let expected = plan.changes();
        assert!(expected >= 3, "{expected}");
        let unchanged = document(&path, &[]).unwrap();
        assert_eq!(unchanged, before);

        // Apply reports the same count and the projection reflects it.
        let plan = plan_layout(&path, &layout).unwrap();
        assert_eq!(apply_layout(&path, plan).unwrap(), expected);
        let after = document(&path, &[]).unwrap();
        let sales = sales_module(&after);
        let order = sales["entities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entity| entity["name"] == "Order")
            .unwrap();
        assert_eq!(order["x"], 120);
        assert_eq!(order["y"], 240);
        let associations = sales["associations"].as_array().unwrap();
        let local = associations
            .iter()
            .find(|association| association["name"] == "Order_Customer")
            .unwrap();
        assert_eq!(local["source_anchor"], "north");
        assert_eq!(local["target_anchor"], "south");
        // The cross-module anchors round-trip through the sidecar table.
        let cross = associations
            .iter()
            .find(|association| association["name"] == "Order_Account")
            .unwrap();
        assert_eq!(cross["source_anchor"], "south");
        assert_eq!(cross["target_anchor"], "north");

        // Re-applying the identical layout changes nothing.
        let plan = plan_layout(&path, &layout).unwrap();
        assert_eq!(plan.changes(), 0);
        let plan = plan_layout(&path, &layout).unwrap();
        assert_eq!(apply_layout(&path, plan).unwrap(), 0);
    }

    #[test]
    fn layouts_are_validated_before_any_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Project.mpr");
        fixture(&path);

        let unknown_module = json!({ "modules": [{ "name": "Ghost", "entities": [] }]});
        let error = plan_layout(&path, &unknown_module).unwrap_err();
        assert!(error.contains("domain module Ghost not found"), "{error}");

        let unknown_entity = json!({ "modules": [{
            "name": "Sales",
            "entities": [{ "id": "not-a-real-id", "x": 0, "y": 0 }],
        }]});
        let error = plan_layout(&path, &unknown_entity).unwrap_err();
        assert!(error.contains("unknown domain entities"), "{error}");

        let before = document(&path, &[]).unwrap();
        let order_id = sales_module(&before)["entities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entity| entity["name"] == "Order")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let out_of_bounds = json!({ "modules": [{
            "name": "Sales",
            "entities": [{ "id": order_id, "x": 100_001, "y": 0 }],
        }]});
        let error = plan_layout(&path, &out_of_bounds).unwrap_err();
        assert!(error.contains("outside the diagram canvas"), "{error}");

        let association_id = sales_module(&before)["associations"].as_array().unwrap()[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let bad_anchor = json!({ "modules": [{
            "name": "Sales",
            "associations": [{
                "id": association_id,
                "source_anchor": "northwest",
                "target_anchor": "west",
            }],
        }]});
        let error = plan_layout(&path, &bad_anchor).unwrap_err();
        assert!(error.contains("unknown association anchor"), "{error}");
    }
}
