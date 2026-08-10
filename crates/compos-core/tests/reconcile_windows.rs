//! Deterministic tests for every crash window of the save transaction
//! (ARCHITECTURE.md §5.3). Each test hand-builds the exact on-disk state a
//! crash would leave and asserts the specific recovery action, so a torture
//! failure is a regression signal rather than the only coverage.

use std::fs;
use std::path::{Path, PathBuf};

use compos_core::{DocId, DocRef, ObjectHash, RevisionId, RevisionOrigin, SaveRequest, Vault};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    doc: DocId,
    rev1: RevisionId,
    rev2: RevisionId,
}

/// A vault with one document `a.md` at rev2 ("two"), rev1 ("one") behind it.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let mut vault = Vault::init(&root).unwrap();
    let save = |vault: &mut Vault, base, content: &[u8]| {
        vault
            .writer()
            .unwrap()
            .save(SaveRequest {
                doc: DocRef::Path("a.md".to_owned()),
                base,
                content: content.to_vec(),
                origin: RevisionOrigin::Editor,
                lease: None,
            })
            .unwrap()
    };
    let first = save(&mut vault, None, b"one");
    let second = save(&mut vault, Some(first.rev.clone()), b"two");
    drop(vault);
    Fixture {
        _dir: dir,
        root,
        doc: first.doc,
        rev1: first.rev,
        rev2: second.rev,
    }
}

/// Write an object directly into the store layout, as step 2 would have.
fn put_object_raw(root: &Path, bytes: &[u8]) -> ObjectHash {
    let hash = ObjectHash::of(bytes);
    let shard = root.join("objects/sha256").join(&hash.hex()[..2]);
    fs::create_dir_all(&shard).unwrap();
    fs::write(shard.join(hash.hex()), bytes).unwrap();
    hash
}

/// Write a raw intent file, as step 3 would have.
fn craft_intent(
    root: &Path,
    rev: &str,
    doc: &DocId,
    parent: Option<&RevisionId>,
    object: &ObjectHash,
    path: &str,
) {
    let parent_json = match parent {
        Some(p) => format!("\"{p}\""),
        None => "null".to_owned(),
    };
    let json = format!(
        "{{\"v\":1,\"ts\":9999999999999,\"doc\":\"{doc}\",\"rev\":\"{rev}\",\"parent\":{parent_json},\"object\":\"{object}\",\"path\":\"{path}\"}}"
    );
    fs::write(root.join("intents").join(format!("{rev}.json")), json).unwrap();
}

fn intents_empty(root: &Path) -> bool {
    fs::read_dir(root.join("intents")).unwrap().count() == 0
}

fn visible(root: &Path, path: &str) -> Option<Vec<u8>> {
    fs::read(root.join("vault").join(path)).ok()
}

#[test]
fn w1_tmp_strays_are_swept() {
    let f = fixture();
    fs::write(f.root.join("tmp/obj-stray"), b"junk").unwrap();
    fs::write(f.root.join("vault/.compos-tmp-stray"), b"junk").unwrap();
    let vault = Vault::open_write(&f.root).unwrap();
    assert_eq!(fs::read_dir(f.root.join("tmp")).unwrap().count(), 0);
    assert!(!f.root.join("vault/.compos-tmp-stray").exists());
    assert!(vault.warnings().is_empty());
}

#[test]
fn w2_orphan_objects_are_kept() {
    let f = fixture();
    let orphan = put_object_raw(&f.root, b"orphan bytes");
    let vault = Vault::open_write(&f.root).unwrap();
    assert!(vault.objects().contains(&orphan));
}

#[test]
fn w3_intent_before_visible_replace_rolls_back() {
    let f = fixture();
    let object = put_object_raw(&f.root, b"three");
    craft_intent(&f.root, "r_crashw3", &f.doc, Some(&f.rev2), &object, "a.md");

    let vault = Vault::open_write(&f.root).unwrap();
    assert!(intents_empty(&f.root));
    assert_eq!(visible(&f.root, "a.md").unwrap(), b"two");
    assert_eq!(vault.index().head(&f.doc).unwrap().rev, f.rev2);
    assert!(vault.warnings().is_empty());
}

