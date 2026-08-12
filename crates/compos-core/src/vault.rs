//! The vault: on-disk layout, format file, single-writer lock, and open
//! paths. Constitutional rule 1 is enforced here by construction — the only
//! way to mutate a vault is a `VaultWriter` obtained from a write-mode
//! `Vault`, and write mode requires the exclusive vault lock.

use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::VAULT_FORMAT;
use crate::derived::{DerivedIndex, SearchHit};
use crate::error::VaultError;
use crate::fsutil;
use crate::ids::{DocId, ProposalId};
use crate::journal::{self, DocIndex, Journal, JournalRecord, RevisionOrigin};
use crate::lease::{Lease, LeaseId, LeaseTable};
use crate::objects::ObjectStore;
use crate::proposal::{
    self, AcceptOutcome, CreateProposal, PROPOSAL_RECORD_VERSION, Proposal, ProposalBody,
    ProposalRecord, ProposalStore, Resolution,
};
use crate::reconcile;
use crate::writer::{DocRef, SaveRequest, VaultWriter, validate_vault_path};

/// `compos.json` — the vault format file. Its version governs the on-disk
/// canonical layout and is distinct from the SQLite schema version
/// (ARCHITECTURE.md §5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultFormatFile {
    pub vault_format: u32,
    pub vault_id: String,
    pub created_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    Read,
    Write,
}

#[derive(Debug)]
pub struct Vault {
    root: PathBuf,
    format: VaultFormatFile,
    mode: OpenMode,
    _lock: File,
    pub(crate) journal: Option<Journal>,
    pub(crate) index: DocIndex,
    pub(crate) objects: ObjectStore,
    pub(crate) leases: LeaseTable,
    pub(crate) derived: Option<DerivedIndex>,
    pub(crate) proposals: ProposalStore,
    pub(crate) warnings: Vec<String>,
}

impl Vault {
    /// Create a new vault at `root` and open it for writing. Idempotent
    /// against a crash mid-init: re-running completes the layout, but a
    /// completed vault refuses re-init.
    pub fn init(root: &Path) -> Result<Vault, VaultError> {
        if root.join("compos.json").exists() {
            return Err(VaultError::AlreadyAVault(root.to_path_buf()));
        }
        for sub in [
            "vault",
            "objects/sha256",
            "journal",
            "intents",
            "tmp",
            "state",
        ] {
            fs::create_dir_all(root.join(sub))?;
        }
        let format = VaultFormatFile {
            vault_format: VAULT_FORMAT,
            vault_id: uuid::Uuid::now_v7().simple().to_string(),
            created_ms: fsutil::now_ms(),
        };
        fsutil::write_atomic(
            &root.join("compos.json"),
            &serde_json::to_vec_pretty(&format)?,
        )?;
        Self::open_write(root)
    }

    /// Open with the exclusive vault lock: repair the journal tail, replay,
    /// validate chains, reconcile crash leftovers.
    pub fn open_write(root: &Path) -> Result<Vault, VaultError> {
        let lock = Self::take_lock(root, true)?;
        let format = Self::read_format(root)?;
        // A vault restored from a backup or checkout may lack the empty
        // working directories (nothing canonical lives in them); recreate
        // the layout idempotently before anything touches it.
        for sub in ["vault", "objects/sha256", "intents", "tmp", "state"] {
            fs::create_dir_all(root.join(sub))?;
        }
        let journal_dir = root.join("journal");
        let journal = Journal::open_write(&journal_dir)?;
        let records = journal::replay(&journal_dir, false)?;
        let index = DocIndex::build(&records)?;
        let objects = ObjectStore::new(root);
        let proposals = ProposalStore::open_write(&journal_dir.join("proposals"))?;
        let mut warnings = reconcile::reconcile(root, &index, &objects)?;

        // Tier-2 derived index: rebuildable, so failure here degrades to a
        // warning — it never blocks canonical access (rule 4).
        let derived = match DerivedIndex::open(&root.join("state")) {
            Ok(mut d) => match d.sync(&format.vault_id, &records, &objects) {
                Ok(_) => Some(d),
                Err(e) => {
                    warnings.push(format!("derived index unavailable: {e}"));
                    None
                }
            },
            Err(e) => {
                warnings.push(format!("derived index unavailable: {e}"));
                None
            }
        };

        Ok(Vault {
            root: root.to_path_buf(),
            format,
            mode: OpenMode::Write,
            _lock: lock,
            journal: Some(journal),
            index,
            objects,
            leases: LeaseTable::default(),
            derived,
            proposals,
            warnings,
        })
    }

