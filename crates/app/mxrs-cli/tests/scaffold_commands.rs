//! CLI contract of the artifact generators. The generated *content* is proven
//! by `mxrs-scaffold`'s own tests (which compile and run the project they
//! scaffold); what matters here is the surface a user types against: action
//! words, option handling, exit codes, and the two renderings.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mxrs_scaffold::SCAFFOLD_COMMANDS;
use serde_json::Value;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mxrs"))
        .args(args)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn project(directory: &Path) -> PathBuf {
    let root = directory.join("app");
    let output = cli(&[
        "new",
        "Order Portal",
        "--output",
        root.to_str().unwrap(),
        "--mxrs-workspace",
        workspace().to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{:?}", output.stderr);
    root
}

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .expect("the CLI crate always lives inside the workspace")
}

fn scaffold(root: &Path, arguments: &[&str]) -> Output {
    let mut args = arguments.to_vec();
    args.extend(["--target", root.to_str().unwrap()]);
    cli(&args)
}

#[test]
fn the_dispatcher_usage_line_matches_the_generator_catalog_for_every_command() {
    let output = cli(&["--commands", "--json"]);
    let catalog: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    for command in SCAFFOLD_COMMANDS {
        let entry = catalog
            .iter()
            .find(|entry| entry["name"] == command.name)
            .unwrap_or_else(|| panic!("{} is not dispatchable", command.name));
        assert_eq!(
            entry["usage"].as_str().unwrap(),
            format!("mxrs {}", mxrs_cli::scaffold::usage(command.kind)),
            "{}",
            command.name
        );
    }
    for required in ["module", "project", "scaffold", "security", "use-case"] {
        assert!(catalog.iter().any(|entry| entry["name"] == required));
    }
}

#[test]
fn generating_an_artifact_reports_created_files_and_the_build_command() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    let output = scaffold(&root, &["entity", "new", "Sales.Order"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let rendered = text(&output);
    assert!(rendered.contains(&format!(
        "  create  {}",
        root.join("src/domain/modules/sales/entities/order.rs").display()
    )));
    assert!(rendered.contains(&format!(
        "  update  {}",
        root.join("src/domain/modules/sales/mod.rs").display()
    )));
    assert!(rendered.ends_with("\nDone. Run:\n  cargo mxrs build\n"));
}

#[test]
fn functional_test_scaffold_is_a_valid_plan_and_never_overwrites() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    let generated = scaffold(&root, &["functional-test", "new", "Sales.Start", "--json"]);
    assert!(generated.status.success(), "{:?}", generated.stderr);
    let suite = root.join("functional_tests/start.json");
    let original = std::fs::read_to_string(&suite).unwrap();
    let document: Value = serde_json::from_str(&original).unwrap();
    assert_eq!(document["tests"][0]["name"], "Start");
    assert_eq!(document["tests"][0]["call"], "Sales.Start");

    let mpr = root.join("Start.mpr");
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.microflow("Start", |_| {});
    });
    mxrs_writer::write_project(&mpr, &builder.build()).unwrap();
    let plan = cli(&[
        "test",
        mpr.to_str().unwrap(),
        suite.to_str().unwrap(),
        "--plan",
        "--json",
    ]);
    assert!(plan.status.success(), "{:?}", plan.stderr);

    assert!(
        !scaffold(&root, &["functional-test", "new", "Sales.Start"])
            .status
            .success()
    );
    assert_eq!(std::fs::read_to_string(&suite).unwrap(), original);
}

#[test]
fn quality_integration_and_ci_scaffolds_emit_consumable_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    assert!(
        scaffold(&root, &["validation", "new", "Sales.ValidateOrder"])
            .status
            .success()
    );
    assert!(
        scaffold(&root, &["integration", "new", "Sales.SyncOrders"])
            .status
            .success()
    );
    // Validations and integrations are server-side use cases, so they land in
    // the application layer rather than next to the model declarations.
    assert!(
        root.join("src/application/modules/sales/validations/validate_order.rs")
            .is_file()
    );
    assert!(
        root.join("src/application/modules/sales/integrations/sync_orders.rs")
            .is_file()
    );
    assert!(!root.join("src/domain/modules/sales/validations").exists());

    assert!(
        scaffold(&root, &["evaluation", "new", "Architecture"])
            .status
            .success()
    );
    let evaluation = root.join("evaluations/architecture.json");
    let definition: Value =
        serde_json::from_str(&std::fs::read_to_string(evaluation).unwrap()).unwrap();
    assert_eq!(definition["checks"].as_array().unwrap().len(), 2);

    assert!(scaffold(&root, &["ci", "init", "github"]).status.success());
    let workflow = std::fs::read_to_string(root.join(".github/workflows/mxrs.yml")).unwrap();
    assert!(workflow.contains("cargo clippy --workspace --all-targets -- -D warnings"));
    assert!(!scaffold(&root, &["ci", "init", "gitlab"]).status.success());
}

