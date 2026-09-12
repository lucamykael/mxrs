//! Compatibility coverage for the old manifest-generated marker path plus
//! the self-hosted markers now emitted by `project!` itself.

include!(concat!(env!("OUT_DIR"), "/mxrs_markers.rs"));

use mxrs_macros::project;
use mxrs_model::Project;

#[test]
fn self_hosted_macro_markers_coexist_with_typegen_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Generated.mpr");

    let definition = project! {
        "11.12.1",
        module Sales {
            entity Customer {
                string Name;
            }
            entity Order {
                string Number = "A-0000";
                decimal Total;
                association Order_Customer -> Sales::Customer as Reference;
            }
        },
        module CRM {
            entity Account {
                string Name;
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let modules = read.modules().unwrap();
    assert_eq!(modules.len(), 2);
    let sales = modules
        .iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();

    let entities = sales.entities();
    assert_eq!(entities.len(), 2);
    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    assert!(!associations[0].is_cross_module());
    let customer = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Customer"))
        .unwrap();
    assert_eq!(
        associations[0].to_entity_id.as_deref(),
        customer.id.as_deref()
    );
}

#[test]
fn a_generated_marker_carries_the_manifests_qualified_name() {
    use mxrs_ir::EntityMarker;
    assert_eq!(Sales::Order::qualified_name(), "Sales.Order");
    assert_eq!(CRM::Account::qualified_name(), "CRM.Account");
}

/// `manifest.json` also declares `Order`'s associations (a same-module
/// `Order_Customer` and a cross-module `Order_Account`) — proving the
/// generated `AssociationMarker` impls resolve correctly closes the loop
/// on widening the manifest to describe associations, not just
/// entities/attributes.
#[test]
fn generated_association_markers_resolve_their_from_and_to() {
    use mxrs_ir::{AssociationMarker, EntityMarker};

    assert_eq!(
        <Sales::Order_Order_Customer as AssociationMarker>::From::qualified_name(),
        "Sales.Order"
    );
    assert_eq!(
        <Sales::Order_Order_Customer as AssociationMarker>::To::qualified_name(),
        "Sales.Customer"
    );
    assert_eq!(
        Sales::Order_Order_Customer::ASSOCIATION_TYPE,
        mxrs_ir::AssociationType::Reference
    );

    assert_eq!(
        <Sales::Order_Order_Account as AssociationMarker>::To::qualified_name(),
        "CRM.Account"
    );
}

#[test]
fn project_macro_carries_domain_metadata_into_the_ir() {
    let definition = project! {
        "11.12.1",
        module Sales {
            entity Customer {
                string Name;
            }
            entity Order {
                string Number {
                    documentation "External order number";
                    length 80;
                    required true;
                    unique true;
                }
                datetime SubmittedAt {
                    localize_date false;
                }
                association Order_Customer -> Sales::Customer as Reference {
                    owner Both;
                    storage Table;
                    documentation "Owning customer";
                }
            }
        }
    };

    let order = definition.modules[0]
        .entities
        .iter()
        .find(|entity| entity.name == "Order")
        .unwrap();
    let number = &order.attributes[0];
    assert_eq!(number.documentation, "External order number");
    assert_eq!(number.length, Some(80));
    assert!(number.required);
    assert!(number.unique);
    assert_eq!(order.attributes[1].localize_date, Some(false));
    let association = &order.associations[0];
    assert_eq!(association.owner, mxrs_ir::AssociationOwner::Both);
    assert_eq!(association.storage, mxrs_ir::AssociationStorage::Table);
    assert_eq!(association.documentation, "Owning customer");
}
