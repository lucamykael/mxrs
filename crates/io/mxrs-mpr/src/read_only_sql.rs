use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

/// This policy belongs to the connection, not to a SQL-text parser: comments,
/// CTEs, casing and SQLite's table-valued PRAGMAs cannot bypass it. New SQLite
/// authorization actions are denied until explicitly reviewed.
pub(crate) fn authorize(context: AuthContext<'_>) -> Authorization {
    let allowed = match context.action {
        AuthAction::Read { .. } | AuthAction::Select | AuthAction::Recursive => true,
        AuthAction::Function { function_name } => !["load_extension", "readfile", "writefile"]
            .iter()
            .any(|name| function_name.eq_ignore_ascii_case(name)),
        AuthAction::Pragma {
            pragma_name,
            pragma_value,
        } => match pragma_name.to_ascii_lowercase().as_str() {
            "table_info" | "table_xinfo" | "table_list" | "index_info" | "index_xinfo"
            | "index_list" | "foreign_key_list" | "foreign_key_check" | "integrity_check"
            | "quick_check" => true,
            "application_id" | "user_version" | "schema_version" | "data_version"
            | "database_list" | "compile_options" | "function_list" | "module_list"
            | "pragma_list" | "collation_list" | "page_count" | "page_size" | "freelist_count"
            | "encoding" | "foreign_keys" | "query_only" => pragma_value.is_none(),
            _ => false,
        },
        _ => false,
    };
    if allowed {
        Authorization::Allow
    } else {
        Authorization::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MprFile, SqlCell};

    #[test]
    fn read_only_queries_cannot_create_external_files_or_change_connection_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Model.mpr");
        drop(MprFile::create(&path, "11.12.1", "schema").unwrap());
        let before = std::fs::read(&path).unwrap();
        let mpr = MprFile::open(&path, true).unwrap();
        let external = directory.path().join("external.sqlite");
        let quoted = external.to_str().unwrap().replace('\'', "''");
        for sql in [
            format!("ATTACH DATABASE '{quoted}' AS external"),
            format!("VACUUM INTO '{quoted}'"),
            "DETACH DATABASE main".into(),
            "PRAGMA writable_schema = ON".into(),
            "PRAGMA query_only = OFF".into(),
            "PRAGMA user_version = 7".into(),
            "PRAGMA journal_mode = WAL".into(),
            "PRAGMA optimize".into(),
            "CREATE TEMP TABLE scratch (value)".into(),
            "DELETE FROM Unit".into(),
            "BEGIN".into(),
            "SAVEPOINT leak".into(),
            "SELECT load_extension('not-a-real-extension')".into(),
        ] {
            assert!(mpr.raw_query(&sql).is_err(), "allowed {sql}");
            assert!(!external.exists(), "created an external file: {sql}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            mpr.raw_query("SELECT COUNT(*) FROM Unit").unwrap().rows,
            [vec![SqlCell::Integer(1)]]
        );
    }

    #[test]
    fn introspection_functions_and_recursive_read_queries_remain_available() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Model.mpr");
        drop(MprFile::create(&path, "11.12.1", "schema").unwrap());
        let mpr = MprFile::open(&path, true).unwrap();
        for sql in [
            "PRAGMA table_info('Unit')",
            "PRAGMA TABLE_XINFO('Unit')",
            "PRAGMA user_version",
            "PRAGMA database_list",
            "PRAGMA quick_check",
            "SELECT name FROM pragma_table_info('Unit')",
            "SELECT lower('HELLO'), length('world')",
            "WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v + 1 FROM n WHERE v < 3) SELECT sum(v) FROM n",
        ] {
            assert!(!mpr.raw_query(sql).unwrap().rows.is_empty(), "{sql}");
        }
    }

    #[test]
    fn unknown_actions_and_file_functions_fail_closed() {
        for action in [
            AuthAction::Unknown {
                code: -1,
                arg1: None,
                arg2: None,
            },
            AuthAction::Function {
                function_name: "READFILE",
            },
            AuthAction::Function {
                function_name: "writefile",
            },
        ] {
            assert_eq!(
                authorize(AuthContext {
                    action,
                    database_name: None,
                    accessor: None
                }),
                Authorization::Deny
            );
        }
    }

    #[test]
    fn writable_debug_handles_keep_explicit_write_support() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Model.mpr");
        let mpr = MprFile::create(&path, "11.12.1", "schema").unwrap();
        mpr.raw_query("PRAGMA user_version = 7").unwrap();
        assert_eq!(
            mpr.raw_query("PRAGMA user_version").unwrap().rows,
            [vec![SqlCell::Integer(7)]]
        );
    }

    #[test]
    fn rejected_read_only_transactions_do_not_enter_a_nested_transaction_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Model.mpr");
        drop(MprFile::create(&path, "11.12.1", "schema").unwrap());
        let mut mpr = MprFile::open(&path, true).unwrap();
        for _ in 0..2 {
            let result =
                mpr.transaction::<()>(|_| panic!("read-only transaction invoked its closure"));
            assert!(matches!(result, Err(crate::MprError::ReadOnly)));
        }
        assert_eq!(mpr.all_units().unwrap().len(), 1);
    }
}
