//! The only evidence that matters for a Rust scaffold: the project it
//! produces still compiles, and the declarations it adds actually reach the
//! writer. mxrb's Ruby templates can mention a DSL method that does not exist
//! because nothing parses them until `project.rb` runs; a Rust template that
//! did the same would break `cargo check` for the user's whole project.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use mxrs_scaffold::lifecycle::{ProjectLayout, migrate_project_layers, project_layout};
use mxrs_scaffold::{
    ArtifactKind, ArtifactScaffold, MxrsDependency, PageChain, ProjectScaffold, ScaffoldError,
    generate_project, inspect_project, registry, scaffold_artifact,
};

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

static NESTED_CARGO: Mutex<()> = Mutex::new(());

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .expect("the scaffold crate always lives inside the workspace")
}

fn application(directory: &Path) -> PathBuf {
    let destination = directory.join("application");
    // Shared dependency caches must not share a final executable name between
    // different fixtures. A per-command mutex alone does not give those
    // artifacts distinct identities across cached builds and test binaries.
    let mut identity = std::collections::hash_map::DefaultHasher::new();
    destination.hash(&mut identity);
    let name = format!("Order Portal {:016x}", identity.finish());
    generate_project(
        &ProjectScaffold::new(name, "11.12.1", &destination)
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

fn cargo_env(root: &Path, arguments: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    let _guard = NESTED_CARGO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Command::new(env!("CARGO"))
        .args(arguments)
        .envs(envs.iter().copied())
        .current_dir(root)
        .env(
            "CARGO_TARGET_DIR",
            nested_cargo::target_dir(workspace().join("target")),
        )
        .output()
        .unwrap()
}

fn cargo(root: &Path, arguments: &[&str]) -> std::process::Output {
    // Keep build and launch together while sharing the nested dependency
    // cache. Each fixture also has its own executable identity above.
    let _guard = NESTED_CARGO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    // A nanoflow is the frontend's: a method of the service of its subject.
    // Nothing names it in Rust: a page of the frontend calls it by name.
    let service =
        std::fs::read_to_string(root.join("frontend/src/services/sales/orderService.ts")).unwrap();
    assert!(
        service.contains("export const OrderService = nanoflowService(\"Sales\", {")
            && service.contains("   * @nanoflow NAN_RefreshOrder\n")
            && service.contains("  async refresh(): Promise<void> {},\n"),
        "{service}"
    );
    assert!(!root.join("src/ui/nanoflows").exists());
    scaffold(&root, ArtifactKind::PublishedRest, "Sales.HandleOrder");
    scaffold(&root, ArtifactKind::ConsumedRest, "Sales.FetchCatalog");
    scaffold(&root, ArtifactKind::JavaAction, "Sales.InvokeCheckout");
    scaffold(&root, ArtifactKind::Repository, "Sales.Orders");
    scaffold(&root, ArtifactKind::Security, "Sales");
    scaffold_artifact(
        &ArtifactScaffold::new(ArtifactKind::DemoUser, "Support", &root)
            .page_roles(vec!["User".to_string()]),
    )
    .unwrap();
    scaffold_artifact(
        &ArtifactScaffold::new(ArtifactKind::Page, "Sales.OrderOverview", &root)
            .page_roles(vec!["Sales.User".to_string()]),
    )
    .unwrap();

    // Everything scaffolded — and the project it was scaffolded into — is
    // already the way rustfmt would leave it: generated source is edited by
    // people, and their first `cargo fmt` should change nothing.
    let formatted = cargo(&root, &["fmt", "--check"]);
    assert!(
        formatted.status.success(),
        "{}",
        String::from_utf8_lossy(&formatted.stdout)
    );

    let output = cargo_env(
        &root,
        &["run", "--offline", "--quiet", "--", "build/Sales.mpr"],
        &[("MXRS_DEMO_USER_SUPPORT_PASSWORD", "FromEnv1!")],
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
    // The demo user reached ProjectSecurity with the password resolved from
    // the environment of the nested build — never from generated source.
    let security = project
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .find(|document| document.get_str("$Type").ok() == Some("Security$ProjectSecurity"))
        .expect("project security document");
    let Some(mxrs_bson::Bson::Array(raw)) = security.get("DemoUsers") else {
        panic!("DemoUsers array missing")
    };
    let users = mxrs_bson::parse_array(Some(raw)).items;
    let [mxrs_bson::Bson::Document(support)] = users.as_slice() else {
        panic!("expected exactly the scaffolded demo user: {users:?}")
    };
    assert_eq!(support.get_str("UserName").unwrap(), "Support");
    assert_eq!(support.get_str("Password").unwrap(), "FromEnv1!");
    assert_eq!(support.get_str("Entity").unwrap(), "System.User");
    let generated = std::fs::read_to_string(root.join("src/domain/demo_users/support.rs")).unwrap();
    assert!(
        !generated.contains("FromEnv1!"),
        "the password value must never appear in generated source"
    );
    let env_file = std::fs::read_to_string(root.join(".env")).unwrap();
    assert!(env_file.contains("MXRS_DEMO_USER_SUPPORT_PASSWORD=Mxrs"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.join(".env"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, ".env must stay private");
    }
    let example = std::fs::read_to_string(root.join(".env.example")).unwrap();
    assert!(example.contains("MXRS_DEMO_USER_SUPPORT_PASSWORD=\n"));
}

#[test]
fn demo_user_scaffolds_fail_closed_on_missing_prerequisites() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    // Security has not been initialized yet.
    assert!(matches!(
        scaffold_artifact(&ArtifactScaffold::new(
            ArtifactKind::DemoUser,
            "Support",
            &root
        )),
        Err(ScaffoldError::SecurityNotInitialized(_))
    ));
    scaffold(&root, ArtifactKind::Module, "Sales");
    scaffold(&root, ArtifactKind::Security, "Sales");
    // A role the security declaration never names.
    assert!(matches!(
        scaffold_artifact(
            &ArtifactScaffold::new(ArtifactKind::DemoUser, "Support", &root)
                .page_roles(vec!["Ghost".to_string()])
        ),
        Err(ScaffoldError::UnknownDemoUserRole(role)) if role == "Ghost"
    ));
    // An entity the domain layer does not declare.
    assert!(matches!(
        scaffold_artifact(
            &ArtifactScaffold::new(ArtifactKind::DemoUser, "Support", &root)
                .demo_entity(Some("Sales.Missing".to_string()))
        ),
        Err(ScaffoldError::UnknownDemoUserEntity(entity)) if entity == "Sales.Missing"
    ));
    // A scaffolded entity satisfies the structural reference check.
    scaffold(&root, ArtifactKind::Entity, "Sales.Account");
    let outcome = scaffold_artifact(
        &ArtifactScaffold::new(ArtifactKind::DemoUser, "Support", &root)
            .demo_entity(Some("Sales.Account".to_string()))
            .dry_run(true),
    )
    .unwrap();
    assert!(outcome.dry_run);
    assert!(
        !root.join("src/domain/security/demo_users").exists(),
        "dry-run writes nothing"
    );
    assert!(!root.join(".env").exists(), "dry-run creates no secret");
}

#[test]
fn scaffolding_the_same_artifact_twice_changes_nothing_the_first_run_wrote() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    let first = scaffold(&root, ArtifactKind::Entity, "Sales.Order");
    let aggregator = root.join("src/domain/entities/sales/mod.rs");
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
    // An entity registers itself, so its folder's index is a list of
    // modules and nothing else: there is no `apply` to keep in step.
    assert_eq!(
        std::fs::read_to_string(&aggregator).unwrap(),
        "//! The Sales module's `entities`.\n\npub mod invoice;\npub mod order;\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/domain/entities/sales/order.rs")).unwrap(),
        "//! Entity `Sales.Order`.\n//!\n//! Fields are attributes (`MxString`, `MxDecimal`, `MxBool`, ...) and\n//! associations (`Reference<T>`, `ReferenceSet<T>`); `#[mxrs(...)]`\n//! states what a type cannot: `length`, `required`, `default`, `index(...)`.\n\nuse mxrs::prelude::*;\n\n#[entity(module = \"Sales\")]\npub struct Order {}\n"
    );
    // The first entity creates the concept index and its module folder index
    // as well; the second reuses both.
    assert_eq!(first.len(), 3);

    let inspection = inspect_project(&root).unwrap();
    assert_eq!(inspection.modules, ["main", "sales"]);
    assert_eq!(inspection.declared_version.as_deref(), Some("11.12.1"));
    assert_eq!(
        inspection.registered_scaffolds,
        ["entity:Sales.Invoice", "entity:Sales.Order", "module:Sales"]
    );
    assert!(inspection.manifest && inspection.domain_module);
}