#[test]
fn a_wrong_action_word_or_missing_project_fails_without_writing_anything() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    for arguments in [
        vec!["entity", "create", "Sales.Order"],
        vec!["entity", "new"],
        vec!["security", "new", "Sales"],
        vec!["module", "init", "Sales"],
        vec!["entity", "new", "Sales.Order", "--role", "Sales.User"],
        vec![
            "page",
            "new",
            "Sales.Home",
            "--target",
            "a",
            "--target",
            "b",
        ],
    ] {
        let output = scaffold(&root, &arguments);
        assert!(!output.status.success(), "{arguments:?}");
    }
    // `mxrs new` ships an empty `modules` aggregator in every layer, so the
    // evidence that nothing was written is the absence of the module itself.
    for layer in ["domain", "application", "presentation"] {
        assert!(!root.join(format!("src/{layer}/modules/sales")).exists());
    }
    let outside = cli(&[
        "entity",
        "new",
        "Sales.Order",
        "--target",
        directory.path().to_str().unwrap(),
    ]);
    assert!(!outside.status.success());
    assert!(
        String::from_utf8(outside.stderr)
            .unwrap()
            .contains("not a Cargo-native MXRS project")
    );
}

#[test]
fn a_dry_run_renders_the_json_document_without_touching_the_project() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    let output = scaffold(
        &root,
        &[
            "page",
            "new",
            "Sales.OrderOverview",
            "--role",
            "Sales.User",
            "--role",
            "Sales.Administrator",
            "--dry-run",
            "--json",
        ],
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["kind"], "page");
    assert_eq!(document["name"], "Sales.OrderOverview");
    assert_eq!(document["dry_run"], true);
    let files = document["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .all(|file| !Path::new(file.as_str().unwrap()).exists())
    );
    assert!(!document["updated"].as_array().unwrap().is_empty());

    let written = scaffold(
        &root,
        &[
            "page",
            "new",
            "Sales.OrderOverview",
            "--role",
            "Sales.User",
            "--json",
        ],
    );
    assert!(written.status.success());
    let written: Value = serde_json::from_slice(&written.stdout).unwrap();
    assert_eq!(written["files"], document["files"]);
    assert!(
        std::fs::read_to_string(
            root.join("src/presentation/modules/sales/pages/order_overview.rs")
        )
        .unwrap()
        .contains("page.allow_role(\"Sales.User\");")
    );
}

#[test]
fn listing_advertises_only_catalogued_generators_and_destroying_removes_their_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    assert!(
        scaffold(&root, &["enumeration", "new", "Sales.Status"])
            .status
            .success()
    );
    let listed = text(&scaffold(&root, &["scaffold", "list"]));
    let (generators, registered) = listed.split_once("\nregistered scaffold\tfiles\n").unwrap();
    let rows = generators.lines().skip(1).collect::<Vec<_>>();
    assert_eq!(rows.len(), SCAFFOLD_COMMANDS.len());
    for command in SCAFFOLD_COMMANDS {
        assert!(rows.contains(&format!("{}\t{}", command.name, command.destination).as_str()));
    }
    assert!(registered.contains("enumeration:Sales.Status\t2"));

    let destroyed = scaffold(&root, &["scaffold", "destroy", "enumeration:Sales.Status"]);
    assert!(destroyed.status.success());
    assert!(text(&destroyed).contains("  remove  "));
    // Only files are removed, as in mxrb: an emptied directory is left in
    // place rather than pruned, so nothing outside the manifest is touched.
    assert!(
        !root
            .join("src/domain/modules/sales/enumerations/mod.rs")
            .exists()
    );
    assert!(
        !root
            .join("src/domain/modules/sales/enumerations/status.rs")
            .exists()
    );
    assert!(
        !scaffold(&root, &["scaffold", "destroy", "enumeration:Sales.Status"])
            .status
            .success()
    );
}

