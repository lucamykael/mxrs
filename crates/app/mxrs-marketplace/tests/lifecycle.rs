//! Install-record-remove round trip: an applied install must be fully
//! described by the lockfile (cached archive, owned files, pre-install
//! backups), and removal must verify that record against reality before
//! deleting anything — then actually restore what the install replaced.

use std::io::Write;
use std::path::{Path, PathBuf};

use mxrs_marketplace::MarketplaceError;
use mxrs_marketplace::lifecycle::{OfficialProvenance, install_module, plan_remove};
use mxrs_marketplace::lock::read_lock;

const SUPPORTED_VERSION: &str = "11.12.1";

fn next_id() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn walk(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// Same fake-package construction as `tests/install.rs` — a real
/// `mxrs-writer` project zipped with a manifest and assets.
fn package(directory: &Path, module_name: &str, assets: &[(&str, &str)]) -> PathBuf {
    let unique = next_id();
    let build_dir = directory.join(format!("build-{unique}"));
    std::fs::create_dir_all(&build_dir).unwrap();
    let project_path = build_dir.join("project.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module(module_name, |module| {
        module.entity("Thing", |entity| {
            entity.string("Name");
        });
    });
    mxrs_writer::write_project(&project_path, &builder.build()).unwrap();

    let files: String = assets
        .iter()
        .map(|(path, _)| format!("      <file path=\"{path}\" />\n"))
        .collect();
    let package_xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <package xmlns=\"http://www.mendix.com/package/1.0/\">\n\
         \x20 <modelerProject xmlns=\"http://www.mendix.com/modelerProject/1.0/\">\n\
         \x20   <module name=\"{module_name}\" />\n\
         \x20   <projectFile path=\"project.mpr\" />\n\
         \x20   <files>\n{files}    </files>\n\
         \x20 </modelerProject>\n\
         </package>\n"
    );

    let archive_path = directory.join(format!("{module_name}-{unique}.mpk"));
    let file = std::fs::File::create(&archive_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("package.xml", options).unwrap();
    zip.write_all(package_xml.as_bytes()).unwrap();
    zip.start_file("project.mpr", options).unwrap();
    zip.write_all(&std::fs::read(&project_path).unwrap())
        .unwrap();
    let sidecar = build_dir.join("mprcontents");
    for file in walk(&sidecar) {
        let relative = file.strip_prefix(&build_dir).unwrap();
        zip.start_file(relative.to_str().unwrap().replace('\\', "/"), options)
            .unwrap();
        zip.write_all(&std::fs::read(&file).unwrap()).unwrap();
    }
    for (path, contents) in assets {
        zip.start_file(*path, options).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    archive_path
}

fn target(directory: &Path) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join("Target.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module("Existing", |module| {
        module.entity("Order", |entity| {
            entity.string("Number");
        });
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();
    path
}

fn module_names(path: &Path) -> Vec<String> {
    let project = mxrs_model::Project::open(path, true).unwrap();
    project
        .modules()
        .unwrap()
        .into_iter()
        .filter_map(|module| module.name)
        .collect()
}

#[test]
fn an_applied_install_is_recorded_and_removal_restores_the_project() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // A pre-existing project file the package will overwrite: removal must
    // bring back these exact bytes.
    let shared_asset = root.join("vendorlib/toolkit.jar");
    std::fs::create_dir_all(shared_asset.parent().unwrap()).unwrap();
    std::fs::write(&shared_asset, "the original jar").unwrap();

    let archive = package(
        root,
        "Toolkit",
        &[
            ("javasource/toolkit/Helper.java", "class Helper {}"),
            ("vendorlib/toolkit.jar", "the packaged jar"),
        ],
    );
    let mpr = target(root);

    let report = install_module(
        &archive,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();
    assert_eq!(report.module_name, "Toolkit");
    assert!(module_names(&mpr).contains(&"Toolkit".to_string()));
    assert_eq!(
        std::fs::read_to_string(&shared_asset).unwrap(),
        "the packaged jar"
    );

    // The lock records identity, owned files, cache, and the backup taken
    // of the overwritten asset.
    let lock = read_lock(root).unwrap();
    let entry = lock.packages.get("Toolkit").unwrap();
    assert_eq!(entry.kind, "module");
    assert_eq!(entry.units, report.units);
    assert!(entry.files.contains(&"vendorlib/toolkit.jar".to_string()));
    assert!(root.join(&entry.archive).is_file());
    let backup = entry.asset_originals["vendorlib/toolkit.jar"]
        .clone()
        .expect("overwritten asset was backed up");
    assert_eq!(
        std::fs::read_to_string(root.join(&backup)).unwrap(),
        "the original jar"
    );
    assert_eq!(
        entry.asset_originals["javasource/toolkit/Helper.java"],
        None
    );

    // Preview does not mutate.
    let plan = plan_remove(root, "toolkit", None).unwrap();
    assert!(plan.safe(), "{:?}", plan.blockers);
    assert!(module_names(&mpr).contains(&"Toolkit".to_string()));

    plan.apply().unwrap();
    assert!(!module_names(&mpr).contains(&"Toolkit".to_string()));
    assert!(!root.join("javasource/toolkit/Helper.java").exists());
    assert_eq!(
        std::fs::read_to_string(&shared_asset).unwrap(),
        "the original jar"
    );
    assert!(read_lock(root).unwrap().packages.is_empty());
    assert!(
        !root
            .join(".mxrs/marketplace")
            .join("Toolkit-unknown.mpk")
            .exists()
    );
}

#[test]
fn removal_refuses_unknown_packages_and_blocks_on_local_edits_and_references() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let archive = package(root, "Toolkit", &[("javasource/toolkit/H.java", "x")]);
    let mpr = target(root);
    install_module(
        &archive,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();

    assert!(matches!(
        plan_remove(root, "Ghost", None),
        Err(MarketplaceError::NotInstalled(_))
    ));

    // A locally edited package asset blocks removal instead of being
    // silently destroyed.
    std::fs::write(root.join("javasource/toolkit/H.java"), "edited!").unwrap();
    let plan = plan_remove(root, "Toolkit", None).unwrap();
    assert!(!plan.safe());
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("package asset changed or missing")),
        "{:?}",
        plan.blockers
    );
    assert!(plan.apply().is_err());
    assert!(module_names(&mpr).contains(&"Toolkit".to_string()));
    std::fs::write(root.join("javasource/toolkit/H.java"), "x").unwrap();

    // A document outside the module that references one of its units blocks
    // removal too.
    let module_id = read_lock(root).unwrap().packages["Toolkit"]
        .module_id
        .clone();
    {
        let mut target = mxrs_mpr::MprFile::open(&mpr, false).unwrap();
        let root_unit = target.root_unit().unwrap().unwrap().unit_id;
        let mut document = mxrs_bson::Document::new();
        document.insert("$Type", "Tests$Reference");
        document.insert("Name", "PointsAtToolkit");
        document.insert("Target", module_id);
        target
            .insert_unit(&root_unit, "Documents", document, None)
            .unwrap();
    }
    let plan = plan_remove(root, "Toolkit", None).unwrap();
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("PointsAtToolkit")),
        "{:?}",
        plan.blockers
    );

    // A wrong --mpr never matches the lock.
    assert!(plan_remove(root, "Toolkit", Some(Path::new("/tmp/other.mpr"))).is_err());
}