#[test]
fn repository_scaffold_separates_the_port_from_its_adapter() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    let files = scaffold(&root, ArtifactKind::Repository, "Sales.Orders");
    assert!(
        files
            .iter()
            .any(|path| path.ends_with("ports/repositories/orders.rs"))
    );
    assert!(
        files
            .iter()
            .any(|path| { path.ends_with("infrastructure/repositories/orders_implementation.rs") })
    );
    let port = std::fs::read_to_string(root.join("src/ports/repositories/orders.rs")).unwrap();
    let adapter = std::fs::read_to_string(
        root.join("src/infrastructure/repositories/orders_implementation.rs"),
    )
    .unwrap();
    assert!(port.contains("pub trait Port"));
    assert!(adapter.contains("impl crate::ports::repositories::orders::Port"));
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
    let aggregator = root.join("src/domain/entities/sales/mod.rs");
    assert!(
        std::fs::read_to_string(&aggregator)
            .unwrap()
            .contains("pub mod order;")
    );
    scaffold(&root, ArtifactKind::Entity, "Sales.Invoice");
    let aggregator_source = std::fs::read_to_string(&aggregator).unwrap();
    assert!(
        aggregator_source.contains("pub mod invoice;\npub mod order;\n"),
        "{aggregator_source}"
    );
    scaffold(&root, ArtifactKind::Module, "Billing");
    assert_eq!(
        std::fs::read_to_string(root.join("src/domain/modules/mod.rs")).unwrap(),
        "//! The Mendix modules this project declares, one file each.\n\npub mod billing;\npub mod main;\npub mod sales;\n"
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
        std::fs::read_to_string(root.join("src/domain/mod.rs"))
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

    // A scaffold never has to understand how a project composes its model:
    // a declaration registers itself, so the only edit to existing source is
    // a `pub mod` line. A layer restructured by hand is therefore scaffolded
    // into, not refused — and what was written there by hand is left alone.
    std::fs::write(
        root.join("src/domain/mod.rs"),
        "pub fn build() -> mxrs::ProjectDecl { unimplemented!() }\n",
    )
    .unwrap();
    scaffold(&root, ArtifactKind::Module, "Sales");
    assert_eq!(
        std::fs::read_to_string(root.join("src/domain/mod.rs")).unwrap(),
        "pub mod modules;\n\npub fn build() -> mxrs::ProjectDecl { unimplemented!() }\n"
    );
    assert!(!root.join("src/modules").exists());
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
        if !advertised.starts_with("src/") && !advertised.starts_with("frontend/src/") {
            continue;
        }
        // A `<Module>` argument names a module rather than an artifact inside
        // one; `module` must name a fresh one, the rest target `Sales`.
        let name = match (command.kind, command.argument) {
            (ArtifactKind::Module, _) => format!("Module{index}"),
            (_, "<Module>") => "Sales".to_string(),
            // A demo user is a plain identifier, and its role must be one the
            // scaffolded security declaration names.
            (ArtifactKind::DemoUser, _) => format!("Artifact{index}"),
            _ => format!("Sales.Artifact{index}"),
        };
        // A microflow joins its subject's service: the second one of a
        // module adds a method to a file the first created.
        let outcome = scaffold_artifact(&ArtifactScaffold::new(command.kind, &name, &root))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let files = [outcome.files, outcome.updated].concat();
        checked += 1;

        // `repository` is the one deliberately two-layer artifact: its port
        // lives in `ports` and its adapter in `infrastructure`.
        let expected: Vec<String> = if advertised.contains('{') {
            vec!["src/ports".to_string(), "src/infrastructure".to_string()]
        } else {
            let module = match (command.kind, command.argument) {
                (ArtifactKind::Module, _) => format!("module{index}"),
                (_, "<Module>") => "sales".to_string(),
                _ => "sales".to_string(),
            };
            vec![advertised.replace("<module>", &module)]
        };
        for destination in &expected {
            assert!(
                files
                    .iter()
                    .any(|file| file.to_string_lossy().contains(destination.as_str())),
                "{name} wrote nothing under {destination}; catalog advertises {advertised}"
            );
        }
    }
    // Guards against the loop silently degenerating if `destination` spellings
    // ever change shape.
    assert_eq!(checked, 17);
}