#[test]
fn w4_visible_replaced_rolls_back_to_parent() {
    let f = fixture();
    let object = put_object_raw(&f.root, b"three");
    craft_intent(&f.root, "r_crashw4", &f.doc, Some(&f.rev2), &object, "a.md");
    fs::write(f.root.join("vault/a.md"), b"three").unwrap();

    let vault = Vault::open_write(&f.root).unwrap();
    assert!(intents_empty(&f.root));
    assert_eq!(visible(&f.root, "a.md").unwrap(), b"two");
    assert!(vault.warnings().is_empty());
}

#[test]
fn w4_new_doc_first_save_rolls_back_to_absence() {
    let f = fixture();
    let object = put_object_raw(&f.root, b"newborn");
    let new_doc = DocId::generate();
    craft_intent(&f.root, "r_crashw4n", &new_doc, None, &object, "new.md");
    fs::write(f.root.join("vault/new.md"), b"newborn").unwrap();

    let vault = Vault::open_write(&f.root).unwrap();
    assert!(intents_empty(&f.root));
    assert!(visible(&f.root, "new.md").is_none());
    assert!(vault.index().doc_by_path("new.md").is_none());
    assert!(vault.warnings().is_empty());
}

#[test]
fn w5_committed_intent_rolls_forward() {
    let f = fixture();
    let object = ObjectHash::of(b"two");
    craft_intent(
        &f.root,
        f.rev2.as_str(),
        &f.doc,
        Some(&f.rev1),
        &object,
        "a.md",
    );

    let vault = Vault::open_write(&f.root).unwrap();
    assert!(intents_empty(&f.root));
    assert_eq!(visible(&f.root, "a.md").unwrap(), b"two");
    assert!(vault.warnings().is_empty());
}

#[test]
fn w5_missing_visible_is_restored_from_object() {
    let f = fixture();
    let object = ObjectHash::of(b"two");
    craft_intent(
        &f.root,
        f.rev2.as_str(),
        &f.doc,
        Some(&f.rev1),
        &object,
        "a.md",
    );
    fs::remove_file(f.root.join("vault/a.md")).unwrap();

    let vault = Vault::open_write(&f.root).unwrap();
    assert_eq!(visible(&f.root, "a.md").unwrap(), b"two");
    assert!(intents_empty(&f.root));
    assert!(vault.warnings().is_empty());
}

#[test]
fn w7_torn_journal_tail_is_truncated() {
    let f = fixture();
    let segment = f.root.join("journal/0000000001.jsonl");
    let mut bytes = fs::read(&segment).unwrap();
    bytes.extend_from_slice(b"{\"torn\":");
    fs::write(&segment, &bytes).unwrap();

    let mut vault = Vault::open_write(&f.root).unwrap();
    assert_eq!(vault.index().head(&f.doc).unwrap().rev, f.rev2);

    // The vault keeps working after repair.
    let out = vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            doc: DocRef::Path("a.md".to_owned()),
            base: Some(f.rev2.clone()),
            content: b"post-repair".to_vec(),
            origin: RevisionOrigin::Editor,
            lease: None,
        })
        .unwrap();
    drop(vault);
    let vault = Vault::open_read(&f.root).unwrap();
    assert_eq!(vault.index().head(&f.doc).unwrap().rev, out.rev);
}

#[test]
fn external_edit_under_crashed_save_is_never_clobbered() {
    let f = fixture();
    let object = put_object_raw(&f.root, b"three");
    craft_intent(
        &f.root,
        "r_crashedge",
        &f.doc,
        Some(&f.rev2),
        &object,
        "a.md",
    );
    fs::write(f.root.join("vault/a.md"), b"hand edit").unwrap();

    let vault = Vault::open_write(&f.root).unwrap();
    assert_eq!(visible(&f.root, "a.md").unwrap(), b"hand edit");
    assert!(intents_empty(&f.root));
    assert_eq!(vault.warnings().len(), 1, "{:?}", vault.warnings());
}
