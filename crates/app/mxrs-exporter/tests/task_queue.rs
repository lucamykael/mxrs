//! Exercises task queues through both exporter surfaces: `export_project`'s
//! standalone source (must include queue declarations and fail closed on an
//! unattested native shape, same as any other editable document) and
//! `import_cargo_project`'s Cargo-native project (must emit editable queues
//! under `src/application/task_queues.rs` and remain buildable after the
//! source `.mpr` is gone — the whole point of `model/imported/` capturing a
//! lossless snapshot). Real nested `cargo` invocations, same rationale and
//! pattern as `compiles.rs`: generated-source correctness isn't provable by
//! string matching alone.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("xtask/Cargo.toml").is_file())
        .expect("workspace root contains xtask/Cargo.toml")
        .to_path_buf()
}

fn target_dir() -> PathBuf {
    nested_cargo::target_dir(workspace_root().join("target"))
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
    let name = format!("exporter-task-queue-fixture-{unique}");
    let cargo_toml = format!(
        "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nmxrs-macros = {{ path = {:?} }}\nmxrs-expr = {{ path = {:?} }}\nmxrs-ir = {{ path = {:?} }}\nmxrs-dsl = {{ path = {:?} }}\n",
        workspace_root().join("crates/authoring/mxrs-macros"),
        workspace_root().join("crates/authoring/mxrs-expr"),
        workspace_root().join("crates/authoring/mxrs-ir"),
        workspace_root().join("crates/authoring/mxrs-dsl"),
    );
    std::fs::write(dir.path().join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), body).unwrap();

    Command::new(env!("CARGO"))
        .arg("build")
        .arg("--offline")
        .current_dir(dir.path())
        .env("CARGO_TARGET_DIR", target_dir())
        .output()
        .expect("failed to invoke cargo for the fixture crate")
}

fn build_project_with_queues() -> mxrs_ir::ProjectDecl {
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Jobs", |module| {
        module.task_queue(
            "Imports",
            mxrs_ir::TaskQueueConfig::Fixed { parallelism: 4 },
            |queue| {
                queue
                    .documentation("Fixed-size import queue")
                    .excluded(true)
                    .export_level(mxrs_ir::ExportLevel::Published);
            },
        );
        module.task_queue(
            "Exports",
            mxrs_ir::TaskQueueConfig::Dynamic {
                parallelism_expression: "cpuCount".to_string(),
                scope: mxrs_ir::TaskQueueScope::ClusterWide,
            },
            |_| {},
        );
    });
    builder.build()
}

#[test]
fn standalone_export_includes_task_queue_declarations_and_compiles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Queues.mpr");
    mxrs_writer::write_project(&path, &build_project_with_queues()).unwrap();

    let source = mxrs_exporter::export_project(&path).unwrap();
    assert!(
        source.contains("mod task_queues"),
        "expected a task_queues module in:\n{source}"
    );
    assert!(
        source.contains("module.task_queue(\"Imports\"")
            && source.contains("TaskQueueConfig::Fixed { parallelism: 4 }"),
        "expected the fixed queue declaration in:\n{source}"
    );
    assert!(
        source.contains("module.task_queue(\"Exports\"")
            && source.contains("parallelism_expression: \"cpuCount\".to_string()")
            && source.contains("TaskQueueScope::ClusterWide"),
        "expected the dynamic queue declaration in:\n{source}"
    );
    assert!(
        source.contains("task_queues::apply"),
        "expected build() to fold task queues into the domain project:\n{source}"
    );

    let output = try_compile(&source);
    assert!(
        output.status.success(),
        "generated source failed to compile:\n{}\n---source---\n{source}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn standalone_export_fails_closed_for_a_task_queue_with_an_unattested_native_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Unattested.mpr");
    mxrs_writer::write_project(&path, &build_project_with_queues()).unwrap();

    // A future Studio Pro field mxrs doesn't model yet: the typed export must
    // refuse to drop it rather than silently emit an incomplete queue.
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let queue_unit = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit).ok().is_some_and(|document| {
                document.get_str("$Type").ok() == Some("Queues$Queue")
                    && document.get_str("Name").ok() == Some("Imports")
            })
        })
        .unwrap();
    let mut document = mpr.parse_contents(&queue_unit).unwrap();
    document.insert("RetryPolicy", "Exponential");
    mpr.update_unit(&queue_unit.unit_id, document).unwrap();
    drop(mpr);

    let error = mxrs_exporter::export_project(&path).unwrap_err();
    let mxrs_exporter::ExportError::Lossy(count, gaps) = &error else {
        panic!("expected a Lossy export error, got {error:?}");
    };
    assert!(*count >= 1);
    assert!(
        gaps.iter()
            .any(|gap| gap.path == "Jobs.Imports" && gap.reason.contains("task queue")),
        "expected a gap naming the unattested queue, got {gaps:?}"
    );
}

