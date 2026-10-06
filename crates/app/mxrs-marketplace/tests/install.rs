//! Installing a package is a write to two places at once — the `.mpr` and the
//! filesystem beside it — so the claims worth testing are that both land, that
//! neither lands when the other fails, and that a hostile package cannot write
//! outside the project.
//!
//! Packages here are built in-process from a real `mxrs-writer` project, so the
//! tests need no network and no downloaded fixture. One `#[ignore]`d test
//! installs the genuine Community Commons `.mpk` when its path is supplied.

use std::io::Write;
use std::path::{Path, PathBuf};

use mxrs_marketplace::{MarketplaceError, plan_install};

/// Builds a `.mpk` around a real `.mpr` containing `module_name`.
/// `mxrs-writer` only builds Mendix 11.12.1 projects, so a package for another
/// model version is produced by building at 11.12.1 and rewriting the recorded
/// version — see [`set_mendix_version`]. The installer reads that field, which
/// is exactly what is under test here.
fn package(
    directory: &Path,
    module_name: &str,
    mendix_version: &str,
    assets: &[(&str, &str)],
    manifest: Option<&str>,
) -> PathBuf {
    // Each call gets its own paths: `write_project` refuses to overwrite, so
    // reusing a name across calls fails with a confusing SQLite error.
    // Each package is built in its own directory: a v2 project keeps its unit
    // contents in a sibling `mprcontents/`, and several projects sharing one
    // directory would merge their sidecars into an unusable mix.
    let unique = next_id();
    let build_dir = directory.join(format!("build-{unique}"));
    std::fs::create_dir_all(&build_dir).unwrap();
    let project_path = build_dir.join("project.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module(module_name, |module| {
        module.entity("Thing", |entity| {
            entity.string("Name");
        });
        module.microflow("ACT_DoThing", |_flow| {});
    });
    mxrs_writer::write_project(&project_path, &builder.build()).unwrap();
    if mendix_version != SUPPORTED_VERSION {
        set_mendix_version(&project_path, mendix_version);
    }

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
    if let Some(manifest) = manifest {
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
    }
    zip.start_file("project.mpr", options).unwrap();
    zip.write_all(&std::fs::read(&project_path).unwrap())
        .unwrap();
    // `mxrs-writer` emits a v2 project, whose unit contents live in a sibling
    // `mprcontents/`. Real published packages embed a self-contained v1
    // project, but shipping the sidecar here exercises the v2 path too.
    // The sidecar is nested (`mprcontents/7a/73/<uuid>.mxunit`), so this walks
    // it rather than listing one level.
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

/// The only Mendix version `mxrs-writer` can scaffold.
const SUPPORTED_VERSION: &str = "11.12.1";

/// Unique suffix so repeated helper calls never collide on a path.
fn next_id() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Rewrites the Mendix version recorded in an `.mpr`, keeping its schema hash.
fn set_mendix_version(path: &Path, version: &str) {
    let mut mpr = mxrs_mpr::MprFile::open(path, false).unwrap();
    let hash = mpr.schema_hash().unwrap().unwrap_or_default();
    mpr.update_version(version, &hash).unwrap();
}

