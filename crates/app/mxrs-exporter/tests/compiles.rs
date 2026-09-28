//! Proves `export_project`'s output is genuinely valid, compilable Rust —
//! not just a string that happens to contain the right substrings (which
//! is all `src/lib.rs`'s own unit tests check). Spins up a real throwaway
//! `cargo build`, same rationale and pattern as `mxrs-macros`'
//! `tests/diagnostics.rs`: this is what "the round trip actually works"
//! means for generated source, and pinning a full expected-output snapshot
//! would rot on every formatting or field-ordering change.

use std::path::{Path, PathBuf};
use std::process::Output;

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .expect("workspace root contains xtask/Cargo.toml")
        .to_path_buf()
}

fn try_compile(body: &str) -> Output {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let unique: String = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let name = format!("exporter-fixture-{unique}");
    let cargo_toml = format!(
        "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nmxrs-macros = {{ path = {:?} }}\nmxrs-expr = {{ path = {:?} }}\nmxrs-ir = {{ path = {:?} }}\nmxrs-dsl = {{ path = {:?} }}\n",
        workspace_root().join("crates/authoring/mxrs-macros"),
        workspace_root().join("crates/authoring/mxrs-expr"),
        workspace_root().join("crates/authoring/mxrs-ir"),
        workspace_root().join("crates/authoring/mxrs-dsl"),
    );
    std::fs::write(dir.path().join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), body).unwrap();

    std::process::Command::new(env!("CARGO"))
        .arg("build")
        .arg("--offline")
        .current_dir(dir.path())
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace_root().join("target")),
        )
        .output()
        .expect("failed to invoke cargo for the fixture crate")
}

#[test]
fn exported_source_compiles_as_a_standalone_crate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
        m.entity("Order", |e| {
            e.persistable(true);
            e.string("Number").default_value = Some("A-0".to_string());
            e.decimal("Total");
            e.association::<markers::Order_Order_Customer>();
        });
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();

    let source = mxrs_exporter::export_project(&path).unwrap();
    let output = try_compile(&source);
    assert!(
        output.status.success(),
        "generated source failed to compile:\n{}\n---source---\n{source}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn generated_build_preserves_opaque_native_fields_and_externally_referenced_page_ids_byte_for_byte()
{
    let directory = tempfile::tempdir().unwrap();
    let original_path = directory.path().join("Original.mpr");
    let generated = directory.path().join("generated");
    let output_path = directory.path().join("Built.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        for name in ["Supported", "Metadata", "Referenced"] {
            module.page(name, |page| {
                page.layout("Atlas_Core.ApplicationLayout", "Main");
                page.text("Preserved");
            });
        }
    });
    mxrs_writer::write_project(&original_path, &builder.build()).unwrap();
    let mut mpr = mxrs_mpr::MprFile::open(&original_path, false).unwrap();
    let mut preserved_ids = Vec::new();
    for unit in mpr.all_units().unwrap() {
        let mut document = mpr.parse_contents(&unit).unwrap();
        if document.get_str("$Type").ok() != Some("Forms$Page") {
            continue;
        }
        match document.get_str("Name").unwrap() {
            "Metadata" => {
                document.insert("CanvasWidth", 1234);
                document.insert("MarkAsUsed", true);
                mpr.update_unit(&unit.unit_id, document).unwrap();
                preserved_ids.push(unit.unit_id);
            }
            "Referenced" => {
                let widget_id = document
                    .get_document("FormCall")
                    .unwrap()
                    .get_array("Arguments")
                    .unwrap()[1]
                    .as_document()
                    .unwrap()
                    .get_array("Widgets")
                    .unwrap()[1]
                    .as_document()
                    .unwrap()
                    .get("$ID")
                    .unwrap()
                    .clone();
                mpr.insert_unit(
                    &unit.container_id,
                    "Documents",
                    mxrs_bson::doc! {
                        "$Type": "Test$ExternalPageReference", "Name": "ExternalReference",
                        "WidgetReference": widget_id,
                    },
                    None,
                )
                .unwrap();
                preserved_ids.push(unit.unit_id);
            }
            _ => {}
        }
    }
    let originals: Vec<_> = preserved_ids
        .iter()
        .map(|id| {
            let unit = mpr.unit(id).unwrap().unwrap();
            (id.clone(), mpr.content_bytes(&unit).unwrap().unwrap())
        })
        .collect();
    drop(mpr);
    let imported =
        mxrs_exporter::import_cargo_project(&original_path, &generated, Some(&workspace_root()))
            .unwrap();
    assert_eq!(imported.page_export.typed_candidates, 1);
    assert_eq!(imported.page_export.opaque, 2);
    let pages_source =
        std::fs::read_to_string(generated.join("src/modules/sales/presentation/pages/mod.rs"))
            .unwrap();
    assert!(
        pages_source.contains("pub mod supported;"),
        "{pages_source}"
    );
    assert!(!pages_source.contains("metadata"), "{pages_source}");
    assert!(!pages_source.contains("referenced"), "{pages_source}");
    let output = std::process::Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(generated.join("Cargo.toml"))
        .arg("--")
        .arg(&output_path)
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace_root().join("target")),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rebuilt = mxrs_mpr::MprFile::open(&output_path, true).unwrap();
    for (id, bytes) in originals {
        let unit = rebuilt.unit(&id).unwrap().unwrap();
        assert_eq!(
            rebuilt.content_bytes(&unit).unwrap().unwrap(),
            bytes,
            "snapshot-backed page changed during generated Cargo build"
        );
    }
}

