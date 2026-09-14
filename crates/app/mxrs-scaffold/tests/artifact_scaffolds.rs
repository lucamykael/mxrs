//! The only evidence that matters for a Rust scaffold: the project it
//! produces still compiles, and the declarations it adds actually reach the
//! writer. mxrb's Ruby templates can mention a DSL method that does not exist
//! because nothing parses them until `project.rb` runs; a Rust template that
//! did the same would break `cargo check` for the user's whole project.

use std::path::{Path, PathBuf};
use std::process::Command;

use mxrs_scaffold::{
    ArtifactKind, ArtifactScaffold, MxrsDependency, PageChain, ProjectScaffold, ScaffoldError,
    generate_project, inspect_project, registry, scaffold_artifact,
};

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .expect("the scaffold crate always lives inside the workspace")
}

fn application(directory: &Path) -> PathBuf {
    let destination = directory.join("application");
    generate_project(
        &ProjectScaffold::new("Order Portal", "11.12.1", &destination)
            .dependency(MxrsDependency::Path(workspace().join("crates/app/mxrs"))),
    )
    .unwrap();
    destination
}

fn scaffold(root: &Path, kind: ArtifactKind, name: &str) -> Vec<PathBuf> {
    scaffold_artifact(&ArtifactScaffold::new(kind, name, root))
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .files
}

fn cargo(root: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO"))
        .args(arguments)
        .current_dir(root)
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace().join("target")),
        )
        .output()
        .unwrap()
}

