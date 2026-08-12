//! The proposal plane end-to-end at the library level (ARCHITECTURE.md §9,
//! §12 Review): create/accept/reject/withdraw through the command registry,
//! accept-time stale recheck, per-hunk subset acceptance, restart survival,
//! and the never-clobber guarantee holding through proposal accepts.

use compos_core::{CommandRegistry, RevisionOrigin, Vault, VaultError};
use serde_json::{Value, json};

fn vault() -> (tempfile::TempDir, Vault, CommandRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let v = Vault::init(&dir.path().join("v")).unwrap();
    (dir, v, CommandRegistry::with_builtins())
}

fn save(
    r: &CommandRegistry,
    v: &mut Vault,
    path: &str,
    content: &str,
    base: Option<&str>,
) -> Value {
    let mut input = json!({"path": path, "content": content});
    if let Some(b) = base {
        input["base"] = json!(b);
    }
    r.invoke(v, "document.save", &input).unwrap()
}

#[test]
fn propose_accept_all_hunks_lands_a_proposal_accept_revision() {
    let (_dir, mut v, r) = vault();
    save(&r, &mut v, "a.md", "one\ntwo\nthree\n", None);

    let p = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({
                "path": "a.md",
                "hunks": [
                    {"start": 0, "del": 1, "ins": "ONE\n"},
                    {"start": 3, "del": 0, "ins": "four\n"},
                ],
                "provenance": {"provider": "test", "model": "none"},
                "evidence": ["unit test"],
            }),
        )
        .unwrap();
    assert_eq!(p["state"], "open");
    assert_eq!(p["stale"], false);
    let pid = p["proposal"].as_str().unwrap().to_owned();

    let out = r
        .invoke(&mut v, "proposal.accept.hunk", &json!({"proposal": pid}))
        .unwrap();
    assert_eq!(out["state"], "accepted");
    assert_eq!(out["accepted_hunks"], json!([0, 1]));

    let read = r
        .invoke(&mut v, "document.read", &json!({"path": "a.md"}))
        .unwrap();
    assert_eq!(read["content"], "ONE\ntwo\nthree\nfour\n");

    // The canonical journal records the accept with its reserved origin.
    let doc = v.index().doc_by_path("a.md").unwrap().clone();
    let history = v.history(&doc).unwrap();
    assert_eq!(
        history.last().unwrap().origin,
        RevisionOrigin::ProposalAccept
    );
    assert_eq!(history.last().unwrap().rev.as_str(), out["rev"]);
}

#[test]
fn subset_acceptance_applies_only_selected_hunks() {
    let (_dir, mut v, r) = vault();
    save(&r, &mut v, "a.md", "one\ntwo\nthree\n", None);
    let p = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({
                "path": "a.md",
                "hunks": [
                    {"start": 0, "del": 1, "ins": "ONE\n"},
                    {"start": 1, "del": 1, "ins": "TWO\n"},
                    {"start": 2, "del": 1, "ins": "THREE\n"},
                ],
            }),
        )
        .unwrap();
    let pid = p["proposal"].as_str().unwrap();

    let out = r
        .invoke(
            &mut v,
            "proposal.accept.hunk",
            &json!({"proposal": pid, "hunks": [2, 0, 2]}),
        )
        .unwrap();
    // Selection is normalized: sorted, deduplicated.
    assert_eq!(out["accepted_hunks"], json!([0, 2]));

    let read = r
        .invoke(&mut v, "document.read", &json!({"path": "a.md"}))
        .unwrap();
    assert_eq!(read["content"], "ONE\ntwo\nTHREE\n");
}

#[test]
fn new_document_proposal_creates_the_doc_on_accept() {
    let (_dir, mut v, r) = vault();
    let p = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({
                "path": "fresh.md",
                "hunks": [{"start": 0, "del": 0, "ins": "born from a proposal\n"}],
            }),
        )
        .unwrap();
    assert_eq!(p["doc"], Value::Null);
    assert_eq!(p["base"], Value::Null);
    let pid = p["proposal"].as_str().unwrap();

    let out = r
        .invoke(&mut v, "proposal.accept.hunk", &json!({"proposal": pid}))
        .unwrap();
    assert_eq!(out["state"], "accepted");
    let read = r
        .invoke(&mut v, "document.read", &json!({"path": "fresh.md"}))
        .unwrap();
    assert_eq!(read["content"], "born from a proposal\n");
}

