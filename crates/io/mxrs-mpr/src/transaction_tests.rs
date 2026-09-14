use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const FIRST: &str = "11111111-1111-4111-8111-111111111111";
const SECOND: &str = "22222222-2222-4222-8222-222222222222";
const NEW: &str = "33333333-3333-4333-8333-333333333333";

fn fixture(v2: bool) -> (tempfile::TempDir, MprFile) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Test.mpr");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE Unit (UnitID BLOB PRIMARY KEY NOT NULL, ContainerID BLOB,
        ContainmentName TEXT, TreeConflict LONG, ContentsHash TEXT, ContentsConflicts TEXT);
        CREATE TABLE _MetaData (_ProductVersion TEXT);
        INSERT INTO _MetaData VALUES ('11.12.1');",
        )
        .unwrap();
    if v2 {
        std::fs::create_dir_all(directory.path().join("mprcontents")).unwrap();
    } else {
        connection
            .execute_batch("ALTER TABLE Unit ADD COLUMN Contents BLOB")
            .unwrap();
    }
    drop(connection);
    let mut mpr = MprFile::open(path, false).unwrap();
    for (id, name) in [(FIRST, "original"), (SECOND, "keep")] {
        mpr.insert_unit(
            FIRST,
            "Documents",
            mxrs_bson::doc! { "$ID": id, "Name": name },
            None,
        )
        .unwrap();
    }
    (directory, mpr)
}

fn change_all(mpr: &mut MprFile) -> Result<()> {
    mpr.update_unit(FIRST, mxrs_bson::doc! { "$ID": FIRST, "Name": "changed" })?;
    mpr.delete_unit(SECOND)?;
    mpr.insert_unit(
        FIRST,
        "Documents",
        mxrs_bson::doc! { "$ID": NEW, "Name": "new" },
        None,
    )?;
    Ok(())
}

fn contents(mpr: &MprFile) -> Vec<(String, Vec<u8>)> {
    let mut result = mpr
        .all_units()
        .unwrap()
        .into_iter()
        .map(|unit| {
            (
                unit.unit_id.clone(),
                mpr.content_bytes(&unit).unwrap().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    result.sort();
    result
}

fn clear_authorizer(mpr: &MprFile) {
    mpr.conn
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
}

fn assert_poisoned_handle_rejects_writes(mpr: &mut MprFile) {
    assert!(matches!(
        mpr.raw_query("COMMIT"),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.raw_query("SELECT 1"),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.transaction(|_| Ok(())),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.update_unit(FIRST, mxrs_bson::doc! {}),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.insert_unit(FIRST, "Documents", mxrs_bson::doc! {}, None),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.delete_unit(FIRST),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.relocate_unit(FIRST, SECOND, "Documents"),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.update_version("11.12.1", "hash"),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.ensure_v2_contract(),
        Err(MprError::RecoveryRequired)
    ));
    let probe = mpr.path().with_extension("backup-probe");
    std::fs::write(&probe, b"keep").unwrap();
    assert!(matches!(
        mpr.backup(&probe),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.restore_from(&probe),
        Err(MprError::RecoveryRequired)
    ));
    assert!(matches!(
        mpr.cleanup_backup(&probe),
        Err(MprError::RecoveryRequired)
    ));
    assert_eq!(std::fs::read(probe).unwrap(), b"keep");
}

// rusqlite 0.40.2 does not map SQLite's COMMIT string: its enum reports
// TransactionOperation::Unknown for COMMIT (not Release, which means RELEASE).
fn is_commit(action: AuthAction<'_>) -> bool {
    matches!(
        action,
        AuthAction::Transaction {
            operation: TransactionOperation::Unknown
        }
    )
}

#[test]
fn failed_begin_and_commit_preserve_v1_v2_bytes_and_allow_a_successful_retry() {
    for v2 in [false, true] {
        for deny_begin in [false, true] {
            let (_directory, mut mpr) = fixture(v2);
            let before = contents(&mpr);
            let sql_before = std::fs::read(mpr.path()).unwrap();
            let stats = mpr.write_stats;
            let ran = Arc::new(AtomicBool::new(false));
            mpr.conn
                .authorizer(Some(move |context: AuthContext<'_>| {
                    if (deny_begin
                        && matches!(
                            context.action,
                            AuthAction::Transaction {
                                operation: TransactionOperation::Begin
                            }
                        ))
                        || (!deny_begin && is_commit(context.action))
                    {
                        Authorization::Deny
                    } else {
                        Authorization::Allow
                    }
                }))
                .unwrap();
            let result = mpr.transaction(|mpr| {
                ran.store(true, Ordering::Relaxed);
                change_all(mpr)
            });
            assert!(matches!(result, Err(MprError::Sqlite(_))));
            clear_authorizer(&mpr);
            assert_eq!(ran.load(Ordering::Relaxed), !deny_begin);
            assert!(mpr.conn.is_autocommit());
            assert!(mpr.v2_transaction.is_none());
            assert_eq!(contents(&mpr), before);
            assert_eq!(std::fs::read(mpr.path()).unwrap(), sql_before);
            assert_eq!(mpr.write_stats, stats);
            assert!(!mpr.transaction_journal_dir().exists());
            mpr.transaction(change_all).unwrap();
            assert!(mpr.unit(SECOND).unwrap().is_none());
            assert!(mpr.unit(NEW).unwrap().is_some());
        }
    }
}

#[test]
fn closure_panics_restore_both_formats_and_resume_the_identical_payload() {
    for v2 in [false, true] {
        let (_directory, mut mpr) = fixture(v2);
        let before = contents(&mpr);
        let stats = mpr.write_stats;
        let payload = Arc::new(String::from("original panic payload"));
        let original = payload.clone();
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _: Result<()> = mpr.transaction(|mpr| {
                change_all(mpr)?;
                std::panic::panic_any(payload);
            });
        }))
        .unwrap_err();
        let resumed = panic.downcast::<Arc<String>>().unwrap();
        assert!(Arc::ptr_eq(&original, &resumed));
        assert_eq!(contents(&mpr), before);
        assert_eq!(mpr.write_stats, stats);
        assert!(mpr.conn.is_autocommit());
        assert!(mpr.v2_transaction.is_none());
        assert!(!mpr.transaction_journal_dir().exists());
        mpr.transaction(change_all).unwrap();
    }
}

