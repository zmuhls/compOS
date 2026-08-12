//! AI proposals: canonical-adjacent durable state (ARCHITECTURE.md §5.2,
//! §9, §12 Review). Proposals live in composd — not in agentd, not in the
//! shell — so agents stay stateless and Review mode survives restarts.
//!
//! Storage is an append-only JSONL event log in `journal/proposals/`, with
//! the same tail-repair and fsync discipline as the revision journal, and
//! state derived by replay — the "journal is truth" philosophy applied to
//! proposals. The subdirectory placement is deliberate: format-1 revision
//! replay globs `*.jsonl` files directly under `journal/` and ignores
//! directories, so old readers never see proposal records (N stays openable
//! by N-1) and new readers open old vaults to an empty proposal log. The
//! ratified `JournalRecord` wire format is untouched; an accepted proposal
//! enters canonical history as an ordinary `proposal-accept` revision
//! through the six-step save transaction.
//!
//! DECISION(user): the proposal record wire format below (short field
//! names, `event`-tagged create/resolve records, line-based hunks) and the
//! `journal/proposals/` placement follow the ratified journal conventions
//! but are not themselves ratified yet. Also open: `proposal.withdraw` is
//! propose-effect and Phase 3 has no per-session identity, so any
//! propose-capable connection may withdraw any open proposal — ownership
//! scoping arrives with agentd session identity. Working defaults ship;
//! ratify or amend before vault format 1 is declared closed for proposals.
//!
//! Version-bump policy, mirroring the journal's three tiers: additive
//! optional fields → no bump; changes old readers would misread (new event
//! kinds, renames, retypes) → bump `v`; layout changes (segmenting, moving
//! the directory) → bump `vault_format`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::VaultError;
use crate::ids::{DocId, ObjectHash, ProposalId, RevisionId};

/// Proposal record schema version (`v` field), independent of the revision
/// journal's record version.
pub const PROPOSAL_RECORD_VERSION: u32 = 1;

const FIRST_SEGMENT: &str = "0000000001.jsonl";

/// One line-based edit against the proposal's base content: delete `del`
/// lines starting at 0-based line `start`, insert `ins` verbatim in their
/// place. Lines are newline-inclusive byte slices, so `ins` must carry its
/// own trailing newline; `start` equal to the base line count with `del == 0`
/// appends at end of file. The ACP adapter (agentd, Phase 3+) converts
/// provider diff formats into exactly this shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub start: u64,
    pub del: u64,
    pub ins: String,
}

/// One event in a proposal's life. Two kinds: `create` opens it with its
/// full content; `resolve` closes it exactly once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalRecord {
    pub v: u32,
    pub ts: u64,
    pub prop: ProposalId,
    #[serde(flatten)]
    pub body: ProposalBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum ProposalBody {
    Create {
        /// The existing document this proposal targets; `None` proposes a
        /// new document at `path` (the writer mints the id at accept time).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        doc: Option<DocId>,
        path: String,
        /// Base revision the hunks apply to; `None` for a new document.
        /// Recorded with its object hash so the base content stays
        /// addressable after the head moves on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<RevisionId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_object: Option<ObjectHash>,
        hunks: Vec<Hunk>,
        /// Provider and model provenance (§7: every AI edit carries it).
        provenance: Value,
        /// Evidence and source references, free-form.
        evidence: Value,
    },
    Resolve {
        resolution: Resolution,
        /// Indexes of the hunks that landed (accepted resolutions only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hunks: Option<Vec<usize>>,
        /// The committed revision (accepted resolutions only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<RevisionId>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    Accepted,
    Rejected,
    Withdrawn,
}

/// A proposal's current state as derived from replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ProposalState {
    Open,
    Accepted {
        hunks: Vec<usize>,
        rev: RevisionId,
        resolved_ms: u64,
    },
    Rejected {
        resolved_ms: u64,
    },
    Withdrawn {
        resolved_ms: u64,
    },
}

impl ProposalState {
    pub fn is_open(&self) -> bool {
        matches!(self, ProposalState::Open)
    }

