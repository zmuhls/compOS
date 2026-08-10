//! The vault: on-disk layout, format file, single-writer lock, and open
//! paths. Constitutional rule 1 is enforced here by construction — the only
//! way to mutate a vault is a `VaultWriter` obtained from a write-mode
//! `Vault`, and write mode requires the exclusive vault lock.

use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::VAULT_FORMAT;
use crate::error::VaultError;
use crate::fsutil;
use crate::ids::DocId;
use crate::journal::{self, DocIndex, Journal, JournalRecord};
use crate::lease::{Lease, LeaseId, LeaseTable};
use crate::objects::ObjectStore;
use crate::reconcile;
use crate::writer::VaultWriter;

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
    warnings: Vec<String>,
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
        let journal_dir = root.join("journal");
        let journal = Journal::open_write(&journal_dir)?;
        let records = journal::replay(&journal_dir, false)?;
        let index = DocIndex::build(&records)?;
        let objects = ObjectStore::new(root);
        let warnings = reconcile::reconcile(root, &index, &objects)?;
        Ok(Vault {
            root: root.to_path_buf(),
            format,
            mode: OpenMode::Write,
            _lock: lock,
            journal: Some(journal),
            index,
            objects,
            leases: LeaseTable::default(),
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

    pub fn release_lease(&mut self, doc: &DocId, id: &LeaseId) {
        self.leases.release(doc, id);
    }

    pub fn index(&self) -> &DocIndex {
        &self.index
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

    pub(crate) fn vault_dir(&self) -> PathBuf {
        self.root.join("vault")
    }

    pub(crate) fn intents_dir(&self) -> PathBuf {
        self.root.join("intents")
    }
}
