//! In-flight state for a v2 "logical transaction": SQLite gives us atomic
//! `Unit`/`_MetaData` row changes, but the `.mxunit` files that hold v2 unit
//! contents live on the filesystem, which has no such guarantee. This stages
//! file writes/deletes in memory during the transaction, applies them to
//! disk (with a journal of originals) right before the SQL transaction
//! commits, and uses a marker row — committed atomically with the SQL
//! changes — to tell, on next open, whether an interrupted transaction's
//! file-level changes need to be rolled back.
//!
//! Ports the `@v2_transaction`-related private methods of
//! `Mxrb::IO::MprFile` in `lib/mxrb/io/mpr_file.rb`.
//!
//! Recovery assumes a trusted project directory and exclusive writer ownership.
//! This journal covers SQL rows and `.mxunit` contents, not arbitrary sidecars
//! written by a transaction closure. Power-loss durability, adversarial journal
//! paths and simultaneous recovery by multiple processes are not established.
//! A recovery failure poisons mutation access until reopen; typed reads on that
//! handle are not a guarantee of a consistent recovered snapshot.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Panic payload used only when a transaction's original panic is followed
/// by a recovery failure. A successful rollback resumes the original payload
/// unchanged. On recovery failure both causes remain available to the caller.
#[derive(Debug)]
pub struct TransactionRecoveryPanic {
    pub original: Box<dyn std::any::Any + Send>,
    pub recovery: crate::MprError,
}

/// A `.mxunit` file that [`apply`](super::mpr_file::MprFile::apply_v2_transaction)
/// has already touched on disk, kept so a failed transaction can be undone.
#[derive(Debug, Clone)]
pub struct AppliedEntry {
    pub path: PathBuf,
    pub backup: PathBuf,
    pub existed: bool,
}

/// Staged, not-yet-applied changes to v2 unit `.mxunit` files for one
/// in-flight transaction.
#[derive(Debug)]
pub struct V2TransactionState {
    pub id: String,
    pub writes: HashMap<String, Vec<u8>>,
    pub deletes: HashSet<String>,
    pub applied: Vec<AppliedEntry>,
    pub journal_dir: Option<PathBuf>,
}

impl V2TransactionState {
    pub fn new(id: String) -> Self {
        Self {
            id,
            writes: HashMap::new(),
            deletes: HashSet::new(),
            applied: Vec::new(),
            journal_dir: None,
        }
    }

    /// Returns `(staged, bytes)`: `staged` is true if this uuid has a
    /// pending write or delete in this transaction (so the caller should
    /// not fall back to reading the on-disk file), `bytes` is the pending
    /// write's content, or `None` for a pending delete.
    pub fn staged_content(&self, uuid: &str) -> Option<Option<&[u8]>> {
        if let Some(bytes) = self.writes.get(uuid) {
            return Some(Some(bytes));
        }
        if self.deletes.contains(uuid) {
            return Some(None);
        }
        None
    }

    pub fn stage_write(&mut self, uuid: &str, bytes: Vec<u8>) {
        self.writes.insert(uuid.to_string(), bytes);
        self.deletes.remove(uuid);
    }

    /// Stages a delete; returns whether the uuid existed before this
    /// transaction touched it (a pending write counts as "existing").
    pub fn stage_delete(&mut self, uuid: &str, existed_on_disk: bool) -> bool {
        let existed = existed_on_disk || self.writes.contains_key(uuid);
        self.writes.remove(uuid);
        self.deletes.insert(uuid.to_string());
        existed
    }

    /// All touched uuids, in a deterministic (sorted) order.
    pub fn touched_uuids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .writes
            .keys()
            .cloned()
            .chain(self.deletes.iter().cloned())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub uuid: String,
    pub relative_path: String,
    pub existed: bool,
    pub action: ManifestAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManifestAction {
    Write,
    Delete,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionManifest {
    pub version: u32,
    pub id: String,
    pub entries: Vec<ManifestEntry>,
}