#[test]
fn sqlite_automatic_rollback_after_a_commit_hook_veto_restores_both_formats() {
    for v2 in [false, true] {
        let (_directory, mut mpr) = fixture(v2);
        let before = contents(&mpr);
        let stats = mpr.write_stats;
        mpr.conn.commit_hook(Some(|| true)).unwrap();
        assert!(matches!(
            mpr.transaction(change_all),
            Err(MprError::Sqlite(_))
        ));
        mpr.conn.commit_hook(None::<fn() -> bool>).unwrap();
        assert!(mpr.conn.is_autocommit());
        assert!(mpr.v2_transaction.is_none());
        assert!(!mpr.transaction_journal_dir().exists());
        assert_eq!(contents(&mpr), before);
        assert_eq!(mpr.write_stats, stats);
        mpr.transaction(change_all).unwrap();
    }
}

#[test]
fn nested_v1_transactions_do_not_abort_the_outer_transaction() {
    let (_directory, mut mpr) = fixture(false);
    mpr.transaction(|mpr| {
        assert!(matches!(
            mpr.transaction(|_| Ok(())),
            Err(MprError::NestedTransaction)
        ));
        change_all(mpr)
    })
    .unwrap();
    assert!(mpr.unit(NEW).unwrap().is_some());
}

#[test]
fn managed_transactions_cannot_be_ended_by_raw_sql_or_filesystem_escape_hatches() {
    for v2 in [false, true] {
        let (_directory, mut mpr) = fixture(v2);
        let before = contents(&mpr);
        let probe = mpr.path().with_extension("backup-probe");
        std::fs::write(&probe, b"keep").unwrap();
        let result = mpr.transaction(|mpr| -> Result<()> {
            change_all(mpr)?;
            for sql in [
                "COMMIT",
                "END",
                "ROLLBACK",
                "SAVEPOINT x",
                "PRAGMA user_version = 7",
            ] {
                assert!(matches!(
                    mpr.raw_query(sql),
                    Err(MprError::ManagedTransactionOperation {
                        operation: "raw SQL"
                    })
                ));
                assert!(!mpr.conn.is_autocommit());
            }
            assert!(matches!(
                mpr.backup(&probe),
                Err(MprError::ManagedTransactionOperation {
                    operation: "backup"
                })
            ));
            assert!(matches!(
                mpr.restore_from(&probe),
                Err(MprError::ManagedTransactionOperation {
                    operation: "restore"
                })
            ));
            assert!(matches!(
                mpr.cleanup_backup(&probe),
                Err(MprError::ManagedTransactionOperation {
                    operation: "backup cleanup"
                })
            ));
            assert_eq!(std::fs::read(&probe).unwrap(), b"keep");
            Err(MprError::IncompletePackage("abort".to_string()))
        });
        assert!(matches!(result, Err(MprError::IncompletePackage(_))));
        assert_eq!(contents(&mpr), before);
        assert!(mpr.conn.is_autocommit());
        mpr.raw_query("PRAGMA user_version = 7").unwrap();
        mpr.transaction(change_all).unwrap();
    }
}

