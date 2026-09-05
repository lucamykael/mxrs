//! Property-based round-trip tests, per the mxrs testing strategy
//! (`decisions/mxrs-rust-rewrite-plan.md`): `encode ∘ decode == id` for the
//! codec's core conversions.

use bson::{doc, Bson};
use mxrs_bson::{blob_to_uuid, build_array, parse_array, uuid_to_blob};
use proptest::prelude::*;

fn arb_uuid() -> impl Strategy<Value = String> {
    any::<[u8; 16]>().prop_map(|bytes| blob_to_uuid(&bytes).unwrap())
}

proptest! {
    #[test]
    fn uuid_to_blob_to_uuid_round_trips(uuid in arb_uuid()) {
        let blob = uuid_to_blob(&uuid).unwrap();
        prop_assert_eq!(blob_to_uuid(&blob).unwrap(), uuid);
    }

    #[test]
    fn blob_to_uuid_to_blob_round_trips(blob in any::<[u8; 16]>()) {
        let uuid = blob_to_uuid(&blob).unwrap();
        prop_assert_eq!(uuid_to_blob(&uuid).unwrap(), blob);
    }

    #[test]
    fn array_marker_round_trips(marker in any::<i32>(), n in 0usize..8) {
        let items: Vec<Bson> = (0..n).map(|i| Bson::String(format!("item-{i}"))).collect();
        let built = build_array(items.clone(), marker);
        let parsed = parse_array(Some(&built));
        prop_assert_eq!(parsed.marker, marker);
        prop_assert_eq!(parsed.items, items);
    }

    #[test]
    fn bson_document_parse_serialize_round_trips(id in arb_uuid(), total in any::<i32>()) {
        let original = doc! { "$ID": id, "$Type": "DomainModels$Entity", "Total": total };
        let bytes = mxrs_bson::serialize(&original).unwrap();
        let parsed = mxrs_bson::parse(&bytes).unwrap();
        prop_assert_eq!(parsed, original);
    }

    #[test]
    fn storage_hash_is_idempotent(id in arb_uuid(), guid in arb_uuid()) {
        // Applying storage_hash twice must be a no-op: it's a projection,
        // not a stateful transform, so re-running it on its own output
        // must not shuffle keys or re-wrap already-binary GUID fields.
        let input = doc! { "Name": "Order", "$Type": "DomainModels$Entity", "$ID": id, "GUID": guid };
        let once = mxrs_bson::storage_hash(&input, false);
        let twice = mxrs_bson::storage_hash(&once, false);
        prop_assert_eq!(once, twice);
    }
}