    /// Open with a shared lock for reading. No repair, no reconciliation:
    /// replay tolerates a torn tail instead.
    pub fn open_read(root: &Path) -> Result<Vault, VaultError> {
        let lock = Self::take_lock(root, false)?;
        let format = Self::read_format(root)?;
        let records = journal::replay(&root.join("journal"), true)?;
        let index = DocIndex::build(&records)?;
        Ok(Vault {
            root: root.to_path_buf(),
            format,
            mode: OpenMode::Read,
            _lock: lock,
            journal: None,
            index,
            objects: ObjectStore::new(root),
            leases: LeaseTable::default(),
            // Read-only attach; possibly stale if no writer has synced it.
            derived: DerivedIndex::open_read_only(&root.join("state")),
            proposals: ProposalStore::open_read(&root.join("journal").join("proposals"))?,
            warnings: Vec::new(),
        })
    }

    fn take_lock(root: &Path, exclusive: bool) -> Result<File, VaultError> {
        if !root.is_dir() {
            return Err(VaultError::NotAVault(root.to_path_buf()));
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("lock"))?;
        let op = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        } | libc::LOCK_NB;
        let rc = unsafe { libc::flock(lock.as_raw_fd(), op) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Err(VaultError::VaultBusy)
            } else {
                Err(VaultError::Io(err))
            };
        }
        Ok(lock)
    }

    fn read_format(root: &Path) -> Result<VaultFormatFile, VaultError> {
        let path = root.join("compos.json");
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(VaultError::NotAVault(root.to_path_buf()));
            }
            Err(e) => return Err(VaultError::Io(e)),
        };
        let format: VaultFormatFile = serde_json::from_slice(&bytes)?;
        if format.vault_format > VAULT_FORMAT {
            return Err(VaultError::FormatUnsupported {
                found: format.vault_format,
                supported: VAULT_FORMAT,
            });
        }
        Ok(format)
    }

    pub fn writer(&mut self) -> Result<VaultWriter<'_>, VaultError> {
        if self.mode != OpenMode::Write {
            return Err(VaultError::ReadOnly);
        }
        Ok(VaultWriter::new(self))
    }

    pub fn acquire_lease(&mut self, doc: DocId) -> Result<Lease, VaultError> {
        if self.mode != OpenMode::Write {
            return Err(VaultError::ReadOnly);
        }
        self.leases.acquire(doc, fsutil::now_ms())
    }

    /// Slide a held lease's TTL forward (ratified lease rule 2).
    pub fn renew_lease(&mut self, doc: &DocId, id: &LeaseId) -> Result<(), VaultError> {
        if self.mode != OpenMode::Write {
            return Err(VaultError::ReadOnly);
        }
        self.leases.renew(doc, id, fsutil::now_ms())
    }

    pub fn release_lease(&mut self, doc: &DocId, id: &LeaseId) {
        self.leases.release(doc, id);
    }

    pub fn index(&self) -> &DocIndex {
        &self.index
    }

    /// The tier-2 derived index, when available.
    pub fn derived(&self) -> Option<&DerivedIndex> {
        self.derived.as_ref()
    }

    /// Full-text search through the derived index.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, VaultError> {
        match &self.derived {
            Some(d) => d.search(query, limit),
            None => Err(VaultError::Derived(
                "index unavailable (open the vault for writing to build it)".into(),
            )),
        }
    }

    pub fn objects(&self) -> &ObjectStore {
        &self.objects
    }

    pub fn format(&self) -> &VaultFormatFile {
        &self.format
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Warnings produced by startup reconciliation (e.g. an external edit
    /// found where a crashed save left an intent).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Full revision history for one document, from a fresh tolerant replay.
    pub fn history(&self, doc: &DocId) -> Result<Vec<JournalRecord>, VaultError> {
        let records = journal::replay(&self.root.join("journal"), true)?;
        Ok(records.into_iter().filter(|r| &r.doc == doc).collect())
    }

    /// Open a proposal against the current head of `path` (or against
    /// nothing, for a new document). Hunks are validated against the base
    /// content here, once — accept only re-checks staleness.
    pub fn create_proposal(&mut self, req: CreateProposal) -> Result<Proposal, VaultError> {
        if self.mode != OpenMode::Write {
            return Err(VaultError::ReadOnly);
        }
        validate_vault_path(&req.path)?;
        let (doc, base, base_object) = match self.index.doc_by_path(&req.path) {
            Some(doc) => {
                let head = self.index.head(doc).expect("indexed doc has a head");
                (
                    Some(doc.clone()),
                    Some(head.rev.clone()),
                    Some(head.object.clone()),
                )
            }
            None => (None, None, None),
        };
        let base_bytes = match &base_object {
            Some(obj) => self.objects.read(obj)?,
            None => Vec::new(),
        };
        proposal::validate_hunks(&req.hunks, proposal::line_count(&base_bytes))?;
        let record = ProposalRecord {
            v: PROPOSAL_RECORD_VERSION,
            ts: fsutil::now_ms(),
            prop: ProposalId::generate(),
            body: ProposalBody::Create {
                doc,
                path: req.path,
                base,
                base_object,
                hunks: req.hunks,
                provenance: req.provenance,
                evidence: req.evidence,
            },
        };
        self.proposals.append(&record)?;
        Ok(self
            .proposals
            .get(&record.prop)
            .expect("just appended")
            .clone())
    }

    pub fn proposal(&self, id: &ProposalId) -> Result<&Proposal, VaultError> {
        self.proposals
            .get(id)
            .ok_or_else(|| VaultError::ProposalNotFound(id.to_string()))
    }

    /// Every proposal, in creation order (UUIDv7 ids sort by time).
    pub fn proposals(&self) -> impl Iterator<Item = &Proposal> {
        self.proposals.iter()
    }

    /// Derived staleness (§12 Review, donor pattern): an open proposal whose
    /// base no longer equals the document head cannot be accepted. Never
    /// stored — always recomputed against the live index.
    pub fn proposal_is_stale(&self, p: &Proposal) -> bool {
        if !p.state.is_open() {
            return false;
        }
        let head_rev = self
            .index
            .doc_by_path(&p.path)
            .and_then(|d| self.index.head(d))
            .map(|h| &h.rev);
        head_rev != p.base.as_ref()
    }

    /// Accept a proposal's hunks (`None` = all of them): re-check staleness
    /// at accept time, splice the selected hunks into the base content, and
    /// commit the result through the ordinary save transaction as a
    /// `proposal-accept` revision. Canonical state moves first; the resolve
    /// record follows — a crash in between leaves the proposal open against
    /// a moved head (derived-stale, rejectable), never an acceptance
    /// claiming a commit that didn't happen.
    pub fn accept_proposal(
        &mut self,
        id: &ProposalId,
        selected: Option<Vec<usize>>,
        lease: Option<LeaseId>,
    ) -> Result<AcceptOutcome, VaultError> {
        if self.mode != OpenMode::Write {
            return Err(VaultError::ReadOnly);
        }
        let p = self.proposal(id)?.clone();
        if !p.state.is_open() {
            return Err(VaultError::ValidationFailed {
                reason: format!("proposal {id} is already {}", p.state.name()),
            });
        }
        // The accept-time stale recheck (§7): staleness at proposal time is
        // caught at create; this catches every commit since.
        let head_rev = self
            .index
            .doc_by_path(&p.path)
            .and_then(|d| self.index.head(d))
            .map(|h| h.rev.clone());
        if head_rev != p.base {
            return Err(VaultError::StaleBase {
                expected: head_rev,
                got: p.base.clone(),
            });
        }
        let mut sel = selected.unwrap_or_else(|| (0..p.hunks.len()).collect());
        sel.sort_unstable();
        sel.dedup();
        if sel.is_empty() {
            return Err(VaultError::ValidationFailed {
                reason: "accepting zero hunks is a no-op; reject instead".into(),
            });
        }
        if let Some(&max) = sel.last()
            && max >= p.hunks.len()
        {
            return Err(VaultError::ValidationFailed {
                reason: format!(
                    "hunk index {max} out of range (proposal has {} hunks)",
                    p.hunks.len()
                ),
            });
        }
        let base_bytes = match &p.base_object {
            Some(obj) => self.objects.read(obj)?,
            None => Vec::new(),
        };
        let merged = proposal::apply_hunks(&base_bytes, &p.hunks, &sel);
        let doc_ref = match &p.doc {
            Some(d) => DocRef::Id(d.clone()),
            None => DocRef::Path(p.path.clone()),
        };
        let save = self.writer()?.save(SaveRequest {
            doc: doc_ref,
            base: p.base.clone(),
            content: merged,
            origin: RevisionOrigin::ProposalAccept,
            lease,
        })?;
        let record = ProposalRecord {
            v: PROPOSAL_RECORD_VERSION,
            ts: fsutil::now_ms(),
            prop: id.clone(),
            body: ProposalBody::Resolve {
                resolution: Resolution::Accepted,
                hunks: Some(sel.clone()),
                rev: Some(save.rev.clone()),
            },
        };
        self.proposals.append(&record)?;
        Ok(AcceptOutcome {
            save,
            accepted_hunks: sel,
            proposal: self.proposal(id)?.clone(),
        })
    }

    /// The reviewer declines the proposal (commit-effect at the boundary).
    pub fn reject_proposal(&mut self, id: &ProposalId) -> Result<Proposal, VaultError> {
        self.resolve_simple(id, Resolution::Rejected)
    }

    /// The proposer takes it back (propose-effect at the boundary).
    pub fn withdraw_proposal(&mut self, id: &ProposalId) -> Result<Proposal, VaultError> {
        self.resolve_simple(id, Resolution::Withdrawn)
    }

    fn resolve_simple(
        &mut self,
        id: &ProposalId,
        resolution: Resolution,
    ) -> Result<Proposal, VaultError> {
        if self.mode != OpenMode::Write {
            return Err(VaultError::ReadOnly);
        }
        let p = self.proposal(id)?;
        if !p.state.is_open() {
            return Err(VaultError::ValidationFailed {
                reason: format!("proposal {id} is already {}", p.state.name()),
            });
        }
        let record = ProposalRecord {
            v: PROPOSAL_RECORD_VERSION,
            ts: fsutil::now_ms(),
            prop: id.clone(),
            body: ProposalBody::Resolve {
                resolution,
                hunks: None,
                rev: None,
            },
        };
        self.proposals.append(&record)?;
        Ok(self.proposal(id)?.clone())
    }

    /// Open proposals a commit to this document may have invalidated —
    /// matched by document id or path, so renames still hit.
    pub fn open_proposals_touching(&self, doc: &DocId, path: &str) -> Vec<&Proposal> {
        self.proposals
            .iter()
            .filter(|p| p.state.is_open())
            .filter(|p| p.doc.as_ref() == Some(doc) || p.path == path)
            .collect()
    }

    pub(crate) fn vault_dir(&self) -> PathBuf {
        self.root.join("vault")
    }

    pub(crate) fn intents_dir(&self) -> PathBuf {
        self.root.join("intents")
    }
}