#[test]
fn inspecting_reports_the_declared_version_modules_and_registered_scaffolds() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );
    assert!(
        scaffold(&root, &["security", "init", "Sales"])
            .status
            .success()
    );
    let output = cli(&["project", "inspect", root.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["declared_version"], "11.12.1");
    assert_eq!(document["manifest"], true);
    assert_eq!(document["domain_module"], true);
    assert_eq!(document["layout"], "layered");
    assert_eq!(document["modules"][0], "sales");
    assert_eq!(document["mprs"].as_array().unwrap().len(), 0);
    assert_eq!(
        document["registered_scaffolds"],
        serde_json::json!(["module:Sales", "security:Sales"])
    );
    let rendered = text(&cli(&["project", "inspect", root.to_str().unwrap()]));
    assert!(rendered.contains("declared_version: 11.12.1\n"));
    assert!(rendered.contains("registered_scaffolds: module:Sales, security:Sales\n"));
}

#[test]
fn upgrade_previews_then_transactionally_updates_a_generated_project() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    let command = |apply: bool| {
        let mut args = vec![
            "upgrade",
            "--mendix",
            "11.13.0",
            "--target",
            root.to_str().unwrap(),
            "--json",
        ];
        if apply {
            args.push("--apply");
        }
        cli(&args)
    };
    let preview = command(false);
    assert!(preview.status.success(), "{:?}", preview.stderr);
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["from"], "11.12.1");
    assert!(
        std::fs::read_to_string(root.join("src/lib.rs"))
            .unwrap()
            .contains("11.12.1")
    );

    let applied = command(true);
    assert!(applied.status.success(), "{:?}", applied.stderr);
    assert!(
        std::fs::read_to_string(root.join("src/lib.rs"))
            .unwrap()
            .contains("11.13.0")
    );
    assert!(
        !cli(&[
            "upgrade",
            "--mendix",
            "bad",
            "--target",
            root.to_str().unwrap()
        ])
        .status
        .success()
    );
}

#[test]
fn upgrade_without_a_version_migrates_a_pre_layered_project_and_reports_every_change() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    std::fs::remove_dir_all(root.join("src/application")).unwrap();
    std::fs::remove_dir_all(root.join("src/presentation")).unwrap();
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
    let domain = std::fs::read(root.join("src/domain/mod.rs")).unwrap();

    let before = cli(&["project", "inspect", root.to_str().unwrap(), "--json"]);
    assert!(before.status.success(), "{:?}", before.stderr);
    let before: Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(before["layout"], "pre-layered");

    let preview = cli(&["upgrade", "--target", root.to_str().unwrap(), "--json"]);
    assert!(preview.status.success(), "{:?}", preview.stderr);
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["from"], "11.12.1");
    assert_eq!(preview["to"], "11.12.1");
    assert_eq!(preview["migrated_layers"], true);
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["created"].as_array().unwrap().len(), 4);
    assert_eq!(preview["updated"].as_array().unwrap().len(), 1);
    assert!(!root.join("src/application/mod.rs").exists());

    let applied = cli(&["upgrade", "--target", root.to_str().unwrap(), "--apply"]);
    assert!(applied.status.success(), "{:?}", applied.stderr);
    let rendered = text(&applied);
    assert!(rendered.contains("layers: migrated"));
    assert!(rendered.contains("  create  "));
    assert!(rendered.contains("  update  "));
    assert_eq!(
        std::fs::read(root.join("src/domain/mod.rs")).unwrap(),
        domain
    );

    let after = cli(&["project", "inspect", root.to_str().unwrap(), "--json"]);
    let after: Value = serde_json::from_slice(&after.stdout).unwrap();
    assert_eq!(after["layout"], "layered");

    let repeated = cli(&[
        "upgrade",
        "--target",
        root.to_str().unwrap(),
        "--apply",
        "--json",
    ]);
    assert!(repeated.status.success(), "{:?}", repeated.stderr);
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["migrated_layers"], false);
    assert!(repeated["files"].as_array().unwrap().is_empty());
}

