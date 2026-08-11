//! Rule 4 CI gate (ARCHITECTURE.md §2): the derived SQLite database is
//! deletable at any time; a rebuild from the journal must reproduce
//! identical state and identical search results.

use std::fs;

use compos_core::{DERIVED_SCHEMA_VERSION, DocRef, RevisionOrigin, SaveRequest, Vault};

fn save(vault: &mut Vault, path: &str, content: &str) {
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
        .unwrap();
}

fn populate(vault: &mut Vault) {
    save(vault, "notes/alpha.md", "# Alpha\nthe quick brown fox\n");
    save(vault, "notes/beta.md", "# Beta\njumps over the lazy dog\n");
    save(vault, "essays/gamma.md", "# Gamma\nquick thinking wins\n");
    // Multi-revision document: only the head body should be indexed.
    save(vault, "notes/alpha.md", "# Alpha\nslow purple elephant\n");
    // Non-UTF8 content must be indexable (lossy) without breaking rebuilds.
    let base = None;
    vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            doc: DocRef::Path("bin/blob.dat".to_owned()),
            base,
            content: vec![0xff, 0xfe, b'q', b'u', b'i', b'c', b'k'],
            origin: RevisionOrigin::Import,
            lease: None,
        })
        .unwrap();
}

const QUERIES: &[&str] = &["quick", "lazy", "alpha", "elephant", "fox"];

#[test]
fn deleted_index_rebuilds_identically() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    populate(&mut vault);

    let live = vault.derived().expect("derived index available");
    let live_docs = live.docs_snapshot().unwrap();
    let live_revs = live.revisions_snapshot().unwrap();
    let live_hits: Vec<_> = QUERIES
        .iter()
        .map(|q| live.search(q, 50).unwrap())
        .collect();
    assert_eq!(live_docs.len(), 4);
    assert_eq!(live_revs.len(), 5);
    // "quick" must match gamma (head), blob (lossy), but NOT alpha (old body).
    let quick_paths: Vec<_> = live_hits[0].iter().map(|h| h.path.as_str()).collect();
    assert!(quick_paths.contains(&"essays/gamma.md"));
    assert!(quick_paths.contains(&"bin/blob.dat"));
    assert!(!quick_paths.contains(&"notes/alpha.md"));
    drop(vault);

    // The gate: delete the database (and WAL debris), reopen, compare.
    for f in ["compos.db", "compos.db-wal", "compos.db-shm"] {
        let _ = fs::remove_file(root.join("state").join(f));
    }
    let vault = Vault::open_write(&root).unwrap();
    let rebuilt = vault.derived().expect("rebuilt index available");
    assert_eq!(rebuilt.docs_snapshot().unwrap(), live_docs);
    assert_eq!(rebuilt.revisions_snapshot().unwrap(), live_revs);
    for (q, expected) in QUERIES.iter().zip(&live_hits) {
        assert_eq!(&rebuilt.search(q, 50).unwrap(), expected, "query {q:?}");
    }
}

#[test]
fn incremental_and_rebuilt_agree() {
    // The live index built record-by-record (save step 6) must equal an
    // index built by one catch-up replay — same code path both ways is the
    // one-source-of-truth rule made testable.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    populate(&mut vault);
    let live_docs = vault.derived().unwrap().docs_snapshot().unwrap();
    let live_revs = vault.derived().unwrap().revisions_snapshot().unwrap();
    drop(vault);

    // Reopen without deleting: sync should be a no-op catch-up, not a drift.
    let vault = Vault::open_write(&root).unwrap();
    assert_eq!(vault.derived().unwrap().docs_snapshot().unwrap(), live_docs);
    assert_eq!(
        vault.derived().unwrap().revisions_snapshot().unwrap(),
        live_revs
    );
}

#[test]
fn schema_skew_rebuilds_transparently() {
    // Tier-2 N/N-1 policy: user_version older or newer than this build →
    // destroy + rebuild, silently, with identical results.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    populate(&mut vault);
    let live_docs = vault.derived().unwrap().docs_snapshot().unwrap();
    drop(vault);

    for skew in [DERIVED_SCHEMA_VERSION + 1, 99] {
        let conn = rusqlite::Connection::open(root.join("state").join("compos.db")).unwrap();
        conn.pragma_update(None, "user_version", skew).unwrap();
        drop(conn);

        let vault = Vault::open_write(&root).unwrap();
        assert!(vault.warnings().is_empty(), "skew must not even warn");
        assert_eq!(vault.derived().unwrap().docs_snapshot().unwrap(), live_docs);
        drop(vault);
    }
}

#[test]
fn read_only_open_serves_search() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    populate(&mut vault);
    drop(vault);

    let vault = Vault::open_read(&root).unwrap();
    let hits = vault.search("elephant", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "notes/alpha.md");
}

#[test]
fn bad_query_is_validation_failed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    populate(&mut vault);
    let err = vault.search("\"unbalanced", 10).unwrap_err();
    assert!(matches!(
        err,
        compos_core::VaultError::ValidationFailed { .. }
    ));
}
