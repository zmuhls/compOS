//! End-to-end daemon smoke: composd serves the RPC boundary over its UDS,
//! and the notify watcher converts an out-of-band edit into an `external`
//! revision while the daemon runs.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn daemon_serves_rpc_and_watches_vault() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("v");
    let socket = dir.path().join("d.sock");

    // Pre-create the vault with the CLI creation path (init then release).
    let status = Command::new(env!("CARGO_BIN_EXE_composd"))
        .arg("--self-check")
        .status()
        .unwrap();
    assert!(status.success(), "self-check must pass");
    compos_core_init(&root);

    let mut child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_composd"))
            .arg("--vault")
            .arg(&root)
            .arg("--ws-port")
            .arg("0")
            .arg("--socket")
            .arg(&socket)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn composd"),
    );

    // Wait for the daemon to report readiness.
    let stdout = child.0.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut line = String::new();
    loop {
        assert!(Instant::now() < deadline, "daemon never became ready");
        line.clear();
        assert!(reader.read_line(&mut line).unwrap() > 0, "stdout closed");
        if line.trim() == "listening" {
            break;
        }
    }

    // Speak JSON-RPC over the UDS.
    let mut stream = UnixStream::connect(&socket).expect("connect uds");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut rpc_reader = BufReader::new(stream.try_clone().unwrap());
    let mut call = |id: u64, method: &str, params: Value| -> Value {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(stream, "{req}").unwrap();
        let mut resp = String::new();
        loop {
            resp.clear();
            rpc_reader.read_line(&mut resp).unwrap();
            let v: Value = serde_json::from_str(&resp).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    };

    let hello = call(1, "hello", json!({"role": "shell"}));
    assert_eq!(hello["result"]["role_granted"], "shell");

    let saved = call(
        2,
        "commands.invoke",
        json!({"command": "document.save",
               "input": {"path": "via-rpc.md", "content": "daemon save\n"}}),
    );
    assert!(saved["result"]["rev"].as_str().unwrap().starts_with("r_"));
    assert_eq!(
        std::fs::read(root.join("vault/via-rpc.md")).unwrap(),
        b"daemon save\n"
    );

    // Out-of-band edit: the watcher must journal it as `external`.
    std::fs::write(root.join("vault/dropped.md"), b"external bytes\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let hist = call(
            3,
            "commands.invoke",
            json!({"command": "document.history", "input": {"path": "dropped.md"}}),
        );
        if hist["result"]["revisions"]
            .as_array()
            .is_some_and(|revs| revs.iter().any(|r| r["origin"] == "external"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "watcher never converted the external edit: {hist}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Create a vault via the public library API, then drop it so the daemon
/// can take the exclusive lock.
fn compos_core_init(root: &std::path::Path) {
    // composd links compos-core; reuse it rather than shelling out to
    // composctl (which is a different crate's binary).
    let vault = compos_core::Vault::init(root).unwrap();
    drop(vault);
}
