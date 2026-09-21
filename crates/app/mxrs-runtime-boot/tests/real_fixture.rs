//! Boots the real mxrb-generated fixture projects end to end — the same
//! `.mpr` files the oracle-diff gate runs the real mxrb against — proving
//! the whole path: MPR open, module decoding, `Security$ProjectSecurity`
//! discovery, schema and policy derivation, runtime construction.

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../xtask/fixtures/{name}/{name}.mpr"))
}

#[test]
fn boots_the_real_fixture_projects_end_to_end() {
    for name in ["minimal", "with_page"] {
        let path = fixture(name);
        let boot = mxrs_runtime_boot::boot(&path).unwrap_or_else(|error| {
            panic!("{name}: {error}");
        });

        // The derived inventory matches what the model itself declares.
        let project = mxrs_model::Project::open(&path, true).unwrap();
        let modules = project.modules().unwrap();
        let named_entities: usize = modules
            .iter()
            .map(|module| {
                module
                    .entities()
                    .iter()
                    .filter(|entity| entity.name.is_some())
                    .count()
            })
            .sum();
        assert_eq!(boot.entities, named_entities, "{name}");
        let named_flows: usize = modules
            .iter()
            .map(|module| {
                module
                    .microflows
                    .iter()
                    .chain(&module.nanoflows)
                    .chain(&module.rules)
                    .filter(|flow| flow.name.is_some())
                    .count()
            })
            .sum();
        assert!(boot.documents <= named_flows, "{name}");

        // mxrb scaffolds projects at SecurityLevel CheckNothing; the derived
        // policy must agree instead of failing closed on a healthy fixture.
        assert!(!boot.security.enabled, "{name}");
        assert!(boot.skipped_xpath_rules.is_empty(), "{name}");

        // The derived schema and policy boot an actual runtime.
        let store = mxrs_runtime::Store::new(boot.schema.clone());
        let _runtime = mxrs_runtime::Runtime::new(store, boot.security.clone());
    }
}
