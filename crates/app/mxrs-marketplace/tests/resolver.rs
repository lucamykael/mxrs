//! Offline dependency resolution: dependencies are discovered from the
//! unresolved references inside the installed package's own model, already
//! locked modules satisfy them from the local cache, missing ones without
//! a credential become named blockers instead of guesses, and required
//! widget bundles surface as honest blockers naming the official content.

use std::io::Write;
use std::path::{Path, PathBuf};

use mxrs_dsl::CallArgument;
use mxrs_ir::MicroflowRef;
use mxrs_marketplace::lifecycle::{OfficialProvenance, install_module};
use mxrs_marketplace::resolver::plan_dependencies;
use mxrs_marketplace::{Download, Transport};

const SUPPORTED_VERSION: &str = "11.12.1";

/// A transport the offline tests must never reach.
struct Unreachable;
impl Transport for Unreachable {
    fn get(&self, url: &str, _authorization: &str) -> mxrs_marketplace::Result<String> {
        panic!("offline test hit the network: {url}");
    }
    fn download(
        &self,
        url: &str,
        _authorization: Option<&str>,
        _destination: &Path,
    ) -> mxrs_marketplace::Result<Download> {
        panic!("offline test hit the network: {url}");
    }
}

#[allow(dead_code, non_snake_case, non_camel_case_types)]
mod markers {
    pub mod Helper {
        pub struct ACT_Log;
        impl mxrs_ir::MicroflowMarker for ACT_Log {
            const MODULE: &'static str = "Helper";
            const NAME: &'static str = "ACT_Log";
        }
    }
}

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

fn zip_project(directory: &Path, module_name: &str, build_dir: &Path) -> PathBuf {
    let package_xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <package xmlns=\"http://www.mendix.com/package/1.0/\">\n\
         \x20 <modelerProject xmlns=\"http://www.mendix.com/modelerProject/1.0/\">\n\
         \x20   <module name=\"{module_name}\" />\n\
         \x20   <projectFile path=\"project.mpr\" />\n\
         \x20   <files>\n    </files>\n\
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
    zip.write_all(&std::fs::read(build_dir.join("project.mpr")).unwrap())
        .unwrap();
    for file in walk(&build_dir.join("mprcontents")) {
        let relative = file.strip_prefix(build_dir).unwrap();
        zip.start_file(relative.to_str().unwrap().replace('\\', "/"), options)
            .unwrap();
        zip.write_all(&std::fs::read(&file).unwrap()).unwrap();
    }
    zip.finish().unwrap();
    archive_path
}

/// A package whose model calls `Helper.ACT_Log` — an unresolved reference
/// until a module named `Helper` exists next to it. The writer refuses to
/// author a dangling call directly, so the package is built with both
/// modules and `Helper` is then deleted from the packaged copy — the same
/// shape a real exported module has after its dependency is stripped.
fn dependent_package(directory: &Path) -> PathBuf {
    let build_dir = directory.join(format!("build-{}", next_id()));
    std::fs::create_dir_all(&build_dir).unwrap();
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module("Helper", |module| {
        module.microflow("ACT_Log", |_flow| {});
    });
    builder.module("Toolkit", |module| {
        module.microflow("ACT_UseHelper", |flow| {
            flow.call_microflow(
                MicroflowRef::<markers::Helper::ACT_Log>::new(),
                None,
                false,
                Vec::<CallArgument>::new(),
            );
        });
    });
    let project = build_dir.join("project.mpr");
    mxrs_writer::write_project(&project, &builder.build()).unwrap();
    {
        let mut mpr = mxrs_mpr::MprFile::open(&project, false).unwrap();
        let helper_id = mpr
            .units_by_containment("Modules")
            .unwrap()
            .into_iter()
            .find(|unit| {
                mpr.parse_contents(unit)
                    .ok()
                    .and_then(|doc| doc.get_str("Name").ok().map(str::to_string))
                    .as_deref()
                    == Some("Helper")
            })
            .unwrap()
            .unit_id;
        let ids = mxrs_marketplace::lifecycle::subtree_ids(&mpr, &helper_id).unwrap();
        mpr.transaction(|mpr| {
            for id in ids.iter().rev() {
                mpr.delete_unit(id)?;
            }
            Ok(())
        })
        .unwrap();
    }
    zip_project(directory, "Toolkit", &build_dir)
}

fn helper_package(directory: &Path) -> PathBuf {
    let build_dir = directory.join(format!("build-{}", next_id()));
    std::fs::create_dir_all(&build_dir).unwrap();
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module("Helper", |module| {
        module.microflow("ACT_Log", |_flow| {});
    });
    mxrs_writer::write_project(build_dir.join("project.mpr"), &builder.build()).unwrap();
    zip_project(directory, "Helper", &build_dir)
}

fn target(directory: &Path) -> PathBuf {
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

#[test]
fn a_missing_dependency_without_a_credential_is_a_named_blocker() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root);
    let archive = dependent_package(root);
    install_module(
        &archive,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();

    let plan =
        plan_dependencies::<Unreachable>(root, "Toolkit", None, Some(SUPPORTED_VERSION)).unwrap();
    assert_eq!(plan.root, "Toolkit");
    assert!(!plan.safe());
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("Helper") && blocker.contains("credential")),
        "{:?}",
        plan.blockers
    );
    assert!(plan.apply().is_err());
}

#[test]
fn an_already_locked_dependency_resolves_from_the_local_cache() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root);
    // Helper is installed (and therefore locked + cached) first, then the
    // dependent module; its dependency resolves offline with no API.
    let helper = helper_package(root);
    install_module(
        &helper,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();
    let toolkit = dependent_package(root);
    install_module(
        &toolkit,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();

    let api: Option<&mxrs_marketplace::ContentApi<Unreachable>> = None;
    let plan = plan_dependencies(root, "Toolkit", api, Some(SUPPORTED_VERSION)).unwrap();
    assert!(plan.safe(), "{:?}", plan.blockers);
    // Helper is already installed, so there is nothing left to install —
    // exactly mxrb's `@resolved ... unless installed?` behavior.
    assert!(plan.changes().is_empty(), "{:?}", plan.changes());
    plan.apply().unwrap();
}

#[test]
fn required_widget_bundles_surface_as_honest_blockers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root);
    // A package with a Data Grid 2 page requires the DataWidgets bundle.
    let build_dir = root.join(format!("build-{}", next_id()));
    std::fs::create_dir_all(&build_dir).unwrap();
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module("Grids", |module| {
        module.page("Dashboard", |page| {
            page.layout("Atlas_Core.ApplicationLayout", "Main");
            page.data_grid_2(|_grid| {});
        });
    });
    mxrs_writer::write_project(build_dir.join("project.mpr"), &builder.build()).unwrap();
    let archive = zip_project(root, "Grids", &build_dir);
    install_module(
        &archive,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();

    let plan =
        plan_dependencies::<Unreachable>(root, "Grids", None, Some(SUPPORTED_VERSION)).unwrap();
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("116540") && blocker.contains("not ported")),
        "{:?}",
        plan.blockers
    );
}
