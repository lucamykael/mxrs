//! `mxrs-bson::{parse, serialize}` is the single busiest hot path in the
//! whole workspace: every unit `mxrs-mpr` reads or writes funnels through
//! it. Performance is this project's whole pitch over `mxrb` (Ruby) — see
//! `decisions/mxrs-rust-rewrite-plan.md` in this project's ai-memory — but
//! that pitch had never actually been measured anywhere in the codebase
//! before this bench existed. This is a first slice, not full coverage:
//! one representative document shape, not a corpus. Widen alongside
//! whichever crate's hot path needs its own measurement next (a real
//! `.mpr`-sized round trip via `mxrs-mpr`, a `mxrs-compiler-flow` compile
//! pass, ...) rather than trying to cover everything in one bench file.
//!
//! Run with `cargo bench -p mxrs-bson`.

use divan::Bencher;
use mxrs_bson::{Binary, BinarySubtype, Bson, Document, doc, parse, serialize};

fn main() {
    divan::main();
}

/// A domain-model entity document shaped like real Mendix `.mpr` content:
/// a GUID-valued `$ID` binary, nested attribute sub-documents, and a
/// marker-prefixed array — the three conventions `mxrs-bson` exists to
/// handle (see the crate's own top-level doc comment) — rather than a
/// trivial flat document that would under-represent real parsing cost.
fn sample_entity_document() -> Document {
    let id = Binary {
        subtype: BinarySubtype::Generic,
        bytes: vec![0u8; 16],
    };
    let attribute = |name: &str| {
        doc! {
            "$ID": Binary { subtype: BinarySubtype::Generic, bytes: vec![1u8; 16] },
            "$Type": "DomainModels$Attribute",
            "Name": name,
            "Documentation": "",
            "Type": { "$ID": Binary { subtype: BinarySubtype::Generic, bytes: vec![2u8; 16] }, "$Type": "DomainModels$StringAttributeType", "Length": 200i32 },
        }
    };
    doc! {
        "$ID": id,
        "$Type": "DomainModels$Entity",
        "Name": "Order",
        "Documentation": "",
        "Attributes": [
            Bson::Int32(1),
            Bson::Document(attribute("Number")),
            Bson::Document(attribute("Status")),
            Bson::Document(attribute("Total")),
            Bson::Document(attribute("Notes")),
        ],
    }
}

#[divan::bench]
fn serialize_entity(bencher: Bencher) {
    let document = sample_entity_document();
    bencher.bench(|| serialize(divan::black_box(&document)).unwrap());
}

#[divan::bench]
fn parse_entity(bencher: Bencher) {
    let bytes = serialize(&sample_entity_document()).unwrap();
    bencher.bench(|| parse(divan::black_box(&bytes)).unwrap());
}

#[divan::bench]
fn round_trip_entity(bencher: Bencher) {
    let document = sample_entity_document();
    bencher.bench(|| {
        let bytes = serialize(divan::black_box(&document)).unwrap();
        parse(divan::black_box(&bytes)).unwrap()
    });
}