/// Module content composes through `src/domain/mod.rs`'s `build()` and the
/// crate root. A project missing either is reported rather than repaired by
/// guesswork, so a scaffold never invents the shape it wanted to find.
#[test]
fn scaffolding_into_a_project_missing_its_composition_root_fails_closed() {
    for (removed, kind, name) in [
        (
            "src/domain/mod.rs",
            ArtifactKind::UseCase,
            "Sales.ACT_CreateOrder",
        ),
        (
            "src/lib.rs",
            ArtifactKind::Nanoflow,
            "Sales.NAN_RefreshOrder",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = application(directory.path());
        scaffold(&root, ArtifactKind::Module, "Sales");
        std::fs::remove_file(root.join(removed)).unwrap();
        let error = scaffold_artifact(&ArtifactScaffold::new(kind, name, &root)).unwrap_err();
        assert!(
            matches!(
                error,
                ScaffoldError::AggregatorNotFound(_)
                    | ScaffoldError::ProjectNotFound(_)
                    | ScaffoldError::ModuleNotFound(_)
            ),
            "{removed}: {error:?}"
        );
        assert!(!root.join("src/modules").exists(), "{removed}");
    }
}

#[test]
fn a_migrated_pre_layered_project_compiles_and_accepts_new_layered_scaffolds() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    // A pre-layered project builds its whole model by hand in one `domain`
    // module, and its application attribute names no entry point.
    std::fs::remove_dir_all(root.join("src/domain")).unwrap();
    std::fs::create_dir_all(root.join("src/domain")).unwrap();
    std::fs::write(
        root.join("src/domain/mod.rs"),
        concat!(
            "pub fn build() -> mxrs::ProjectDecl {\n",
            "    let mut builder = mxrs::ProjectBuilder::new(\"11.12.1\");\n",
            "    builder.module(\"Main\", |_module| {});\n",
            "    builder.build()\n",
            "}\n"
        ),
    )
    .unwrap();
    let domain = std::fs::read(root.join("src/domain/mod.rs")).unwrap();
    std::fs::remove_dir_all(root.join("src/services")).unwrap();
    // Nor has it a frontend declaring the navigation.
    std::fs::remove_dir_all(root.join("frontend")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        concat!(
            "mod domain;\n",
            "pub mod infrastructure;\n\n",
            "#[mxrs::application(version = \"11.12.1\")]\n",
            "pub struct Application;\n"
        ),
    )
    .unwrap();
    assert_eq!(project_layout(&root).unwrap(), ProjectLayout::PreLayered);

    let preview = migrate_project_layers(&root, false).unwrap();
    assert!(preview.migrated_layers);
    assert!(!root.join("src/services/mod.rs").exists());
    let applied = migrate_project_layers(&root, true).unwrap();
    assert!(applied.migrated_layers);
    assert_eq!(
        std::fs::read(root.join("src/domain/mod.rs")).unwrap(),
        domain
    );
    assert_eq!(project_layout(&root).unwrap(), ProjectLayout::Layered);

    // The migrated project still composes its hand-built model through an
    // explicit entry point; what is scaffolded from here on registers
    // itself beside it.
    scaffold(&root, ArtifactKind::Module, "Sales");
    scaffold(&root, ArtifactKind::UseCase, "Sales.ACT_CreateOrder");
    scaffold(&root, ArtifactKind::Nanoflow, "Sales.NAN_RefreshOrder");
    let output = cargo(
        &root,
        &["run", "--offline", "--quiet", "--", "build/Migrated.mpr"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let project = mxrs_model::Project::open(root.join("build/Migrated.mpr"), true).unwrap();
    let sales = project
        .modules()
        .unwrap()
        .into_iter()
        .find(|module| module.name.as_deref() == Some("Sales"))
        .expect("the migrated composition root includes layered scaffolds");
    assert!(
        sales
            .microflows
            .iter()
            .any(|flow| flow.name.as_deref() == Some("ACT_CreateOrder"))
    );
    assert!(
        sales
            .nanoflows
            .iter()
            .any(|flow| flow.name.as_deref() == Some("NAN_RefreshOrder"))
    );
}

/// A layer index a user emptied by hand is filled back in, once per concept:
/// the scaffold declares the module it needs and touches nothing else, so
/// there is no composition call to splice and no shape to recognize.
#[test]
fn a_hand_emptied_layer_index_is_declared_into_again() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    let domain = root.join("src/domain/mod.rs");
    std::fs::write(&domain, "").unwrap();

    scaffold(&root, ArtifactKind::Module, "Sales");
    scaffold(&root, ArtifactKind::Entity, "Sales.Order");
    scaffold(&root, ArtifactKind::Entity, "Sales.Invoice");
    scaffold(&root, ArtifactKind::UseCase, "Sales.ACT_CreateOrder");
    scaffold(&root, ArtifactKind::Nanoflow, "Sales.NAN_RefreshOrder");

    // Exactly one declaration per concept, not one per scaffolded artifact.
    assert_eq!(
        std::fs::read_to_string(&domain).unwrap(),
        "pub mod entities;\npub mod modules;\n"
    );
    let entities = std::fs::read_to_string(root.join("src/domain/entities/mod.rs")).unwrap();
    assert_eq!(
        entities,
        "//! Every module's `entities`, one folder per Mendix module.\n\npub mod sales;\n"
    );

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
    let navigation = project.navigation().unwrap();
    let responsive = navigation
        .profiles
        .iter()
        .find(|profile| profile.name == "Responsive")
        .expect("the generated application has a Responsive profile");
    let item = responsive
        .menu_items
        .iter()
        .find(|item| item.caption.get("en_US") == Some(&"Order Overview".to_string()))
        .expect("the page chain is reachable from navigation");
    assert!(item.page.is_some());
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
    assert!(!root.join("src/ui/pages/sales").exists());
}

#[test]
fn presentation_initialization_previews_compiles_and_keeps_a_layout_already_declared() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    let options = ArtifactScaffold::new(ArtifactKind::Presentation, "Sales", &root);
    assert!(scaffold_artifact(&options).is_err());
    let declared = root.join("frontend/src/components/layout/sales/ApplicationLayout.tsx");
    assert!(!declared.exists());
    scaffold(&root, ArtifactKind::Module, "Sales");
    // A layout the project still declares in Rust is not declared again.
    let rust = root.join("src/ui/layouts/sales/application_layout.rs");
    std::fs::create_dir_all(rust.parent().unwrap()).unwrap();
    std::fs::write(&rust, "custom layout source").unwrap();
    assert!(scaffold_artifact(&options).is_err());
    assert_eq!(
        std::fs::read_to_string(&rust).unwrap(),
        "custom layout source"
    );
    assert!(!declared.exists());
    std::fs::remove_dir_all(root.join("src/ui/layouts")).unwrap();
    let before = registry::entries(&root).unwrap();
    let elements = root.join("frontend/src/mxrs/elements.ts");
    let known = std::fs::read_to_string(&elements).unwrap();
    let preview = scaffold_artifact(&options.clone().dry_run(true)).unwrap();
    assert!(!declared.exists());
    assert_eq!(std::fs::read_to_string(&elements).unwrap(), known);
    assert_eq!(registry::entries(&root).unwrap(), before);
    let applied = scaffold_artifact(&options).unwrap();
    assert_eq!(preview.files, applied.files);
    assert_eq!(preview.updated, applied.updated);
    // The layout is the frontend's, and the elements it is written with
    // follow the ones the project already had.
    let source = std::fs::read_to_string(&declared).unwrap();
    assert!(
        source.contains("export default layout(\n  \"Sales\",")
            && source.contains("name=\"ApplicationLayout\""),
        "{source}"
    );
    let grown = std::fs::read_to_string(&elements).unwrap();
    assert!(grown.starts_with(known.trim_end()) && grown.len() > known.len());
    // Declared once: a second run is refused and changes nothing.
    assert!(scaffold_artifact(&options).is_err());
    assert_eq!(std::fs::read_to_string(&declared).unwrap(), source);
    let output = cargo(
        &root,
        &[
            "run",
            "--offline",
            "--quiet",
            "--",
            "build/Presentation.mpr",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let project = mxrs_model::Project::open(root.join("build/Presentation.mpr"), true).unwrap();
    assert!(project.all_units().unwrap().iter().any(|unit| {
        let doc = project.mpr().parse_contents(unit).unwrap();
        doc.get_str("Name").ok() == Some("ApplicationLayout")
            && unit.containment_name == "Documents"
    }));
}

/// A scaffolded microflow joins the service its subject names, under the
/// name the service's own attribute gives it; the same flow twice is
/// refused rather than declared again.
#[test]
fn a_scaffolded_flow_is_named_by_the_service_it_joins() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    let service = root.join("src/services/main/main_service.rs");

    scaffold(&root, ArtifactKind::UseCase, "Main.ACT_Main_Report");
    // `Main_Export` opens with the word of an existing service, the module's
    // own, which states no subject: the flow's name is stated.
    scaffold_artifact(&ArtifactScaffold::new(
        ArtifactKind::UseCase,
        "Main.ACT_Main_Export",
        &root,
    ))
    .unwrap();
    let source = std::fs::read_to_string(&service).unwrap();
    assert!(
        source.contains("#[microflow(ACT, name = \"ACT_Main_Export\")]\n    pub fn export("),
        "{source}"
    );
    assert_eq!(source.matches("#[service(").count(), 1, "{source}");
    let again = scaffold_artifact(&ArtifactScaffold::new(
        ArtifactKind::UseCase,
        "Main.ACT_Main_Export",
        &root,
    ));
    assert!(
        matches!(&again, Err(mxrs_scaffold::ScaffoldError::FileExists(_))),
        "{again:?}"
    );
    assert_eq!(std::fs::read_to_string(&service).unwrap(), source);

    // A subject declared by a DTO is imported from where the DTO lives.
    let dtos = root.join("src/domain/dtos/main");
    std::fs::create_dir_all(&dtos).unwrap();
    std::fs::write(dtos.join("order_request.rs"), "pub struct OrderRequest;\n").unwrap();
    scaffold(&root, ArtifactKind::UseCase, "Main.ACT_OrderRequest_Send");
    let request =
        std::fs::read_to_string(root.join("src/services/main/order_request_service.rs")).unwrap();
    assert!(
        request.contains("use crate::domain::dtos::main::order_request::OrderRequest;")
            && request.contains("#[service(module = \"Main\", subject = OrderRequest)]")
            && request.contains("#[microflow(ACT)]\n    pub fn send("),
        "{request}"
    );
}

#[test]
fn a_scaffolded_nanoflow_joins_its_service_without_touching_other_imports() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    let services = root.join("frontend/src/services/main");

    // Before the module has an `Order`, the flow is the module's own.
    scaffold(&root, ArtifactKind::Nanoflow, "Main.ACT_Order_Open");
    let main = services.join("mainService.ts");
    assert!(
        std::fs::read_to_string(&main)
            .unwrap()
            .contains("@nanoflow ACT_Order_Open\n")
    );
    // Once it has one the subject rule would place it elsewhere: the service
    // that declares it still does, and a second declaration is refused.
    scaffold(&root, ArtifactKind::Entity, "Main.Order");
    let again = scaffold_artifact(&ArtifactScaffold::new(
        ArtifactKind::Nanoflow,
        "Main.ACT_Order_Open",
        &root,
    ));
    assert!(
        matches!(&again, Err(mxrs_scaffold::ScaffoldError::FileExists(at)) if at.contains("mainService.ts")),
        "{again:?}"
    );
    assert!(!services.join("orderService.ts").exists());

    // Another import above the vocabulary's is another statement.
    std::fs::write(
        &main,
        "import { OtherService } from \"@/services/main/otherService\";\nimport { closePage, nanoflowService, type MxObject } from \"@/mxrs/flows\";\n\nexport const MainService = nanoflowService(\"Main\", {\n  /** @nanoflow ACT_Close */\n  async close(order: MxObject<\"Main.Order\">): Promise<void> {\n    await closePage();\n    await OtherService.f();\n  },\n});\n",
    )
    .unwrap();
    scaffold(&root, ArtifactKind::Nanoflow, "Main.ACT_Refresh");
    let source = std::fs::read_to_string(&main).unwrap();
    assert!(
        source.starts_with(
            "import { OtherService } from \"@/services/main/otherService\";\nimport { closePage, nanoflowService, type MxObject } from \"@/mxrs/flows\";\n"
        ) && source.contains("@nanoflow ACT_Refresh\n"),
        "{source}"
    );
}

