//! The constitution conformance suite (ARCHITECTURE.md §3, layer 3).
//!
//! One suite, run identically on every host profile — it *is* the
//! definition of profile equivalence. Each test names the constitutional
//! rule it enforces. Some assertions overlap other test files; that is
//! deliberate: this file is the contract's single narrative, and it must
//! keep passing even if the specialized suites are reorganized.

use std::time::Duration;

use compos_core::{CommandRegistry, NetworkPolicy, Vault, VaultError};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

async fn start_server() -> (compos_rpc::RpcHandle, tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let vault = Vault::init(&root).unwrap();
    let config = compos_rpc::RpcConfig {
        socket_path: dir.path().join("c.sock"),
        ws_addr: "127.0.0.1:0".parse().unwrap(),
        token_file: root.join("state").join("rpc-token"),
    };
    let handle = compos_rpc::start(vault, CommandRegistry::with_builtins(), config)
        .await
        .unwrap();
    (handle, dir, root)
}

async fn rpc(
    stream: &mut (
        tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
        tokio::net::unix::OwnedWriteHalf,
    ),
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    stream
        .1
        .write_all(format!("{req}\n").as_bytes())
        .await
        .unwrap();
    loop {
        let line = tokio::time::timeout(Duration::from_secs(5), stream.0.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["id"] == json!(id) {
            return v;
        }
    }
}

async fn connect(
    path: &std::path::Path,
) -> (
    tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    tokio::net::unix::OwnedWriteHalf,
) {
    let s = UnixStream::connect(path).await.unwrap();
    let (r, w) = s.into_split();
    (BufReader::new(r).lines(), w)
}

/// Rule 1: composd is the sole writer; a second writer fails loudly.
#[test]
fn rule1_second_writer_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let _first = Vault::init(&root).unwrap();
    match Vault::open_write(&root) {
        Err(VaultError::VaultBusy) => {}
        other => panic!("second writer must get VaultBusy, got {other:?}"),
    }
}

/// Rule 2 (the testable core of it): every Phase-1 command declares
/// `network: never` — writing, reading, saving, and search cannot even
/// express a network dependency.
#[test]
fn rule2_no_command_touches_the_network() {
    let registry = CommandRegistry::with_builtins();
    for spec in registry.list() {
        assert_eq!(
            spec.network,
            NetworkPolicy::Never,
            "{} must not require network",
            spec.id
        );
    }
}

/// Rule 3: canonical work remains ordinary files — a save through the API
/// is byte-identical on disk, readable with no CompOS code at all.
#[tokio::test]
async fn rule3_canonical_bytes_are_plain_files() {
    let (handle, _dir, root) = start_server().await;
    let mut c = connect(&handle.socket_path).await;
    rpc(&mut c, 1, "hello", json!({"role": "shell"})).await;
    rpc(
        &mut c,
        2,
        "commands.invoke",
        json!({"command": "document.save",
               "input": {"path": "plain.md", "content": "just a file\n"}}),
    )
    .await;
    assert_eq!(
        std::fs::read(root.join("vault/plain.md")).unwrap(),
        b"just a file\n"
    );
}

/// Rule 4: deleting the derived database and rebuilding reproduces
/// identical search results.
#[test]
fn rule4_derived_state_is_disposable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let registry = CommandRegistry::with_builtins();
    let mut vault = Vault::init(&root).unwrap();
    registry
        .invoke(
            &mut vault,
            "document.save",
            &json!({"path": "s.md", "content": "searchable prose\n"}),
        )
        .unwrap();
    let before = registry
        .invoke(&mut vault, "search.query", &json!({"query": "searchable"}))
        .unwrap();
    drop(vault);

    for f in ["compos.db", "compos.db-wal", "compos.db-shm"] {
        let _ = std::fs::remove_file(root.join("state").join(f));
    }
    let mut vault = Vault::open_write(&root).unwrap();
    let after = registry
        .invoke(&mut vault, "search.query", &json!({"query": "searchable"}))
        .unwrap();
    assert_eq!(before, after);
}

/// Rule 6 (and rule 5's boundary): an agent-role connection invoking any
/// commit- or system-effect command receives CAPABILITY_DENIED at the
/// boundary; nothing is committed.
#[tokio::test]
async fn rule6_agent_cannot_commit() {
    let (handle, _dir, root) = start_server().await;
    let mut agent = connect(&handle.socket_path).await;
    rpc(&mut agent, 1, "hello", json!({"role": "agent"})).await;

    let denied = rpc(
        &mut agent,
        2,
        "commands.invoke",
        json!({"command": "document.save",
               "input": {"path": "agent.md", "content": "should never land"}}),
    )
    .await;
    assert_eq!(denied["error"]["data"]["type"], "CAPABILITY_DENIED");
    assert!(
        !root.join("vault/agent.md").exists(),
        "denied commit must leave no trace"
    );
}

/// Never-clobber: an out-of-band file edit becomes an `external` revision
/// and never silently overwrites.
#[tokio::test]
async fn never_clobber_external_edits_become_revisions() {
    let (handle, _dir, root) = start_server().await;
    let mut c = connect(&handle.socket_path).await;
    rpc(&mut c, 1, "hello", json!({"role": "shell"})).await;
    rpc(
        &mut c,
        2,
        "commands.invoke",
        json!({"command": "document.save",
               "input": {"path": "e.md", "content": "committed\n"}}),
    )
    .await;

    // Out-of-band edit, then scan through the API surface.
    std::fs::write(root.join("vault/e.md"), b"outside bytes\n").unwrap();
    let scan = rpc(
        &mut c,
        3,
        "commands.invoke",
        json!({"command": "vault.scan", "input": {}}),
    )
    .await;
    assert_eq!(scan["result"]["converted"].as_array().unwrap().len(), 1);

    let hist = rpc(
        &mut c,
        4,
        "commands.invoke",
        json!({"command": "document.history", "input": {"path": "e.md"}}),
    )
    .await;
    let revs = hist["result"]["revisions"].as_array().unwrap();
    assert_eq!(revs.len(), 2);
    assert_eq!(revs[1]["origin"], "external");
    assert_eq!(
        std::fs::read(root.join("vault/e.md")).unwrap(),
        b"outside bytes\n",
        "the user's bytes stay exactly as written"
    );
}
