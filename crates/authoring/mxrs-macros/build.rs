//! Generates marker types from `tests/fixtures/manifest.json` via
//! `mxrs-typegen`, for `tests/typegen_integration.rs` — the end-to-end
//! proof that Phase 4 (manifest → marker codegen) and Phase 8
//! (`project! {}` sugar) actually compose: a real `mxrs-typegen` build.rs
//! step feeding real generated markers into `project!`'s association
//! targets, not hand-written test-only marker structs like this crate's
//! other test files use.
//!
//! Not needed by `mxrs-macros`' own library code (the proc-macro itself
//! has no manifest to generate from — it expands whatever marker path the
//! *caller* already has in scope) — this build.rs exists purely to set up
//! that one integration test.

fn main() {
    let manifest_path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/manifest.json");
    println!("cargo:rerun-if-changed={manifest_path}");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo during a build.rs run");
    let out_path = std::path::Path::new(&out_dir).join("mxrs_markers.rs");
    mxrs_typegen::generate_from_path(manifest_path, &out_path)
        .expect("tests/fixtures/manifest.json should generate valid marker source");
}