/// A target project with its own unrelated module. Assets install beside it,
/// so callers that check installed files pass this directory as the root.
fn target(directory: &Path, mendix_version: &str) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join("Target.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module("Existing", |module| {
        module.entity("Order", |entity| {
            entity.string("Number");
        });
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();
    if mendix_version != SUPPORTED_VERSION {
        set_mendix_version(&path, mendix_version);
    }
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
fn installing_adds_the_module_and_its_assets_and_leaves_the_project_readable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let archive = package(
        root,
        "Toolkit",
        "11.12.1",
        &[
            ("javasource/toolkit/Helper.java", "class Helper {}"),
            ("vendorlib/toolkit.jar", "not-really-a-jar"),
        ],
        None,
    );
    let mpr = target(root, "11.12.1");

    let plan = plan_install(&archive, &mpr, None, false).unwrap();
    assert_eq!(plan.module_name, "Toolkit");
    assert_eq!(plan.source_version.as_deref(), Some("11.12.1"));
    assert_eq!(plan.target_version.as_deref(), Some("11.12.1"));
    // The module unit plus everything beneath it.
    assert!(plan.units.len() > 1, "{:?}", plan.units);
    assert_eq!(plan.files.len(), 2);
    assert_eq!(plan.overwrites(), 0);

    // Planning alone writes nothing.
    assert_eq!(module_names(&mpr), ["Existing"]);
    assert!(!root.join("javasource/toolkit/Helper.java").exists());

    let report = plan_install(&archive, &mpr, None, false)
        .unwrap()
        .apply()
        .unwrap();
    assert_eq!(report.module_name, "Toolkit");
    assert_eq!(report.files, 2);
    assert_eq!(report.overwritten, 0);

    let mut names = module_names(&mpr);
    names.sort();
    assert_eq!(names, ["Existing", "Toolkit"]);
    // The imported module's own content survived the move.
    let project = mxrs_model::Project::open(&mpr, true).unwrap();
    let toolkit = project
        .modules()
        .unwrap()
        .into_iter()
        .find(|module| module.name.as_deref() == Some("Toolkit"))
        .unwrap();
    assert_eq!(
        toolkit
            .entities()
            .iter()
            .filter_map(|entity| entity.name.as_deref())
            .collect::<Vec<_>>(),
        ["Thing"]
    );
    assert!(
        toolkit
            .microflows
            .iter()
            .any(|flow| flow.name.as_deref() == Some("ACT_DoThing"))
    );
    // Installed, the module is the Marketplace's, as Studio Pro marks one.
    assert!(toolkit.from_app_store);

    assert_eq!(
        std::fs::read_to_string(root.join("javasource/toolkit/Helper.java")).unwrap(),
        "class Helper {}"
    );
    assert!(root.join("vendorlib/toolkit.jar").exists());
}

#[test]
fn installing_the_same_module_twice_is_refused_rather_than_duplicated() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let archive = package(root, "Toolkit", "11.12.1", &[], None);
    let mpr = target(root, "11.12.1");

    plan_install(&archive, &mpr, None, false)
        .unwrap()
        .apply()
        .unwrap();
    assert!(matches!(
        plan_install(&archive, &mpr, None, false),
        Err(MarketplaceError::ModuleAlreadyInstalled(name)) if name == "Toolkit"
    ));
    // Still exactly one copy.
    assert_eq!(
        module_names(&mpr)
            .iter()
            .filter(|name| *name == "Toolkit")
            .count(),
        1
    );
}

#[test]
fn a_model_version_mismatch_is_refused_unless_the_upgrade_is_opted_into() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let older = package(root, "Toolkit", "10.24.0", &[], None);
    let mpr = target(root, "11.12.1");

    assert!(matches!(
        plan_install(&older, &mpr, None, false),
        Err(MarketplaceError::ModelVersionMismatch { .. })
    ));
    // Opted in, importing forward is allowed.
    assert!(plan_install(&older, &mpr, None, true).is_ok());

    // Backward is refused even when opted in: nothing here downgrades a model.
    let newer = package(root, "Future", "11.12.1", &[], None);
    let old_target = target(&root.join("old"), "10.24.0");
    assert!(matches!(
        plan_install(&newer, &old_target, None, true),
        Err(MarketplaceError::ModelVersionMismatch { .. })
    ));
}

#[test]
fn a_manifest_that_disagrees_with_its_own_model_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let archive = package(
        root,
        "Toolkit",
        "11.12.1",
        &[],
        Some(r#"{"package":{"version":"1.0.0","type":"Module"},"model-version":"10.24.0"}"#),
    );
    let mpr = target(root, "11.12.1");
    assert!(matches!(
        plan_install(&archive, &mpr, None, false),
        Err(MarketplaceError::ManifestVersionMismatch { .. })
    ));
}

#[test]
fn a_package_declaring_a_path_outside_the_project_is_refused_before_anything_is_written() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root, "11.12.1");

    for hostile in ["../escaped.txt", ".git/hooks/pre-commit", "/etc/passwd"] {
        let archive = package(root, "Hostile", "11.12.1", &[(hostile, "payload")], None);
        let error = plan_install(&archive, &mpr, None, false).unwrap_err();
        assert!(
            matches!(
                error,
                MarketplaceError::UnsafePackagePath(_) | MarketplaceError::ProtectedPackagePath(_)
            ),
            "{hostile} produced {error}"
        );
    }
    // Nothing escaped, and the target is untouched.
    assert!(!root.parent().unwrap().join("escaped.txt").exists());
    assert_eq!(module_names(&mpr), ["Existing"]);
}

