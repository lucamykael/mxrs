//! Exercises `synchronize_task_queues_with_identity`, mirroring mxrb's
//! `ArtifactDocuments#task_queue`: `Queues$Queue` documents with a nested
//! `Queues$BasicQueueConfig` that switches between a fixed integer
//! parallelism and a dynamic expression (optionally cluster-wide). Follows
//! the same build/write/read-back-via-`mxrs-model`-or-raw-BSON shape as
//! `end_to_end.rs`'s `regular_expressions_round_trip_every_semantic_field`
//! and the folder/future-field patterns in `synchronize_project.rs`.

use mxrs_dsl::{ExportLevel, ProjectBuilder, TaskQueueConfig, TaskQueueScope};
use mxrs_model::Project;

/// Reads back one `Documents` unit of `document_type` by `Name`, anywhere in
/// the project (mirrors `end_to_end.rs`'s private `document_by_name`, kept
/// separate per-file since each `tests/*.rs` binary is its own crate).
fn document_by_name(project: &Project, document_type: &str, name: &str) -> mxrs_bson::Document {
    project
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .find(|document| {
            document.get_str("$Type").ok() == Some(document_type)
                && document.get_str("Name").ok() == Some(name)
        })
        .unwrap_or_else(|| panic!("no {document_type} named {name:?} was written"))
}

fn task_queue_document(path: &std::path::Path, name: &str) -> mxrs_bson::Document {
    let project = Project::open(path, true).unwrap();
    document_by_name(&project, "Queues$Queue", name)
}

#[test]
fn task_queues_round_trip_fixed_and_dynamic_configs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Queues.mpr");

    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Jobs", |module| {
        module.task_queue(
            "Imports",
            TaskQueueConfig::Fixed { parallelism: 4 },
            |queue| {
                queue
                    .documentation("Fixed-size import queue")
                    .excluded(true)
                    .export_level(ExportLevel::Published);
            },
        );
        module.task_queue(
            "Exports",
            TaskQueueConfig::Dynamic {
                parallelism_expression: "4".to_string(),
                scope: TaskQueueScope::ClusterWide,
            },
            |queue| {
                queue.documentation("Cluster-wide export queue");
            },
        );
        module.task_queue(
            "Notifications",
            TaskQueueConfig::Dynamic {
                parallelism_expression: "2".to_string(),
                scope: TaskQueueScope::PerNode,
            },
            |_| {},
        );
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let imports = task_queue_document(&path, "Imports");
    assert_eq!(
        imports.get_str("Documentation").unwrap(),
        "Fixed-size import queue"
    );
    assert!(imports.get_bool("Excluded").unwrap());
    assert_eq!(imports.get_str("ExportLevel").unwrap(), "Published");
    let imports_config = imports.get_document("Config").unwrap();
    assert_eq!(
        imports_config.get_str("$Type").unwrap(),
        "Queues$BasicQueueConfig"
    );
    // Studio 11 projects widen keyed integers to `Int64` on write (mirrors
    // mxrb's `int64_properties` storage convention), so even this
    // logically-i32 field reads back as `Int64`.
    assert_eq!(imports_config.get_i64("Parallelism").unwrap(), 4);
    assert!(imports_config.get("ParallelismExpression").is_none());
    assert!(imports_config.get("ClusterWide").is_none());

    let exports = task_queue_document(&path, "Exports");
    assert!(!exports.get_bool("Excluded").unwrap());
    assert_eq!(exports.get_str("ExportLevel").unwrap(), "Hidden");
    let exports_config = exports.get_document("Config").unwrap();
    assert_eq!(
        exports_config.get_str("ParallelismExpression").unwrap(),
        "4"
    );
    assert!(exports_config.get_bool("ClusterWide").unwrap());
    assert!(exports_config.get("Parallelism").is_none());

    let notifications = task_queue_document(&path, "Notifications");
    let notifications_config = notifications.get_document("Config").unwrap();
    assert_eq!(
        notifications_config
            .get_str("ParallelismExpression")
            .unwrap(),
        "2"
    );
    assert!(!notifications_config.get_bool("ClusterWide").unwrap());

    // The queue and its `Config` sub-document are separately identified.
    assert_ne!(
        mxrs_bson::extract_id(imports.get("$ID").unwrap()),
        mxrs_bson::extract_id(imports_config.get("$ID").unwrap())
    );
}