    pub fn name(&self) -> &'static str {
        match self {
            ProposalState::Open => "open",
            ProposalState::Accepted { .. } => "accepted",
            ProposalState::Rejected { .. } => "rejected",
            ProposalState::Withdrawn { .. } => "withdrawn",
        }
    }
}

/// Full derived state of one proposal.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub id: ProposalId,
    pub doc: Option<DocId>,
    pub path: String,
    pub base: Option<RevisionId>,
    pub base_object: Option<ObjectHash>,
    pub hunks: Vec<Hunk>,
    pub provenance: Value,
    pub evidence: Value,
    pub created_ms: u64,
    pub state: ProposalState,
}

/// Arguments for opening a proposal (the `proposal.create` command).
#[derive(Debug, Clone)]
pub struct CreateProposal {
    pub path: String,
    pub hunks: Vec<Hunk>,
    pub provenance: Value,
    pub evidence: Value,
}

/// What an accepted proposal produced: the committed revision plus the
/// normalized set of hunks that landed.
#[derive(Debug, Clone)]
pub struct AcceptOutcome {
    pub save: crate::writer::SaveOutcome,
    pub accepted_hunks: Vec<usize>,
    pub proposal: Proposal,
}

/// The proposal log: append handle (write mode) plus the replay-derived
/// index. BTreeMap keyed by UUIDv7 id keeps listings in creation order.
#[derive(Debug, Default)]
pub(crate) struct ProposalStore {
    file: Option<File>,
    index: BTreeMap<ProposalId, Proposal>,
}

impl ProposalStore {
    /// Open for appending: repair a torn tail (an unterminated final record
    /// was never acknowledged — same argument as journal window W7), replay
    /// strictly, open the last segment in append mode.
    pub fn open_write(dir: &Path) -> Result<Self, VaultError> {
        fs::create_dir_all(dir)?;
        let segment_path = match segments(dir)?.pop() {
            Some(last) => last,
            None => dir.join(FIRST_SEGMENT),
        };
        if segment_path.exists() {
            repair_tail(&segment_path)?;
        }
        let index = build_index(&replay(dir, false)?)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&segment_path)?;
        Ok(Self {
            file: Some(file),
            index,
        })
    }

    /// Read-only view: tolerate a torn tail, never repair. A missing
    /// directory is an empty log (pre-proposal vaults).
    pub fn open_read(dir: &Path) -> Result<Self, VaultError> {
        if !dir.is_dir() {
            return Ok(Self::default());
        }
        let index = build_index(&replay(dir, true)?)?;
        Ok(Self { file: None, index })
    }

    /// Append one record durably (write + flush + fsync) and fold it into
    /// the index. Callers return to their clients only after this.
    pub fn append(&mut self, record: &ProposalRecord) -> Result<(), VaultError> {
        let file = self.file.as_mut().ok_or(VaultError::ReadOnly)?;
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.flush()?;
        file.sync_all()?;
        apply(&mut self.index, record).map_err(|reason| VaultError::JournalCorrupt {
            segment: "proposals".to_owned(),
            line: 0,
            reason,
        })?;
        Ok(())
    }

    pub fn get(&self, id: &ProposalId) -> Option<&Proposal> {
        self.index.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Proposal> {
        self.index.values()
    }
}

fn segments(dir: &Path) -> Result<Vec<PathBuf>, VaultError> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    out.sort();
    Ok(out)
}

