//! Ratified lease semantics through the public vault API: optional and
//! advisory over base-matching, holder saves slide the TTL, release frees
//! immediately. (Clock-dependent expiry cases are unit-tested in lease.rs
//! with a simulated clock.)

use compos_core::{DocRef, RevisionOrigin, SaveRequest, Vault, VaultError};

fn req(path: &str, base: Option<compos_core::RevisionId>, content: &str) -> SaveRequest {
    SaveRequest {
        doc: DocRef::Path(path.to_owned()),
        base,
        content: content.as_bytes().to_vec(),
        origin: RevisionOrigin::Editor,
        lease: None,
    }
}

#[test]
fn lease_is_advisory_over_base_matching() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::init(&dir.path().join("v")).unwrap();

    // No lease anywhere: saves gate on base alone (ratified rule 1).
    let first = vault
        .writer()
        .unwrap()
        .save(req("a.md", None, "one"))
        .unwrap();

    // A holder appears.
    let lease = vault.acquire_lease(first.doc.clone()).unwrap();

    // Another session with the right base but no lease is held off.
    let err = vault
        .writer()
        .unwrap()
        .save(req("a.md", Some(first.rev.clone()), "intruder"))
        .unwrap_err();
    assert!(matches!(err, VaultError::LeaseHeld));

    // The holder saves fine (and this slides the TTL — rule 2).
    let second = vault
        .writer()
        .unwrap()
        .save(SaveRequest {
            lease: Some(lease.id.clone()),
            ..req("a.md", Some(first.rev), "holder edit")
        })
        .unwrap();

    // Explicit renew succeeds while live.
    vault.renew_lease(&first.doc, &lease.id).unwrap();

    // A second acquire while live is refused.
    assert!(matches!(
        vault.acquire_lease(first.doc.clone()),
        Err(VaultError::LeaseHeld)
    ));

    // Release frees the document immediately.
    vault.release_lease(&first.doc, &lease.id);
    vault
        .writer()
        .unwrap()
        .save(req("a.md", Some(second.rev), "free again"))
        .unwrap();
}
