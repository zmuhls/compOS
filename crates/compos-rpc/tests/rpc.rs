//! RPC boundary tests (§6): role negotiation, the fixed method surface,
//! capability caps (the rule-6 conformance check lives here), token auth on
//! WebSocket, peer-cred UDS flow, and event sequencing.

use std::path::PathBuf;
use std::time::Duration;

use compos_core::{CommandRegistry, Vault};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::Message;

async fn start_server() -> (compos_rpc::RpcHandle, tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let vault = Vault::init(&root).unwrap();
    let config = compos_rpc::RpcConfig {
        socket_path: dir.path().join("d.sock"),
        ws_addr: "127.0.0.1:0".parse().unwrap(),
        token_file: root.join("state").join("rpc-token"),
    };
    let handle = compos_rpc::start(vault, CommandRegistry::with_builtins(), config)
        .await
        .unwrap();
    (handle, dir, root)
}

struct UdsClient {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
}

impl UdsClient {
    async fn connect(path: &std::path::Path) -> Self {
        let stream = UnixStream::connect(path).await.unwrap();
        let (read, write) = stream.into_split();
        Self {
            lines: BufReader::new(read).lines(),
            write,
            next_id: 0,
        }
    }

    /// Send one request and read frames until its response arrives
    /// (server-pushed notifications may interleave and are discarded).
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write
            .write_all(format!("{req}\n").as_bytes())
            .await
            .unwrap();
        loop {
            let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
                .await
                .expect("response deadline")
                .unwrap()
                .expect("connection open");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    /// Read frames until a server-pushed `event` notification arrives.
    async fn next_event(&mut self) -> Value {
        loop {
            let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
                .await
                .expect("event deadline")
                .unwrap()
                .expect("connection open");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["method"] == "event" {
                return v["params"].clone();
            }
        }
    }
}

fn err_type(resp: &Value) -> &str {
    resp["error"]["data"]["type"].as_str().unwrap_or("")
}

#[tokio::test]
async fn uds_hello_save_and_typed_errors() {
    let (handle, _dir, _root) = start_server().await;
    let mut c = UdsClient::connect(&handle.socket_path).await;

    let hello = c.call("hello", json!({"role": "shell"})).await;
    assert_eq!(hello["result"]["protocol"], 1);
    assert_eq!(hello["result"]["role_granted"], "shell");
    assert_eq!(hello["result"]["capabilities"][0], "effect:commit");

    let list = c.call("commands.list", json!({})).await;
    let ids: Vec<&str> = list["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"document.save"));

    let desc = c
        .call("commands.describe", json!({"command": "document.save"}))
        .await;
    assert_eq!(desc["result"]["command"]["effect"], "commit");

    let saved = c
        .call(
            "commands.invoke",
            json!({"command": "document.save",
                   "input": {"path": "a.md", "content": "hello rpc\n"}}),
        )
        .await;
    assert!(saved["result"]["rev"].as_str().unwrap().starts_with("r_"));

    // Stale save → STALE_BASE with its §6 code.
    let stale = c
        .call(
            "commands.invoke",
            json!({"command": "document.save",
                   "input": {"path": "a.md", "content": "clobber?\n"}}),
        )
        .await;
    assert_eq!(stale["error"]["code"], 1001);
    assert_eq!(err_type(&stale), "STALE_BASE");

    // Bad input → VALIDATION_FAILED; unknown command → COMMAND_UNKNOWN;
    // unknown method → METHOD_NOT_FOUND; phantom job → JOB_UNKNOWN.
    let bad = c
        .call(
            "commands.invoke",
            json!({"command": "document.save", "input": {"path": "a.md"}}),
        )
        .await;
    assert_eq!(err_type(&bad), "VALIDATION_FAILED");
    let unknown = c
        .call(
            "commands.invoke",
            json!({"command": "no.such", "input": {}}),
        )
        .await;
    assert_eq!(err_type(&unknown), "COMMAND_UNKNOWN");
    let method = c.call("bogus.method", json!({})).await;
    assert_eq!(method["error"]["code"], -32601);
    let job = c.call("jobs.cancel", json!({"job_id": "j_1"})).await;
    assert_eq!(err_type(&job), "JOB_UNKNOWN");
}

#[tokio::test]
async fn agent_role_is_capped_at_propose() {
    // Constitutional rule 6, enforced at the boundary: agent connections
    // can read, but every commit-effect command is refused before dispatch.
    let (handle, _dir, _root) = start_server().await;

    let mut shell = UdsClient::connect(&handle.socket_path).await;
    shell.call("hello", json!({"role": "shell"})).await;
    shell
        .call(
            "commands.invoke",
            json!({"command": "document.save",
                   "input": {"path": "a.md", "content": "seed\n"}}),
        )
        .await;

    let mut agent = UdsClient::connect(&handle.socket_path).await;
    let hello = agent.call("hello", json!({"role": "agent"})).await;
    assert_eq!(hello["result"]["capabilities"][0], "effect:propose");

    let read = agent
        .call(
            "commands.invoke",
            json!({"command": "document.read", "input": {"path": "a.md"}}),
        )
        .await;
    assert_eq!(read["result"]["content"], "seed\n");

    for denied_cmd in ["document.save", "vault.scan", "system.health.inspect"] {
        let denied = agent
            .call(
                "commands.invoke",
                json!({"command": denied_cmd,
                       "input": {"path": "a.md", "content": "x"}}),
            )
            .await;
        assert_eq!(denied["error"]["code"], 1003, "{denied_cmd} must be denied");
        assert_eq!(err_type(&denied), "CAPABILITY_DENIED");
    }

    // The denied save left no trace.
    let hist = agent
        .call(
            "commands.invoke",
            json!({"command": "document.history", "input": {"path": "a.md"}}),
        )
        .await;
    assert_eq!(hist["result"]["revisions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn no_hello_no_service() {
    let (handle, _dir, _root) = start_server().await;
    let mut c = UdsClient::connect(&handle.socket_path).await;
    let resp = c
        .call(
            "commands.invoke",
            json!({"command": "vault.status", "input": {}}),
        )
        .await;
    assert_eq!(err_type(&resp), "CAPABILITY_DENIED");
}

#[tokio::test]
async fn events_carry_topic_sequence() {
    let (handle, _dir, _root) = start_server().await;

    let mut watcher = UdsClient::connect(&handle.socket_path).await;
    watcher.call("hello", json!({"role": "shell"})).await;
    let sub = watcher
        .call(
            "events.subscribe",
            json!({"topics": ["revision.committed"]}),
        )
        .await;
    assert_eq!(sub["result"]["subscribed"][0]["seq"], 0);

    // Unknown topics are a loud failure, not a silent no-op.
    let bad = watcher
        .call("events.subscribe", json!({"topics": ["nope.topic"]}))
        .await;
    assert_eq!(err_type(&bad), "VALIDATION_FAILED");

    let mut saver = UdsClient::connect(&handle.socket_path).await;
    saver.call("hello", json!({"role": "shell"})).await;
    saver
        .call(
            "commands.invoke",
            json!({"command": "document.save",
                   "input": {"path": "n.md", "content": "event me\n"}}),
        )
        .await;

    let ev = watcher.next_event().await;
    assert_eq!(ev["topic"], "revision.committed");
    assert_eq!(ev["seq"], 1);
    assert_eq!(ev["payload"]["path"], "n.md");
    assert_eq!(ev["payload"]["origin"], "editor");
}

#[tokio::test]
async fn websocket_requires_token_and_path() {
    let (handle, _dir, root) = start_server().await;
    let url = format!("ws://{}/rpc", handle.ws_addr);

    // Wrong path refused at handshake.
    let bad_path = format!("ws://{}/other", handle.ws_addr);
    assert!(tokio_tungstenite::connect_async(&bad_path).await.is_err());

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Bad token → CAPABILITY_DENIED.
    ws.send(Message::Text(
        json!({"jsonrpc": "2.0", "id": 1, "method": "hello",
               "params": {"role": "shell", "token": "wrong"}})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let resp: Value = match ws.next().await.unwrap().unwrap() {
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
        other => panic!("unexpected frame {other:?}"),
    };
    assert_eq!(resp["error"]["data"]["type"], "CAPABILITY_DENIED");

    // The real token comes from the 0600 file the daemon wrote.
    let token_path = root.join("state").join("rpc-token");
    let token = std::fs::read_to_string(&token_path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "token file must be 0600");

    ws.send(Message::Text(
        json!({"jsonrpc": "2.0", "id": 2, "method": "hello",
               "params": {"role": "shell", "token": token}})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let resp: Value = match ws.next().await.unwrap().unwrap() {
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
        other => panic!("unexpected frame {other:?}"),
    };
    assert_eq!(resp["result"]["role_granted"], "shell");

    // Same payloads as UDS: a save works over WebSocket.
    ws.send(Message::Text(
        json!({"jsonrpc": "2.0", "id": 3, "method": "commands.invoke",
               "params": {"command": "document.save",
                          "input": {"path": "ws.md", "content": "via websocket\n"}}})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let resp: Value = match ws.next().await.unwrap().unwrap() {
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
        other => panic!("unexpected frame {other:?}"),
    };
    assert_eq!(resp["result"]["path"], "ws.md");
}
