use std::fs;

use mxrs_bson::{Bson, doc};
use mxrs_dsl::ProjectBuilder;
use mxrs_ir::Ref;
use mxrs_model::association::AssociationType;
use mxrs_model::attribute::AttributeType;
use mxrs_mpr::MprFile;

struct Detail;

impl mxrs_ir::EntityMarker for Detail {
    const MODULE: &'static str = "Demo";
    const NAME: &'static str = "Detail";
}

#[test]
fn generates_every_referenced_proxy_family_without_overwriting_files() {
    let temp = tempfile::tempdir().unwrap();
    let mpr_path = temp.path().join("Demo.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Demo", |module| {
        module.entity("Record", |entity| {
            entity.string("Name");
            let state = entity.string("State");
            state.attribute_type = AttributeType::Enum;
            state.enumeration = Some("Demo.State".into());
            entity.association(
                "Record_Detail",
                Ref::<Detail>::new(),
                AssociationType::Reference,
            );
        });
        module.entity("Detail", |entity| {
            entity.integer("Number");
        });
        module.entity("Unused", |_| {});
        module.microflow("Run", |_| {});
    });
    mxrs_writer::write_project(&mpr_path, &project.build()).unwrap();

    let module_id = {
        let mpr = MprFile::open(&mpr_path, true).unwrap();
        mpr.units_by_containment("Modules")
            .unwrap()
            .into_iter()
            .find(|unit| {
                mpr.parse_contents(unit)
                    .ok()
                    .and_then(|document| document.get_str("Name").ok().map(str::to_string))
                    .as_deref()
                    == Some("Demo")
            })
            .unwrap()
            .unit_id
    };
    let mut mpr = MprFile::open(&mpr_path, false).unwrap();
    mpr.insert_unit(
        &module_id,
        "Documents",
        doc! {
            "$ID": "11111111-1111-4111-8111-111111111111",
            "$Type": "Enumerations$Enumeration",
            "Name": "State",
            "Values": mxrs_bson::build_array(vec![
                Bson::Document(doc! { "$ID": "22222222-2222-4222-8222-222222222222", "$Type": "Enumerations$EnumerationValue", "Name": "Open" }),
                Bson::Document(doc! { "$ID": "33333333-3333-4333-8333-333333333333", "$Type": "Enumerations$EnumerationValue", "Name": "Closed" }),
            ], 3),
        },
        None,
    )
    .unwrap();
    mpr.insert_unit(
        &module_id,
        "Documents",
        doc! {
            "$ID": "44444444-4444-4444-8444-444444444444",
            "$Type": "Constants$Constant",
            "Name": "Limit",
            "Type": { "$ID": "55555555-5555-4555-8555-555555555555", "$Type": "DataTypes$IntegerType" },
        },
        None,
    )
    .unwrap();
    mpr.insert_unit(
        &module_id,
        "Documents",
        doc! {
            "$ID": "66666666-6666-4666-8666-666666666666",
            "$Type": "JavaActions$JavaAction",
            "Name": "Publish",
        },
        None,
    )
    .unwrap();
    drop(mpr);

    let action_path = temp.path().join("javasource/demo/actions/Publish.java");
    fs::create_dir_all(action_path.parent().unwrap()).unwrap();
    fs::write(
        &action_path,
        "package demo.actions; public class Publish {\n\
         demo.proxies.Record record; demo.proxies.State state;\n\
         Object limit = demo.proxies.constants.Constants.getLimit();\n\
         void run() { demo.proxies.microflows.Microflows.run(null); }\n\
         }",
    )
    .unwrap();
    fs::write(
        temp.path().join("javasource/demo/LegacyEncoding.java"),
        b"class LegacyEncoding { // \xff\n}",
    )
    .unwrap();

    let generator = mxrs_javagen::JavaProxyGenerator::new(&mpr_path, temp.path()).unwrap();
    assert!(
        generator
            .indexed_entity_names()
            .contains(&"Demo.Record".to_string()),
        "indexed={:?}",
        generator.indexed_entity_names()
    );
    let generated = generator.generate().unwrap();

    let record_path = temp.path().join("javasource/demo/proxies/Record.java");
    let generated_paths: Vec<_> = fs::read_dir(temp.path().join("javasource/demo/proxies"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        record_path.is_file(),
        "generated={generated}, paths={generated_paths:?}"
    );
    let record = fs::read_to_string(record_path).unwrap();
    assert!(record.contains("class Record"));
    assert!(record.contains("demo.proxies.State getState"));
    assert!(record.contains("demo.proxies.Detail getRecord_Detail"));
    assert!(
        temp.path()
            .join("javasource/demo/proxies/Detail.java")
            .is_file()
    );
    assert!(
        !temp
            .path()
            .join("javasource/demo/proxies/Unused.java")
            .exists()
    );
    assert!(
        fs::read_to_string(temp.path().join("javasource/demo/proxies/State.java"))
            .unwrap()
            .contains("Open, Closed")
    );
    assert!(
        fs::read_to_string(
            temp.path()
                .join("javasource/demo/proxies/constants/Constants.java")
        )
        .unwrap()
        .contains("getLimit")
    );
    assert!(
        fs::read_to_string(
            temp.path()
                .join("javasource/demo/proxies/microflows/Microflows.java")
        )
        .unwrap()
        .contains("Core.microflowCall(\"Demo.Run\")")
    );
    assert!(
        fs::read_to_string(
            temp.path()
                .join("javasource/system/UserActionsRegistrar.java")
        )
        .unwrap()
        .contains("registrator.registerUserAction(demo.actions.Publish.class)")
    );
    assert_eq!(generated, 6);

    fs::write(
        temp.path().join("javasource/demo/proxies/Record.java"),
        "user owned",
    )
    .unwrap();
    assert_eq!(generator.generate().unwrap(), 0);
    assert_eq!(
        fs::read_to_string(temp.path().join("javasource/demo/proxies/Record.java")).unwrap(),
        "user owned"
    );
}

#[test]
fn generates_a_referenced_system_entity_proxy_from_the_embedded_seed() {
    let temp = tempfile::tempdir().unwrap();
    let mpr_path = temp.path().join("Demo.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Demo", |_| {});
    mxrs_writer::write_project(&mpr_path, &project.build()).unwrap();
    let source = temp.path().join("javasource/demo/UseSystem.java");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(source, "class UseSystem { system.proxies.User user; }").unwrap();

    let generator = mxrs_javagen::JavaProxyGenerator::new(&mpr_path, temp.path()).unwrap();
    assert!(generator.generate().unwrap() >= 1);
    assert!(
        temp.path()
            .join("javasource/system/proxies/User.java")
            .is_file()
    );
}
