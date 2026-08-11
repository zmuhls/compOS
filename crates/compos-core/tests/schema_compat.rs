//! The N/N-1 schema harness for tier-1 canonical state (ARCHITECTURE.md
//! §5.2, §17): every vault format this build supports has a frozen golden
//! fixture under `tests/fixtures/vault-format-<N>/`, generated once by the
//! build that introduced the format and never regenerated. A format bump
//! that cannot open its predecessor's fixture fails here.
//!
//! The tier-2 (SQLite) side of the harness lives in `derived_rebuild.rs`:
//! its N/N-1 policy is destroy-and-rebuild, proven there.

use std::fs;
use std::path::{Path, PathBuf};

use compos_core::{ObjectHash, VAULT_FORMAT, Vault, VaultError};

fn fixture_dir(version: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("vault-format-{version}"))
}

/// Copy a fixture into a writable temp location (opens mutate: lock file,
/// reconciliation, derived index). Object files are 0444 in the fixture;
/// the copies stay immutable, which is exactly how a live vault holds them.
fn copy_fixture(version: u32, dest: &Path) {
    copy_tree(&fixture_dir(version), dest);
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().filter_map(Result::ok) {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn every_supported_format_has_a_frozen_fixture() {
    // The tripwire: bumping VAULT_FORMAT without freezing a fixture for the
    // new format (and keeping the old ones opening) fails immediately.
    for version in 1..=VAULT_FORMAT {
        assert!(
            fixture_dir(version).join("compos.json").is_file(),
            "missing golden fixture for vault format {version}; \
             freeze one before shipping the format"
        );
    }
}

#[test]
fn golden_fixtures_open_for_writing_with_full_semantics() {
    for version in 1..=VAULT_FORMAT {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("v");
        copy_fixture(version, &root);

        let mut vault = Vault::open_write(&root).unwrap();
        assert!(vault.warnings().is_empty(), "clean fixture must not warn");
        assert_eq!(vault.format().vault_format, version);

        if version == 1 {
            assert_eq!(vault.index().len(), 2);
            let kappa = vault.index().doc_by_path("notes/kappa.md").unwrap().clone();
            let head = vault.index().head(&kappa).unwrap().clone();
            let bytes = vault.objects().read(&head.object).unwrap();
            assert_eq!(bytes, b"k2 quick\n");
            assert_eq!(head.object, ObjectHash::of(b"k2 quick\n"));
            assert_eq!(vault.history(&kappa).unwrap().len(), 2);

            // The derived index rebuilds from scratch on first open (the
            // fixture ships no state/) and serves search.
            let hits = vault.search("quick", 10).unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].path, "notes/kappa.md");

            // And the vault still accepts new work: the whole point of a
            // migration is continued write access.
            let base = Some(head.rev.clone());
            vault
                .writer()
                .unwrap()
                .save(compos_core::SaveRequest {
                    doc: compos_core::DocRef::Id(kappa.clone()),
                    base,
                    content: b"k3 after reopen\n".to_vec(),
                    origin: compos_core::RevisionOrigin::Editor,
                    lease: None,
                })
                .unwrap();
            assert_eq!(vault.history(&kappa).unwrap().len(), 3);
        }
    }
}

#[test]
fn golden_fixtures_open_read_only() {
    for version in 1..=VAULT_FORMAT {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("v");
        copy_fixture(version, &root);
        let vault = Vault::open_read(&root).unwrap();
        assert!(!vault.index().is_empty());
    }
}

#[test]
fn future_vault_format_is_refused_both_modes() {
    // The other half of N/N-1: this build must refuse formats from the
    // future loudly instead of misreading them.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    copy_fixture(1, &root);
    let future = format!(
        "{{\"vault_format\":{},\"vault_id\":\"ffffffffffffffffffffffffffffffff\",\"created_ms\":0}}",
        VAULT_FORMAT + 1
    );
    fs::write(root.join("compos.json"), future).unwrap();

    for result in [
        Vault::open_write(&root).map(|_| ()),
        Vault::open_read(&root).map(|_| ()),
    ] {
        match result {
            Err(VaultError::FormatUnsupported { found, supported }) => {
                assert_eq!(found, VAULT_FORMAT + 1);
                assert_eq!(supported, VAULT_FORMAT);
            }
            other => panic!("expected FormatUnsupported, got {other:?}"),
        }
    }
}
