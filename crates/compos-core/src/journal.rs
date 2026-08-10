//! The append-only revision journal: canonical tier-1 state
//! (ARCHITECTURE.md §5.2). One JSON record per line; document heads are
//! derived by replay, last record wins — there is no pointer file, so there
//! is exactly one source of truth. The future SQLite rebuild consumes the
//! same replay this module provides.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::VaultError;
use crate::ids::{DocId, ObjectHash, RevisionId};

/// Journal record schema version (`v` field), distinct from the vault
/// format version in compos.json.
pub const JOURNAL_RECORD_VERSION: u32 = 1;

const FIRST_SEGMENT: &str = "0000000001.jsonl";

// DECISION(user): ratify the journal record wire format before the first
// real (non-test) vault exists. This is permanent tier-1 canonical data —
// every future subsystem (SQLite rebuild, `.compos` bundle records, external
// revisions, proposals) replays or emits exactly this shape. Open choices
// (5–10 lines of judgment, recorded here as a comment or amended fields):
//   1. Final field names (`doc`/`rev`/`object` vs longer names)?
//   2. Keep `path` in-record (rename = new record, replay tracks moves) vs
//      a separate doc-id→path sidecar?
//   3. `ts` as integer milliseconds vs RFC 3339 string?
//   4. Policy line: what changes bump `v` (per-record schema) vs
//      `vault_format` (on-disk layout)?
// The slice runs on these scaffold defaults until ratified or amended.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRecord {
    pub v: u32,
    pub ts: u64,
    pub doc: DocId,
    pub rev: RevisionId,
    pub parent: Option<RevisionId>,
    pub object: ObjectHash,
    pub path: String,
    pub origin: RevisionOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RevisionOrigin {
    Editor,
    External,
    ProposalAccept,
    Import,
}

/// Append handle over the journal directory. Constructed only by a
/// write-mode vault after tail repair.
#[derive(Debug)]
pub struct Journal {
    file: File,
}

impl Journal {
    /// Open for appending: repair a torn tail on the last segment (a torn
    /// final record was never fsync-acknowledged, so truncating it is safe —
    /// crash window W7), then open the segment in append mode.
    pub(crate) fn open_write(dir: &Path) -> Result<Self, VaultError> {
        fs::create_dir_all(dir)?;
        let segment_path = match segments(dir)?.pop() {
            Some(last) => last,
            None => dir.join(FIRST_SEGMENT),
        };
        if segment_path.exists() {
            repair_tail(&segment_path)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&segment_path)?;
        Ok(Self { file })
    }

    /// Append one record: serialize, write, flush, fsync. The caller's
    /// acknowledgment point (save transaction step 5) is this fsync.
    pub(crate) fn append(&mut self, record: &JournalRecord) -> Result<(), VaultError> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(())
    }
}

fn segments(dir: &Path) -> Result<Vec<PathBuf>, VaultError> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    out.sort();
    Ok(out)
}

/// Truncate the segment to the end of its last valid, newline-terminated
/// record.
fn repair_tail(segment: &Path) -> Result<(), VaultError> {
    let bytes = fs::read(segment)?;
    let mut valid_end = 0usize;
    let mut start = 0usize;
    while start < bytes.len() {
        let Some(nl) = bytes[start..].iter().position(|&b| b == b'\n') else {
            break; // unterminated tail
        };
        let line = &bytes[start..start + nl];
        if serde_json::from_slice::<JournalRecord>(line).is_err() {
            break; // torn or garbage line: everything from here on is dropped
        }
        valid_end = start + nl + 1;
        start = valid_end;
    }
    if valid_end != bytes.len() {
        let f = OpenOptions::new().write(true).open(segment)?;
        f.set_len(valid_end as u64)?;
        f.sync_all()?;
    }
    Ok(())
}

/// Replay every segment in order. In tolerant mode (read-only opens) a torn
/// or unparseable *final* line of the *final* segment is ignored; in strict
/// mode (write opens run after tail repair) any bad line is corruption.
pub(crate) fn replay(dir: &Path, tolerant: bool) -> Result<Vec<JournalRecord>, VaultError> {
    let segs = segments(dir)?;
    let mut records = Vec::new();
    for (si, seg) in segs.iter().enumerate() {
        let is_last_segment = si + 1 == segs.len();
        let bytes = fs::read(seg)?;
        let mut start = 0usize;
        let mut line_no = 0u64;
        while start < bytes.len() {
            line_no += 1;
            let (line, next, terminated) = match bytes[start..].iter().position(|&b| b == b'\n') {
                Some(nl) => (&bytes[start..start + nl], start + nl + 1, true),
                None => (&bytes[start..], bytes.len(), false),
            };
            match serde_json::from_slice::<JournalRecord>(line) {
                Ok(rec) if terminated => records.push(rec),
                Ok(_) | Err(_) => {
                    let is_final_line = next >= bytes.len();
                    if tolerant && is_last_segment && is_final_line {
                        break; // torn tail: never acknowledged, ignore
                    }
                    return Err(VaultError::JournalCorrupt {
                        segment: seg
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        line: line_no,
                        reason: if terminated {
                            "unparseable record".to_owned()
                        } else {
                            "unterminated record".to_owned()
                        },
                    });
                }
            }
            start = next;
        }
    }
    Ok(records)
}

/// A document's current state as derived from replay.
#[derive(Debug, Clone)]
pub struct DocHead {
    pub rev: RevisionId,
    pub object: ObjectHash,
    pub path: String,
}

/// Derived head state for every document, plus the set of all revision ids
/// ever committed (used by startup reconciliation).
#[derive(Debug, Default)]
pub struct DocIndex {
    heads: HashMap<DocId, DocHead>,
    by_path: HashMap<String, DocId>,
    revs: HashSet<RevisionId>,
}

impl DocIndex {
    /// Build from replayed records, validating each document's linear chain:
    /// a record's parent must equal the document's current head.
    pub(crate) fn build(records: &[JournalRecord]) -> Result<Self, VaultError> {
        let mut index = DocIndex::default();
        for (i, rec) in records.iter().enumerate() {
            let expected = index.heads.get(&rec.doc).map(|h| h.rev.clone());
            if rec.parent != expected {
                return Err(VaultError::JournalCorrupt {
                    segment: String::new(),
                    line: i as u64 + 1,
                    reason: format!(
                        "chain break for {}: parent {:?} but head {:?}",
                        rec.doc, rec.parent, expected
                    ),
                });
            }
            index.apply(rec);
        }
        Ok(index)
    }

    pub(crate) fn apply(&mut self, rec: &JournalRecord) {
        if let Some(prev) = self.heads.get(&rec.doc)
            && prev.path != rec.path
        {
            self.by_path.remove(&prev.path);
        }
        self.heads.insert(
            rec.doc.clone(),
            DocHead {
                rev: rec.rev.clone(),
                object: rec.object.clone(),
                path: rec.path.clone(),
            },
        );
        self.by_path.insert(rec.path.clone(), rec.doc.clone());
        self.revs.insert(rec.rev.clone());
    }

    pub fn head(&self, doc: &DocId) -> Option<&DocHead> {
        self.heads.get(doc)
    }

    pub fn doc_by_path(&self, path: &str) -> Option<&DocId> {
        self.by_path.get(path)
    }

    pub fn contains_rev(&self, rev: &RevisionId) -> bool {
        self.revs.contains(rev)
    }

    pub fn iter_heads(&self) -> impl Iterator<Item = (&DocId, &DocHead)> {
        self.heads.iter()
    }

    pub fn len(&self) -> usize {
        self.heads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heads.is_empty()
    }
}
