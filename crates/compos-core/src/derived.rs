//! `state/compos.db` — the durable-derived tier (ARCHITECTURE.md §5.2,
//! tier 2). Strictly a cache over journal replay: every row is rebuildable,
//! and the whole N/N-1 story for this tier is "any schema skew deletes the
//! file and rebuilds it" (rule 4). Nothing canonical may ever depend on
//! anything in this module.
//!
//! Crash safety: each applied record updates `meta.applied_records` and
//! `meta.last_rev` in the same SQLite transaction, so after a kill the next
//! open sees a consistent prefix of the journal and catches up from there.

use std::fs;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};

use crate::error::VaultError;
use crate::ids::DocId;
use crate::journal::JournalRecord;
use crate::objects::ObjectStore;

/// Bumping this constant is the entire migration story for tier 2: an index
/// whose `PRAGMA user_version` differs — older *or* newer — is destroyed and
/// rebuilt from the journal.
pub const DERIVED_SCHEMA_VERSION: i64 = 1;

const DB_FILE: &str = "compos.db";

const SCHEMA: &str = "
BEGIN;
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT;
CREATE TABLE docs (
  id             INTEGER PRIMARY KEY,
  doc_id         TEXT NOT NULL UNIQUE,
  path           TEXT NOT NULL UNIQUE,
  head_rev       TEXT NOT NULL,
  head_object    TEXT NOT NULL,
  head_ts        INTEGER NOT NULL,
  revision_count INTEGER NOT NULL
) STRICT;
CREATE TABLE revisions (
  rev    TEXT PRIMARY KEY,
  doc_id TEXT NOT NULL,
  parent TEXT,
  object TEXT NOT NULL,
  path   TEXT NOT NULL,
  origin TEXT NOT NULL,
  ts     INTEGER NOT NULL,
  seq    INTEGER NOT NULL UNIQUE
) STRICT;
CREATE VIRTUAL TABLE docs_fts USING fts5(path, body);
COMMIT;
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub doc: DocId,
    pub path: String,
    pub snippet: String,
}

#[derive(Debug)]
pub struct DerivedIndex {
    conn: Connection,
}

impl DerivedIndex {
    /// Open (or create) the index read-write. Any unreadable file or schema
    /// skew destroys the cache and starts fresh — never an error the caller
    /// has to resolve.
    pub(crate) fn open(state_dir: &Path) -> Result<Self, VaultError> {
        fs::create_dir_all(state_dir)?;
        let path = state_dir.join(DB_FILE);
        if let Ok(Some(conn)) = Self::attach(&path) {
            return Ok(Self { conn });
        }
        Self::destroy(&path)?;
        match Self::attach(&path).map_err(derr)? {
            Some(conn) => Ok(Self { conn }),
            None => Err(VaultError::Derived(
                "fresh index reports wrong schema version".into(),
            )),
        }
    }

    /// Open read-only for queries (read-mode vaults). Returns `None` rather
    /// than repairing anything: only the write path may destroy the cache.
    pub(crate) fn open_read_only(state_dir: &Path) -> Option<Self> {
        let path = state_dir.join(DB_FILE);
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .ok()?;
        (version == DERIVED_SCHEMA_VERSION).then_some(Self { conn })
    }

