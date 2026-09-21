//! Offline dependency resolution: dependencies are discovered from the
//! unresolved references inside the installed package's own model, already
//! locked modules satisfy them from the local cache, missing ones without
//! a credential become named blockers instead of guesses, required widget
//! bundles without a credential surface as honest blockers naming the
//! official content, and — with a fake Content API — a required standalone
//! widget resolves, downloads, is verified to provide the id, and installs.

use std::io::Write;
use std::path::{Path, PathBuf};

use mxrs_dsl::CallArgument;
use mxrs_ir::MicroflowRef;
use mxrs_marketplace::lifecycle::{OfficialProvenance, install_module};
use mxrs_marketplace::resolver::plan_dependencies;
use mxrs_marketplace::{ContentApi, Download, Pat, Transport};

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
    // Every real project ships Atlas_Core out of the box; the fixture pages
    // built below reference `Atlas_Core.ApplicationLayout`, and this keeps
    // that a locally satisfied reference rather than an extra dependency.
    builder.module("Atlas_Core", |module| {
        module.layout("ApplicationLayout", |layout| {
            layout.placeholder("Main");
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

    // Without a credential the required widget bundle is a named, honest
    // blocker rather than a silent skip or a guess.
    let plan =
        plan_dependencies::<Unreachable>(root, "Grids", None, Some(SUPPORTED_VERSION)).unwrap();
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("116540") && blocker.contains("credential")),
        "{:?}",
        plan.blockers
    );
}

/// Serves the well-known Combobox widget content (id 219304) so the
/// resolver's live candidate path can be exercised offline.
struct FakeWidgetContentApi {
    archive: PathBuf,
}

impl Transport for FakeWidgetContentApi {
    fn get(&self, url: &str, _authorization: &str) -> mxrs_marketplace::Result<String> {
        if url.contains("/content/219304/versions") {
            return Ok(r#"{"items":[{"name":"Combo box","versionId":"v1","versionNumber":"2.9.0","minSupportedMendixVersion":"9.0.0","versionType":"Regular"}]}"#.into());
        }
        if url.contains("/content/219304") {
            return Ok(r#"{"contentId":219304,"publisher":"Mendix","type":"Widget","isPrivate":false,"isCompanyApproved":true,"latestVersion":{"name":"Combo box","versionId":"v1","versionNumber":"2.9.0"}}"#.into());
        }
        Err(mxrs_marketplace::MarketplaceError::Status {
            status: 404,
            url: url.to_string(),
        })
    }

    fn download(
        &self,
        _url: &str,
        _authorization: Option<&str>,
        destination: &Path,
    ) -> mxrs_marketplace::Result<Download> {
        std::fs::copy(&self.archive, destination).unwrap();
        Ok(Download::Written(
            std::fs::metadata(destination).unwrap().len(),
        ))
    }
}

fn combobox_widget_archive(path: &Path) {
    let package_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.mendix.com/package/1.0/">
  <clientModule name="Combobox" version="2.9.0" xmlns="http://www.mendix.com/clientModule/1.0/">
    <widgetFiles><widgetFile path="Combobox.xml" /></widgetFiles>
  </clientModule>
</package>"#;
    let widget_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<widget id="com.mendix.widget.web.combobox.Combobox" pluginWidget="true" xmlns="http://www.mendix.com/widget/1.0/">
  <name>Combo box</name>
</widget>"#;
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("package.xml", options).unwrap();
    zip.write_all(package_xml.as_bytes()).unwrap();
    zip.start_file("Combobox.xml", options).unwrap();
    zip.write_all(widget_xml.as_bytes()).unwrap();
    zip.finish().unwrap();
}

#[test]
fn a_required_standalone_widget_resolves_downloads_verifies_and_installs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root);
    // A package whose page embeds a real Combo box pluggable widget.
    let build_dir = root.join(format!("build-{}", next_id()));
    std::fs::create_dir_all(&build_dir).unwrap();
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module("Forms", |module| {
        module.page("Entry", |page| {
            page.layout("Atlas_Core.ApplicationLayout", "Main");
            page.combo_box(|_combo_box| {});
        });
    });
    mxrs_writer::write_project(build_dir.join("project.mpr"), &builder.build()).unwrap();
    let archive = zip_project(root, "Forms", &build_dir);
    install_module(
        &archive,
        &mpr,
        Some(root),
        false,
        &OfficialProvenance::default(),
    )
    .unwrap();

    let widget_archive = root.join("combobox-candidate.mpk");
    combobox_widget_archive(&widget_archive);
    let api = ContentApi::new(
        FakeWidgetContentApi {
            archive: widget_archive,
        },
        Pat::new("test-token").unwrap(),
    );

    let plan = plan_dependencies(root, "Forms", Some(&api), Some(SUPPORTED_VERSION)).unwrap();
    assert!(plan.safe(), "{:?}", plan.blockers);
    assert_eq!(plan.widget_dependencies.len(), 1);
    assert_eq!(plan.widget_dependencies[0].content_id, "219304");
    assert!(
        plan.changes()
            .iter()
            .any(|change| change.contains("widget bundle Combo box")),
        "{:?}",
        plan.changes()
    );

    plan.apply().unwrap();
    assert!(
        root.join("widgets/com.mendix.widget.web.Combobox.mpk")
            .is_file()
    );
    let lock = mxrs_marketplace::lock::read_lock(root).unwrap();
    let entry = lock.packages.get("Combobox").expect("widget locked");
    assert_eq!(entry.kind, "widget");
    assert_eq!(entry.content_id.as_deref(), Some("219304"));

    // Re-resolving now finds the widget already installed and requires
    // nothing further.
    let plan = plan_dependencies(root, "Forms", Some(&api), Some(SUPPORTED_VERSION)).unwrap();
    assert!(plan.safe(), "{:?}", plan.blockers);
    assert!(plan.widget_dependencies.is_empty());
}

/// Serves a fixed content id's search-independent detail/versions for an
/// update, and downloads a caller-supplied archive.
struct FakeUpdateContentApi {
    content_id: &'static str,
    version_number: &'static str,
    archive: PathBuf,
}

impl Transport for FakeUpdateContentApi {
    fn get(&self, url: &str, _authorization: &str) -> mxrs_marketplace::Result<String> {
        if url.contains(&format!("/content/{}/versions", self.content_id)) {
            return Ok(format!(
                r#"{{"items":[{{"name":"Toolkit","versionId":"v2","versionNumber":"{}","minSupportedMendixVersion":"9.0.0","versionType":"Regular"}}]}}"#,
                self.version_number
            ));
        }
        if url.contains(&format!("/content/{}", self.content_id)) {
            return Ok(format!(
                r#"{{"contentId":{},"publisher":"Mendix","type":"Module","isPrivate":false,"isCompanyApproved":true,"latestVersion":{{"name":"Toolkit","versionId":"v2","versionNumber":"{}"}}}}"#,
                self.content_id, self.version_number
            ));
        }
        Err(mxrs_marketplace::MarketplaceError::Status {
            status: 404,
            url: url.to_string(),
        })
    }

    fn download(
        &self,
        _url: &str,
        _authorization: Option<&str>,
        destination: &Path,
    ) -> mxrs_marketplace::Result<Download> {
        std::fs::copy(&self.archive, destination).unwrap();
        Ok(Download::Written(
            std::fs::metadata(destination).unwrap().len(),
        ))
    }
}

#[test]
fn plan_update_official_resolves_downloads_and_updates_the_installed_module() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mpr = target(root);
    let build_dir = root.join(format!("build-{}", next_id()));
    std::fs::create_dir_all(&build_dir).unwrap();
    let mut builder = mxrs_dsl::ProjectBuilder::new(SUPPORTED_VERSION);
    builder.module("Toolkit", |module| {
        module.microflow("ACT_Original", |_flow| {});
    });
    mxrs_writer::write_project(build_dir.join("project.mpr"), &builder.build()).unwrap();
    let v1 = zip_project(root, "Toolkit", &build_dir);
    install_module(
        &v1,
        &mpr,
        Some(root),
        false,
        &mxrs_marketplace::lifecycle::OfficialProvenance {
            content_id: Some("777".into()),
            version: Some("1.0.0".into()),
            source: Some("mendix".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let v2 = zip_project(root, "Toolkit", &build_dir);
    let api = ContentApi::new(
        FakeUpdateContentApi {
            content_id: "777",
            version_number: "2.0.0",
            archive: v2,
        },
        Pat::new("test-token").unwrap(),
    );

    let plan = mxrs_marketplace::resolver::plan_update_official(
        root,
        "Toolkit",
        None,
        Some(SUPPORTED_VERSION),
        &api,
    )
    .unwrap();
    assert!(plan.safe(), "{:?}", plan.blockers);
    assert_eq!(plan.installed_version.as_deref(), Some("1.0.0"));
    assert_eq!(plan.target_version, "2.0.0");
    plan.apply().unwrap();

    let lock = mxrs_marketplace::lock::read_lock(root).unwrap();
    assert_eq!(lock.packages["Toolkit"].version.as_deref(), Some("2.0.0"));
}