#[test]
fn stale_proposal_is_flagged_and_accept_is_rejected() {
    let (_dir, mut v, r) = vault();
    let first = save(&r, &mut v, "a.md", "one\ntwo\n", None);
    let p = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({"path": "a.md", "hunks": [{"start": 0, "del": 1, "ins": "ONE\n"}]}),
        )
        .unwrap();
    let pid = p["proposal"].as_str().unwrap().to_owned();

    // The head moves on beneath the proposal.
    save(
        &r,
        &mut v,
        "a.md",
        "one\ntwo\nthree\n",
        Some(first["rev"].as_str().unwrap()),
    );

    // Derived staleness shows in list/get without any stored marker.
    let listed = r
        .invoke(&mut v, "proposal.list", &json!({"path": "a.md"}))
        .unwrap();
    assert_eq!(listed["proposals"][0]["stale"], true);
    assert_eq!(listed["proposals"][0]["state"], "open");

    // The accept-time recheck refuses; the proposal stays open.
    let err = r
        .invoke(&mut v, "proposal.accept.hunk", &json!({"proposal": pid}))
        .unwrap_err();
    assert!(matches!(err, VaultError::StaleBase { .. }), "got {err:?}");
    let got = r
        .invoke(&mut v, "proposal.get", &json!({"proposal": pid}))
        .unwrap();
    assert_eq!(got["state"], "open");

    // The document is untouched by the failed accept.
    let read = r
        .invoke(&mut v, "document.read", &json!({"path": "a.md"}))
        .unwrap();
    assert_eq!(read["content"], "one\ntwo\nthree\n");
}

#[test]
fn never_clobber_holds_through_proposal_accept() {
    let (dir, mut v, r) = vault();
    let root = dir.path().join("v");
    save(&r, &mut v, "a.md", "committed\n", None);
    let p = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({"path": "a.md", "hunks": [{"start": 0, "del": 1, "ins": "proposed\n"}]}),
        )
        .unwrap();
    let pid = p["proposal"].as_str().unwrap();

    // The user edits the file out-of-band while the proposal is open.
    std::fs::write(root.join("vault/a.md"), b"user bytes\n").unwrap();

    // Accept must not clobber: the guard converts the user's bytes into an
    // external revision first, and the accept fails StaleBase.
    let err = r
        .invoke(&mut v, "proposal.accept.hunk", &json!({"proposal": pid}))
        .unwrap_err();
    assert!(matches!(err, VaultError::StaleBase { .. }), "got {err:?}");
    assert_eq!(
        std::fs::read(root.join("vault/a.md")).unwrap(),
        b"user bytes\n",
        "the user's bytes stay exactly as written"
    );
    let doc = v.index().doc_by_path("a.md").unwrap().clone();
    let history = v.history(&doc).unwrap();
    assert_eq!(history.last().unwrap().origin, RevisionOrigin::External);
}

#[test]
fn reject_withdraw_and_double_resolution() {
    let (_dir, mut v, r) = vault();
    save(&r, &mut v, "a.md", "one\n", None);
    let make = |v: &mut Vault, r: &CommandRegistry| {
        r.invoke(
            v,
            "proposal.create",
            &json!({"path": "a.md", "hunks": [{"start": 0, "del": 1, "ins": "x\n"}]}),
        )
        .unwrap()["proposal"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let rejected = make(&mut v, &r);
    let out = r
        .invoke(&mut v, "proposal.reject", &json!({"proposal": rejected}))
        .unwrap();
    assert_eq!(out["state"], "rejected");

    let withdrawn = make(&mut v, &r);
    let out = r
        .invoke(&mut v, "proposal.withdraw", &json!({"proposal": withdrawn}))
        .unwrap();
    assert_eq!(out["state"], "withdrawn");

    // A resolved proposal cannot be resolved again, in any direction.
    for cmd in [
        "proposal.reject",
        "proposal.withdraw",
        "proposal.accept.hunk",
    ] {
        let err = r
            .invoke(&mut v, cmd, &json!({"proposal": rejected}))
            .unwrap_err();
        assert!(
            matches!(err, VaultError::ValidationFailed { .. }),
            "{cmd} on a resolved proposal: got {err:?}"
        );
    }

    // Unknown proposals are their own typed error.
    let err = r
        .invoke(&mut v, "proposal.get", &json!({"proposal": "pr_missing"}))
        .unwrap_err();
    assert!(
        matches!(err, VaultError::ProposalNotFound(_)),
        "got {err:?}"
    );
}

#[test]
fn create_validates_hunks_against_the_base() {
    let (_dir, mut v, r) = vault();
    save(&r, &mut v, "a.md", "one\ntwo\n", None);

    // Out of range.
    let err = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({"path": "a.md", "hunks": [{"start": 5, "del": 1, "ins": "x\n"}]}),
        )
        .unwrap_err();
    assert!(matches!(err, VaultError::ValidationFailed { .. }));

    // Overlapping.
    let err = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({"path": "a.md", "hunks": [
                {"start": 0, "del": 2, "ins": "x\n"},
                {"start": 1, "del": 1, "ins": "y\n"},
            ]}),
        )
        .unwrap_err();
    assert!(matches!(err, VaultError::ValidationFailed { .. }));

    // A proposal against a path that must not exist yet, with a non-empty
    // base expectation: hunks beyond line 0 of an empty base are invalid.
    let err = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({"path": "new.md", "hunks": [{"start": 1, "del": 0, "ins": "x\n"}]}),
        )
        .unwrap_err();
    assert!(matches!(err, VaultError::ValidationFailed { .. }));

    // Escaping paths are refused like everywhere else.
    let err = r
        .invoke(
            &mut v,
            "proposal.create",
            &json!({"path": "../escape.md", "hunks": [{"start": 0, "del": 0, "ins": "x\n"}]}),
        )
        .unwrap_err();
    assert!(matches!(err, VaultError::InvalidPath { .. }));
}

