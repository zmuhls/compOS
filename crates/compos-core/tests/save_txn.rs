//! Save-transaction behavior: happy path, staleness, dedupe, locking,
//! format gating.

use compos_core::{DocRef, ObjectHash, RevisionOrigin, SaveRequest, Vault, VaultError};

fn save(
    vault: &mut Vault,
    path: &str,
    base: Option<compos_core::RevisionId>,
    content: &[u8],
) -> Result<compos_core::SaveOutcome, VaultError> {
    vault.writer()?.save(SaveRequest {
        doc: DocRef::Path(path.to_owned()),
        base,
        content: content.to_vec(),
        origin: RevisionOrigin::Editor,
        lease: None,
    })
}

#[test]
fn save_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();

    let out = save(&mut vault, "notes/a.md", None, b"hello world").unwrap();
    assert_eq!(out.path, "notes/a.md");
    assert_eq!(out.object, ObjectHash::of(b"hello world"));

    // Visible file and object store agree with the head.
    assert_eq!(
        std::fs::read(root.join("vault/notes/a.md")).unwrap(),
        b"hello world"
    );
    assert_eq!(vault.objects().read(&out.object).unwrap(), b"hello world");
    let head = vault.index().head(&out.doc).unwrap();
    assert_eq!(head.rev, out.rev);

    // Reopen from disk: replay reproduces the same head.
    drop(vault);
    let vault = Vault::open_read(&root).unwrap();
    let doc = vault.index().doc_by_path("notes/a.md").unwrap();
    assert_eq!(vault.index().head(doc).unwrap().object, out.object);
}

#[test]
fn stale_base_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::init(&dir.path().join("v")).unwrap();
    let first = save(&mut vault, "a.md", None, b"one").unwrap();

    // Same base again (None) is now stale.
    let err = save(&mut vault, "a.md", None, b"two").unwrap_err();
    assert!(matches!(err, VaultError::StaleBase { .. }), "{err}");

    // Correct base advances the chain.
    let second = save(&mut vault, "a.md", Some(first.rev.clone()), b"two").unwrap();
    let history = vault.history(&second.doc).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].parent.as_ref(), Some(&first.rev));
}

#[test]
fn identical_content_dedupes_to_one_object() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::init(&dir.path().join("v")).unwrap();
    let a = save(&mut vault, "a.md", None, b"same bytes").unwrap();
    let b = save(&mut vault, "b.md", None, b"same bytes").unwrap();
    assert_ne!(a.doc, b.doc);
    assert_eq!(a.object, b.object);
}

#[test]
fn second_writer_gets_vault_busy() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let _vault = Vault::init(&root).unwrap();
    let err = Vault::open_write(&root).unwrap_err();
    assert!(matches!(err, VaultError::VaultBusy), "{err}");
}

#[test]
fn read_only_vault_refuses_writer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    drop(Vault::init(&root).unwrap());
    let mut vault = Vault::open_read(&root).unwrap();
    assert!(matches!(vault.writer(), Err(VaultError::ReadOnly)));
}

#[test]
fn future_vault_format_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    drop(Vault::init(&root).unwrap());
    let format = std::fs::read_to_string(root.join("compos.json")).unwrap();
    std::fs::write(
        root.join("compos.json"),
        format.replace("\"vault_format\": 1", "\"vault_format\": 99"),
    )
    .unwrap();
    let err = Vault::open_write(&root).unwrap_err();
    assert!(
        matches!(err, VaultError::FormatUnsupported { found: 99, .. }),
        "{err}"
    );
}

#[test]
fn traversal_paths_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::init(&dir.path().join("v")).unwrap();
    for bad in ["../escape.md", "/abs.md", "a/../b.md", ""] {
        let err = save(&mut vault, bad, None, b"x").unwrap_err();
        assert!(matches!(err, VaultError::InvalidPath { .. }), "{bad}");
    }
}

#[test]
fn save_by_doc_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::init(&dir.path().join("v")).unwrap();
    let first = save(&mut vault, "a.md", None, b"one").unwrap();

    let out = vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            doc: DocRef::Id(first.doc.clone()),
            base: Some(first.rev),
            content: b"two".to_vec(),
            origin: RevisionOrigin::Editor,
            lease: None,
        })
        .unwrap();
    assert_eq!(out.doc, first.doc);
    assert_eq!(out.path, "a.md");

    let missing = vault.writer().unwrap().save(SaveRequest {
        doc: DocRef::Id(compos_core::DocId::generate()),
        base: None,
        content: b"x".to_vec(),
        origin: RevisionOrigin::Editor,
        lease: None,
    });
    assert!(matches!(missing, Err(VaultError::DocNotFound(_))));
}
