//! Low-level SQLite wrapper for `.mpr` files.
//!
//! Schema reality (from reverse engineering, see `lib/mxrb/io/mpr_file.rb`):
//! ```text
//! Unit(UnitID BLOB, ContainerID BLOB, ContainmentName TEXT,
//!      TreeConflict LONG, ContentsHash TEXT, ContentsConflict(s) TEXT, Contents BLOB)
//! _MetaData(_ProductVersion TEXT, _BuildVersion TEXT, _SchemaHash TEXT)
//!   (older MPRs use MendixVersion instead of _ProductVersion)
//! ```
//! `UnitID`/`ContainerID` are 16-byte MS-GUID blobs, not integers. `Contents`
//! is a BSON blob (v1) or absent/NULL (v2, where `.mxunit` files under the
//! `mprcontents` sibling directory hold the data). A unit's `$Type` lives in
//! its BSON contents, not in a separate column.
//!
//! This crate targets Mendix 11.x for v1 (see the mxrs plan's locked MVP
//! scope), so only the v2 storage path is exercised; v1↔v2 migration and the
//! sidecar tables owned by other subsystems (semantic index cache, vector
//! index, architecture metadata, ruby_app sources, domain-diagram anchors)
//! are deliberately out of scope here — see `decisions/mxrs-rust-rewrite-plan.md`.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::error::{MprError, Result};
use crate::format::{self, StorageFormat};
use crate::mxunit;
use crate::transaction::V2TransactionState;

/// A `Unit` table row plus its decoded UUID columns. `contents` is only
/// populated for v1 inline-BSON storage; for v2, use
/// [`MprFile::content_bytes`]/[`MprFile::parse_contents`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawUnit {
    pub unit_id: String,
    pub container_id: String,
    pub containment_name: String,
    pub contents_hash: Option<String>,
    pub contents: Option<Vec<u8>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WriteStats {
    pub inserted: u64,
    pub updated: u64,
    pub skipped: u64,
    pub deleted: u64,
}

/// [`MprFile::raw_query`]'s result — column names plus each row's cells in
/// the same order, since a debug query's shape isn't known ahead of time
/// (unlike [`RawUnit`], which always has the same fixed columns).
#[derive(Debug, Clone, PartialEq)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlCell>>,
}

/// SQLite's own dynamic column typing, carried through undecoded — a
/// `Contents` blob is opaque BSON here, not parsed (that's
/// [`MprFile::parse_contents`]'s job on an actual [`RawUnit`], not a debug
/// query result of arbitrary shape).
#[derive(Debug, Clone, PartialEq)]
pub enum SqlCell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqlCell {
    fn from_value_ref(value: rusqlite::types::ValueRef<'_>) -> rusqlite::Result<Self> {
        Ok(match value {
            rusqlite::types::ValueRef::Null => SqlCell::Null,
            rusqlite::types::ValueRef::Integer(i) => SqlCell::Integer(i),
            rusqlite::types::ValueRef::Real(f) => SqlCell::Real(f),
            rusqlite::types::ValueRef::Text(t) => {
                SqlCell::Text(String::from_utf8_lossy(t).into_owned())
            }
            rusqlite::types::ValueRef::Blob(b) => SqlCell::Blob(b.to_vec()),
        })
    }
}

impl std::fmt::Display for SqlCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SqlCell::Null => write!(f, "NULL"),
            SqlCell::Integer(i) => write!(f, "{i}"),
            SqlCell::Real(r) => write!(f, "{r}"),
            SqlCell::Text(s) => write!(f, "{s}"),
            SqlCell::Blob(b) => write!(f, "<{} byte blob>", b.len()),
        }
    }
}

/// A dynamically-typed SQL parameter, used where the column list (and thus
/// the parameter list) is only known at runtime — mirrors Ruby's duck-typed
/// bind array in `insert_unit`.
enum SqlValue {
    Blob(Vec<u8>),
    Text(String),
    Int(i64),
    Null,
}

impl rusqlite::ToSql for SqlValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(match self {
            SqlValue::Blob(b) => rusqlite::types::ToSqlOutput::from(b.as_slice()),
            SqlValue::Text(s) => rusqlite::types::ToSqlOutput::from(s.as_str()),
            SqlValue::Int(i) => rusqlite::types::ToSqlOutput::from(*i),
            SqlValue::Null => rusqlite::types::ToSqlOutput::from(rusqlite::types::Null),
        })
    }
}

pub struct MprFile {
    conn: Connection,
    path: PathBuf,
    readonly: bool,
    format: StorageFormat,
    write_stats: WriteStats,
    pub(crate) v2_transaction: Option<V2TransactionState>,
}

impl MprFile {
    /// Creates a brand-new v2-storage `.mpr` at `path` — a self-referential
    /// root `Projects$Project` unit plus the sibling `mprcontents` directory
    /// — and opens it read-write. Mirrors `Writer#create_project!`'s v2
    /// branch; v1 (inline-BSON) creation is out of this crate's locked MVP
    /// scope. `schema_hash` is caller-supplied (via `mxrs-schema::schema_hash`)
    /// rather than looked up here, keeping this crate independent of the
    /// version/schema registry.
    pub fn create(path: impl AsRef<Path>, version: &str, schema_hash: &str) -> Result<Self> {
        let path = std::path::absolute(path.as_ref())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE _MetaData (
                _FormatVersion INTEGER, _ProductVersion TEXT,
                _BuildVersion TEXT, _SchemaHash TEXT
            );
            CREATE TABLE Unit (
                UnitID BLOB PRIMARY KEY NOT NULL, ContainerID BLOB,
                ContainmentName TEXT, TreeConflict LONG,
                ContentsHash TEXT, ContentsConflicts TEXT
            );",
        )?;
        conn.execute(
            "INSERT INTO _MetaData (_FormatVersion, _ProductVersion, _BuildVersion, _SchemaHash) \
             VALUES (2, ?1, ?1, ?2)",
            rusqlite::params![version, schema_hash],
        )?;

        let contents_dir = format::contents_dir(&path);
        std::fs::create_dir_all(&contents_dir)?;

        let root_id = uuid::Uuid::new_v4().to_string();
        let doc = mxrs_bson::doc! { "$ID": root_id.clone(), "$Type": "Projects$Project", "IsSystemProject": false };
        let bytes = mxrs_bson::serialize(&doc)?;
        let blob = mxrs_bson::uuid_to_blob(&root_id)?.to_vec();
        conn.execute(
            "INSERT INTO Unit (UnitID, ContainerID, ContainmentName, TreeConflict, ContentsHash, ContentsConflicts) \
             VALUES (?1, ?1, '', 0, ?2, '')",
            rusqlite::params![blob, mxrs_bson::contents_hash(&bytes)],
        )?;
        mxunit::write_atomic(&mxunit::path_for(&contents_dir, &root_id), &bytes)?;
        drop(conn);

