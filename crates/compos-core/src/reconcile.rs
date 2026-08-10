//! Startup reconciliation (ARCHITECTURE.md §5.3): classify and repair the
//! leftovers of a crashed save transaction. The journal is truth — a
//! revision in the journal rolls forward, a revision not in the journal
//! rolls back, and because acknowledgment happens strictly after the journal
//! fsync, no acknowledged write is ever rolled back.
//!
//! Crash windows (see reconcile_windows.rs for the deterministic tests):
//!   W1 tmp strays            → delete
//!   W2 orphan objects        → keep (content-addressed, GC later)
//!   W3 intent, no record, visible == parent   → roll back: delete intent
//!   W4 intent, no record, visible == intended → roll back: restore parent
//!   W5 intent + record       → roll forward: verify visible, delete intent
//!   W6 resurrected intent    → same as W5 (idempotent)
//!   W7 torn journal tail     → truncated by Journal::open_write before this
//!   edge: visible matches neither → external edit; warn, never clobber

use std::fs;
use std::path::Path;

use crate::error::VaultError;
use crate::fsutil;
use crate::ids::ObjectHash;
use crate::journal::DocIndex;
use crate::objects::ObjectStore;
use crate::writer::WriteIntent;

pub(crate) fn reconcile(
    root: &Path,
    index: &DocIndex,
    objects: &ObjectStore,
) -> Result<Vec<String>, VaultError> {
    let mut warnings = Vec::new();

    // W1: object-staging strays.
    let tmp = root.join("tmp");
    if tmp.is_dir() {
        for entry in fs::read_dir(&tmp)?.filter_map(Result::ok) {
            let p = entry.path();
            if p.is_dir() {
                fs::remove_dir_all(&p)?;
            } else {
                fs::remove_file(&p)?;
            }
        }
    }

    // Visible-replace strays from a step-4 crash.
    remove_tmp_strays(&root.join("vault"))?;

    // Intents, oldest first so repeated-crash leftovers replay in order.
    // Removal always uses the actual directory entry path: a crash between
    // the intent's temp write and its rename leaves a fully-parseable
    // `.compos-tmp-*` file whose renamed name never came to exist.
    let intents_dir = root.join("intents");
    let mut intents: Vec<(std::path::PathBuf, WriteIntent)> = Vec::new();
    if intents_dir.is_dir() {
        for entry in fs::read_dir(&intents_dir)?.filter_map(Result::ok) {
            let file = entry.path();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".compos-tmp-")
            {
                // The intent was never registered (step 3 did not complete),
                // so the save never touched the visible file: pure roll-back.
                fs::remove_file(&file)?;
                continue;
            }
            let bytes = fs::read(&file)?;
            match serde_json::from_slice::<WriteIntent>(&bytes) {
                Ok(intent) => intents.push((file, intent)),
                Err(_) => {
                    warnings.push(format!(
                        "unreadable write intent {:?}; removing",
                        entry.file_name()
                    ));
                    fs::remove_file(&file)?;
                }
            }
        }
    }
    intents.sort_by_key(|(_, i)| i.ts);

    for (intent_path, intent) in intents {
        let visible = root.join("vault").join(&intent.path);

        if index.contains_rev(&intent.rev) {
            // W5 / W6: committed. If the revision is still the head, make
            // sure the visible file reflects it (a step-4/5 ordering
            // guarantee, unless the user edited the file after the crash).
            if let Some(head) = index.head(&intent.doc)
                && head.rev == intent.rev
            {
                match visible_hash(&visible)? {
                    Some(h) if h == head.object => {}
                    None => {
                        let bytes = objects.read(&head.object)?;
                        restore_visible(&visible, &bytes)?;
                    }
                    Some(_) => warnings.push(format!(
                        "'{}' differs from its committed revision; leaving file as-is (external edit?)",
                        intent.path
                    )),
                }
            }
            fs::remove_file(&intent_path)?;
            continue;
        }

        // Not in the journal: the save was never acknowledged. Roll back.
        let parent_object: Option<&ObjectHash> = index
            .head(&intent.doc)
            .and_then(|head| (Some(&head.rev) == intent.parent.as_ref()).then_some(&head.object));

        match visible_hash(&visible)? {
            // W3: visible still matches the parent (or doc never existed).
            None if intent.parent.is_none() => {
                fs::remove_file(&intent_path)?;
            }
            Some(ref h) if Some(h) == parent_object => {
                fs::remove_file(&intent_path)?;
            }
            // W4: visible was already replaced with the never-committed
            // content. Restore the parent revision from its object.
            Some(ref h) if *h == intent.object => {
                match parent_object {
                    Some(po) => {
                        let bytes = objects.read(po)?;
                        restore_visible(&visible, &bytes)?;
                    }
                    None => {
                        // First save of a new document: rolling back means
                        // the document never existed.
                        fs::remove_file(&visible)?;
                        if let Some(dir) = visible.parent() {
                            fsutil::fsync_dir(dir)?;
                        }
                    }
                }
                fs::remove_file(&intent_path)?;
            }
            // Edge: matches neither side. Never clobber user bytes.
            other => {
                warnings.push(format!(
                    "crashed save of '{}' found the file in an unexpected state ({}); leaving file as-is",
                    intent.path,
                    if other.is_some() { "external edit?" } else { "missing" },
                ));
                fs::remove_file(&intent_path)?;
            }
        }
    }

    Ok(warnings)
}

fn visible_hash(path: &Path) -> Result<Option<ObjectHash>, VaultError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(ObjectHash::of(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(VaultError::Io(e)),
    }
}

fn restore_visible(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fsutil::write_atomic(path, bytes)?;
    Ok(())
}

fn remove_tmp_strays(dir: &Path) -> Result<(), VaultError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)?.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            remove_tmp_strays(&p)?;
        } else if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".compos-tmp-")
        {
            fs::remove_file(&p)?;
        }
    }
    Ok(())
}
