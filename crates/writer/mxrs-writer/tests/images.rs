//! Image collections, written from their declarations: each image its
//! bytes and format, and a second build of the same declarations changes
//! nothing.

use mxrs_dsl::ProjectBuilder;
use mxrs_ir::ImageFormat;
use mxrs_model::Project;

const PIXEL: &[u8] = b"\x89PNG\r\n\x1a\n-pixel-";

fn collections(path: &std::path::Path) -> Vec<mxrs_bson::Document> {
    let project = Project::open(path, true).unwrap();
    project
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .filter(|document| document.get_str("$Type").ok() == Some("Images$ImageCollection"))
        .collect()
}

#[test]
fn an_image_collection_is_written_with_its_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("App.mpr");
    let declaration = || {
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.image_collection("Images", |images| {
                images.image("logo", ImageFormat::Svg, b"<svg/>").image(
                    "pixel",
                    ImageFormat::Png,
                    PIXEL,
                );
            });
        });
        project.build()
    };
    mxrs_writer::write_project(&path, &declaration()).unwrap();
    let stored = collections(&path);
    assert_eq!(stored.len(), 1);
    let images = stored[0].get_array("Images").unwrap();
    assert_eq!(images[0], mxrs_bson::Bson::Int32(3));
    let pixel = images[2].as_document().unwrap();
    assert_eq!(pixel.get_str("Name").unwrap(), "pixel");
    assert_eq!(pixel.get_str("ImageFormat").unwrap(), "Png");
    let mxrs_bson::Bson::Binary(bytes) = pixel.get("Image").unwrap() else {
        panic!("an image is its bytes");
    };
    assert_eq!(bytes.bytes, PIXEL);

    mxrs_writer::synchronize_project(&path, &declaration()).unwrap();
    assert_eq!(collections(&path), stored);
}