/// What a page shares with the module's other pages outlives it: the layout
/// is a scaffold of its own, the elements are the project's, and the page
/// takes only its own file and its navigation item with it.
#[test]
fn destroying_a_page_leaves_what_other_pages_are_written_with() {
    let directory = tempfile::tempdir().unwrap();
    let root = application(directory.path());
    scaffold(&root, ArtifactKind::Module, "Sales");
    let navigation = root.join("frontend/src/navigation/index.ts");
    let fresh = std::fs::read_to_string(&navigation).unwrap();
    scaffold(&root, ArtifactKind::Page, "Sales.Plain");
    scaffold_artifact(
        &ArtifactScaffold::new(ArtifactKind::Page, "Sales.Dash", &root)
            .page_template(Some("dashboard".to_string())),
    )
    .unwrap();
    let layout = root.join("frontend/src/components/layout/sales/ApplicationLayout.tsx");
    let elements = root.join("frontend/src/mxrs/elements.ts");
    let keys: Vec<String> = registry::entries(&root)
        .unwrap()
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    assert!(
        keys.contains(&"layout:Sales.ApplicationLayout".to_string()),
        "{keys:?}"
    );
    assert!(
        std::fs::read_to_string(&navigation)
            .unwrap()
            .contains("page: \"Sales.Dash\"")
    );

    registry::destroy(&root, "page:Sales.Plain").unwrap();
    assert!(!root.join("frontend/src/pages/sales/Plain.tsx").exists());
    assert!(layout.is_file() && elements.is_file());
    registry::destroy(&root, "page:Sales.Dash").unwrap();
    assert!(!root.join("frontend/src/pages/sales/Dash.tsx").exists());
    assert!(layout.is_file() && elements.is_file());
    // The item the page added to the navigation went with it.
    assert_eq!(std::fs::read_to_string(&navigation).unwrap(), fresh);

    // A name a file system may not tell from a page's is not a new page.
    scaffold(&root, ArtifactKind::Page, "Sales.Plain");
    let again = scaffold_artifact(&ArtifactScaffold::new(
        ArtifactKind::Page,
        "Sales.plain",
        &root,
    ));
    assert!(
        matches!(&again, Err(ScaffoldError::FileExists(_))),
        "{again:?}"
    );
}
