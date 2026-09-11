//! Exercises `WidgetsCompiler` against a real project: built via
//! `mxrs-dsl`, persisted via `mxrs-writer`, read back via `mxrs-model` —
//! the same writer→reader round trip `mxrs-compiler-domain`/
//! `mxrs-compiler-flow`'s own end-to-end tests use.
//!
//! **Scope caveat**: `mxrs-dsl` has no builder surface yet for
//! navigation/menus/settings customization — `mxrs-writer/src/scaffold.rs`
//! always emits an empty `Navigation$NavigationDocument` and a hardcoded
//! `Security$ProjectSecurity` (not a `Settings$ProjectSettings`; that one
//! comes from the embedded project template instead). This test therefore
//! only proves the compiler runs end-to-end against real scaffold-written
//! documents — the richer field-specific edge cases (`IsOffline`,
//! `OfflineEntityConfigsRuntime` preservation, nested text references,
//! `Settings` node fallback ordering) are covered by each module's own
//! hand-built-fixture unit tests, not reachable via the DSL today.

use mxrs_bson::Bson;
use mxrs_compiler_widgets::WidgetsCompiler;
use mxrs_dsl::ProjectBuilder;
use mxrs_model::Project;

fn write_fixture(path: &std::path::Path) {
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Sales", |m| {
        m.entity("Customer", |e| {
            e.string("Name");
        });
    });
    mxrs_writer::write_project(path, &project.build()).unwrap();
}

#[test]
fn compiles_the_scaffold_written_navigation_document() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Nav.mpr");
    write_fixture(&path);

    let project = Project::open(&path, true).unwrap();
    let units = project.all_units().unwrap();
    let nav_unit = units
        .iter()
        .find(|u| {
            project
                .mpr()
                .parse_contents(u)
                .ok()
                .and_then(|d| d.get_str("$Type").ok().map(str::to_string))
                .as_deref()
                == Some("Navigation$NavigationDocument")
        })
        .expect("scaffold should have written a Navigation$NavigationDocument");
    let source = project.mpr().parse_contents(nav_unit).unwrap();

    let compiler = WidgetsCompiler::new(&[]).unwrap();
    let compiled = compiler.compile_navigation(&source).unwrap();

    assert_eq!(
        compiled.get_str("$Type").unwrap(),
        "Navigation$NavigationDocument"
    );
    let Some(Bson::Array(profiles)) = compiled.get("Profiles") else {
        panic!("expected a Profiles array");
    };
    // Scaffold writes zero profiles — only the marker int survives.
    assert!(profiles.len() <= 1);
}

#[test]
fn compiles_the_scaffold_written_settings_document() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Settings.mpr");
    write_fixture(&path);

    let project = Project::open(&path, true).unwrap();
    let units = project.all_units().unwrap();
    let settings_unit = units
        .iter()
        .find(|u| {
            project
                .mpr()
                .parse_contents(u)
                .ok()
                .and_then(|d| d.get_str("$Type").ok().map(str::to_string))
                .as_deref()
                == Some("Settings$ProjectSettings")
        })
        .expect("scaffold should have written a Settings$ProjectSettings");
    let source = project.mpr().parse_contents(settings_unit).unwrap();

    let compiler = WidgetsCompiler::new(&[]).unwrap();
    let compiled = compiler.compile_settings(&source).unwrap();

    assert_eq!(
        compiled.get_str("$Type").unwrap(),
        "Settings$ProjectSettings"
    );
    assert!(compiled.contains_key("Settings"));
}

#[test]
fn compiling_a_settings_document_through_the_artifact_path_fails_loudly() {
    // Settings$ProjectSettings isn't one of ArtifactCompiler's six handled
    // types — compiling it through compile_artifact must fail loudly
    // rather than silently produce a bogus document, per this workspace's
    // house rule (see `mxrs-compiler-flow::CompilerError`'s own doc
    // comments for the same "fail loud, don't guess" convention).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Sec.mpr");
    write_fixture(&path);

    let project = Project::open(&path, true).unwrap();
    let units = project.all_units().unwrap();
    let settings_unit = units
        .iter()
        .find(|u| {
            project
                .mpr()
                .parse_contents(u)
                .ok()
                .and_then(|d| d.get_str("$Type").ok().map(str::to_string))
                .as_deref()
                == Some("Settings$ProjectSettings")
        })
        .expect("scaffold should have written a Settings$ProjectSettings");
    let source = project.mpr().parse_contents(settings_unit).unwrap();

    let compiler = WidgetsCompiler::new(&[]).unwrap();
    let err = compiler
        .compile_artifact(&source, Some("Sales"))
        .unwrap_err();
    assert!(matches!(
        err,
        mxrs_compiler_widgets::CompilerError::UnsupportedArtifactType { .. }
    ));
}

#[test]
fn builds_the_project_scoped_page_and_web_operation_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("WebOperations.mpr");
    write_fixture(&path);

    let project = Project::open(&path, true).unwrap();
    let compiler = WidgetsCompiler::for_project(&project, &[]).unwrap();

    // The minimal writer fixture has no page yet, but exercises the actual
    // raw-unit/module/security indexing path rather than the in-memory test
    // constructor used by the focused operation tests.
    assert!(compiler.compile_web_operations().is_empty());
}