    fn attach(path: &Path) -> rusqlite::Result<Option<Connection>> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            // Fresh (or pre-schema) file: initialize atomically, then stamp
            // the version so a crash in between reads as version 0 again.
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", DERIVED_SCHEMA_VERSION)?;
            return Ok(Some(conn));
        }
        if version != DERIVED_SCHEMA_VERSION {
            return Ok(None);
        }
        Ok(Some(conn))
    }

    fn destroy(path: &Path) -> Result<(), VaultError> {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for suffix in ["", "-wal", "-shm"] {
            match fs::remove_file(path.with_file_name(format!("{name}{suffix}"))) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(VaultError::Io(e)),
            }
        }
        Ok(())
    }

    /// Bring the index up to date with a full journal replay. If the stored
    /// state is not a prefix of `records` (different vault, foreign journal,
    /// or impossible counter) everything is re-derived. Returns whether a
    /// full rebuild happened.
    pub(crate) fn sync(
        &mut self,
        vault_id: &str,
        records: &[JournalRecord],
        objects: &ObjectStore,
    ) -> Result<bool, VaultError> {
        let stored_vault = self.meta_get("vault_id").map_err(derr)?;
        let applied: usize = self
            .meta_get("applied_records")
            .map_err(derr)?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let last_rev = self.meta_get("last_rev").map_err(derr)?;

        let prefix_ok = stored_vault.as_deref() == Some(vault_id)
            && applied <= records.len()
            && (applied == 0
                || records
                    .get(applied - 1)
                    .is_some_and(|r| Some(r.rev.as_str()) == last_rev.as_deref()));

        let rebuilt = !prefix_ok;
        let tx = self.conn.transaction().map_err(derr)?;
        if rebuilt {
            tx.execute_batch(
                "DELETE FROM docs; DELETE FROM revisions; DELETE FROM docs_fts; DELETE FROM meta;",
            )
            .map_err(derr)?;
            meta_set(&tx, "vault_id", vault_id).map_err(derr)?;
        }
        let start = if rebuilt { 0 } else { applied };
        for (i, rec) in records.iter().enumerate().skip(start) {
            let content = objects.read(&rec.object)?;
            apply_record(&tx, rec, &content, (i + 1) as i64).map_err(derr)?;
        }
        tx.commit().map_err(derr)?;
        Ok(rebuilt)
    }

    /// Incremental feed from save-transaction step 6. The caller already has
    /// the content bytes, so no object read happens on the save path.
    pub(crate) fn apply_one(
        &mut self,
        rec: &JournalRecord,
        content: &[u8],
    ) -> Result<(), VaultError> {
        let applied: i64 = self
            .meta_get("applied_records")
            .map_err(derr)?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let tx = self.conn.transaction().map_err(derr)?;
        apply_record(&tx, rec, content, applied + 1).map_err(derr)?;
        tx.commit().map_err(derr)?;
        Ok(())
    }

    /// Full-text search over document bodies and paths, ranked by BM25.
    /// FTS5 query-syntax errors surface as `ValidationFailed`.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, VaultError> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT d.doc_id, d.path, snippet(docs_fts, 1, '[', ']', '…', 12)
                 FROM docs_fts JOIN docs d ON d.id = docs_fts.rowid
                 WHERE docs_fts MATCH ?1
                 ORDER BY bm25(docs_fts), d.path
                 LIMIT ?2",
            )
            .map_err(derr)?;
        let rows = stmt.query_map(params![query, limit], |row| {
            Ok(SearchHit {
                doc: DocId::from_string(row.get(0)?),
                path: row.get(1)?,
                snippet: row.get(2)?,
            })
        });
        match rows {
            Ok(rows) => rows.collect::<Result<Vec<_>, _>>().map_err(|e| {
                // Errors during row iteration on a MATCH query are almost
                // always query-syntax problems — the user's input, not ours.
                VaultError::ValidationFailed {
                    reason: format!("invalid search query: {e}"),
                }
            }),
            Err(e) => Err(VaultError::ValidationFailed {
                reason: format!("invalid search query: {e}"),
            }),
        }
    }

    /// Logical dump of `docs`, ordered — the rule-4 rebuild-equality probe.
    pub fn docs_snapshot(&self) -> Result<Vec<(String, String, String, String)>, VaultError> {
        let mut stmt = self
            .conn
            .prepare("SELECT doc_id, path, head_rev, head_object FROM docs ORDER BY doc_id")
            .map_err(derr)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(derr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(derr)
    }

    /// Logical dump of `revisions` in journal order.
    #[allow(clippy::type_complexity)]
    pub fn revisions_snapshot(
        &self,
    ) -> Result<Vec<(String, String, Option<String>, String, String)>, VaultError> {
        let mut stmt = self
            .conn
            .prepare("SELECT rev, doc_id, parent, object, origin FROM revisions ORDER BY seq")
            .map_err(derr)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .map_err(derr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(derr)
    }

    pub fn doc_count(&self) -> Result<u64, VaultError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM docs", [], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(derr)
    }

    pub fn revision_count(&self) -> Result<u64, VaultError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(derr)
    }

    fn meta_get(&self, key: &str) -> rusqlite::Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
    }
}

fn apply_record(
    tx: &rusqlite::Transaction<'_>,
    rec: &JournalRecord,
    content: &[u8],
    seq: i64,
) -> rusqlite::Result<()> {
    use rusqlite::OptionalExtension;

    let origin = serde_json::to_value(rec.origin)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "editor".to_owned());

    let existing: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, path FROM docs WHERE doc_id = ?1",
            params![rec.doc.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let doc_rowid = match existing {
        Some((id, _old_path)) => {
            tx.execute(
                "UPDATE docs SET path = ?2, head_rev = ?3, head_object = ?4, head_ts = ?5,
                        revision_count = revision_count + 1
                 WHERE id = ?1",
                params![
                    id,
                    rec.path,
                    rec.rev.as_str(),
                    rec.object.as_str(),
                    rec.ts as i64
                ],
            )?;
            id
        }
        None => {
            tx.execute(
                "INSERT INTO docs (doc_id, path, head_rev, head_object, head_ts, revision_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                params![
                    rec.doc.as_str(),
                    rec.path,
                    rec.rev.as_str(),
                    rec.object.as_str(),
                    rec.ts as i64
                ],
            )?;
            tx.last_insert_rowid()
        }
    };

    tx.execute(
        "INSERT INTO revisions (rev, doc_id, parent, object, path, origin, ts, seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            rec.rev.as_str(),
            rec.doc.as_str(),
            rec.parent.as_ref().map(|p| p.as_str()),
            rec.object.as_str(),
            rec.path,
            origin,
            rec.ts as i64,
            seq
        ],
    )?;

    // Deterministic body projection: lossy UTF-8 keeps rebuilds bit-for-bit
    // reproducible even for non-text objects.
    let body = String::from_utf8_lossy(content);
    tx.execute("DELETE FROM docs_fts WHERE rowid = ?1", params![doc_rowid])?;
    tx.execute(
        "INSERT INTO docs_fts (rowid, path, body) VALUES (?1, ?2, ?3)",
        params![doc_rowid, rec.path, body],
    )?;

    meta_set(tx, "applied_records", &seq.to_string())?;
    meta_set(tx, "last_rev", rec.rev.as_str())?;
    Ok(())
}

fn meta_set(conn: &rusqlite::Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn derr(e: rusqlite::Error) -> VaultError {
    VaultError::Derived(e.to_string())
}