#[test]
fn proposals_survive_restart_in_both_modes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let r = CommandRegistry::with_builtins();

    let (open_id, accepted_id) = {
        let mut v = Vault::init(&root).unwrap();
        save(&r, &mut v, "a.md", "one\ntwo\n", None);
        let open = r
            .invoke(
                &mut v,
                "proposal.create",
                &json!({"path": "a.md", "hunks": [{"start": 0, "del": 1, "ins": "ONE\n"}]}),
            )
            .unwrap();
        let accepted = r
            .invoke(
                &mut v,
                "proposal.create",
                &json!({"path": "a.md", "hunks": [{"start": 1, "del": 1, "ins": "TWO\n"}]}),
            )
            .unwrap();
        // Accepting the second makes the first stale — and both must
        // survive the restart exactly as they are now.
        r.invoke(
            &mut v,
            "proposal.accept.hunk",
            &json!({"proposal": accepted["proposal"]}),
        )
        .unwrap();
        (
            open["proposal"].as_str().unwrap().to_owned(),
            accepted["proposal"].as_str().unwrap().to_owned(),
        )
    };

    // Write-mode reopen: full state, derived staleness intact.
    {
        let mut v = Vault::open_write(&root).unwrap();
        let listed = r.invoke(&mut v, "proposal.list", &json!({})).unwrap();
        assert_eq!(listed["proposals"].as_array().unwrap().len(), 2);
        let by_id = |id: &str| {
            listed["proposals"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["proposal"] == json!(id))
                .unwrap()
                .clone()
        };
        assert_eq!(by_id(&open_id)["state"], "open");
        assert_eq!(by_id(&open_id)["stale"], true);
        assert_eq!(by_id(&accepted_id)["state"], "accepted");
        assert!(by_id(&accepted_id)["rev"].is_string());
    }

    // Read-mode open sees the same proposals.
    {
        let v = Vault::open_read(&root).unwrap();
        assert_eq!(v.proposals().count(), 2);
    }
}

#[test]
fn torn_proposal_tail_is_repaired_on_write_open() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let r = CommandRegistry::with_builtins();
    {
        let mut v = Vault::init(&root).unwrap();
        save(&r, &mut v, "a.md", "one\n", None);
        r.invoke(
            &mut v,
            "proposal.create",
            &json!({"path": "a.md", "hunks": [{"start": 0, "del": 1, "ins": "X\n"}]}),
        )
        .unwrap();
    }

    // Simulate a crash mid-append: a torn, unterminated record at the tail.
    let seg = root.join("journal/proposals/0000000001.jsonl");
    let mut bytes = std::fs::read(&seg).unwrap();
    bytes.extend_from_slice(b"{\"v\":1,\"ts\":12,\"prop\":\"pr_torn\",\"event\":\"cre");
    std::fs::write(&seg, &bytes).unwrap();

    // Read mode tolerates the torn tail without repairing it.
    {
        let v = Vault::open_read(&root).unwrap();
        assert_eq!(v.proposals().count(), 1);
    }
    // Write mode repairs it; the surviving proposal is intact and usable.
    {
        let mut v = Vault::open_write(&root).unwrap();
        assert_eq!(v.proposals().count(), 1);
        let listed = r.invoke(&mut v, "proposal.list", &json!({})).unwrap();
        assert_eq!(listed["proposals"][0]["state"], "open");
    }
    let repaired = std::fs::read(&seg).unwrap();
    assert!(
        repaired.ends_with(b"\n"),
        "tail truncated to a valid record"
    );
}

#[test]
fn effect_classes_carry_the_boundary_contract() {
    use compos_core::Effect;
    let r = CommandRegistry::with_builtins();
    assert_eq!(
        r.describe("proposal.create").unwrap().effect,
        Effect::Propose
    );
    assert_eq!(
        r.describe("proposal.withdraw").unwrap().effect,
        Effect::Propose
    );
    assert_eq!(r.describe("proposal.list").unwrap().effect, Effect::Read);
    assert_eq!(r.describe("proposal.get").unwrap().effect, Effect::Read);
    assert_eq!(
        r.describe("proposal.accept.hunk").unwrap().effect,
        Effect::Commit
    );
    assert_eq!(
        r.describe("proposal.reject").unwrap().effect,
        Effect::Commit
    );
}
