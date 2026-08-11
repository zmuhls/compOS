//! External-edit conversion (ARCHITECTURE.md §5.3): out-of-band changes to
//! files under `vault/` become explicit `external` revisions through the
//! ordinary save transaction — never a silent overwrite. The composd watcher
//! calls this on filesystem events; callers may also run it at startup to
//! capture edits made while no writer was alive.
//!
//! Scope rules for the scan:
//! - only regular files count; symlinks and other specials are ignored
//! - any path component starting with `.` is ignored (hidden files, editor
//!   droppings like `.DS_Store`, and the reserved `.compos-tmp-*` namespace)
//! - a head whose visible file is missing is reported, not resurrected and
//!   not deleted — there are no delete semantics in the Phase-1 model
//!
//! Races: bytes are read, then committed via `VaultWriter::save`, which
//! re-replaces the file with the same bytes. An editor writing in that gap
//! simply triggers the next scan; the bytes at rest are always captured.

use std::fs;
use std::path::Path;

use crate::error::VaultError;
use crate::ids::ObjectHash;
use crate::journal::RevisionOrigin;
use crate::vault::Vault;
use crate::writer::{DocRef, SaveOutcome, SaveRequest};

/// What one scan found and did.
#[derive(Debug, Default)]
pub struct ExternalScan {
    /// External revisions committed (changed or newly discovered files).
    pub converted: Vec<SaveOutcome>,
    /// Paths of documents whose visible file has gone missing.
    pub missing: Vec<String>,
}

impl Vault {
    /// Sweep `vault/` for out-of-band changes and commit each as an
    /// `external` revision. Requires a write-mode vault.
    pub fn scan_external(&mut self) -> Result<ExternalScan, VaultError> {
        let mut scan = ExternalScan::default();
        let vault_dir = self.vault_dir();

        // Collect first (immutable borrow), then commit (mutable borrow).
        let mut pending: Vec<(String, Vec<u8>)> = Vec::new();
        let mut on_disk: Vec<String> = Vec::new();
        walk(&vault_dir, &vault_dir, &mut |rel, file| {
            let bytes = fs::read(file)?;
            on_disk.push(rel.to_owned());
            let changed = match self.index().doc_by_path(rel) {
                Some(doc) => {
                    let head = self.index().head(doc).expect("indexed doc has a head");
                    ObjectHash::of(&bytes) != head.object
                }
                None => true,
            };
            if changed {
                pending.push((rel.to_owned(), bytes));
            }
            Ok(())
        })?;

        for (path, bytes) in pending {
            let base = self
                .index()
                .doc_by_path(&path)
                .and_then(|d| self.index().head(d))
                .map(|h| h.rev.clone());
            let outcome = self.writer()?.save(SaveRequest {
                doc: DocRef::Path(path),
                base,
                content: bytes,
                origin: RevisionOrigin::External,
                lease: None,
            })?;
            scan.converted.push(outcome);
        }

        for (_, head) in self.index().iter_heads() {
            if !on_disk.iter().any(|p| p == &head.path) {
                scan.missing.push(head.path.clone());
            }
        }
        scan.missing.sort();
        Ok(scan)
    }
}

fn walk(
    root: &Path,
    dir: &Path,
    visit: &mut impl FnMut(&str, &Path) -> Result<(), VaultError>,
) -> Result<(), VaultError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)?.filter_map(Result::ok) {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk(root, &path, visit)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walk stays under root")
                .to_string_lossy()
                .into_owned();
            visit(&rel, &path)?;
        }
    }
    Ok(())
}
