use std::path::PathBuf;

use mxrs_schema::ModelPackage;

#[test]
#[ignore = "requires an explicit MXRS_MODEL_PACKAGE_ORACLE private corpus path"]
fn rewrites_a_real_model_package_byte_identically() {
    let source = std::env::var_os("MXRS_MODEL_PACKAGE_ORACLE")
        .map(PathBuf::from)
        .expect("set MXRS_MODEL_PACKAGE_ORACLE to an existing official deployment/model/model.mdp before explicitly running this ignored test");
    assert!(
        source.is_file(),
        "MXRS_MODEL_PACKAGE_ORACLE is not a readable model package file: {}",
        source.display()
    );

    let package = ModelPackage::read(&source).expect("read real model.mdp");
    assert!(
        !package.entries().is_empty(),
        "real model.mdp must not be empty"
    );
    let directory = tempfile::tempdir().expect("create oracle output directory");
    let output = directory.path().join("model.mdp");
    package.write(&output).expect("rewrite real model.mdp");

    assert_eq!(
        std::fs::read(&output).expect("read rewritten model.mdp"),
        std::fs::read(&source).expect("read source model.mdp"),
        "decode/encode must preserve the full ordered BSON stream byte-identically"
    );
    println!(
        "model package oracle: {} ordered entries round-tripped byte-identically",
        package.entries().len()
    );
}
