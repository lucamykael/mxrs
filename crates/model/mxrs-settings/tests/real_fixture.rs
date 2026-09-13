//! Validates the settings codec against a real mxrb-generated `.mpr`
//! (`xtask/fixtures/minimal`, produced entirely by mxrb itself — see
//! `decisions/mxrs-rust-rewrite-plan.md`'s Phase 1 closure), not just
//! synthetic fixtures.
//!
//! Decoding then re-encoding (with the original as baseline) doesn't come
//! out byte-identical to the raw parsed document on its own: `$ID` fields
//! are BINARY_UUID_KEYs, converted from string to a binary blob only by
//! `mxrs_bson::storage_hash` — the same transform `mxrs-mpr`'s
//! `serialize_contents` applies before writing. Applying that transform
//! here is what a real read-modify-write round trip through the full mxrs
//! stack would do, and confirms byte-for-byte fidelity against mxrb's own
//! output.

use std::path::PathBuf;

use mxrs_mpr::MprFile;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../xtask/fixtures/minimal/minimal.mpr")
}

#[test]
fn round_trips_the_real_project_settings_unit_byte_identical() {
    let mpr = MprFile::open(fixture_path(), true).unwrap();
    let unit = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find(|u| {
            mpr.parse_contents(u)
                .map(|d| d.get_str("$Type").ok() == Some("Settings$ProjectSettings"))
                .unwrap_or(false)
        })
        .expect("fixture must contain a Settings$ProjectSettings unit");
    let original = mpr.parse_contents(&unit).unwrap();

    let model = mxrs_settings::decode(&original).unwrap();
    let encoded = mxrs_settings::encode(&model, Some(&original)).unwrap();
    let transformed = mxrs_bson::storage_hash(&encoded, true);

    assert_eq!(transformed, original);
}
