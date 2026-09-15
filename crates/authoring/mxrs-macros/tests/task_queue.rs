//! Exercises `project! {}`'s `task_queue` grammar through the real
//! pipeline: macro expansion -> `mxrs_ir::ProjectDecl` ->
//! `mxrs_writer::write_project` -> read back raw BSON, proving the macro's
//! `task_queue` item is genuinely equivalent to a hand-written
//! `ModuleBuilder::task_queue` call. Mirrors `end_to_end.rs`'s shape;
//! compile-failure diagnostics for the same grammar live in
//! `task_queue_diagnostics.rs` (a real nested `cargo build`, same as
//! `diagnostics.rs`, since a parse failure can't be caught in-process).

use mxrs_ir::{TaskQueueConfig, TaskQueueScope};
use mxrs_macros::project;
use mxrs_model::Project;

#[test]
fn a_fixed_task_queue_expands_to_the_same_decl_as_the_hand_written_builder() {
    let definition = project! {
        "11.12.1",
        module Jobs {
            task_queue Imports {
                parallelism 4;
                documentation "Fixed-size import queue";
                excluded true;
                export_level Published;
            }
        }
    };

    let queue = &definition.modules[0].task_queues[0];
    assert_eq!(queue.name, "Imports");
    assert_eq!(queue.config, TaskQueueConfig::Fixed { parallelism: 4 });
    assert_eq!(queue.documentation, "Fixed-size import queue");
    assert!(queue.excluded);
    assert_eq!(queue.export_level, mxrs_ir::ExportLevel::Published);
}

#[test]
fn a_dynamic_task_queue_without_scope_defaults_to_per_node() {
    let definition = project! {
        "11.12.1",
        module Jobs {
            task_queue Exports {
                parallelism_expression "cpuCount * 2";
            }
        }
    };

    let queue = &definition.modules[0].task_queues[0];
    assert_eq!(
        queue.config,
        TaskQueueConfig::Dynamic {
            parallelism_expression: "cpuCount * 2".to_string(),
            scope: TaskQueueScope::PerNode,
        }
    );
    // Every option besides the config itself is optional and keeps
    // `TaskQueueDecl::new`'s own defaults.
    assert_eq!(queue.documentation, "");
    assert!(!queue.excluded);
    assert_eq!(queue.export_level, mxrs_ir::ExportLevel::Hidden);
}

#[test]
fn a_dynamic_task_queue_can_declare_a_cluster_wide_scope() {
    let definition = project! {
        "11.12.1",
        module Jobs {
            task_queue Notifications {
                parallelism_expression "4";
                scope ClusterWide;
            }
        }
    };

    let queue = &definition.modules[0].task_queues[0];
    assert_eq!(
        queue.config,
        TaskQueueConfig::Dynamic {
            parallelism_expression: "4".to_string(),
            scope: TaskQueueScope::ClusterWide,
        }
    );
}

#[test]
fn a_declared_task_queue_writes_and_reads_back_through_the_real_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("MacroQueue.mpr");

    let definition = project! {
        "11.12.1",
        module Jobs {
            task_queue Imports {
                parallelism 4;
            }
            task_queue Exports {
                parallelism_expression "cpuCount";
                scope ClusterWide;
            }
        }
    };
    mxrs_writer::write_project(&path, &definition).unwrap();

    let project = Project::open(&path, true).unwrap();
    let document = project
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .find(|document| {
            document.get_str("$Type").ok() == Some("Queues$Queue")
                && document.get_str("Name").ok() == Some("Exports")
        })
        .unwrap();
    let config = document.get_document("Config").unwrap();
    assert_eq!(config.get_str("ParallelismExpression").unwrap(), "cpuCount");
    assert!(config.get_bool("ClusterWide").unwrap());
}

#[test]
fn multiple_task_queues_in_one_module_are_all_declared() {
    let definition = project! {
        "11.12.1",
        module Jobs {
            task_queue Imports { parallelism 4; }
            task_queue Exports { parallelism_expression "2"; }
        }
    };

    let mut names: Vec<&str> = definition.modules[0]
        .task_queues
        .iter()
        .map(|queue| queue.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["Exports", "Imports"]);
}