#[test]
fn keyword_and_colliding_page_function_names_compile_without_renaming_native_pages() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("Pages.mpr");
    let generated = directory.path().join("generated");
    let rebuilt = directory.path().join("Rebuilt.mpr");
    let names = [
        "Match",
        "match",
        "Gen",
        "OrderEdit",
        "order_edit",
        "order_edit_2",
    ];
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    for module_name in ["Sales", "Support"] {
        builder.module(module_name, |module| {
            module.layout("ApplicationLayout", |layout| {
                layout.placeholder("Main");
            });
            for name in names {
                module.page(name, |page| {
                    page.layout(format!("{module_name}.ApplicationLayout"), "Main")
                        .text(name);
                });
            }
        });
    }
    mxrs_writer::write_project(&original, &builder.build()).unwrap();
    let report =
        mxrs_exporter::import_cargo_project(&original, &generated, Some(&workspace_root()))
            .unwrap();
    assert_eq!(report.page_export.typed_candidates, 2 * names.len());
    let output = std::process::Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(generated.join("Cargo.toml"))
        .arg("--")
        .arg(&rebuilt)
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace_root().join("target")),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let project = mxrs_model::Project::open(&rebuilt, true).unwrap();
    let mut actual: Vec<_> = project
        .modules()
        .unwrap()
        .into_iter()
        .flat_map(|module| {
            let module_name = module.name.unwrap();
            module
                .pages
                .into_iter()
                .map(move |page| format!("{module_name}.{}", page.name.unwrap()))
        })
        .collect();
    actual.sort();
    let mut expected: Vec<_> = ["Sales", "Support"]
        .iter()
        .flat_map(|module| names.iter().map(move |name| format!("{module}.{name}")))
        .collect();
    expected.sort();
    assert_eq!(actual, expected);
}

#[allow(dead_code, non_snake_case, non_camel_case_types)]
mod markers {
    pub struct Customer;
    impl mxrs_ir::EntityMarker for Customer {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "Customer";
    }
    pub struct Order;
    impl mxrs_ir::EntityMarker for Order {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "Order";
    }
    pub struct Order_Order_Customer;
    impl mxrs_ir::AssociationMarker for Order_Order_Customer {
        type From = Order;
        type To = Customer;
        const NAME: &'static str = "Order_Customer";
        const ASSOCIATION_TYPE: mxrs_ir::AssociationType = mxrs_ir::AssociationType::Reference;
    }
}