#[test]
fn task_queue_fixed_parallelism_of_exactly_i32_max_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Boundary.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Jobs", |module| {
        module.task_queue(
            "AtLimit",
            TaskQueueConfig::Fixed {
                parallelism: i32::MAX as u32,
            },
            |_| {},
        );
    });
    mxrs_writer::write_project(&path, &project.build()).unwrap();

    let document = task_queue_document(&path, "AtLimit");
    assert_eq!(
        document
            .get_document("Config")
            .unwrap()
            .get_i64("Parallelism")
            .unwrap(),
        i64::from(i32::MAX)
    );
}

#[test]
fn task_queue_fixed_parallelism_of_zero_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Zero.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Jobs", |module| {
        module.task_queue("Empty", TaskQueueConfig::Fixed { parallelism: 0 }, |_| {});
    });

    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::InvalidTaskQueue { name, reason }
            if name == "Jobs.Empty" && reason.contains("between 1 and 2147483647")
    ));
}

#[test]
fn task_queue_fixed_parallelism_over_i32_max_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Overflow.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Jobs", |module| {
        module.task_queue(
            "TooWide",
            TaskQueueConfig::Fixed {
                parallelism: i32::MAX as u32 + 1,
            },
            |_| {},
        );
    });

    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::InvalidTaskQueue { name, reason }
            if name == "Jobs.TooWide" && reason.contains("between 1 and 2147483647")
    ));
}

#[test]
fn task_queue_dynamic_expression_must_not_be_empty_or_blank() {
    for (label, expression) in [("Blank", ""), ("Whitespace", "   ")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{label}.mpr"));
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Jobs", |module| {
            module.task_queue(
                label,
                TaskQueueConfig::Dynamic {
                    parallelism_expression: expression.to_string(),
                    scope: TaskQueueScope::PerNode,
                },
                |_| {},
            );
        });

        let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
        assert!(
            matches!(
                &error,
                mxrs_writer::WriterError::InvalidTaskQueue { name, reason }
                    if name == &format!("Jobs.{label}")
                        && reason.contains("expression must not be empty")
            ),
            "expression {expression:?} should have been rejected, got {error:?}"
        );
    }
}

#[test]
fn task_queue_name_must_not_be_empty_or_blank() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("BlankName.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Jobs", |module| {
        module.task_queue("   ", TaskQueueConfig::Fixed { parallelism: 1 }, |_| {});
    });

    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::InvalidTaskQueue { reason, .. }
            if reason.contains("name must not be empty")
    ));
}

#[test]
fn task_queue_duplicate_names_within_a_module_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Duplicate.mpr");
    let mut project = ProjectBuilder::new("11.12.1");
    project.module("Jobs", |module| {
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 1 }, |_| {});
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 2 }, |_| {});
    });

    let error = mxrs_writer::write_project(&path, &project.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::DuplicateTaskQueue { module_name, name }
            if module_name == "Jobs" && name == "Imports"
    ));
}

#[test]
fn task_queue_and_config_identities_are_stable_and_distinct_across_resync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Identity.mpr");
    let build = || {
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Jobs", |module| {
            module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 4 }, |_| {});
        });
        project.build()
    };
    mxrs_writer::write_project(&path, &build()).unwrap();

    let before = task_queue_document(&path, "Imports");
    let queue_id = mxrs_bson::extract_id(before.get("$ID").unwrap()).unwrap();
    let config_id =
        mxrs_bson::extract_id(before.get_document("Config").unwrap().get("$ID").unwrap()).unwrap();
    assert_ne!(queue_id, config_id);

    mxrs_writer::synchronize_project(&path, &build()).unwrap();

    let after = task_queue_document(&path, "Imports");
    assert_eq!(
        mxrs_bson::extract_id(after.get("$ID").unwrap()),
        Some(queue_id)
    );
    assert_eq!(
        mxrs_bson::extract_id(after.get_document("Config").unwrap().get("$ID").unwrap()),
        Some(config_id)
    );
}

