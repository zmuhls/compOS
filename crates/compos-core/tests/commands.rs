//! Command registry tests (§7): the one surface, schema-validated inputs,
//! effect classes, and the built-in Phase-1 commands end-to-end.

use compos_core::{CommandRegistry, Effect, Vault, VaultError};
use serde_json::json;

fn vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    let v = Vault::init(&dir.path().join("v")).unwrap();
    (dir, v)
}

#[test]
fn builtins_register_and_enumerate() {
    let r = CommandRegistry::with_builtins();
    let ids: Vec<_> = r.list().iter().map(|s| s.id.clone()).collect();
    for expected in [
        "document.save",
        "document.read",
        "document.list",
        "document.history",
        "search.query",
        "vault.scan",
        "vault.status",
        "system.health.inspect",
    ] {
        assert!(ids.contains(&expected.to_owned()), "missing {expected}");
    }
    // Effect classes as §7 assigns them.
    assert_eq!(r.describe("document.save").unwrap().effect, Effect::Commit);
    assert_eq!(r.describe("vault.scan").unwrap().effect, Effect::Commit);
    assert_eq!(r.describe("document.read").unwrap().effect, Effect::Read);
    assert_eq!(
        r.describe("system.health.inspect").unwrap().effect,
        Effect::System
    );
}

#[test]
fn save_read_history_round_trip() {
    let (_dir, mut v) = vault();
    let r = CommandRegistry::with_builtins();

    let out = r
        .invoke(
            &mut v,
            "document.save",
            &json!({"path": "a.md", "content": "hello world\n"}),
        )
        .unwrap();
    let rev = out["rev"].as_str().unwrap().to_owned();

    let read = r
        .invoke(&mut v, "document.read", &json!({"path": "a.md"}))
        .unwrap();
    assert_eq!(read["content"], "hello world\n");
    assert_eq!(read["rev"].as_str().unwrap(), rev);

    // Second save must carry the base revision.
    r.invoke(
        &mut v,
        "document.save",
        &json!({"path": "a.md", "content": "v2\n", "base": rev}),
    )
    .unwrap();

    let hist = r
        .invoke(&mut v, "document.history", &json!({"path": "a.md"}))
        .unwrap();
    assert_eq!(hist["revisions"].as_array().unwrap().len(), 2);

    let list = r.invoke(&mut v, "document.list", &json!({})).unwrap();
    assert_eq!(list["documents"].as_array().unwrap().len(), 1);

    let hits = r
        .invoke(&mut v, "search.query", &json!({"query": "v2"}))
        .unwrap();
    assert_eq!(hits["hits"].as_array().unwrap().len(), 1);
}

#[test]
fn input_validation_rejects_bad_shapes() {
    let (_dir, mut v) = vault();
    let r = CommandRegistry::with_builtins();

    for bad in [
        json!({}),                                        // missing required
        json!({"path": "a.md"}),                          // missing content
        json!({"path": "", "content": "x"}),              // minLength
        json!({"path": "a.md", "content": 5}),            // wrong type
        json!({"path": "a.md", "content": "x", "zz": 1}), // unknown field
        // origin outside the client-claimable set
        json!({"path": "a.md", "content": "x", "origin": "external"}),
    ] {
        let err = r.invoke(&mut v, "document.save", &bad).unwrap_err();
        assert!(
            matches!(err, VaultError::ValidationFailed { .. }),
            "input {bad} should fail validation, got {err:?}"
        );
    }
    assert!(v.index().is_empty(), "no save may have gone through");
}

#[test]
fn unknown_command_is_typed() {
    let (_dir, mut v) = vault();
    let r = CommandRegistry::with_builtins();
    let err = r.invoke(&mut v, "no.such.command", &json!({})).unwrap_err();
    assert!(matches!(err, VaultError::CommandUnknown(_)));
}

#[test]
fn duplicate_registration_is_rejected() {
    let mut r = CommandRegistry::with_builtins();
    let spec = r.describe("vault.status").unwrap().clone();
    let err = r
        .register(spec, Box::new(|_, _| Ok(json!({}))))
        .unwrap_err();
    assert!(matches!(err, VaultError::ValidationFailed { .. }));
}

#[test]
fn stale_base_surfaces_through_invoke() {
    let (_dir, mut v) = vault();
    let r = CommandRegistry::with_builtins();
    r.invoke(
        &mut v,
        "document.save",
        &json!({"path": "a.md", "content": "one"}),
    )
    .unwrap();
    // No base on an existing document → StaleBase, the §6 wire error.
    let err = r
        .invoke(
            &mut v,
            "document.save",
            &json!({"path": "a.md", "content": "two"}),
        )
        .unwrap_err();
    assert!(matches!(err, VaultError::StaleBase { .. }));
}

#[test]
fn health_inspect_is_dry_run_on_dev() {
    let (_dir, mut v) = vault();
    let r = CommandRegistry::with_builtins();
    let out = r
        .invoke(&mut v, "system.health.inspect", &json!({}))
        .unwrap();
    assert_eq!(out["dry_run"], true);
    assert!(out["checks"].as_array().unwrap().len() >= 4);
}