#[test]
fn the_constant_and_scheduled_event_generators_write_their_families_and_registry_keys() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );

    let constant = scaffold(&root, &["constant", "new", "Sales.ApiEndpoint"]);
    assert!(constant.status.success(), "{:?}", constant.stderr);
    let path = root.join("src/domain/modules/sales/constants/api_endpoint.rs");
    assert!(text(&constant).contains(&format!("  create  {}", path.display())));
    let source = std::fs::read_to_string(&path).unwrap();
    assert!(source.contains("module.constant(\"ApiEndpoint\""));

    let event = scaffold(&root, &["scheduled-event", "new", "Sales.SE_ExpireCarts"]);
    assert!(event.status.success(), "{:?}", event.stderr);
    let path = root.join("src/application/modules/sales/jobs/se_expire_carts.rs");
    assert!(text(&event).contains(&format!("  create  {}", path.display())));
    // Event and handler land in one file under one name, as in mxrb's
    // "scheduled event and handler" template.
    let source = std::fs::read_to_string(&path).unwrap();
    assert!(source.contains("module.microflow(\"SE_ExpireCarts\""));
    assert!(source.contains("module.scheduled_event(\"SE_ExpireCarts\", \"SE_ExpireCarts\""));

    // Registry keys underscore the command name, so `scaffold destroy` takes
    // `scheduled_event:...` rather than the dashed command spelling.
    let listed = text(&scaffold(&root, &["scaffold", "list"]));
    assert!(listed.contains("constant:Sales.ApiEndpoint"));
    assert!(listed.contains("scheduled_event:Sales.SE_ExpireCarts"));
}

#[test]
fn the_page_template_catalog_renders_as_a_tree_and_as_json() {
    let rendered = text(&cli(&["page", "templates"]));
    assert!(rendered.starts_with("Page templates\n"));
    assert!(rendered.contains("├── General\n"));
    assert!(rendered.contains("└── form-vertical — DataView with vertical inputs and actions"));

    let output = cli(&["page", "templates", "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let catalog: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(catalog[0]["category"], "General");
    assert_eq!(catalog[0]["templates"][0]["name"], "starter");
    assert_eq!(catalog[0]["templates"][0]["data_backed"], false);
    let forms = catalog.last().unwrap();
    assert_eq!(forms["category"], "Forms");
    assert_eq!(forms["templates"][0]["data_backed"], true);

    // `templates` is a second form of the command, not a page name: it must
    // not be combinable with the generator's own options.
    assert!(
        !cli(&["page", "templates", "--chain", "page:microflow"])
            .status
            .success()
    );
}

#[test]
fn a_chained_page_reports_every_file_of_the_slice_and_rejects_an_unknown_chain() {
    let directory = tempfile::tempdir().unwrap();
    let root = project(directory.path());
    assert!(
        scaffold(&root, &["module", "new", "Sales"])
            .status
            .success()
    );

    let bad = scaffold(
        &root,
        &["page", "new", "Sales.Broken", "--chain", "page:rest"],
    );
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown page chain"));
    assert!(!root.join("src/presentation/modules/sales/pages").exists());

    let output = scaffold(
        &root,
        &[
            "page",
            "new",
            "Sales.OrderOverview",
            "--chain",
            "page:nanoflow:microflow",
        ],
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    let rendered = text(&output);
    // The default chain template is data-backed, so the slice is the entity,
    // its loader, both refresh flows and the page — and the slice now spans
    // three layers instead of landing entirely under `src/domain/`.
    for (layer, relative) in [
        ("domain", "entities/order_overview.rs"),
        ("application", "use_cases/act_load_order_overview.rs"),
        ("application", "use_cases/act_refresh_order_overview.rs"),
        ("presentation", "nanoflows/nan_refresh_order_overview.rs"),
        ("presentation", "pages/order_overview.rs"),
    ] {
        let path = root
            .join(format!("src/{layer}/modules/sales"))
            .join(relative);
        assert!(
            rendered.contains(&format!("  create  {}", path.display())),
            "{layer}/{relative}: {rendered}"
        );
    }
    assert!(text(&scaffold(&root, &["scaffold", "list"])).contains("page:Sales.OrderOverview"));
}