#[test]
fn a_failed_install_restores_every_asset_it_had_already_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // The second asset's destination is a directory, which the installer
    // refuses — after the first asset has already been written.
    let archive = package(
        root,
        "Toolkit",
        "11.12.1",
        &[
            ("javasource/First.java", "from package"),
            ("javasource/Second.java", "from package"),
        ],
        None,
    );
    let mpr = target(root, "11.12.1");
    std::fs::create_dir_all(root.join("javasource")).unwrap();
    std::fs::write(root.join("javasource/First.java"), "original").unwrap();
    std::fs::create_dir_all(root.join("javasource/Second.java")).unwrap();

    let plan = plan_install(&archive, &mpr, None, false).unwrap();
    assert_eq!(plan.overwrites(), 1, "First.java already exists");
    let error = plan.apply().unwrap_err();
    assert!(
        matches!(error, MarketplaceError::AssetIsDirectory(name) if name == "javasource/Second.java"),
        "unexpected error"
    );

    // The asset that was replaced before the failure is back to its original
    // content — the whole point of taking backups.
    assert_eq!(
        std::fs::read_to_string(root.join("javasource/First.java")).unwrap(),
        "original"
    );
}

#[test]
fn missing_inputs_are_named_rather_than_panicking() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root, "11.12.1");
    let archive = package(root, "Toolkit", "11.12.1", &[], None);

    assert!(matches!(
        plan_install(Path::new("/nonexistent.mpk"), &mpr, None, false),
        Err(MarketplaceError::PackageNotFound(_))
    ));
    assert!(matches!(
        plan_install(&archive, Path::new("/nonexistent.mpr"), None, false),
        Err(MarketplaceError::TargetNotFound(_))
    ));
    // A file that is not a zip at all.
    let bogus = root.join("bogus.mpk");
    std::fs::write(&bogus, b"this is not a zip").unwrap();
    assert!(matches!(
        plan_install(&bogus, &mpr, None, false),
        Err(MarketplaceError::InvalidPackage { .. })
    ));
}

/// Installs the genuine Community Commons package. Opt-in because it needs a
/// real `.mpk`, which `mxrs marketplace download 170` produces.
///
///   `MXRS_TEST_MPK=/path/to/CommunityCommons.mpk cargo test -p mxrs-marketplace -- --ignored`
#[test]
#[ignore = "requires MXRS_TEST_MPK pointing at a real downloaded .mpk"]
fn a_real_marketplace_package_installs_into_a_real_project() {
    let Ok(archive) = std::env::var("MXRS_TEST_MPK") else {
        panic!("set MXRS_TEST_MPK to a downloaded .mpk");
    };
    let archive = PathBuf::from(archive);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();

    let mut package = mxrs_marketplace::ModulePackage::open(&archive).unwrap();
    let descriptor = package.descriptor().unwrap();
    // Build the target at the package's own Mendix version so the import is a
    // same-version one.
    let source_version = {
        let staging = tempfile::tempdir().unwrap();
        let extracted = package
            .extract_project(&descriptor, staging.path())
            .unwrap();
        mxrs_mpr::MprFile::open(&extracted, true)
            .unwrap()
            .mendix_version()
            .unwrap()
            .expect("a real package declares a Mendix version")
    };
    let mpr = target(root, &source_version);

    let plan = plan_install(&archive, &mpr, None, false).unwrap();
    assert_eq!(plan.module_name, descriptor.module_name);
    assert!(!plan.units.is_empty());
    let report = plan.apply().unwrap();
    assert_eq!(report.module_name, descriptor.module_name);
    assert_eq!(report.files, descriptor.files.len());

    let names = module_names(&mpr);
    assert!(names.contains(&descriptor.module_name), "{names:?}");
    // Declared assets really landed.
    for relative in descriptor.files.iter().take(5) {
        assert!(
            root.join(relative).is_file(),
            "{relative} was not installed"
        );
    }
}
