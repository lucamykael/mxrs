//! Install-record-remove round trip: an applied install must be fully
//! described by the lockfile (cached archive, owned files, pre-install
//! backups), and removal must verify that record against reality before
//! deleting anything — then actually restore what the install replaced.

use std::io::Write;
use std::path::{Path, PathBuf};

use mxrs_marketplace::MarketplaceError;
use mxrs_marketplace::lifecycle::{OfficialProvenance, install_module, plan_remove, plan_update};
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

/// Builds a real `mxrs-writer` project for `module_name`, without zipping
/// it yet — split out from [`package`] so an update test can zip the SAME
/// built project twice with different declared assets, keeping the
/// module's identity (`$ID`) fixed across "versions" the way a real
/// version bump with no model changes does.
fn build_project(directory: &Path, module_name: &str) -> (PathBuf, PathBuf) {
    let build_dir = directory.join(format!("build-{}", next_id()));
    std::fs::create_dir_all(&build_dir).unwrap();
    let project_path = build_dir.join("project.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module(module_name, |module| {
        module.entity("Thing", |entity| {
            entity.string("Name");
        });
    });
    mxrs_writer::write_project(&project_path, &builder.build()).unwrap();
    (build_dir, project_path)
}

/// Zips an already-built project (see [`build_project`]) as a `.mpk` with
/// the given declared assets.
fn zip_package(
    directory: &Path,
    module_name: &str,
    build_dir: &Path,
    project_path: &Path,
    assets: &[(&str, &str)],
) -> PathBuf {
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

    let archive_path = directory.join(format!("{module_name}-{}.mpk", next_id()));
    let file = std::fs::File::create(&archive_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("package.xml", options).unwrap();
    zip.write_all(package_xml.as_bytes()).unwrap();
    zip.start_file("project.mpr", options).unwrap();
    zip.write_all(&std::fs::read(project_path).unwrap())
        .unwrap();
    let sidecar = build_dir.join("mprcontents");
    for file in walk(&sidecar) {
        let relative = file.strip_prefix(build_dir).unwrap();
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

/// Same fake-package construction as `tests/install.rs` — a real
/// `mxrs-writer` project zipped with a manifest and assets.
fn package(directory: &Path, module_name: &str, assets: &[(&str, &str)]) -> PathBuf {
    let (build_dir, project_path) = build_project(directory, module_name);
    zip_package(directory, module_name, &build_dir, &project_path, assets)
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

#[test]
fn a_package_may_not_overwrite_assets_owned_by_another_locked_package() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root);
    let first = package(
        root,
        "Toolkit",
        &[("vendorlib/shared.jar", "toolkit's jar")],
    );
    install_module(
        &first,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();

    // A second package shipping the same file is refused at plan time.
    let second = package(root, "Other", &[("vendorlib/shared.jar", "other's jar")]);
    let error = install_module(
        &second,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap_err();
    assert!(
        matches!(error, MarketplaceError::ProtectedPackagePath(ref path) if path == "vendorlib/shared.jar"),
        "{error}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("vendorlib/shared.jar")).unwrap(),
        "toolkit's jar"
    );
    // The guard never blocks the owner itself: reinstalling Toolkit fails
    // only because its module already exists in the target MPR, not on the
    // protected path.
    let error = install_module(
        &first,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap_err();
    assert!(
        !matches!(error, MarketplaceError::ProtectedPackagePath(_)),
        "{error}"
    );
}

#[test]
fn an_update_replaces_units_keeps_shared_assets_and_restores_obsolete_ones() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root);
    // A file present in both versions, and a per-version file that changes.
    let shared_asset = root.join("vendorlib/shared.jar");
    std::fs::create_dir_all(shared_asset.parent().unwrap()).unwrap();
    std::fs::write(&shared_asset, "pre-existing before either version").unwrap();

    let (build_dir, project_path) = build_project(root, "Toolkit");
    let v1 = zip_package(
        root,
        "Toolkit",
        &build_dir,
        &project_path,
        &[
            ("vendorlib/shared.jar", "shared v1"),
            ("javasource/toolkit/Old.java", "class Old {}"),
        ],
    );
    install_module(
        &v1,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance {
            content_id: Some("999".into()),
            version: Some("1.0.0".into()),
            source: Some("mendix".into()),
            ..OfficialProvenance::default()
        },
    )
    .unwrap();
    let old_cache = read_lock(root).unwrap().packages["Toolkit"].archive.clone();

    // v2 keeps the same shared asset (unchanged content this time) and
    // drops Old.java in favor of New.java.
    let v2 = zip_package(
        root,
        "Toolkit",
        &build_dir,
        &project_path,
        &[
            ("vendorlib/shared.jar", "shared v1"),
            ("javasource/toolkit/New.java", "class New {}"),
        ],
    );
    let provenance = OfficialProvenance {
        content_id: Some("999".into()),
        version: Some("2.0.0".into()),
        source: Some("mendix".into()),
        ..OfficialProvenance::default()
    };
    let plan = plan_update(root, "Toolkit", &v2, "2.0.0", provenance).unwrap();
    assert!(plan.safe(), "{:?}", plan.blockers);
    assert_eq!(plan.installed_version.as_deref(), Some("1.0.0"));
    assert_eq!(plan.target_version, "2.0.0");
    plan.apply().unwrap();

    assert!(root.join("javasource/toolkit/New.java").is_file());
    assert!(
        !root.join("javasource/toolkit/Old.java").exists(),
        "obsolete asset was removed"
    );
    assert_eq!(
        std::fs::read_to_string(&shared_asset).unwrap(),
        "shared v1",
        "an asset present in both versions is left as the new package wrote it"
    );
    let lock = read_lock(root).unwrap();
    let entry = &lock.packages["Toolkit"];
    assert_eq!(entry.version.as_deref(), Some("2.0.0"));
    assert!(root.join(&entry.archive).is_file());
    assert_ne!(
        entry.archive, old_cache,
        "the cache path moved to the new version"
    );
    assert!(
        !root.join(&old_cache).exists(),
        "the old cache was cleaned up"
    );

    // Re-running the same update again is refused: the version already matches.
    let provenance = OfficialProvenance {
        content_id: Some("999".into()),
        version: Some("2.0.0".into()),
        source: Some("mendix".into()),
        ..OfficialProvenance::default()
    };
    let plan = plan_update(root, "Toolkit", &v2, "2.0.0", provenance).unwrap();
    assert!(!plan.safe());
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("already installed")),
        "{:?}",
        plan.blockers
    );
}

#[test]
fn verify_reports_healthy_for_a_real_install_and_catches_a_removed_asset() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root);
    let archive = package(
        root,
        "Toolkit",
        &[("javasource/toolkit/Helper.java", "class Helper {}")],
    );
    install_module(
        &archive,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();

    let results = mxrs_marketplace::verify::verify(root).unwrap();
    assert_eq!(results.len(), 1);
    let (name, result) = &results[0];
    assert_eq!(name, "Toolkit");
    assert!(result.valid, "{result:?}");

    // Deleting a declared asset the lock still expects is caught.
    std::fs::remove_file(root.join("javasource/toolkit/Helper.java")).unwrap();
    let results = mxrs_marketplace::verify::verify(root).unwrap();
    assert!(!results[0].1.valid);
    match &results[0].1.detail {
        mxrs_marketplace::verify::VerifyDetail::Module { files_present, .. } => {
            assert!(!files_present);
        }
        other => panic!("expected Module detail, got {other:?}"),
    }
}
