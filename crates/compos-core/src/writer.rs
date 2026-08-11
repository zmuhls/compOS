//! The save transaction (ARCHITECTURE.md §5.3): the only code path that
//! mutates canonical state. Acknowledgment happens strictly after the
//! journal fsync (step 5), so no acknowledged write is ever rolled back by
//! reconciliation.

use std::fs;
use std::path::Component;

use serde::{Deserialize, Serialize};

use crate::error::VaultError;
use crate::fsutil;
use crate::ids::{DocId, ObjectHash, RevisionId};
use crate::journal::{JOURNAL_RECORD_VERSION, JournalRecord, RevisionOrigin};
use crate::lease::LeaseId;
use crate::vault::Vault;

#[derive(Debug, Clone)]
pub enum DocRef {
    /// Address a document by vault-relative path; creates the document if
    /// the path is unknown.
    Path(String),
    /// Address an existing document by id.
    Id(DocId),
}

#[derive(Debug)]
pub struct SaveRequest {
    pub doc: DocRef,
    /// The revision this save is based on; must equal the document's head
    /// (`None` for a new document). Mismatch → `StaleBase`.
    pub base: Option<RevisionId>,
    pub content: Vec<u8>,
    pub origin: RevisionOrigin,
    pub lease: Option<LeaseId>,
}

#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub doc: DocId,
    pub rev: RevisionId,
    pub object: ObjectHash,
    pub path: String,
}

/// The write intent registered in step 3, durable before the visible file is
/// touched. Reconciliation uses it to classify crash windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WriteIntent {
    pub v: u32,
    pub ts: u64,
    pub doc: DocId,
    pub rev: RevisionId,
    pub parent: Option<RevisionId>,
    pub object: ObjectHash,
    pub path: String,
}

pub struct VaultWriter<'v> {
    vault: &'v mut Vault,
}

impl<'v> VaultWriter<'v> {
    pub(crate) fn new(vault: &'v mut Vault) -> Self {
        Self { vault }
    }

    pub fn save(&mut self, req: SaveRequest) -> Result<SaveOutcome, VaultError> {
        // Resolve document, path, and current head.
        let (doc_id, path) = match &req.doc {
            DocRef::Path(p) => {
                validate_vault_path(p)?;
                match self.vault.index.doc_by_path(p) {
                    Some(id) => (id.clone(), p.clone()),
                    None => (DocId::generate(), p.clone()),
                }
            }
            DocRef::Id(id) => {
                let head = self
                    .vault
                    .index
                    .head(id)
                    .ok_or_else(|| VaultError::DocNotFound(id.to_string()))?;
                (id.clone(), head.path.clone())
            }
        };
        // Never-clobber guard (constitutional rule; §5.3): if the visible
        // file holds bytes matching neither the recorded head nor the
        // incoming content, it was edited out-of-band. Those bytes become an
        // `external` revision *first*; the caller's save then fails
        // `StaleBase` below and must rebase onto the external revision. The
        // conversion deliberately skips the lease check — the bytes already
        // exist, and recording them cannot wait for a lease holder.
        let visible = self.vault.vault_dir().join(&path);
        if let Some(disk) = read_optional(&visible)? {
            let disk_hash = ObjectHash::of(&disk);
            let head_object = self.vault.index.head(&doc_id).map(|h| h.object.clone());
            if Some(&disk_hash) != head_object.as_ref() && disk_hash != ObjectHash::of(&req.content)
            {
                let parent = self.vault.index.head(&doc_id).map(|h| h.rev.clone());
                self.commit(&doc_id, &path, &disk, RevisionOrigin::External, parent)?;
            }
        }

        let expected = self.vault.index.head(&doc_id).map(|h| h.rev.clone());

        // Step 1: base revision and lease.
        if req.base != expected {
            return Err(VaultError::StaleBase {
                expected,
                got: req.base,
            });
        }
        self.vault
            .leases
            .check(&doc_id, req.lease.as_ref(), fsutil::now_ms())?;

        self.commit(&doc_id, &path, &req.content, req.origin, expected)
    }

