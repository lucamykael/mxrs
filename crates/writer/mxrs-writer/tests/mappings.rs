//! JSON structures and the mappings of them, written from their
//! declarations: a structure as the elements its snippet derives, a mapping
//! as its structure's tree with the entity and attribute of each element.

use mxrs_dsl::ProjectBuilder;
use mxrs_model::Project;

fn documents(path: &std::path::Path, ty: &str) -> Vec<mxrs_bson::Document> {
    let project = Project::open(path, true).unwrap();
    project
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .filter(|document| document.get_str("$Type").ok() == Some(ty))
        .collect()
}

fn declare(module_name: &str, project: &mut ProjectBuilder) {
    project.module("Integration", |module| {
        module.json_structure(
            "JSON_Order",
            r#"{"number": "A-1", "lines": [{"sku": "S", "qty": 2}]}"#,
            |json| {
                json.element("(Object)|number", |element| {
                    element.max_length(20);
                });
            },
        );
    });
    project.module(module_name, |module| {
        module.import_mapping("IMM_Order", "Integration.JSON_Order", |mapping| {
            mapping.object("(Object)", "Sales.Order", |order| {
                order.value("number").key();
                order.array("lines", |lines| {
                    lines.object("(Object)", "Sales.Line", |line| {
                        line.value("qty").attribute("Quantity");
                    });
                });
            });
        });
    });
}

/// A mapping of another module's structure is written once that structure
/// is: each element its structure's, with what the declaration maps it to.
#[test]
fn a_mapping_is_written_from_the_structure_it_maps() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("App.mpr");
    let declaration = || {
        let mut project = ProjectBuilder::new("11.12.1");
        declare("Sales", &mut project);
        project.build()
    };
    mxrs_writer::write_project(&path, &declaration()).unwrap();

    let structure = &documents(&path, "JsonStructures$JsonStructure")[0];
    let number = structure.get_array("Elements").unwrap()[1]
        .as_document()
        .unwrap()
        .get_array("Children")
        .unwrap()[1]
        .as_document()
        .unwrap()
        .clone();
    assert_eq!(number.get_i64("MaxLength").unwrap(), 20);

    let mapping = &documents(&path, "ImportMappings$ImportMapping")[0];
    assert_eq!(
        mapping.get_str("JsonStructure").unwrap(),
        "Integration.JSON_Order"
    );
    let root = mapping.get_array("Elements").unwrap()[1]
        .as_document()
        .unwrap();
    assert_eq!(root.get_str("Entity").unwrap(), "Sales.Order");
    let children = root.get_array("Children").unwrap();
    let number = children[1].as_document().unwrap();
    assert_eq!(number.get_str("Attribute").unwrap(), "Sales.Order.Number");
    assert!(number.get_bool("IsKey").unwrap());
    assert_eq!(number.get_i64("MaxLength").unwrap(), 20);
    let line = children[2]
        .as_document()
        .unwrap()
        .get_array("Children")
        .unwrap()[1]
        .as_document()
        .unwrap();
    assert_eq!(line.get_str("Association").unwrap(), "Sales.Line_Order");
    let quantity = line.get_array("Children").unwrap()[1]
        .as_document()
        .unwrap();
    assert_eq!(
        quantity.get_str("Attribute").unwrap(),
        "Sales.Line.Quantity"
    );

    // A second build of the same declarations changes nothing.
    let before = documents(&path, "ImportMappings$ImportMapping");
    mxrs_writer::synchronize_project(&path, &declaration()).unwrap();
    assert_eq!(documents(&path, "ImportMappings$ImportMapping"), before);
}

/// A mapping of a structure the model does not have is refused, naming it.
#[test]
fn a_mapping_of_no_structure_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("App.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |module| {
        module.import_mapping("IMM_Order", "Integration.JSON_Missing", |mapping| {
            mapping.object("(Object)", "Sales.Order", |_| {});
        });
    });
    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Sales.IMM_Order: the model has no JSON structure Integration.JSON_Missing"
    );
}
