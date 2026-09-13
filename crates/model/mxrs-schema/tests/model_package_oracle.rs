use std::path::PathBuf;

use mxrs_schema::ModelPackage;

#[test]
#[ignore = "requires MXRS_MODEL_PACKAGE_ORACLE or a local SPC deployment"]
fn rewrites_a_real_model_package_byte_identically() {
    let source = std::env::var_os("MXRS_MODEL_PACKAGE_ORACLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                "/home/mykael/Personal_Projects/spc-zero-errors-ruby/build/deployment/model/model.mdp",
            )
        });
    if !source.is_file() {
        eprintln!(
            "model package oracle skipped: {} is unavailable",
            source.display()
        );
        return;
    }

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
}
