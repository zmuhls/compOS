//! External-edit conversion tests: out-of-band changes become `external`
//! revisions and silent overwrite is impossible — both through the explicit
//! scan and through the save transaction's never-clobber guard.

use std::fs;

use compos_core::{DocRef, ObjectHash, RevisionOrigin, SaveRequest, Vault, VaultError};

fn save(vault: &mut Vault, path: &str, content: &str) -> compos_core::SaveOutcome {
    let base = vault
        .index()
        .doc_by_path(path)
        .and_then(|d| vault.index().head(d))
        .map(|h| h.rev.clone());
    vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            doc: DocRef::Path(path.to_owned()),
            base,
            content: content.as_bytes().to_vec(),
            origin: RevisionOrigin::Editor,
            lease: None,
        })
        .unwrap()
}

#[test]
fn out_of_band_edit_becomes_external_revision() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    let first = save(&mut vault, "notes/a.md", "original\n");

    fs::write(root.join("vault/notes/a.md"), b"edited outside\n").unwrap();

    let scan = vault.scan_external().unwrap();
    assert_eq!(scan.converted.len(), 1);
    assert!(scan.missing.is_empty());
    let converted = &scan.converted[0];
    assert_eq!(converted.path, "notes/a.md");
    assert_eq!(converted.doc, first.doc, "same document, new revision");
    assert_eq!(converted.object, ObjectHash::of(b"edited outside\n"));

    let history = vault.history(&first.doc).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].origin, RevisionOrigin::External);
    assert_eq!(history[1].parent.as_ref(), Some(&first.rev));

    // The bytes on disk are untouched by the conversion.
    assert_eq!(
        fs::read(root.join("vault/notes/a.md")).unwrap(),
        b"edited outside\n"
    );
}

#[test]
fn unknown_file_becomes_new_external_document() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();

    fs::create_dir_all(root.join("vault/drop")).unwrap();
    fs::write(root.join("vault/drop/new.md"), b"dropped in\n").unwrap();

    let scan = vault.scan_external().unwrap();
    assert_eq!(scan.converted.len(), 1);
    let doc = vault.index().doc_by_path("drop/new.md").unwrap().clone();
    let history = vault.history(&doc).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].origin, RevisionOrigin::External);
    assert!(history[0].parent.is_none());
}

#[test]
fn scan_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    save(&mut vault, "a.md", "one\n");
    fs::write(root.join("vault/a.md"), b"two\n").unwrap();

    assert_eq!(vault.scan_external().unwrap().converted.len(), 1);
    assert_eq!(vault.scan_external().unwrap().converted.len(), 0);
}

#[test]
fn missing_file_is_reported_not_resurrected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    save(&mut vault, "gone.md", "here today\n");
    fs::remove_file(root.join("vault/gone.md")).unwrap();

    let scan = vault.scan_external().unwrap();
    assert!(scan.converted.is_empty());
    assert_eq!(scan.missing, vec!["gone.md".to_owned()]);
    assert!(!root.join("vault/gone.md").exists(), "must not resurrect");
}

#[test]
fn hidden_files_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    fs::write(root.join("vault/.DS_Store"), b"finder junk").unwrap();
    fs::create_dir_all(root.join("vault/.obsidian")).unwrap();
    fs::write(root.join("vault/.obsidian/config"), b"{}").unwrap();
    fs::write(root.join("vault/.compos-tmp-xyz"), b"stray").unwrap();

    let scan = vault.scan_external().unwrap();
    assert!(scan.converted.is_empty());
    assert!(vault.index().is_empty());
}

#[test]
fn save_guard_converts_before_stale_base() {
    // The heart of never-clobber: a stale editor save cannot destroy an
    // out-of-band edit — the edit is journaled first, then StaleBase forces
    // the editor to rebase.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    let first = save(&mut vault, "a.md", "committed\n");

    fs::write(root.join("vault/a.md"), b"external edit\n").unwrap();

    let err = vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            doc: DocRef::Path("a.md".to_owned()),
            base: Some(first.rev.clone()),
            content: b"editor save\n".to_vec(),
            origin: RevisionOrigin::Editor,
            lease: None,
        })
        .unwrap_err();
    assert!(matches!(err, VaultError::StaleBase { .. }));

    // The external bytes were captured as a revision, not clobbered.
    let history = vault.history(&first.doc).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].origin, RevisionOrigin::External);
    assert_eq!(history[1].object, ObjectHash::of(b"external edit\n"));
    assert_eq!(
        fs::read(root.join("vault/a.md")).unwrap(),
        b"external edit\n"
    );

    // Rebasing on the external head succeeds.
    let head = vault.index().head(&first.doc).unwrap().rev.clone();
    let out = vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            doc: DocRef::Path("a.md".to_owned()),
            base: Some(head),
            content: b"editor save\n".to_vec(),
            origin: RevisionOrigin::Editor,
            lease: None,
        })
        .unwrap();
    assert_eq!(vault.history(&first.doc).unwrap().len(), 3);
    assert_eq!(out.object, ObjectHash::of(b"editor save\n"));
}

#[test]
fn save_guard_captures_foreign_file_at_new_path() {
    // Saving to a path that already holds an unknown file first records
    // that file as an external document, then rejects the save as stale.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    fs::write(root.join("vault/found.md"), b"was here first\n").unwrap();

    let err = vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            doc: DocRef::Path("found.md".to_owned()),
            base: None,
            content: b"newcomer\n".to_vec(),
            origin: RevisionOrigin::Editor,
            lease: None,
        })
        .unwrap_err();
    assert!(matches!(err, VaultError::StaleBase { .. }));

    let doc = vault.index().doc_by_path("found.md").unwrap().clone();
    let history = vault.history(&doc).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].origin, RevisionOrigin::External);
    assert_eq!(history[0].object, ObjectHash::of(b"was here first\n"));
}

#[test]
fn identical_disk_bytes_do_not_trigger_guard() {
    // Re-saving identical content with the right base is an ordinary save;
    // a disk file matching the incoming content is not "external".
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    let first = save(&mut vault, "a.md", "same\n");
    let out = vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            doc: DocRef::Path("a.md".to_owned()),
            base: Some(first.rev),
            content: b"same\n".to_vec(),
            origin: RevisionOrigin::Editor,
            lease: None,
        })
        .unwrap();
    let history = vault.history(&out.doc).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|r| r.origin == RevisionOrigin::Editor));
}