    /// Steps 2–6 of the save transaction, shared by ordinary saves and
    /// external-revision conversion.
    fn commit(
        &mut self,
        doc_id: &DocId,
        path: &str,
        content: &[u8],
        origin: RevisionOrigin,
        parent: Option<RevisionId>,
    ) -> Result<SaveOutcome, VaultError> {
        // Step 2: immutable content object, flushed and fsynced.
        let object = self.vault.objects.put(content)?;

        // Step 3: durable write intent, before the visible file changes.
        let rev = RevisionId::generate();
        let intent = WriteIntent {
            v: JOURNAL_RECORD_VERSION,
            ts: fsutil::now_ms(),
            doc: doc_id.clone(),
            rev: rev.clone(),
            parent: parent.clone(),
            object: object.clone(),
            path: path.to_owned(),
        };
        let intent_path = self.vault.intents_dir().join(format!("{rev}.json"));
        fsutil::write_atomic(&intent_path, &serde_json::to_vec(&intent)?)?;

        // Step 4: atomic replace of the visible file.
        let visible = self.vault.vault_dir().join(path);
        if let Some(parent_dir) = visible.parent() {
            fs::create_dir_all(parent_dir)?;
        }
        fsutil::write_atomic(&visible, content)?;

        // Step 5: journal append + fsync — the acknowledgment point.
        let record = JournalRecord {
            v: JOURNAL_RECORD_VERSION,
            ts: intent.ts,
            doc: doc_id.clone(),
            rev: rev.clone(),
            parent,
            object: object.clone(),
            path: path.to_owned(),
            origin,
        };
        self.vault
            .journal
            .as_mut()
            .expect("write-mode vault always has a journal")
            .append(&record)?;

        // Step 6: clear the intent (resurrection is harmless — W6), advance
        // the in-memory head, refresh the derived index. The save is already
        // acknowledged (step 5), so a derived failure degrades to a warning
        // and the index rebuilds on the next open — it never fails the save.
        let _ = fs::remove_file(&intent_path);
        self.vault.index.apply(&record);
        if let Some(d) = self.vault.derived.as_mut()
            && let Err(e) = d.apply_one(&record, content)
        {
            self.vault.warnings.push(format!(
                "derived index update failed ({e}); disabled until next open"
            ));
            self.vault.derived = None;
        }

        Ok(SaveOutcome {
            doc: doc_id.clone(),
            rev,
            object,
            path: path.to_owned(),
        })
    }
}

fn read_optional(path: &std::path::Path) -> Result<Option<Vec<u8>>, VaultError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(VaultError::Io(e)),
    }
}

/// Vault paths are relative, normal-component-only, and never collide with
/// the atomic-write temp namespace.
fn validate_vault_path(path: &str) -> Result<(), VaultError> {
    let reject = |reason: &str| {
        Err(VaultError::InvalidPath {
            path: path.to_owned(),
            reason: reason.to_owned(),
        })
    };
    if path.is_empty() {
        return reject("empty path");
    }
    if path.contains('\\') {
        return reject("backslash not allowed");
    }
    let p = std::path::Path::new(path);
    if p.components().count() == 0 {
        return reject("empty path");
    }
    for comp in p.components() {
        match comp {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if name.starts_with(".compos-tmp-") {
                    return reject("reserved temp-file prefix");
                }
            }
            Component::CurDir => return reject("'.' component not allowed"),
            Component::ParentDir => return reject("'..' component not allowed"),
            Component::RootDir | Component::Prefix(_) => {
                return reject("path must be relative");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_vault_path;

    #[test]
    fn path_validation() {
        assert!(validate_vault_path("a.md").is_ok());
        assert!(validate_vault_path("notes/deep/a.md").is_ok());
        assert!(validate_vault_path("").is_err());
        assert!(validate_vault_path("/etc/passwd").is_err());
        assert!(validate_vault_path("../escape.md").is_err());
        assert!(validate_vault_path("notes/../escape.md").is_err());
        assert!(validate_vault_path("./a.md").is_err());
        assert!(validate_vault_path("notes/.compos-tmp-x").is_err());
        assert!(validate_vault_path("a\\b.md").is_err());
    }
}