#[test]
fn denied_rollback_reports_both_errors_and_retains_recovery_state() {
    for v2 in [false, true] {
        let (_directory, mut mpr) = fixture(v2);
        let before = contents(&mpr);
        mpr.conn
            .authorizer(Some(|context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Transaction {
                        operation: TransactionOperation::Rollback
                    }
                ) {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }))
            .unwrap();
        let error = mpr
            .transaction(|mpr| -> Result<()> {
                change_all(mpr)?;
                Err(MprError::IncompletePackage("original error".to_string()))
            })
            .unwrap_err();
        assert!(
            matches!(error, MprError::TransactionRecovery { original, recovery }
            if matches!(*original, MprError::IncompletePackage(_)) && matches!(*recovery, MprError::Sqlite(_)))
        );
        assert!(!mpr.conn.is_autocommit());
        assert_poisoned_handle_rejects_writes(&mut mpr);
        clear_authorizer(&mpr);
        let path = mpr.path().to_path_buf();
        drop(mpr);
        let mut recovered = MprFile::open(path, false).unwrap();
        assert_eq!(contents(&recovered), before);
        recovered.transaction(change_all).unwrap();
    }
}

#[test]
fn panic_and_failed_rollback_keep_both_causes_in_a_typed_payload() {
    let (_directory, mut mpr) = fixture(true);
    let before = contents(&mpr);
    mpr.conn
        .authorizer(Some(|context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Transaction {
                    operation: TransactionOperation::Rollback
                }
            ) {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _: Result<()> = mpr.transaction(|mpr| {
            change_all(mpr)?;
            std::panic::panic_any(String::from("original"));
        });
    }))
    .unwrap_err();
    let recovery = panic
        .downcast::<crate::transaction::TransactionRecoveryPanic>()
        .unwrap();
    assert_eq!(*recovery.original.downcast::<String>().unwrap(), "original");
    assert!(matches!(recovery.recovery, MprError::Sqlite(_)));
    assert_poisoned_handle_rejects_writes(&mut mpr);
    let path = mpr.path().to_path_buf();
    drop(mpr);
    assert_eq!(contents(&MprFile::open(path, false).unwrap()), before);
}

#[test]
fn a_failed_backup_does_not_delete_the_original_and_restart_finishes_recovery() {
    let (_directory, mut mpr) = fixture(true);
    let before = contents(&mpr);
    let obstruction = mpr.transaction_journal_dir().join("original");
    let blocked = obstruction.clone();
    mpr.conn
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Insert {
                    table_name: "_MxrbFileTransaction"
                }
            ) {
                std::fs::write(&blocked, b"not a directory").unwrap();
            }
            Authorization::Allow
        }))
        .unwrap();
    assert!(mpr.transaction(change_all).is_err());
    clear_authorizer(&mpr);
    for (id, bytes) in &before {
        assert_eq!(
            std::fs::read(mxunit::path_for(&mpr.contents_dir(), id)).unwrap(),
            *bytes
        );
    }
    // Remove only the test-created obstruction, allowing normal journal recovery.
    std::fs::remove_file(obstruction).unwrap();
    let path = mpr.path().to_path_buf();
    drop(mpr);
    let mut recovered = MprFile::open(path, false).unwrap();
    assert_eq!(contents(&recovered), before);
    recovered.transaction(change_all).unwrap();
}