#[test]
fn import_cargo_project_emits_task_queues_under_the_application_layer() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("Original.mpr");
    let generated = dir.path().join("generated");
    mxrs_writer::write_project(&original, &build_project_with_queues()).unwrap();

    mxrs_exporter::import_cargo_project(&original, &generated, Some(&workspace_root())).unwrap();

    let task_queues_path = generated.join("src/application/task_queues.rs");
    assert!(
        task_queues_path.is_file(),
        "expected {} to exist",
        task_queues_path.display()
    );
    let task_queues_source = std::fs::read_to_string(&task_queues_path).unwrap();
    // `cargo fmt` runs over imported source, so the call may wrap across
    // lines — assert on the tokens rather than one contiguous line.
    assert!(task_queues_source.contains("\"Imports\""));
    assert!(task_queues_source.contains("TaskQueueConfig::Fixed { parallelism: 4 }"));
    // The fully-configured fixed queue gets a named closure parameter with
    // every option restated.
    assert!(task_queues_source.contains("|queue| {"));
    assert!(task_queues_source.contains("queue.documentation(\"Fixed-size import queue\");"));
    assert!(task_queues_source.contains("queue.excluded(true);"));
    assert!(task_queues_source.contains("queue.export_level(ExportLevel::Published);"));
    // The all-defaults dynamic queue collapses to an ignored closure
    // parameter with no restated options, matching every other editable
    // document renderer's minimal-emission convention.
    assert!(task_queues_source.contains("\"Exports\""));
    assert!(task_queues_source.contains("|_| {}"));

    let application_source =
        std::fs::read_to_string(generated.join("src/application/mod.rs")).unwrap();
    assert!(application_source.contains("pub mod task_queues;"));
    // The queues register themselves, into the module that owns them.
    assert!(
        !application_source.contains("apply"),
        "{application_source}"
    );
    assert!(
        task_queues_source.contains("#[declaration(module = \"Jobs\", stage = TaskQueue)]"),
        "{task_queues_source}"
    );
}

#[test]
fn imported_task_queues_survive_deleting_the_source_mpr_and_rebuild_warning_free() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("Original.mpr");
    let generated = dir.path().join("generated");
    let rebuilt = dir.path().join("Rebuilt.mpr");
    mxrs_writer::write_project(&original, &build_project_with_queues()).unwrap();

    mxrs_exporter::import_cargo_project(&original, &generated, Some(&workspace_root())).unwrap();

    // Portability: the generated project must stand on its own — nothing in
    // it may still reach back into the deleted source `.mpr`. Everything it
    // needs was already captured losslessly under `model/imported/`.
    std::fs::remove_file(&original).unwrap();

    let check = Command::new(env!("CARGO"))
        .args(["check", "--offline", "--manifest-path"])
        .arg(generated.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", target_dir())
        .env("RUSTFLAGS", "-D warnings")
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "generated project failed `cargo check -D warnings` after the source .mpr was deleted:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );

    let run = Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(generated.join("Cargo.toml"))
        .arg("--")
        .arg(&rebuilt)
        .env("CARGO_TARGET_DIR", target_dir())
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "generated project failed to rebuild an .mpr:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );

    let project = mxrs_model::Project::open(&rebuilt, true).unwrap();
    let queues: Vec<_> = project
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .filter(|document| document.get_str("$Type").ok() == Some("Queues$Queue"))
        .collect();
    assert_eq!(queues.len(), 2, "expected both queues to survive rebuild");

    let imports = queues
        .iter()
        .find(|document| document.get_str("Name").ok() == Some("Imports"))
        .unwrap();
    assert_eq!(
        imports
            .get_document("Config")
            .unwrap()
            .get_i64("Parallelism")
            .unwrap(),
        4
    );

    let exports = queues
        .iter()
        .find(|document| document.get_str("Name").ok() == Some("Exports"))
        .unwrap();
    let exports_config = exports.get_document("Config").unwrap();
    assert_eq!(
        exports_config.get_str("ParallelismExpression").unwrap(),
        "cpuCount"
    );
    assert!(exports_config.get_bool("ClusterWide").unwrap());
}

#[test]
fn portability_audit_reports_task_queues_as_fully_typed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Portable.mpr");
    mxrs_writer::write_project(&path, &build_project_with_queues()).unwrap();

    let report = mxrs_exporter::audit_portability(&path).unwrap();
    let family = report
        .families
        .iter()
        .find(|family| family.native_type == "Queues$Queue")
        .expect("Queues$Queue family present in the report");
    assert_eq!(family.total, 2);
    assert_eq!(family.typed, 2);
    assert_eq!(family.partial, 0);
    assert_eq!(family.preserved, 0);
    assert_eq!(family.status, mxrs_exporter::PortabilityStatus::Typed);
}