#[test]
fn resynchronizing_a_task_queue_updates_declared_fields_and_preserves_unknown_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Resync.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Jobs", |module| {
        module.task_queue(
            "Imports",
            TaskQueueConfig::Fixed { parallelism: 4 },
            |queue| {
                queue.documentation("v1").excluded(false);
            },
        );
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    // Simulate native fields mxrs does not yet model, at both the queue and
    // the nested config level — a future Studio Pro field must survive a
    // resync that only touches known fields.
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let queue_unit = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .is_some_and(|document| document.get_str("$Type").ok() == Some("Queues$Queue"))
        })
        .unwrap();
    let queue_id = queue_unit.unit_id.clone();
    let mut document = mpr.parse_contents(&queue_unit).unwrap();
    document.insert("FutureQueueField", "preserved");
    let mut config = document.get_document("Config").unwrap().clone();
    config.insert("FutureConfigField", "preserved-too");
    document.insert("Config", config);
    mpr.update_unit(&queue_id, document).unwrap();
    drop(mpr);

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Jobs", |module| {
        module.task_queue(
            "Imports",
            TaskQueueConfig::Fixed { parallelism: 8 },
            |queue| {
                queue
                    .documentation("v2")
                    .excluded(true)
                    .export_level(ExportLevel::Published);
            },
        );
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let document = task_queue_document(&path, "Imports");
    // Declared fields, unlike constants' `Excluded`/`ExportLevel`, are fully
    // controlled by `TaskQueueDecl` and must be overwritten on every sync.
    assert_eq!(document.get_str("Documentation").unwrap(), "v2");
    assert!(document.get_bool("Excluded").unwrap());
    assert_eq!(document.get_str("ExportLevel").unwrap(), "Published");
    assert_eq!(document.get_str("FutureQueueField").unwrap(), "preserved");
    let config = document.get_document("Config").unwrap();
    assert_eq!(config.get_i64("Parallelism").unwrap(), 8);
    assert_eq!(
        config.get_str("FutureConfigField").unwrap(),
        "preserved-too"
    );
}

#[test]
fn resynchronizing_switches_fixed_to_dynamic_and_removes_the_opposite_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FixedToDynamic.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Jobs", |module| {
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 4 }, |_| {});
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Jobs", |module| {
        module.task_queue(
            "Imports",
            TaskQueueConfig::Dynamic {
                parallelism_expression: "cpuCount * 2".to_string(),
                scope: TaskQueueScope::ClusterWide,
            },
            |_| {},
        );
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let config = task_queue_document(&path, "Imports")
        .get_document("Config")
        .unwrap()
        .clone();
    assert!(config.get("Parallelism").is_none());
    assert_eq!(
        config.get_str("ParallelismExpression").unwrap(),
        "cpuCount * 2"
    );
    assert!(config.get_bool("ClusterWide").unwrap());
}

#[test]
fn resynchronizing_switches_dynamic_to_fixed_and_removes_the_opposite_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("DynamicToFixed.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Jobs", |module| {
        module.task_queue(
            "Imports",
            TaskQueueConfig::Dynamic {
                parallelism_expression: "cpuCount".to_string(),
                scope: TaskQueueScope::ClusterWide,
            },
            |_| {},
        );
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Jobs", |module| {
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 6 }, |_| {});
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let config = task_queue_document(&path, "Imports")
        .get_document("Config")
        .unwrap()
        .clone();
    assert_eq!(config.get_i64("Parallelism").unwrap(), 6);
    assert!(config.get("ParallelismExpression").is_none());
    assert!(config.get("ClusterWide").is_none());
}