#[test]
fn failed_file_rollback_preserves_the_backup_until_restart_can_restore_it() {
    let (_directory, mut mpr) = fixture(true);
    let before = contents(&mpr);
    let unit_path = mxunit::path_for(&mpr.contents_dir(), FIRST);
    let obstructed = unit_path.clone();
    mpr.conn
        .authorizer(Some(move |context: AuthContext<'_>| {
            if is_commit(context.action) {
                std::fs::remove_file(&obstructed).unwrap();
                std::fs::create_dir(&obstructed).unwrap();
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
    let error = mpr.transaction(change_all).unwrap_err();
    assert!(matches!(error, MprError::TransactionRecovery { .. }));
    assert_poisoned_handle_rejects_writes(&mut mpr);
    clear_authorizer(&mpr);
    let relative = unit_path.strip_prefix(mpr.contents_dir()).unwrap();
    let backup = mpr
        .transaction_journal_dir()
        .join("original")
        .join(relative);
    assert_eq!(std::fs::read(backup).unwrap(), before[0].1);
    assert!(mpr.transaction_manifest_path().is_file());
    assert!(mpr.conn.is_autocommit());
    std::fs::remove_dir(unit_path).unwrap();
    let path = mpr.path().to_path_buf();
    drop(mpr);
    let mut recovered = MprFile::open(path, false).unwrap();
    assert_eq!(contents(&recovered), before);
    recovered.transaction(change_all).unwrap();
}

#[test]
fn a_committed_cleanup_failure_keeps_its_marker_and_never_rolls_back_durable_data() {
    let (directory, mut mpr) = fixture(true);
    let journal = mpr.transaction_journal_dir();
    let saved = directory.path().join("saved-journal");
    let callback_journal = journal.clone();
    let callback_saved = saved.clone();
    mpr.conn
        .authorizer(Some(move |context: AuthContext<'_>| {
            if is_commit(context.action) {
                std::fs::rename(&callback_journal, &callback_saved).unwrap();
                std::fs::write(&callback_journal, b"cleanup obstruction").unwrap();
            }
            Authorization::Allow
        }))
        .unwrap();
    assert!(matches!(
        mpr.transaction(change_all),
        Err(MprError::CommittedTransactionCleanup(_))
    ));
    assert_poisoned_handle_rejects_writes(&mut mpr);
    clear_authorizer(&mpr);
    let id = mpr.v2_transaction.as_ref().unwrap().id.clone();
    assert!(mpr.v2_transaction_committed(&id).unwrap());
    assert!(mpr.conn.is_autocommit());
    let committed = contents(&mpr);
    std::fs::remove_file(&journal).unwrap();
    std::fs::rename(saved, &journal).unwrap();
    let path = mpr.path().to_path_buf();
    drop(mpr);
    let recovered = MprFile::open(path, false).unwrap();
    assert_eq!(contents(&recovered), committed);
    assert!(!journal.exists());
}

#[test]
fn marker_lookup_errors_leave_an_interrupted_committed_journal_untouched() {
    let (_directory, mut mpr) = fixture(true);
    mpr.conn.execute_batch("BEGIN").unwrap();
    mpr.v2_transaction = Some(V2TransactionState::new(uuid::Uuid::new_v4().to_string()));
    change_all(&mut mpr).unwrap();
    mpr.apply_v2_transaction().unwrap();
    mpr.conn.execute_batch("COMMIT").unwrap();
    let committed = contents(&mpr);
    let manifest = std::fs::read(mpr.transaction_manifest_path()).unwrap();
    mpr.conn
        .authorizer(Some(|context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "_MxrbFileTransaction",
                    ..
                }
            ) {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
    assert!(matches!(
        mpr.recover_interrupted_v2_transaction(),
        Err(MprError::Sqlite(_))
    ));
    assert!(matches!(
        mpr.abort_transaction(mpr.write_stats),
        Err(MprError::Sqlite(_))
    ));
    assert_eq!(
        std::fs::read(mpr.transaction_manifest_path()).unwrap(),
        manifest
    );
    for (id, bytes) in &committed {
        assert_eq!(
            std::fs::read(mxunit::path_for(&mpr.contents_dir(), id)).unwrap(),
            *bytes
        );
    }
    clear_authorizer(&mpr);
    let path = mpr.path().to_path_buf();
    drop(mpr);
    assert_eq!(contents(&MprFile::open(path, false).unwrap()), committed);
}

#[test]
fn cleanup_after_an_already_durable_commit_never_restores_uncommitted_backups() {
    let (_directory, mut mpr) = fixture(true);
    let stats = mpr.write_stats;
    mpr.conn.execute_batch("BEGIN").unwrap();
    mpr.v2_transaction = Some(V2TransactionState::new(uuid::Uuid::new_v4().to_string()));
    change_all(&mut mpr).unwrap();
    mpr.apply_v2_transaction().unwrap();
    mpr.conn.execute_batch("COMMIT").unwrap();
    let committed = contents(&mpr);
    mpr.abort_transaction(stats).unwrap();
    assert_eq!(contents(&mpr), committed);
    assert_ne!(mpr.write_stats, stats);
    assert!(mpr.v2_transaction.is_none());
    assert!(!mpr.transaction_journal_dir().exists());
}

#[test]
fn lowercase_transaction_marker_is_recognized_on_restart_and_cleaned_up() {
    let (_directory, mut mpr) = fixture(true);
    mpr.conn
        .execute_batch("CREATE TABLE _mxrbfiletransaction (ID TEXT PRIMARY KEY NOT NULL); BEGIN")
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    mpr.v2_transaction = Some(V2TransactionState::new(id.clone()));
    change_all(&mut mpr).unwrap();
    mpr.apply_v2_transaction().unwrap();
    mpr.conn.execute_batch("COMMIT").unwrap();
    assert!(mpr.v2_transaction_committed(&id).unwrap());
    let committed = contents(&mpr);
    let path = mpr.path().to_path_buf();
    drop(mpr);
    let recovered = MprFile::open(path, false).unwrap();
    assert_eq!(contents(&recovered), committed);
    assert!(!recovered.transaction_journal_dir().exists());
    assert!(
        !recovered
            .tables()
            .unwrap()
            .iter()
            .any(|table| table.eq_ignore_ascii_case("_MxrbFileTransaction"))
    );
}

#[test]
fn malformed_uuid_blobs_in_either_column_return_conversion_errors_without_writes() {
    for v2 in [false, true] {
        for (index, column) in [(0, "UnitID"), (1, "ContainerID")] {
            for size in [0, 1, 15, 17] {
                let (_directory, mpr) = fixture(v2);
                mpr.conn
                    .execute(
                        &format!("UPDATE Unit SET {column} = ?1 WHERE UnitID = ?2"),
                        rusqlite::params![
                            vec![1_u8; size],
                            mxrs_bson::uuid_to_blob(FIRST).unwrap().to_vec()
                        ],
                    )
                    .unwrap();
                let before = std::fs::read(mpr.path()).unwrap();
                assert!(
                    matches!(mpr.all_units(), Err(MprError::Sqlite(rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Blob, _))) if column == index)
                );
                assert_eq!(std::fs::read(mpr.path()).unwrap(), before);
            }
        }
    }
}

#[test]
fn manually_constructed_raw_units_cannot_panic_or_escape_the_content_directory() {
    for v2 in [false, true] {
        let (_directory, mpr) = fixture(v2);
        for invalid in [
            "",
            "a",
            "---",
            "../outside",
            "/tmp/outside",
            "é",
            "éééééééééééééééé",
            "00000000-0000-0000-0000-00000000000g",
        ] {
            for inline in [None, Some(Vec::new()), Some(vec![1, 2, 3])] {
                let unit = RawUnit {
                    unit_id: invalid.to_string(),
                    container_id: FIRST.to_string(),
                    containment_name: "Documents".to_string(),
                    contents_hash: None,
                    contents: inline,
                };
                assert!(matches!(
                    mpr.content_bytes(&unit),
                    Err(MprError::Bson(mxrs_bson::BsonCodecError::InvalidUuid(_)))
                ));
                assert!(mpr.content_path(&unit).is_none());
            }
        }
        let existing = mpr.unit(FIRST).unwrap().unwrap();
        assert!(mpr.content_bytes(&existing).unwrap().is_some());
        assert_eq!(mpr.content_path(&existing).is_some(), v2);
    }
}
