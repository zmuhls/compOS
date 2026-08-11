//! The round-trip fixture harness (§8) — the first CI gate of the codec
//! surface. Every registered codec is run over every fixture in
//! `tests/fixtures/roundtrip/<codec-id>/`, asserting the law:
//!
//!   logical_digest(resource) == logical_digest(import(export(resource)))
//!   object hashes before export == object hashes after import
//!
//! New codecs inherit this gate by existing: a registered codec with no
//! fixture directory fails the corpus check.

use std::fs;
use std::path::{Path, PathBuf};

use compos_core::{
    CodecRegistry, DocRef, Fidelity, ObjectHash, RevisionOrigin, SaveRequest, Vault, logical_digest,
};

fn corpus_dir(codec_id: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/roundtrip")
        .join(codec_id)
}

fn fixtures_for(codec_id: &str) -> Vec<(String, Vec<u8>)> {
    let dir = corpus_dir(codec_id);
    let mut out: Vec<(String, Vec<u8>)> = fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("codec '{codec_id}' has no fixture corpus at {dir:?}"))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "empty fixture corpus for '{codec_id}'");
    out
}

#[test]
fn every_codec_satisfies_the_round_trip_law() {
    let registry = CodecRegistry::with_builtins();
    for id in registry.ids() {
        let codec = registry.get(id).unwrap();
        for (name, original) in fixtures_for(id) {
            let canonical = codec.import(&original).unwrap();
            let exported = codec.export(&canonical).unwrap();
            let reimported = codec.import(&exported).unwrap();

            assert_eq!(
                logical_digest(&canonical),
                logical_digest(&reimported),
                "{id}/{name}: logical digest changed across export→import"
            );
            if codec.fidelity() == Fidelity::Lossless {
                assert_eq!(
                    exported, original,
                    "{id}/{name}: lossless codec must reproduce exact bytes"
                );
            }
        }
    }
}

#[test]
fn markdown_identity_holds_through_the_vault() {
    // The law end-to-end: ingest through the save transaction (rule 8's
    // basic form), export from the object store, compare hashes and bytes.
    let registry = CodecRegistry::with_builtins();
    let codec = registry.get("markdown").unwrap();

    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::init(&dir.path().join("v")).unwrap();

    for (name, original) in fixtures_for("markdown") {
        let canonical = codec.import(&original).unwrap();
        let out = vault
            .writer()
            .unwrap()
            .save(SaveRequest {
                doc: DocRef::Path(format!("imports/{name}")),
                base: None,
                content: canonical.clone(),
                origin: RevisionOrigin::Import,
                lease: None,
            })
            .unwrap();

        // Object identity before export...
        assert_eq!(out.object, ObjectHash::of(&canonical));

        // ...equals object identity after a full export→import cycle.
        let stored = vault.objects().read(&out.object).unwrap();
        let exported = codec.export(&stored).unwrap();
        let reimported = codec.import(&exported).unwrap();
        assert_eq!(ObjectHash::of(&reimported), out.object, "{name}");
        assert_eq!(exported, original, "{name}: byte-identical export");
    }
}

#[test]
fn duplicate_codec_ids_are_rejected() {
    let mut registry = CodecRegistry::with_builtins();
    let err = registry
        .register(Box::new(compos_core::MarkdownIdentity))
        .unwrap_err();
    assert!(matches!(
        err,
        compos_core::VaultError::ValidationFailed { .. }
    ));
}