#[test]
fn every_scaffolded_artifact_compiles_and_reaches_the_written_model() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    scaffold(&root, ArtifactKind::Entity, "Sales.Order");
    scaffold(&root, ArtifactKind::Enumeration, "Sales.PaymentStatus");
    scaffold(&root, ArtifactKind::Constant, "Sales.ApiEndpoint");
    scaffold(&root, ArtifactKind::ScheduledEvent, "Sales.SE_ExpireCarts");
    scaffold(&root, ArtifactKind::UseCase, "Sales.ACT_CreateOrder");
    scaffold(&root, ArtifactKind::Validation, "Sales.VAL_Order");
    scaffold(&root, ArtifactKind::Integration, "Sales.INT_Orders");
    scaffold(&root, ArtifactKind::Nanoflow, "Sales.NAN_RefreshOrder");
    scaffold(&root, ArtifactKind::PublishedRest, "Sales.HandleOrder");
    scaffold(&root, ArtifactKind::ConsumedRest, "Sales.FetchCatalog");
    scaffold(&root, ArtifactKind::JavaAction, "Sales.InvokeCheckout");
    scaffold(&root, ArtifactKind::Repository, "Sales.Orders");
    scaffold(&root, ArtifactKind::Security, "Sales");
    scaffold_artifact(
        &ArtifactScaffold::new(ArtifactKind::Page, "Sales.OrderOverview", &root)
            .page_roles(vec!["Sales.User".to_string()]),
    )
    .unwrap();

    let output = cargo(
        &root,
        &["run", "--offline", "--quiet", "--", "build/Sales.mpr"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let project = mxrs_model::Project::open(root.join("build/Sales.mpr"), true).unwrap();
    let module = project
        .modules()
        .unwrap()
        .into_iter()
        .find(|module| module.name.as_deref() == Some("Sales"))
        .expect("the scaffolded module reached the model");
    let entities = module
        .domain_model
        .as_ref()
        .map(|domain| {
            domain
                .entities
                .iter()
                .filter_map(|entity| entity.name.as_deref())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(entities, ["Order"]);
    let microflows = module
        .microflows
        .iter()
        .filter_map(|flow| flow.name.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "ACT_CreateOrder",
        "HandleOrder",
        "FetchCatalog",
        "InvokeCheckout",
        // The scheduled-event scaffold generates its handler too — a job
        // pointing at a microflow that does not exist would not build.
        "SE_ExpireCarts",
    ] {
        assert!(microflows.contains(&expected), "{expected}: {microflows:?}");
    }
    // Constants and scheduled events have no typed accessor on `Module`
    // (constants are materialized separately, as in mxrb), so these are read
    // from the raw documents.
    let documents: Vec<(String, String)> = project
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .filter_map(|document| {
            Some((
                document.get_str("$Type").ok()?.to_string(),
                document.get_str("Name").ok()?.to_string(),
            ))
        })
        .collect();
    for expected in [
        ("Constants$Constant", "ApiEndpoint"),
        ("ScheduledEvents$ScheduledEvent", "SE_ExpireCarts"),
    ] {
        let expected = (expected.0.to_string(), expected.1.to_string());
        assert!(documents.contains(&expected), "{expected:?}: {documents:?}");
    }
    assert_eq!(
        module
            .nanoflows
            .iter()
            .filter_map(|flow| flow.name.as_deref())
            .collect::<Vec<_>>(),
        ["NAN_RefreshOrder"]
    );
    assert_eq!(
        module
            .pages
            .iter()
            .filter_map(|page| page.name.as_deref())
            .collect::<Vec<_>>(),
        ["OrderOverview"]
    );
    let roles = module
        .module_roles
        .iter()
        .filter_map(|role| role.name.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(roles, ["User", "Administrator"]);
}

#[test]
fn scaffolding_the_same_artifact_twice_changes_nothing_the_first_run_wrote() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    let first = scaffold(&root, ArtifactKind::Entity, "Sales.Order");
    let aggregator = root.join("src/domain/modules/sales/entities/mod.rs");
    let before = std::fs::read_to_string(&aggregator).unwrap();
    assert!(matches!(
        scaffold_artifact(&ArtifactScaffold::new(
            ArtifactKind::Entity,
            "Sales.Order",
            &root
        )),
        Err(ScaffoldError::FileExists(_))
    ));
    assert_eq!(std::fs::read_to_string(&aggregator).unwrap(), before);
    assert!(matches!(
        scaffold_artifact(&ArtifactScaffold::new(ArtifactKind::Module, "Sales", &root)),
        Err(ScaffoldError::ModuleExists(_))
    ));

    scaffold(&root, ArtifactKind::Entity, "Sales.Invoice");
    let after = std::fs::read_to_string(&aggregator).unwrap();
    assert!(after.contains("pub mod order;\npub mod invoice;\n"));
    assert!(after.contains("    order::declare,\n    invoice::declare,\n"));
    // The first entity also creates the family aggregator; the second reuses it.
    assert_eq!(first.len(), 2);

    let inspection = inspect_project(&root).unwrap();
    assert_eq!(inspection.modules, ["sales"]);
    assert_eq!(inspection.declared_version.as_deref(), Some("11.12.1"));
    assert_eq!(
        inspection.registered_scaffolds,
        ["entity:Sales.Invoice", "entity:Sales.Order", "module:Sales"]
    );
    assert!(inspection.manifest && inspection.domain_module);
}

#[test]
fn repository_scaffold_separates_the_application_port_from_its_adapter() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    let files = scaffold(&root, ArtifactKind::Repository, "Sales.Orders");
    assert!(
        files
            .iter()
            .any(|path| path.ends_with("application/repositories/orders.rs"))
    );
    assert!(
        files
            .iter()
            .any(|path| { path.ends_with("infrastructure/repositories/orders_implementation.rs") })
    );
    let port =
        std::fs::read_to_string(root.join("src/application/repositories/orders.rs")).unwrap();
    let adapter = std::fs::read_to_string(
        root.join("src/infrastructure/repositories/orders_implementation.rs"),
    )
    .unwrap();
    assert!(port.contains("pub trait Port"));
    assert!(adapter.contains("impl crate::application::repositories::orders::Port"));
    let output = cargo(&root, &["check", "--offline", "--quiet"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_formatted_project_can_still_be_scaffolded_into() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    scaffold(&root, ArtifactKind::Entity, "Sales.Order");
    let output = cargo(&root, &["fmt"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let aggregator = root.join("src/domain/modules/sales/entities/mod.rs");
    assert!(
        std::fs::read_to_string(&aggregator)
            .unwrap()
            .contains("&[order::declare];")
    );
    scaffold(&root, ArtifactKind::Entity, "Sales.Invoice");
    assert!(
        std::fs::read_to_string(&aggregator)
            .unwrap()
            .contains("    order::declare,\n    invoice::declare,\n")
    );
    let output = cargo(&root, &["build", "--offline", "--quiet"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_dry_run_reports_the_same_paths_without_writing_or_registering_any() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    let preview = scaffold_artifact(
        &ArtifactScaffold::new(ArtifactKind::Entity, "Sales.Order", &root).dry_run(true),
    )
    .unwrap();
    assert!(preview.dry_run);
    assert!(preview.files.iter().all(|file| !file.exists()));
    assert!(registry::entries(&root).unwrap().len() == 1);
    let real = scaffold(&root, ArtifactKind::Entity, "Sales.Order");
    assert_eq!(real, preview.files);
    assert!(real.iter().all(|file| file.is_file()));
}

#[test]
fn destroying_a_scaffold_removes_exactly_the_files_it_created() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    let created = scaffold(&root, ArtifactKind::Entity, "Sales.Order");
    let removal = registry::destroy(&root, "entity:Sales.Order").unwrap();
    assert_eq!(removal.files, created);
    assert!(created.iter().all(|file| !file.exists()));
    // Edits to a file another scaffold created are deliberately not reverted;
    // see `registry`'s doc comment for why a manifest cannot safely undo an
    // in-file append.
    assert!(
        std::fs::read_to_string(root.join("src/domain/modules/sales/mod.rs"))
            .unwrap()
            .contains("pub mod entities;")
    );
}

#[test]
fn names_and_projects_that_cannot_be_scaffolded_are_reported_not_guessed() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    for (kind, name) in [
        (ArtifactKind::Entity, "Order"),
        (ArtifactKind::Entity, "Sales."),
        (ArtifactKind::Entity, ".Order"),
        (ArtifactKind::Entity, "Sales.Order.Line"),
        (ArtifactKind::Entity, "1Sales.Order"),
        (ArtifactKind::Entity, "Sales.Owner"),
        (ArtifactKind::Module, "Sales-Portal"),
    ] {
        assert!(
            scaffold_artifact(&ArtifactScaffold::new(kind, name, &root)).is_err(),
            "{name}"
        );
    }
    assert!(matches!(
        scaffold_artifact(&ArtifactScaffold::new(
            ArtifactKind::Entity,
            "Sales.Order",
            &root
        )),
        Err(ScaffoldError::ModuleNotFound(_))
    ));
    assert!(matches!(
        scaffold_artifact(&ArtifactScaffold::new(
            ArtifactKind::Module,
            "Sales",
            directory.path()
        )),
        Err(ScaffoldError::ProjectNotFound(_))
    ));

    // `mxrs new` now pre-wires `src/domain/modules/mod.rs`, so the guard that
    // refuses to guess how to edit an unrecognized `build()` is only reachable
    // for source generated before the layering split. Reproduce that shape
    // rather than dropping the guard from the suite.
    std::fs::remove_dir_all(root.join("src/domain/modules")).unwrap();
    std::fs::write(
        root.join("src/domain/mod.rs"),
        "pub fn build() -> mxrs::ProjectDecl { unimplemented!() }\n",
    )
    .unwrap();
    assert!(matches!(
        scaffold_artifact(&ArtifactScaffold::new(ArtifactKind::Module, "Sales", &root)),
        Err(ScaffoldError::UnrecognizedProjectBuild(_))
    ));
    assert!(!root.join("src/domain/modules").exists());
}

/// Every layered artifact family must land in the layer its catalog entry
/// advertises. A destination that drifted from the code would send a page into
/// `src/domain/` — the exact mixing the layering increment exists to remove —
/// and `mxrs add --help` would then describe a path that does not exist.
///
/// Driven off `SCAFFOLD_COMMANDS` rather than a hand-written list, so a command
/// added later cannot quietly opt out of the check: every entry that advertises
/// a path under `src/` is scaffolded and verified.
#[test]
fn every_scaffold_lands_in_the_layer_its_catalogued_destination_names() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");

    let mut checked = 0;
    for (index, command) in mxrs_scaffold::SCAFFOLD_COMMANDS.iter().enumerate() {
        let advertised = command.destination;
        // `ci`, `evaluation` and `functional-test` write outside the layered
        // source tree, so "which layer" is not a question they answer.
        if !advertised.starts_with("src/") {
            continue;
        }
        // A `<Module>` argument names a module rather than an artifact inside
        // one; `module` must name a fresh one, the rest target `Sales`.
        let name = match (command.kind, command.argument) {
            (ArtifactKind::Module, _) => format!("Module{index}"),
            (_, "<Module>") => "Sales".to_string(),
            _ => format!("Sales.Artifact{index}"),
        };
        let files = scaffold(&root, command.kind, &name);
        checked += 1;

        // `repository` is the one deliberately two-layer artifact: its port
        // lives in `application` and its adapter in `infrastructure`.
        let layers: Vec<&str> = if advertised.contains('{') {
            vec!["src/application", "src/infrastructure"]
        } else {
            vec![&advertised[..advertised[4..].find('/').unwrap() + 4]]
        };
        for layer in &layers {
            assert!(
                files
                    .iter()
                    .any(|file| file.to_string_lossy().contains(layer)),
                "{name} wrote nothing under {layer}; catalog advertises {advertised}"
            );
        }
        // Application and presentation artifacts must never leak back into the
        // domain tree, which is what the pre-split catalog did.
        if !layers.contains(&"src/domain") {
            assert!(
                !files
                    .iter()
                    .any(|file| file.to_string_lossy().contains("src/domain/modules")),
                "{name} wrote into src/domain/modules despite {advertised}"
            );
        }
    }
    // Guards against the loop silently degenerating if `destination` spellings
    // ever change shape.
    assert_eq!(checked, 15);
}

/// A project generated before the layering split has no `src/application/` or
/// `src/presentation/` tree. Scaffolding into it must fail closed and name the
/// aggregator it cannot find, rather than inventing a layer around it.
#[test]
fn scaffolding_a_layer_a_pre_split_project_lacks_fails_closed() {
    for (layer, kind, name) in [
        (
            "application",
            ArtifactKind::UseCase,
            "Sales.ACT_CreateOrder",
        ),
        (
            "presentation",
            ArtifactKind::Nanoflow,
            "Sales.NAN_RefreshOrder",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = application(directory.path());
        scaffold(&root, ArtifactKind::Module, "Sales");
        std::fs::remove_dir_all(root.join(format!("src/{layer}"))).unwrap();
        assert!(
            matches!(
                scaffold_artifact(&ArtifactScaffold::new(kind, name, &root)),
                Err(ScaffoldError::AggregatorNotFound(_))
            ),
            "{layer}"
        );
        assert!(!root.join(format!("src/{layer}")).exists());
    }
}

/// Both generators pre-write every layer's `modules` aggregator *and* its
/// `apply` call, so the code that splices that call back in is only reached for
/// a project whose layer has been edited by hand. Each layer has its own
/// expected tail there, and a wrong one reports `UnrecognizedProjectBuild` on
/// source mxrs itself emitted — a refusal the user cannot act on. Reproduce
/// that shape per layer and require the rewiring to still build.
#[test]
fn a_hand_removed_layer_apply_call_is_spliced_back_instead_of_refused() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    let calls = [
        ("domain", "modules::apply(&mut project);"),
        ("application", "modules::apply(&mut project);"),
        ("presentation", "modules::apply(project);"),
    ];
    for (layer, call) in calls {
        let path = root.join(format!("src/{layer}/mod.rs"));
        let source = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, source.replace(&format!("    {call}\n"), "")).unwrap();
        std::fs::remove_dir_all(root.join(format!("src/{layer}/modules"))).unwrap();
    }

    scaffold(&root, ArtifactKind::Module, "Sales");
    scaffold(&root, ArtifactKind::Entity, "Sales.Order");
    scaffold(&root, ArtifactKind::UseCase, "Sales.ACT_CreateOrder");
    scaffold(&root, ArtifactKind::Nanoflow, "Sales.NAN_RefreshOrder");

    for (layer, call) in calls {
        let source = std::fs::read_to_string(root.join(format!("src/{layer}/mod.rs"))).unwrap();
        assert!(source.contains("pub mod modules;"), "{layer}: {source}");
        // Exactly one call, not one per scaffolded artifact.
        assert_eq!(source.matches(call).count(), 1, "{layer}: {source}");
    }

    let output = cargo(
        &root,
        &["run", "--offline", "--quiet", "--", "build/Rewired.mpr"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let project = mxrs_model::Project::open(root.join("build/Rewired.mpr"), true).unwrap();
    let module = project
        .modules()
        .unwrap()
        .into_iter()
        .find(|module| module.name.as_deref() == Some("Sales"))
        .expect("the rewired module reached the model");
    assert!(
        module
            .microflows
            .iter()
            .any(|flow| flow.name.as_deref() == Some("ACT_CreateOrder"))
    );
    assert!(
        module
            .nanoflows
            .iter()
            .any(|flow| flow.name.as_deref() == Some("NAN_RefreshOrder"))
    );
}

/// The only claim worth making about a generated vertical slice: the project
/// still compiles, and every artifact the chain promised reaches the `.mpr`.
/// A template that merely produced plausible-looking Rust would pass a string
/// assertion and fail here.
#[test]
fn a_page_chain_generates_a_slice_that_compiles_and_reaches_the_written_model() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    scaffold_artifact(
        &ArtifactScaffold::new(ArtifactKind::Page, "Sales.OrderOverview", &root)
            .page_chain(Some(PageChain::NanoflowMicroflow)),
    )
    .unwrap();

    let output = cargo(
        &root,
        &["run", "--offline", "--quiet", "--", "build/Sales.mpr"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let project = mxrs_model::Project::open(root.join("build/Sales.mpr"), true).unwrap();
    let module = project
        .modules()
        .unwrap()
        .into_iter()
        .find(|module| module.name.as_deref() == Some("Sales"))
        .expect("the scaffolded module reached the model");
    // The default chain template is data-backed, so the slice includes the
    // backing entity and its loader alongside the two refresh flows.
    let entities = module
        .entities()
        .iter()
        .filter_map(|entity| entity.name.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(entities, ["OrderOverview"]);
    let attributes = module.entities()[0]
        .attributes
        .iter()
        .filter_map(|attribute| attribute.name.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(attributes, ["Reference", "Total", "Active"]);
    let microflows = module
        .microflows
        .iter()
        .filter_map(|flow| flow.name.as_deref())
        .collect::<Vec<_>>();
    for expected in ["ACT_LoadOrderOverview", "ACT_RefreshOrderOverview"] {
        assert!(microflows.contains(&expected), "{expected}: {microflows:?}");
    }
    assert_eq!(
        module
            .nanoflows
            .iter()
            .filter_map(|flow| flow.name.as_deref())
            .collect::<Vec<_>>(),
        ["NAN_RefreshOrderOverview"]
    );
    assert_eq!(
        module
            .pages
            .iter()
            .filter_map(|page| page.name.as_deref())
            .collect::<Vec<_>>(),
        ["OrderOverview"]
    );
}

#[test]
fn every_catalogued_page_template_compiles_on_its_own() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    for template in mxrs_scaffold::page_templates::ENTRIES {
        // One page per template, named after it so the generated slices do
        // not collide.
        let name = format!(
            "Sales.{}Page",
            template
                .name
                .split('-')
                .map(|part| {
                    let mut characters = part.chars();
                    match characters.next() {
                        Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<String>()
        );
        scaffold_artifact(
            &ArtifactScaffold::new(ArtifactKind::Page, &name, &root)
                .page_template(Some(template.name.to_string())),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    }

    let output = cargo(&root, &["build", "--offline", "--quiet"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_unknown_template_or_chain_is_rejected_before_anything_is_written() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    assert!(matches!(
        PageChain::parse("page:microflow:nanoflow"),
        Err(ScaffoldError::UnknownPageChain(value)) if value == "page:microflow:nanoflow"
    ));
    let error = scaffold_artifact(
        &ArtifactScaffold::new(ArtifactKind::Page, "Sales.Broken", &root)
            .page_template(Some("form-horizontal".into())),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ScaffoldError::UnknownPageTemplate(value) if value == "form-horizontal"
    ));
    assert!(!root.join("src/domain/modules/sales/pages").exists());
}