/// Truncate the segment to the end of its last valid, newline-terminated
/// record — the journal's tail-repair logic applied to proposal records.
fn repair_tail(segment: &Path) -> Result<(), VaultError> {
    let bytes = fs::read(segment)?;
    let mut valid_end = 0usize;
    let mut start = 0usize;
    while start < bytes.len() {
        let Some(nl) = bytes[start..].iter().position(|&b| b == b'\n') else {
            break;
        };
        let line = &bytes[start..start + nl];
        if serde_json::from_slice::<ProposalRecord>(line).is_err() {
            break;
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

fn replay(dir: &Path, tolerant: bool) -> Result<Vec<ProposalRecord>, VaultError> {
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
            match serde_json::from_slice::<ProposalRecord>(line) {
                Ok(rec) if terminated => records.push(rec),
                Ok(_) | Err(_) => {
                    let is_final_line = next >= bytes.len();
                    if tolerant && is_last_segment && is_final_line {
                        break;
                    }
                    return Err(VaultError::JournalCorrupt {
                        segment: format!(
                            "proposals/{}",
                            seg.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        line: line_no,
                        reason: if terminated {
                            "unparseable proposal record".to_owned()
                        } else {
                            "unterminated proposal record".to_owned()
                        },
                    });
                }
            }
            start = next;
        }
    }
    Ok(records)
}

fn build_index(records: &[ProposalRecord]) -> Result<BTreeMap<ProposalId, Proposal>, VaultError> {
    let mut index = BTreeMap::new();
    for (i, rec) in records.iter().enumerate() {
        apply(&mut index, rec).map_err(|reason| VaultError::JournalCorrupt {
            segment: "proposals".to_owned(),
            line: i as u64 + 1,
            reason,
        })?;
    }
    Ok(index)
}

/// Fold one record into the index, enforcing the event sequence: create
/// first, resolve exactly once, nothing after.
fn apply(index: &mut BTreeMap<ProposalId, Proposal>, rec: &ProposalRecord) -> Result<(), String> {
    match &rec.body {
        ProposalBody::Create {
            doc,
            path,
            base,
            base_object,
            hunks,
            provenance,
            evidence,
        } => {
            if index.contains_key(&rec.prop) {
                return Err(format!("duplicate create for {}", rec.prop));
            }
            index.insert(
                rec.prop.clone(),
                Proposal {
                    id: rec.prop.clone(),
                    doc: doc.clone(),
                    path: path.clone(),
                    base: base.clone(),
                    base_object: base_object.clone(),
                    hunks: hunks.clone(),
                    provenance: provenance.clone(),
                    evidence: evidence.clone(),
                    created_ms: rec.ts,
                    state: ProposalState::Open,
                },
            );
        }
        ProposalBody::Resolve {
            resolution,
            hunks,
            rev,
        } => {
            let p = index
                .get_mut(&rec.prop)
                .ok_or_else(|| format!("resolve for unknown proposal {}", rec.prop))?;
            if !p.state.is_open() {
                return Err(format!("second resolve for {}", rec.prop));
            }
            p.state = match resolution {
                Resolution::Accepted => ProposalState::Accepted {
                    hunks: hunks.clone().unwrap_or_default(),
                    rev: rev
                        .clone()
                        .ok_or_else(|| format!("accepted resolve for {} lacks rev", rec.prop))?,
                    resolved_ms: rec.ts,
                },
                Resolution::Rejected => ProposalState::Rejected {
                    resolved_ms: rec.ts,
                },
                Resolution::Withdrawn => ProposalState::Withdrawn {
                    resolved_ms: rec.ts,
                },
            };
        }
    }
    Ok(())
}

/// Number of newline-inclusive lines in `content` — the coordinate space
/// hunks address.
pub fn line_count(content: &[u8]) -> usize {
    content.split_inclusive(|&b| b == b'\n').count()
}

/// Validate a hunk list against a base of `lines` lines: at least one hunk,
/// each in range, ordered, non-overlapping. Two pure inserts at the same
/// line are legal and apply in index order.
pub fn validate_hunks(hunks: &[Hunk], lines: usize) -> Result<(), VaultError> {
    let fail = |reason: String| Err(VaultError::ValidationFailed { reason });
    if hunks.is_empty() {
        return fail("a proposal needs at least one hunk".into());
    }
    let mut prev_end = 0u64;
    let mut first = true;
    for (i, h) in hunks.iter().enumerate() {
        let end = h.start.saturating_add(h.del);
        if end > lines as u64 {
            return fail(format!(
                "hunk {i} spans lines {}..{end} but the base has {lines} lines",
                h.start
            ));
        }
        if !first && h.start < prev_end {
            return fail(format!("hunk {i} overlaps or reorders the previous hunk"));
        }
        if h.del == 0 && h.ins.is_empty() {
            return fail(format!("hunk {i} deletes nothing and inserts nothing"));
        }
        prev_end = end;
        first = false;
    }
    Ok(())
}

/// Apply the selected hunks (sorted, deduplicated, in-range indexes into
/// `hunks`) to `base`. Byte-honest: untouched lines pass through verbatim,
/// `ins` is spliced exactly as given.
pub fn apply_hunks(base: &[u8], hunks: &[Hunk], selected: &[usize]) -> Vec<u8> {
    let lines: Vec<&[u8]> = base.split_inclusive(|&b| b == b'\n').collect();
    let mut out = Vec::with_capacity(base.len());
    let mut cursor = 0usize;
    for &hi in selected {
        let h = &hunks[hi];
        let start = h.start as usize;
        for line in &lines[cursor..start] {
            out.extend_from_slice(line);
        }
        out.extend_from_slice(h.ins.as_bytes());
        cursor = start + h.del as usize;
    }
    for line in &lines[cursor..] {
        out.extend_from_slice(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(start: u64, del: u64, ins: &str) -> Hunk {
        Hunk {
            start,
            del,
            ins: ins.to_owned(),
        }
    }

    #[test]
    fn line_counting_is_newline_inclusive() {
        assert_eq!(line_count(b""), 0);
        assert_eq!(line_count(b"a"), 1);
        assert_eq!(line_count(b"a\n"), 1);
        assert_eq!(line_count(b"a\nb"), 2);
        assert_eq!(line_count(b"a\nb\n"), 2);
    }

    #[test]
    fn hunk_validation() {
        // Replace line 1 of 3.
        assert!(validate_hunks(&[h(1, 1, "x\n")], 3).is_ok());
        // Append at EOF.
        assert!(validate_hunks(&[h(3, 0, "x\n")], 3).is_ok());
        // Out of range.
        assert!(validate_hunks(&[h(3, 1, "x\n")], 3).is_err());
        assert!(validate_hunks(&[h(4, 0, "x\n")], 3).is_err());
        // Overlap and reorder.
        assert!(validate_hunks(&[h(0, 2, "x\n"), h(1, 1, "y\n")], 3).is_err());
        assert!(validate_hunks(&[h(2, 1, "x\n"), h(0, 1, "y\n")], 3).is_err());
        // Adjacent is fine; empty list and no-op hunks are not.
        assert!(validate_hunks(&[h(0, 1, "x\n"), h(1, 1, "y\n")], 3).is_ok());
        assert!(validate_hunks(&[], 3).is_err());
        assert!(validate_hunks(&[h(0, 0, "")], 3).is_err());
        // Two inserts at one point are legal.
        assert!(validate_hunks(&[h(1, 0, "x\n"), h(1, 0, "y\n")], 3).is_ok());
    }

    #[test]
    fn apply_replaces_deletes_inserts_appends() {
        let base = b"one\ntwo\nthree\n";
        let hunks = vec![
            h(0, 1, "ONE\n"),      // replace line 0
            h(1, 1, ""),           // delete line 1
            h(2, 0, "before\n"),   // insert before line 2
            h(3, 0, "appended\n"), // append at EOF
        ];
        let all: Vec<usize> = (0..hunks.len()).collect();
        assert_eq!(
            apply_hunks(base, &hunks, &all),
            b"ONE\nbefore\nthree\nappended\n"
        );
        // Subset selection: only the replace and the append.
        assert_eq!(
            apply_hunks(base, &hunks, &[0, 3]),
            b"ONE\ntwo\nthree\nappended\n"
        );
    }

    #[test]
    fn apply_is_byte_honest_without_trailing_newline() {
        let base = b"one\ntwo";
        // Appending after a final line that lacks its newline: the insert is
        // spliced verbatim — no newline is invented.
        assert_eq!(
            apply_hunks(base, &[h(2, 0, "three")], &[0]),
            b"one\ntwothree"
        );
        // New-document proposal: empty base, one insert.
        assert_eq!(apply_hunks(b"", &[h(0, 0, "fresh\n")], &[0]), b"fresh\n");
    }
}