        Self::open(&path, false)
    }

    pub fn open(path: impl AsRef<Path>, readonly: bool) -> Result<Self> {
        let path = std::path::absolute(path.as_ref())?;
        let flags = OpenFlags::SQLITE_OPEN_NO_MUTEX
            | if readonly {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            };
        let conn = Connection::open_with_flags(&path, flags)
            .map_err(|e| MprError::NotSqlite(format!("cannot open {}: {e}", path.display())))?;
        validate(&path, &conn)?;
        let format = format::detect_format(&conn, &path)?;

        let mut mpr = Self {
            conn,
            path,
            readonly,
            format,
            write_stats: WriteStats::default(),
            v2_transaction: None,
        };
        mpr.recover_interrupted_v2_transaction()?;
        Ok(mpr)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn readonly(&self) -> bool {
        self.readonly
    }

    pub fn format(&self) -> StorageFormat {
        self.format
    }

    pub fn write_stats(&self) -> WriteStats {
        self.write_stats
    }

    /// Runs an arbitrary read query against the underlying SQLite store —
    /// mirrors `Mxrb::Project#query`/`bin/mxrb`'s `sql` command, a debugging
    /// escape hatch for inspecting a `.mpr`'s raw storage shape (schema
    /// exploration, ad-hoc row counts, ...), not a supported data-access API.
    /// Like the Ruby original, this doesn't restrict the query to `SELECT` —
    /// callers on a `readonly: true`-opened file are protected by SQLite's
    /// own read-only connection mode; callers on a writable file are not.
    pub fn raw_query(&self, sql: &str) -> Result<SqlResult> {
        let mut stmt = self.conn.prepare(sql)?;
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt
            .query_map([], |row| {
                (0..columns.len())
                    .map(|i| SqlCell::from_value_ref(row.get_ref(i)?))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(SqlResult { columns, rows })
    }

    pub fn tables(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names)
    }

    // ── Metadata ─────────────────────────────────────────────────────────

    pub fn mendix_version(&self) -> Result<Option<String>> {
        if let Ok(v) =
            self.conn
                .query_row("SELECT _ProductVersion FROM _MetaData LIMIT 1", [], |row| {
                    row.get::<_, Option<String>>(0)
                })
        {
            return Ok(v);
        }
        if let Ok(v) =
            self.conn
                .query_row("SELECT MendixVersion FROM _MetaData LIMIT 1", [], |row| {
                    row.get::<_, Option<String>>(0)
                })
        {
            return Ok(v);
        }
        Ok(None)
    }

    pub fn update_version(&mut self, version: &str, schema_hash: &str) -> Result<()> {
        let result = self.conn.execute(
            "UPDATE _MetaData SET _ProductVersion = ?1, _BuildVersion = ?2, _SchemaHash = ?3",
            rusqlite::params![version, version, schema_hash],
        );
        if result.is_err() {
            self.conn.execute(
                "UPDATE _MetaData SET MendixVersion = ?1",
                rusqlite::params![version],
            )?;
        }
        Ok(())
    }

    // ── Unit access ──────────────────────────────────────────────────────

    /// The root Unit is the one where `UnitID == ContainerID`.
    pub fn root_unit(&self) -> Result<Option<RawUnit>> {
        let sql = format!(
            "SELECT {} FROM Unit WHERE UnitID = ContainerID LIMIT 1",
            self.unit_select_columns()?
        );
        self.query_optional_unit(&sql, [])
    }

    pub fn units_by_containment(&self, name: &str) -> Result<Vec<RawUnit>> {
        let sql = format!(
            "SELECT {} FROM Unit WHERE ContainmentName = ?1",
            self.unit_select_columns()?
        );
        self.query_units(&sql, rusqlite::params![name])
    }

    pub fn children_of(&self, parent_uuid: &str) -> Result<Vec<RawUnit>> {
        let blob = mxrs_bson::uuid_to_blob(parent_uuid)?.to_vec();
        let sql = format!(
            "SELECT {} FROM Unit WHERE ContainerID = ?1 AND UnitID != ContainerID",
            self.unit_select_columns()?
        );
        self.query_units(&sql, rusqlite::params![blob])
    }

    pub fn unit(&self, uuid: &str) -> Result<Option<RawUnit>> {
        let blob = mxrs_bson::uuid_to_blob(uuid)?.to_vec();
        let sql = format!(
            "SELECT {} FROM Unit WHERE UnitID = ?1",
            self.unit_select_columns()?
        );
        self.query_optional_unit(&sql, rusqlite::params![blob])
    }

    pub fn all_units(&self) -> Result<Vec<RawUnit>> {
        let sql = format!("SELECT {} FROM Unit", self.unit_select_columns()?);
        self.query_units(&sql, [])
    }

    pub fn parse_contents(&self, unit: &RawUnit) -> Result<mxrs_bson::Document> {
        match self.content_bytes(unit)? {
            Some(bytes) if !bytes.is_empty() => Ok(mxrs_bson::parse(&bytes)?),
            _ => Ok(mxrs_bson::Document::new()),
        }
    }

    pub fn content_bytes(&self, unit: &RawUnit) -> Result<Option<Vec<u8>>> {
        let inline_empty = unit.contents.as_deref().is_none_or(<[u8]>::is_empty);
        if !(inline_empty && self.format == StorageFormat::V2) {
            return Ok(unit.contents.clone());
        }

        if let Some(state) = &self.v2_transaction {
            if let Some(staged) = state.staged_content(&unit.unit_id) {
                return Ok(staged.map(<[u8]>::to_vec));
            }
        }

        let path = mxunit::path_for(&self.contents_dir(), &unit.unit_id);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(std::fs::read(path)?))
    }

    pub fn content_path(&self, unit: &RawUnit) -> Option<PathBuf> {
        if self.format != StorageFormat::V2 {
            return None;
        }
        Some(mxunit::path_for(&self.contents_dir(), &unit.unit_id))
    }

    pub fn content_files(&self) -> Result<Vec<PathBuf>> {
        if self.format != StorageFormat::V2 {
            return Ok(vec![]);
        }
        let dir = self.contents_dir();
        if !dir.is_dir() {
            return Ok(vec![]);
        }
        let mut result = Vec::new();
        Self::walk_mxunit(&dir, &mut result)?;
        result.sort();
        Ok(result)
    }

    // ── Writes ───────────────────────────────────────────────────────────

    /// Inserts a new unit and returns its assigned UUID. `contents_doc`'s
    /// `$ID` (if present) wins over `unit_uuid`; a random UUID is generated
    /// if neither is supplied. When `contents_doc` has no `$ID` at all, one
    /// is added as its first key.
    pub fn insert_unit(
        &mut self,
        container_uuid: &str,
        containment_name: &str,
        contents_doc: mxrs_bson::Document,
        unit_uuid: Option<&str>,
    ) -> Result<String> {
        if self.readonly {
            return Err(MprError::ReadOnly);
        }

        let extracted = contents_doc.get("$ID").and_then(mxrs_bson::extract_id);
        let uuid = extracted
            .or_else(|| unit_uuid.map(str::to_string))
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let mut doc = contents_doc;
        if !doc.contains_key("$ID") {
            let mut prefixed = mxrs_bson::Document::new();
            prefixed.insert("$ID", uuid.clone());
            for (k, v) in &doc {
                prefixed.insert(k.clone(), v.clone());
            }
            doc = prefixed;
        }

        let unit_blob = mxrs_bson::uuid_to_blob(&uuid)?.to_vec();
        let parent_blob = mxrs_bson::uuid_to_blob(container_uuid)?.to_vec();
        let bson_bytes = self.serialize_contents(&doc)?;
        let hash = mxrs_bson::contents_hash(&bson_bytes);

        let has_contents = format::contents_column(&self.conn)?;
        let conflicts = conflicts_column(&self.conn)?;

        let mut columns: Vec<&str> = vec![
            "UnitID",
            "ContainerID",
            "ContainmentName",
            "TreeConflict",
            "ContentsHash",
        ];
        let mut values: Vec<SqlValue> = vec![
            SqlValue::Blob(unit_blob),
            SqlValue::Blob(parent_blob),
            SqlValue::Text(containment_name.to_string()),
            SqlValue::Int(0),
            SqlValue::Text(hash),
        ];
        if let Some(col) = &conflicts {
            columns.push(col.as_str());
            values.push(SqlValue::Text(String::new()));
        }
        if has_contents {
            columns.push("Contents");
            values.push(if self.format == StorageFormat::V2 {
                SqlValue::Null
            } else {
                SqlValue::Blob(bson_bytes.clone())
            });
        }

        let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT INTO Unit ({}) VALUES ({})",
            columns.join(", "),
            placeholders.join(", ")
        );
        self.conn
            .execute(&sql, rusqlite::params_from_iter(values.iter()))?;

        if self.format == StorageFormat::V2 {
            self.write_v2_unit(&uuid, &bson_bytes)?;
        }
        self.write_stats.inserted += 1;
        Ok(uuid)
    }

    /// Updates an existing unit's contents, recalculating `ContentsHash`.
    /// Returns `false` (a no-op) when the new content hashes identically to
    /// what's already stored.
    pub fn update_unit(&mut self, uuid: &str, contents_doc: mxrs_bson::Document) -> Result<bool> {
        if self.readonly {
            return Err(MprError::ReadOnly);
        }

        let blob = mxrs_bson::uuid_to_blob(uuid)?.to_vec();
        let bson_bytes = self.serialize_contents(&contents_doc)?;
        let hash = mxrs_bson::contents_hash(&bson_bytes);

        if let Some(current) = self.unit(uuid)? {
            if current.contents_hash.as_deref() == Some(hash.as_str()) {
                self.write_stats.skipped += 1;
                return Ok(false);
            }
        }

        if format::contents_column(&self.conn)? {
            let stored: Option<Vec<u8>> = if self.format == StorageFormat::V2 {
                None
            } else {
                Some(bson_bytes.clone())
            };
            self.conn.execute(
                "UPDATE Unit SET Contents = ?1, ContentsHash = ?2 WHERE UnitID = ?3",
                rusqlite::params![stored, hash, blob],
            )?;
        } else {
            self.conn.execute(
                "UPDATE Unit SET ContentsHash = ?1 WHERE UnitID = ?2",
                rusqlite::params![hash, blob],
            )?;
        }

        if self.format == StorageFormat::V2 {
            self.write_v2_unit(uuid, &bson_bytes)?;
        }
        self.write_stats.updated += 1;
        Ok(true)
    }

    pub fn delete_unit(&mut self, uuid: &str) -> Result<Vec<PathBuf>> {
        if self.readonly {
            return Err(MprError::ReadOnly);
        }
        let blob = mxrs_bson::uuid_to_blob(uuid)?.to_vec();
        self.conn.execute(
            "DELETE FROM Unit WHERE UnitID = ?1",
            rusqlite::params![blob],
        )?;
        let removed = if self.format == StorageFormat::V2 {
            self.delete_v2_unit(uuid)?
        } else {
            vec![]
        };
        self.write_stats.deleted += 1;
        Ok(removed)
    }

    pub fn relocate_unit(
        &mut self,
        uuid: &str,
        container_uuid: &str,
        containment_name: &str,
    ) -> Result<()> {
        if self.readonly {
            return Err(MprError::ReadOnly);
        }
        self.conn.execute(
            "UPDATE Unit SET ContainerID = ?1, ContainmentName = ?2 WHERE UnitID = ?3",
            rusqlite::params![
                mxrs_bson::uuid_to_blob(container_uuid)?.to_vec(),
                containment_name,
                mxrs_bson::uuid_to_blob(uuid)?.to_vec()
            ],
        )?;
        Ok(())
    }

    // ── Transactions ─────────────────────────────────────────────────────

    /// Runs `f` inside a SQL transaction. For v2 storage, `.mxunit` file
    /// writes/deletes made through `f` are staged in memory and only
    /// physically applied to disk right before the SQL transaction commits
    /// (backed by a journal so an interrupted transaction can be rolled
    /// back or recovered on next [`MprFile::open`]) — see the
    /// [`crate::transaction`] module docs for the full scheme.
    pub fn transaction<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.format != StorageFormat::V2 {
            self.conn.execute_batch("BEGIN")?;
            return match f(self) {
                Ok(value) => {
                    self.conn.execute_batch("COMMIT")?;
                    Ok(value)
                }
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            };
        }

        if self.v2_transaction.is_some() {
            return Err(MprError::NestedTransaction);
        }
        self.with_v2_transaction(f)
    }

    fn with_v2_transaction<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let id = uuid::Uuid::new_v4().to_string();
        self.v2_transaction = Some(V2TransactionState::new(id.clone()));
        self.conn.execute_batch("BEGIN")?;

        let value = match f(self) {
            Ok(value) => value,
            Err(e) => return self.abort_v2_transaction(e),
        };
        if let Err(e) = self.apply_v2_transaction() {
            return self.abort_v2_transaction(e);
        }

        self.conn.execute_batch("COMMIT")?;
        self.cleanup_v2_transaction();
        self.clear_v2_transaction_marker(&id);
        self.v2_transaction = None;
        Ok(value)
    }

    fn abort_v2_transaction<T>(&mut self, error: MprError) -> Result<T> {
        let _ = self.conn.execute_batch("ROLLBACK");
        self.rollback_v2_transaction();
        self.cleanup_v2_transaction();
        self.v2_transaction = None;
        Err(error)
    }

    fn apply_v2_transaction(&mut self) -> Result<()> {
        let identifiers = self
            .v2_transaction
            .as_ref()
            .expect("apply_v2_transaction called outside a transaction")
            .touched_uuids();
        if identifiers.is_empty() {
            return Ok(());
        }

        let journal_dir = self.transaction_journal_dir();
        if journal_dir.exists() {
            return Err(MprError::IncompletePackage(format!(
                "stale MPR transaction journal exists: {}",
                journal_dir.display()
            )));
        }
        self.write_v2_transaction_manifest(&identifiers)?;
        self.v2_transaction.as_mut().unwrap().journal_dir = Some(journal_dir);

        let id = self.v2_transaction.as_ref().unwrap().id.clone();
        self.register_v2_transaction_marker(&id)?;

        for uuid in &identifiers {
            self.apply_v2_transaction_unit(uuid)?;
        }
        Ok(())
    }

    fn apply_v2_transaction_unit(&mut self, uuid: &str) -> Result<()> {
        let dir = self.contents_dir();
        let path = mxunit::path_for(&dir, uuid);
        let journal_dir = self
            .v2_transaction
            .as_ref()
            .and_then(|s| s.journal_dir.clone())
            .expect("journal_dir set by write_v2_transaction_manifest before this runs");
        let relative = path.strip_prefix(&dir).unwrap_or(&path).to_path_buf();
        let backup = journal_dir.join("original").join(&relative);
        let existed = path.is_file();

        self.v2_transaction
            .as_mut()
            .unwrap()
            .applied
            .push(crate::transaction::AppliedEntry {
                path: path.clone(),
                backup: backup.clone(),
                existed,
            });

        if existed {
            if let Some(parent) = backup.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&path, &backup)?;
        }

        let bytes = self
            .v2_transaction
            .as_ref()
            .unwrap()
            .writes
            .get(uuid)
            .cloned();
        if let Some(bytes) = bytes {
            mxunit::write_atomic(&path, &bytes)?;
        }
        Ok(())
    }

    fn rollback_v2_transaction(&mut self) {
        let Some(state) = &self.v2_transaction else {
            return;
        };
        for entry in state.applied.iter().rev() {
            let _ = std::fs::remove_file(&entry.path);
            if entry.existed && entry.backup.is_file() {
                if let Some(parent) = entry.path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::rename(&entry.backup, &entry.path);
            }
        }
    }

    fn cleanup_v2_transaction(&mut self) {
        if let Some(dir) = self
            .v2_transaction
            .as_ref()
            .and_then(|s| s.journal_dir.clone())
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn transaction_journal_dir(&self) -> PathBuf {
        let mut s = self.path.clone().into_os_string();
        s.push(".mxrb-transaction");
        PathBuf::from(s)
    }

    fn transaction_manifest_path(&self) -> PathBuf {
        self.transaction_journal_dir().join("journal.json")
    }

    fn write_v2_transaction_manifest(&self, identifiers: &[String]) -> Result<()> {
        let dir = self.contents_dir();
        let state = self.v2_transaction.as_ref().expect("transaction active");
        let entries: Vec<crate::transaction::ManifestEntry> = identifiers
            .iter()
            .map(|uuid| {
                let path = mxunit::path_for(&dir, uuid);
                let relative = path
                    .strip_prefix(&dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                crate::transaction::ManifestEntry {
                    uuid: uuid.clone(),
                    relative_path: relative,
                    existed: path.is_file(),
                    action: if state.writes.contains_key(uuid) {
                        crate::transaction::ManifestAction::Write
                    } else {
                        crate::transaction::ManifestAction::Delete
                    },
                }
            })
            .collect();
        let manifest = crate::transaction::TransactionManifest {
            version: 1,
            id: state.id.clone(),
            entries,
        };

        let journal_dir = self.transaction_journal_dir();
        std::fs::create_dir_all(&journal_dir)?;
        let json = serde_json::to_vec_pretty(&manifest)?;
        mxunit::write_atomic(&self.transaction_manifest_path(), &json)?;
        Ok(())
    }

    fn register_v2_transaction_marker(&mut self, id: &str) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _MxrbFileTransaction (ID TEXT PRIMARY KEY NOT NULL)",
        )?;
        self.conn.execute(
            "INSERT INTO _MxrbFileTransaction (ID) VALUES (?1)",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    /// Best-effort: mirrors Ruby's `rescue SQLite3::Exception; nil` — a
    /// failure here must never mask the outcome of the transaction it's
    /// cleaning up after.
    fn clear_v2_transaction_marker(&mut self, id: &str) {
        let has_table = self
            .tables()
            .map(|t| t.iter().any(|n| n == "_MxrbFileTransaction"))
            .unwrap_or(false);
        if !has_table {
            return;
        }
        let _ = self.conn.execute(
            "DELETE FROM _MxrbFileTransaction WHERE ID = ?1",
            rusqlite::params![id],
        );
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM _MxrbFileTransaction", [], |r| {
                r.get(0)
            })
            .unwrap_or(1);
        if count == 0 {
            let _ = self.conn.execute_batch("DROP TABLE _MxrbFileTransaction");
        }
    }

    // ── Backup / restore ─────────────────────────────────────────────────

    /// Creates a consistent point-in-time backup via SQLite's `VACUUM INTO`,
    /// falling back to a WAL checkpoint + file copy on older SQLite builds.
    pub fn backup(&mut self, dest_path: impl AsRef<Path>) -> Result<()> {
        if self.readonly {
            return Err(MprError::ReadOnly);
        }
        let dest_path = dest_path.as_ref();
        self.cleanup_backup(dest_path);
        if let Err(e) = self.backup_inner(dest_path) {
            self.cleanup_backup(dest_path);
            return Err(e);
        }
        Ok(())
    }

    fn backup_inner(&mut self, dest_path: &Path) -> Result<()> {
        let dest_str = dest_path.to_string_lossy().into_owned();
        if self
            .conn
            .execute("VACUUM INTO ?1", rusqlite::params![dest_str])
            .is_err()
        {
            let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(FULL)");
            std::fs::copy(&self.path, dest_path)?;
        }
        if self.format == StorageFormat::V2 {
            self.backup_v2_contents(dest_path)?;
        }
        Ok(())
    }

    /// Removes all artifacts created by [`MprFile::backup`] for `dest_path`.
    pub fn cleanup_backup(&self, dest_path: &Path) {
        let _ = std::fs::remove_file(dest_path);
        let _ = std::fs::remove_dir_all(Self::backup_contents_dir(dest_path));
    }

    fn backup_contents_dir(dest_path: &Path) -> PathBuf {
        let mut s = dest_path.as_os_str().to_owned();
        s.push(".mprcontents");
        PathBuf::from(s)
    }

    fn backup_v2_contents(&self, dest_path: &Path) -> Result<()> {
        let snapshot_dir = Self::backup_contents_dir(dest_path);
        let _ = std::fs::remove_dir_all(&snapshot_dir);
        copy_dir_recursive(&self.contents_dir(), &snapshot_dir)
    }

    /// Restores this database from a backup file, replacing current
    /// contents, and reopens the underlying SQLite connection.
    pub fn restore_from(&mut self, backup_path: impl AsRef<Path>) -> Result<()> {
        if self.readonly {
            return Err(MprError::ReadOnly);
        }
        let backup_path = backup_path.as_ref();
        let needs_snapshot = self.preflight_backup(backup_path)?;

        std::fs::copy(backup_path, &self.path)?;
        let flags = OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_READ_WRITE;
        self.conn = Connection::open_with_flags(&self.path, flags).map_err(|e| {
            MprError::NotSqlite(format!("cannot open {}: {e}", self.path.display()))
        })?;

        if needs_snapshot {
            std::fs::create_dir_all(self.contents_dir())?;
            self.restore_v2_contents(backup_path)?;
        }
        self.format = format::detect_format(&self.conn, &self.path)?;
        Ok(())
    }

    fn preflight_backup(&self, backup_path: &Path) -> Result<bool> {
        use std::io::Read;
        let mut magic = [0u8; 16];
        let magic_ok = std::fs::File::open(backup_path)
            .and_then(|mut f| f.read_exact(&mut magic))
            .is_ok()
            && magic.starts_with(b"SQLite format 3");
        if !magic_ok {
            return Err(MprError::NotSqlite(backup_path.display().to_string()));
        }

        let backup_conn = Connection::open_with_flags(
            backup_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| MprError::NotSqlite(format!("cannot open {}: {e}", backup_path.display())))?;
        let unit_table_count: i64 = backup_conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='Unit'",
            [],
            |r| r.get(0),
        )?;
        if unit_table_count == 0 {
            return Err(MprError::MissingUnitTable(
                backup_path.display().to_string(),
            ));
        }

        let has_contents_column = format::contents_column(&backup_conn)?;
        let snapshot_dir = Self::backup_contents_dir(backup_path);
        let needs_snapshot = !has_contents_column || snapshot_dir.is_dir();
        if needs_snapshot && !snapshot_dir.is_dir() {
            return Err(MprError::IncompletePackage(format!(
                "{}: v2 MPR backup is missing contents snapshot {}",
                backup_path.display(),
                snapshot_dir.display()
            )));
        }
        Ok(needs_snapshot)
    }

    fn restore_v2_contents(&self, backup_path: &Path) -> Result<()> {
        let snapshot_dir = Self::backup_contents_dir(backup_path);
        let dir = self.contents_dir();

        let mut snapshot_files = Vec::new();
        Self::walk_mxunit(&snapshot_dir, &mut snapshot_files)?;
        let snapshot_relative: Vec<PathBuf> = snapshot_files
            .iter()
            .map(|p| p.strip_prefix(&snapshot_dir).unwrap().to_path_buf())
            .collect();

        let mut live_files = Vec::new();
        Self::walk_mxunit(&dir, &mut live_files)?;
        let live_relative: Vec<PathBuf> = live_files
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_path_buf())
            .collect();

        for relative in &snapshot_relative {
            let destination = dir.join(relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(snapshot_dir.join(relative), destination)?;
        }
        for relative in &live_relative {
            if !snapshot_relative.contains(relative) {
                let _ = std::fs::remove_file(dir.join(relative));
            }
        }
        Ok(())
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    pub(crate) fn contents_dir(&self) -> PathBuf {
        format::contents_dir(&self.path)
    }

    fn unit_select_columns(&self) -> Result<String> {
        let contents = if format::contents_column(&self.conn)? {
            "Contents"
        } else {
            "NULL AS Contents"
        };
        Ok(format!(
            "UnitID, ContainerID, ContainmentName, ContentsHash, {contents}"
        ))
    }

    fn query_units<P: rusqlite::Params>(&self, sql: &str, params: P) -> Result<Vec<RawUnit>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, row_to_raw_unit)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    fn query_optional_unit<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<Option<RawUnit>> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query_map(params, row_to_raw_unit)?;
        rows.next().transpose().map_err(MprError::from)
    }

    /// Studio Pro 11 serializes integer-valued model properties as BSON
    /// int64 while keeping the Mendix array marker as BSON int32.
    fn serialize_contents(&self, doc: &mxrs_bson::Document) -> Result<Vec<u8>> {
        let int64_properties = self.mendix_version()?.as_deref() == Some("11.12.1");
        let transformed = mxrs_bson::storage_hash(doc, int64_properties);
        Ok(mxrs_bson::serialize(&transformed)?)
    }

    fn write_v2_unit(&mut self, uuid: &str, bytes: &[u8]) -> Result<()> {
        if let Some(state) = &mut self.v2_transaction {
            state.stage_write(uuid, bytes.to_vec());
            Ok(())
        } else {
            mxunit::write_atomic(&mxunit::path_for(&self.contents_dir(), uuid), bytes)
        }
    }

    fn delete_v2_unit(&mut self, uuid: &str) -> Result<Vec<PathBuf>> {
        let path = mxunit::path_for(&self.contents_dir(), uuid);
        if let Some(state) = &mut self.v2_transaction {
            let existed = state.stage_delete(uuid, path.is_file());
            Ok(if existed { vec![path] } else { vec![] })
        } else {
            let existed = path.is_file();
            if existed {
                std::fs::remove_file(&path)?;
            }
            Ok(if existed { vec![path] } else { vec![] })
        }
    }

    fn walk_mxunit(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                Self::walk_mxunit(&path, out)?;
            } else if path.extension().and_then(|e| e.to_str()) == Some("mxunit") {
                out.push(path);
            }
        }
        Ok(())
    }

    /// On-open recovery of an interrupted v2 staged transaction: if a
    /// journal directory survived from a previous run, its manifest tells
    /// us whether the SQL transaction it belonged to actually committed
    /// (via the `_MxrbFileTransaction` marker row). If it didn't, the
    /// `.mxunit` files that were already written to disk before the SQL
    /// commit point must be rolled back to their pre-transaction state.
    fn recover_interrupted_v2_transaction(&mut self) -> Result<()> {
        let journal_dir = self.transaction_journal_dir();
        if !journal_dir.is_dir() {
            return Ok(());
        }
        if self.readonly {
            return Err(MprError::IncompletePackage(format!(
                "MPR has an interrupted file transaction; open writable to recover: {}",
                journal_dir.display()
            )));
        }

        let manifest_bytes = std::fs::read(self.transaction_manifest_path())?;
        let manifest: crate::transaction::TransactionManifest =
            serde_json::from_slice(&manifest_bytes)?;

        let has_marker_table = self.tables()?.iter().any(|t| t == "_MxrbFileTransaction");
        let committed = has_marker_table
            && self
                .conn
                .query_row(
                    "SELECT 1 FROM _MxrbFileTransaction WHERE ID = ?1",
                    rusqlite::params![manifest.id],
                    |_| Ok(()),
                )
                .is_ok();

        if !committed {
            self.restore_interrupted_v2_files(&manifest.entries)?;
        }
        std::fs::remove_dir_all(&journal_dir)?;
        self.clear_v2_transaction_marker(&manifest.id);
        Ok(())
    }

    fn restore_interrupted_v2_files(
        &self,
        entries: &[crate::transaction::ManifestEntry],
    ) -> Result<()> {
        let dir = self.contents_dir();
        let journal_dir = self.transaction_journal_dir();
        for entry in entries.iter().rev() {
            let path = dir.join(&entry.relative_path);
            let backup = journal_dir.join("original").join(&entry.relative_path);
            if entry.existed && backup.is_file() {
                let _ = std::fs::remove_file(&path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&backup, &path)?;
            } else if !entry.existed {
                let _ = std::fs::remove_file(&path);
            }
        }
        Ok(())
    }

    /// Studio Pro 11 requires both the `_Transaction` marker table and an
    /// `mprname` sidecar file for externally stored v2 unit contents.
    pub fn ensure_v2_contract(&mut self) -> Result<()> {
        if self.format != StorageFormat::V2 {
            return Ok(());
        }
        if self.readonly {
            return Err(MprError::ReadOnly);
        }

        self.conn
            .execute_batch("CREATE TABLE IF NOT EXISTS _Transaction (LastTransactionID TEXT)")?;
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT LastTransactionID FROM _Transaction LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        if existing.as_deref().unwrap_or("").is_empty() {
            self.conn.execute(
                "INSERT INTO _Transaction (LastTransactionID) VALUES (?1)",
                rusqlite::params![uuid::Uuid::new_v4().to_string()],
            )?;
        }
        self.write_mpr_name()
    }

    fn write_mpr_name(&self) -> Result<()> {
        let dir = self.contents_dir();
        let target = dir.join("mprname");
        let expected = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if target.is_file() && std::fs::read(&target).ok().as_deref() == Some(expected.as_bytes()) {
            return Ok(());
        }
        std::fs::create_dir_all(&dir)?;
        mxunit::write_atomic(&target, expected.as_bytes())
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

fn row_to_raw_unit(row: &rusqlite::Row) -> rusqlite::Result<RawUnit> {
    let unit_id: Vec<u8> = row.get(0)?;
    let container_id: Vec<u8> = row.get(1)?;
    let containment_name: String = row.get(2)?;
    let contents_hash: Option<String> = row.get(3)?;
    let contents: Option<Vec<u8>> = row.get(4)?;
    Ok(RawUnit {
        unit_id: mxrs_bson::blob_to_uuid(&unit_id).unwrap_or_default(),
        container_id: mxrs_bson::blob_to_uuid(&container_id).unwrap_or_default(),
        containment_name,
        contents_hash,
        contents,
    })
}

fn conflicts_column(conn: &Connection) -> Result<Option<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(Unit)")?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(std::result::Result::ok)
        .collect();
    if names.iter().any(|n| n == "ContentsConflicts") {
        Ok(Some("ContentsConflicts".to_string()))
    } else if names.iter().any(|n| n == "ContentsConflict") {
        Ok(Some("ContentsConflict".to_string()))
    } else {
        Ok(None)
    }
}

