//! A JavaScript action a module declares is written as the document Studio
//! Pro stores, and rewriting it from the same declaration changes nothing.

use mxrs_dsl::ProjectBuilder;
use mxrs_expr::{MxBool, MxString};

fn action(path: &std::path::Path) -> (String, mxrs_bson::Document) {
    let mpr = mxrs_mpr::MprFile::open(path, true).unwrap();
    mpr.all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let document = mpr.parse_contents(&unit).ok()?;
            (document.get_str("$Type").ok() == Some("JavaScriptActions$JavaScriptAction"))
                .then_some((unit.unit_id, document))
        })
        .expect("the action reached the model")
}

#[test]
fn a_javascript_action_is_written_as_studio_pro_stores_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Actions.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.javascript_action("OpenMap", |action| {
            action
                .takes::<MxString>("Address")
                .returns::<MxBool>()
                .platform(mxrs_ir::JavaScriptPlatform::Web);
        });
    });
    let declaration = project.build();
    mxrs_writer::write_project(&path, &declaration).unwrap();
    let (id, document) = action(&path);
    assert_eq!(
        document.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "$ID",
            "$Type",
            "ActionDefaultReturnName",
            "Documentation",
            "Excluded",
            "ExportLevel",
            "JavaReturnType",
            "MicroflowActionInfo",
            "Name",
            "Parameters",
            "Platform",
            "TypeParameters",
        ]
    );
    assert_eq!(document.get_str("Platform").unwrap(), "Web");
    assert_eq!(
        document
            .get_document("JavaReturnType")
            .unwrap()
            .get_str("$Type")
            .unwrap(),
        "CodeActions$BooleanType"
    );
    let parameters = document.get_array("Parameters").unwrap();
    let address = parameters[1].as_document().unwrap();
    assert_eq!(address.get_str("Name").unwrap(), "Address");
    assert!(address.get("$ID").is_some());

    // Written again from the same declaration, the stored action is kept.
    mxrs_writer::synchronize_project(&path, &declaration).unwrap();
    assert_eq!(action(&path), (id, document));
}