#[test]
fn resynchronizing_a_task_queue_with_an_unsupported_existing_config_type_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("UnsupportedConfig.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Jobs", |module| {
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 4 }, |_| {});
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    // Simulate a future/unattested queue config native shape mxrs does not
    // understand yet; it must refuse to replace it rather than guess.
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let queue_unit = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .is_some_and(|document| document.get_str("$Type").ok() == Some("Queues$Queue"))
        })
        .unwrap();
    let queue_id = queue_unit.unit_id.clone();
    let mut document = mpr.parse_contents(&queue_unit).unwrap();
    let mut config = document.get_document("Config").unwrap().clone();
    config.insert("$Type", "Queues$FutureQueueConfig");
    document.insert("Config", config);
    mpr.update_unit(&queue_id, document).unwrap();
    drop(mpr);

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Jobs", |module| {
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 8 }, |_| {});
    });
    let error = mxrs_writer::synchronize_project(&path, &updated.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::InvalidTaskQueue { name, reason }
            if name == "Jobs.Imports" && reason.contains("not a supported basic config")
    ));
}

#[test]
fn resynchronizing_a_task_queue_with_a_missing_existing_config_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("MissingConfig.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Jobs", |module| {
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 4 }, |_| {});
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let queue_unit = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .is_some_and(|document| document.get_str("$Type").ok() == Some("Queues$Queue"))
        })
        .unwrap();
    let queue_id = queue_unit.unit_id.clone();
    let mut document = mpr.parse_contents(&queue_unit).unwrap();
    document.remove("Config");
    mpr.update_unit(&queue_id, document).unwrap();
    drop(mpr);

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Jobs", |module| {
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 8 }, |_| {});
    });
    let error = mxrs_writer::synchronize_project(&path, &updated.build()).unwrap_err();
    assert!(matches!(
        error,
        mxrs_writer::WriterError::InvalidTaskQueue { name, reason }
            if name == "Jobs.Imports" && reason.contains("configuration is missing")
    ));
}

#[test]
fn synchronize_project_updates_task_queues_inside_folders_without_duplicating_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("NestedQueues.mpr");
    let mut initial = ProjectBuilder::new("11.12.1");
    initial.module("Jobs", |module| {
        module.task_queue("Imports", TaskQueueConfig::Fixed { parallelism: 4 }, |_| {});
    });
    mxrs_writer::write_project(&path, &initial.build()).unwrap();

    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let module = mpr
        .units_by_containment("Modules")
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .and_then(|document| document.get_str("Name").ok().map(str::to_string))
                .as_deref()
                == Some("Jobs")
        })
        .unwrap();
    let folder_id = mpr
        .insert_unit(
            &module.unit_id,
            "Folders",
            mxrs_bson::doc! { "$Type": "Projects$Folder", "Name": "Queues" },
            None,
        )
        .unwrap();
    let queue_unit_id = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .find(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .is_some_and(|document| document.get_str("$Type").ok() == Some("Queues$Queue"))
        })
        .unwrap()
        .unit_id;
    mpr.relocate_unit(&queue_unit_id, &folder_id, "Documents")
        .unwrap();
    drop(mpr);

    let mut updated = ProjectBuilder::new("11.12.1");
    updated.module("Jobs", |module| {
        module.task_queue(
            "Imports",
            TaskQueueConfig::Fixed { parallelism: 12 },
            |_| {},
        );
    });
    mxrs_writer::synchronize_project(&path, &updated.build()).unwrap();

    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    let queues: Vec<_> = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .filter(|unit| {
            mpr.parse_contents(unit)
                .ok()
                .is_some_and(|document| document.get_str("$Type").ok() == Some("Queues$Queue"))
        })
        .collect();
    assert_eq!(queues.len(), 1, "resync must not duplicate the queue");
    assert_eq!(queues[0].unit_id, queue_unit_id);
    assert_eq!(queues[0].container_id, folder_id);
    let document = mpr.parse_contents(&queues[0]).unwrap();
    assert_eq!(
        document
            .get_document("Config")
            .unwrap()
            .get_i64("Parallelism")
            .unwrap(),
        12
    );
}