fn validate(path: &Path, conn: &Connection) -> Result<()> {
    use std::io::Read;
    let mut magic = [0u8; 16];
    let magic_ok = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && magic.starts_with(b"SQLite format 3");
    if !magic_ok {
        return Err(MprError::NotSqlite(path.display().to_string()));
    }

    let table_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='Unit'",
        [],
        |row| row.get(0),
    )?;
    if table_count == 0 {
        return Err(MprError::MissingUnitTable(path.display().to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_v2_fixture(dir: &Path) -> PathBuf {
        let mpr_path = dir.join("Test.mpr");
        let conn = Connection::open(&mpr_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE Unit (
                UnitID BLOB PRIMARY KEY NOT NULL, ContainerID BLOB,
                ContainmentName TEXT, TreeConflict LONG,
                ContentsHash TEXT, ContentsConflicts TEXT
            );
            CREATE TABLE _MetaData (_ProductVersion TEXT, _BuildVersion TEXT, _SchemaHash TEXT);
            INSERT INTO _MetaData (_ProductVersion, _BuildVersion, _SchemaHash)
                VALUES ('11.12.1', '11.12.1', 'test-hash');",
        )
        .unwrap();
        drop(conn);
        std::fs::create_dir_all(dir.join("mprcontents")).unwrap();
        mpr_path
    }

    #[test]
    fn opens_and_detects_v2_format_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mpr = MprFile::open(&path, false).unwrap();
        assert_eq!(mpr.format(), StorageFormat::V2);
        assert_eq!(mpr.mendix_version().unwrap().as_deref(), Some("11.12.1"));
    }

    #[test]
    fn raw_query_returns_columns_and_typed_cells() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mpr = MprFile::open(&path, false).unwrap();
        let result = mpr
            .raw_query("SELECT _ProductVersion, _BuildVersion FROM _MetaData")
            .unwrap();
        assert_eq!(result.columns, vec!["_ProductVersion", "_BuildVersion"]);
        assert_eq!(
            result.rows,
            vec![vec![
                SqlCell::Text("11.12.1".to_string()),
                SqlCell::Text("11.12.1".to_string()),
            ]]
        );
    }

    #[test]
    fn raw_query_reports_null_for_no_matching_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mpr = MprFile::open(&path, false).unwrap();
        let result = mpr.raw_query("SELECT COUNT(*) FROM Unit").unwrap();
        assert_eq!(result.rows, vec![vec![SqlCell::Integer(0)]]);
    }

    #[test]
    fn rejects_non_sqlite_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-db.mpr");
        std::fs::write(&path, b"not a sqlite file").unwrap();
        assert!(matches!(
            MprFile::open(&path, false),
            Err(MprError::NotSqlite(_))
        ));
    }

    #[test]
    fn rejects_sqlite_files_without_a_unit_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.mpr");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE Other (x)")
            .unwrap();
        assert!(matches!(
            MprFile::open(&path, false),
            Err(MprError::MissingUnitTable(_))
        ));
    }

    #[test]
    fn rejects_missing_contents_dir_when_contents_column_absent() {
        let dir = tempfile::tempdir().unwrap();
        let mpr_path = dir.path().join("Test.mpr");
        Connection::open(&mpr_path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE Unit (UnitID BLOB, ContainerID BLOB, ContainmentName TEXT, ContentsHash TEXT);
                 CREATE TABLE _MetaData (_ProductVersion TEXT);",
            )
            .unwrap();
        // No mprcontents/ directory created.
        assert!(matches!(
            MprFile::open(&mpr_path, false),
            Err(MprError::IncompletePackage(_))
        ));
    }

    fn root_doc(uuid: &str) -> mxrs_bson::Document {
        mxrs_bson::doc! { "$ID": uuid, "$Type": "Projects$Project", "Name": "Test" }
    }

    #[test]
    fn create_makes_a_fresh_v2_mpr_with_a_self_referential_root_project_unit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("New.mpr");
        let mpr = MprFile::create(&path, "11.12.1", "test-hash").unwrap();

        assert_eq!(mpr.format(), StorageFormat::V2);
        assert_eq!(mpr.mendix_version().unwrap().as_deref(), Some("11.12.1"));
        assert!(!mpr.readonly());

        let root = mpr.root_unit().unwrap().unwrap();
        assert_eq!(root.unit_id, root.container_id);
        let doc = mpr.parse_contents(&root).unwrap();
        assert_eq!(doc.get_str("$Type").unwrap(), "Projects$Project");
        assert!(dir.path().join("mprcontents").is_dir());
    }

    #[test]
    fn inserts_and_reads_back_a_self_referential_root_unit() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();

        let root_uuid = uuid::Uuid::new_v4().to_string();
        let inserted = mpr
            .insert_unit(
                &root_uuid,
                "Documents",
                root_doc(&root_uuid),
                Some(&root_uuid),
            )
            .unwrap();
        assert_eq!(inserted, root_uuid);

        let root = mpr.root_unit().unwrap().unwrap();
        assert_eq!(root.unit_id, root_uuid);
        assert_eq!(root.container_id, root_uuid);
        assert_eq!(
            mpr.parse_contents(&root).unwrap().get_str("Name").unwrap(),
            "Test"
        );
        assert_eq!(mpr.write_stats().inserted, 1);
    }

    #[test]
    fn insert_unit_without_dollar_id_gets_one_assigned_as_first_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let root_uuid = uuid::Uuid::new_v4().to_string();
        mpr.insert_unit(
            &root_uuid,
            "Documents",
            root_doc(&root_uuid),
            Some(&root_uuid),
        )
        .unwrap();

        let uuid = mpr
            .insert_unit(
                &root_uuid,
                "Documents",
                mxrs_bson::doc! { "$Type": "X", "Name": "child" },
                None,
            )
            .unwrap();
        let unit = mpr.unit(&uuid).unwrap().unwrap();
        let doc = mpr.parse_contents(&unit).unwrap();
        let keys: Vec<&String> = doc.keys().collect();
        assert_eq!(keys.first().map(|k| k.as_str()), Some("$ID"));
        // $ID is itself a BINARY_UUID_KEY, so it round-trips as a binary
        // blob, not a plain string — extract_id() handles both shapes.
        assert_eq!(mxrs_bson::extract_id(doc.get("$ID").unwrap()), Some(uuid));
    }

    #[test]
    fn children_of_and_units_by_containment_find_inserted_units() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let root_uuid = uuid::Uuid::new_v4().to_string();
        mpr.insert_unit(
            &root_uuid,
            "Documents",
            root_doc(&root_uuid),
            Some(&root_uuid),
        )
        .unwrap();
        let child_uuid = mpr
            .insert_unit(
                &root_uuid,
                "Modules",
                mxrs_bson::doc! { "$Type": "Projects$Module", "Name": "Sales" },
                None,
            )
            .unwrap();

        let children = mpr.children_of(&root_uuid).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].unit_id, child_uuid);

        let by_containment = mpr.units_by_containment("Modules").unwrap();
        assert_eq!(by_containment.len(), 1);
        assert_eq!(by_containment[0].unit_id, child_uuid);

        assert_eq!(mpr.all_units().unwrap().len(), 2);
    }

    #[test]
    fn update_unit_skips_when_content_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let uuid = mpr
            .insert_unit(
                &uuid::Uuid::new_v4().to_string(),
                "Documents",
                mxrs_bson::doc! { "Name": "A" },
                None,
            )
            .unwrap();

        let changed = mpr
            .update_unit(&uuid, mxrs_bson::doc! { "$ID": uuid.clone(), "Name": "A" })
            .unwrap();
        assert!(!changed);
        assert_eq!(mpr.write_stats().skipped, 1);

        let changed = mpr
            .update_unit(&uuid, mxrs_bson::doc! { "$ID": uuid.clone(), "Name": "B" })
            .unwrap();
        assert!(changed);
        let unit = mpr.unit(&uuid).unwrap().unwrap();
        assert_eq!(
            mpr.parse_contents(&unit).unwrap().get_str("Name").unwrap(),
            "B"
        );
    }

    #[test]
    fn delete_unit_removes_row_and_mxunit_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let uuid = mpr
            .insert_unit(
                &uuid::Uuid::new_v4().to_string(),
                "Documents",
                mxrs_bson::doc! { "Name": "A" },
                None,
            )
            .unwrap();
        let content_path = mpr
            .unit(&uuid)
            .unwrap()
            .map(|u| mpr.content_path(&u).unwrap())
            .unwrap();
        assert!(content_path.is_file());

        let removed = mpr.delete_unit(&uuid).unwrap();
        assert_eq!(removed, vec![content_path.clone()]);
        assert!(mpr.unit(&uuid).unwrap().is_none());
        assert!(!content_path.is_file());
    }

    #[test]
    fn relocate_unit_changes_container_and_containment() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let root_a = uuid::Uuid::new_v4().to_string();
        let root_b = uuid::Uuid::new_v4().to_string();
        let uuid = mpr
            .insert_unit(&root_a, "Documents", mxrs_bson::doc! { "Name": "A" }, None)
            .unwrap();

        mpr.relocate_unit(&uuid, &root_b, "Folders").unwrap();
        let unit = mpr.unit(&uuid).unwrap().unwrap();
        assert_eq!(unit.container_id, root_b);
        assert_eq!(unit.containment_name, "Folders");
    }

    #[test]
    fn readonly_mode_rejects_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        MprFile::open(&path, false)
            .unwrap()
            .insert_unit(
                &uuid::Uuid::new_v4().to_string(),
                "Documents",
                mxrs_bson::doc! { "Name": "A" },
                None,
            )
            .unwrap();

        let mut mpr = MprFile::open(&path, true).unwrap();
        assert!(mpr.readonly());
        let result = mpr.insert_unit(
            &uuid::Uuid::new_v4().to_string(),
            "Documents",
            mxrs_bson::doc! {},
            None,
        );
        assert!(matches!(result, Err(MprError::ReadOnly)));
    }

    #[test]
    fn round_trips_a_guid_shaped_field_through_sqlite_and_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let guid = uuid::Uuid::new_v4().to_string();
        let uuid = mpr
            .insert_unit(
                &uuid::Uuid::new_v4().to_string(),
                "Documents",
                mxrs_bson::doc! { "GUID": guid.clone(), "Name": "A" },
                None,
            )
            .unwrap();

        let unit = mpr.unit(&uuid).unwrap().unwrap();
        let doc = mpr.parse_contents(&unit).unwrap();
        // On disk the GUID-shaped field is stored as a binary blob, not a
        // plain string — confirm the round trip decodes back to the same
        // UUID via mxrs_bson's $ID-style extraction.
        assert!(matches!(doc.get("GUID"), Some(mxrs_bson::Bson::Binary(_))));
        assert_eq!(mxrs_bson::extract_id(doc.get("GUID").unwrap()), Some(guid));
    }

    #[test]
    fn transaction_commits_all_writes_together() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let root = uuid::Uuid::new_v4().to_string();

        mpr.transaction(|mpr| {
            mpr.insert_unit(&root, "Documents", mxrs_bson::doc! { "Name": "A" }, None)?;
            mpr.insert_unit(&root, "Documents", mxrs_bson::doc! { "Name": "B" }, None)?;
            Ok(())
        })
        .unwrap();

        assert_eq!(mpr.all_units().unwrap().len(), 2);
        assert!(!mpr.transaction_journal_dir().exists());
        assert!(!mpr
            .tables()
            .unwrap()
            .iter()
            .any(|t| t == "_MxrbFileTransaction"));
    }

    #[test]
    fn transaction_rolls_back_sql_and_files_together_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let root = uuid::Uuid::new_v4().to_string();

        let result: Result<()> = mpr.transaction(|mpr| {
            mpr.insert_unit(&root, "Documents", mxrs_bson::doc! { "Name": "A" }, None)?;
            Err(MprError::IncompletePackage("simulated failure".into()))
        });

        assert!(result.is_err());
        assert_eq!(mpr.all_units().unwrap().len(), 0);
        assert!(!mpr.transaction_journal_dir().exists());
        assert!(mpr.content_files().unwrap().is_empty());
    }

    #[test]
    fn nested_transactions_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();

        let result: Result<()> = mpr.transaction(|mpr| {
            let inner: Result<()> = mpr.transaction(|_| Ok(()));
            assert!(matches!(inner, Err(MprError::NestedTransaction)));
            Ok(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn backup_and_restore_round_trip_units_and_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let mut mpr = MprFile::open(&path, false).unwrap();
        let uuid = mpr
            .insert_unit(
                &uuid::Uuid::new_v4().to_string(),
                "Documents",
                mxrs_bson::doc! { "Name": "A" },
                None,
            )
            .unwrap();

        let backup_path = dir.path().join("Test.mpr.bak");
        mpr.backup(&backup_path).unwrap();

        mpr.update_unit(&uuid, mxrs_bson::doc! { "$ID": uuid.clone(), "Name": "B" })
            .unwrap();
        assert_eq!(
            mpr.parse_contents(&mpr.unit(&uuid).unwrap().unwrap())
                .unwrap()
                .get_str("Name")
                .unwrap(),
            "B"
        );

        mpr.restore_from(&backup_path).unwrap();
        assert_eq!(
            mpr.parse_contents(&mpr.unit(&uuid).unwrap().unwrap())
                .unwrap()
                .get_str("Name")
                .unwrap(),
            "A"
        );

        mpr.cleanup_backup(&backup_path);
        assert!(!backup_path.is_file());
    }

    /// Simulates a process crash between `apply_v2_transaction!` writing the
    /// new `.mxunit` content and the SQL transaction actually committing:
    /// the journal/manifest exist, but no `_MxrbFileTransaction` marker row
    /// was ever durably written. On next open, mxrs must detect this and
    /// restore the original file content.
    #[test]
    fn recovers_an_interrupted_uncommitted_transaction_by_restoring_originals() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let (uuid, original_bytes) = {
            let mut mpr = MprFile::open(&path, false).unwrap();
            let uuid = mpr
                .insert_unit(
                    &uuid::Uuid::new_v4().to_string(),
                    "Documents",
                    mxrs_bson::doc! { "Name": "A" },
                    None,
                )
                .unwrap();
            let bytes = std::fs::read(
                mpr.content_path(&mpr.unit(&uuid).unwrap().unwrap())
                    .unwrap(),
            )
            .unwrap();
            (uuid, bytes)
        };

        simulate_interrupted_v2_transaction(
            &path,
            &uuid,
            &original_bytes,
            b"new-but-uncommitted",
            false,
        );

        let mpr = MprFile::open(&path, false).unwrap();
        let live_path = mpr
            .content_path(&mpr.unit(&uuid).unwrap().unwrap())
            .unwrap();
        assert_eq!(std::fs::read(live_path).unwrap(), original_bytes);
        assert!(!mpr.transaction_journal_dir().exists());
    }

    /// Same setup, but this time the `_MxrbFileTransaction` marker row *did*
    /// commit before the crash — the file-level change is final and must be
    /// left alone; only the leftover journal/marker need cleaning up.
    #[test]
    fn recovers_an_interrupted_committed_transaction_by_leaving_new_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_v2_fixture(dir.path());
        let (uuid, original_bytes) = {
            let mut mpr = MprFile::open(&path, false).unwrap();
            let uuid = mpr
                .insert_unit(
                    &uuid::Uuid::new_v4().to_string(),
                    "Documents",
                    mxrs_bson::doc! { "Name": "A" },
                    None,
                )
                .unwrap();
            let bytes = std::fs::read(
                mpr.content_path(&mpr.unit(&uuid).unwrap().unwrap())
                    .unwrap(),
            )
            .unwrap();
            (uuid, bytes)
        };

        simulate_interrupted_v2_transaction(
            &path,
            &uuid,
            &original_bytes,
            b"new-and-committed",
            true,
        );

        let mpr = MprFile::open(&path, false).unwrap();
        let live_path = mpr
            .content_path(&mpr.unit(&uuid).unwrap().unwrap())
            .unwrap();
        assert_eq!(std::fs::read(live_path).unwrap(), b"new-and-committed");
        assert!(!mpr.transaction_journal_dir().exists());
        assert!(!mpr
            .tables()
            .unwrap()
            .iter()
            .any(|t| t == "_MxrbFileTransaction"));
    }

    /// Hand-builds the on-disk state a crash would leave behind, bypassing
    /// the private transaction machinery: writes `new_bytes` over the live
    /// `.mxunit` file (as `apply_v2_transaction_unit!` would, before the SQL
    /// commit point), stashes `original_bytes` under the journal's
    /// `original/` backup path, and writes a matching manifest. `committed`
    /// controls whether the `_MxrbFileTransaction` marker row is present.
    fn simulate_interrupted_v2_transaction(
        mpr_path: &Path,
        uuid: &str,
        original_bytes: &[u8],
        new_bytes: &[u8],
        committed: bool,
    ) {
        let dir = format::contents_dir(mpr_path);
        let live_path = mxunit::path_for(&dir, uuid);
        std::fs::write(&live_path, new_bytes).unwrap();

        let mut journal_dir = mpr_path.as_os_str().to_owned();
        journal_dir.push(".mxrb-transaction");
        let journal_dir = PathBuf::from(journal_dir);
        let relative = live_path.strip_prefix(&dir).unwrap().to_path_buf();
        let backup_path = journal_dir.join("original").join(&relative);
        std::fs::create_dir_all(backup_path.parent().unwrap()).unwrap();
        std::fs::write(&backup_path, original_bytes).unwrap();

        let id = uuid::Uuid::new_v4().to_string();
        let manifest = crate::transaction::TransactionManifest {
            version: 1,
            id: id.clone(),
            entries: vec![crate::transaction::ManifestEntry {
                uuid: uuid.to_string(),
                relative_path: relative.to_string_lossy().replace('\\', "/"),
                existed: true,
                action: crate::transaction::ManifestAction::Write,
            }],
        };
        std::fs::write(
            journal_dir.join("journal.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        if committed {
            let conn = Connection::open(mpr_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS _MxrbFileTransaction (ID TEXT PRIMARY KEY NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO _MxrbFileTransaction (ID) VALUES (?1)",
                rusqlite::params![id],
            )
            .unwrap();
        }
    }
}
