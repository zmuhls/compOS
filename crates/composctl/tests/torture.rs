//! The power-loss torture gate (ARCHITECTURE.md §16, Phase 1): kill -9 a
//! child performing randomized saves, then verify that every acknowledged
//! write survived, chains are intact, and reconciliation left no debris.
//!
//! Iterations: `TORTURE_ITERS` env var, default 100 (CI-tolerable). Run
//! `TORTURE_ITERS=1000 cargo test -p composctl --test torture` before
//! declaring a milestone done. `TORTURE_DOCS` (default 8) widens the
//! synthetic vault — the Phase-1 exit gate runs it at 500.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use compos_core::{DocRef, ObjectHash, RevisionId, RevisionOrigin, SaveRequest, Vault};

type Ack = (String, String, String); // path, rev, object

#[test]
fn kill9_save_torture() {
    let iters: u32 = std::env::var("TORTURE_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let docs: u32 = std::env::var("TORTURE_DOCS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");

    // Seed the synthetic vault so every document the children will touch
    // exists up front — the Phase-1 exit gate tortures a full 500-document
    // vault, not an empty one that happens to allow 500 paths.
    {
        let mut vault = Vault::init(&root).expect("init synthetic vault");
        for di in 0..docs {
            vault
                .writer()
                .unwrap()
                .save(SaveRequest {
                    doc: DocRef::Path(format!("notes/doc-{di}.md")),
                    base: None,
                    content: format!("# doc {di}\nseed\n").into_bytes(),
                    origin: RevisionOrigin::Editor,
                    lease: None,
                })
                .expect("seed save");
        }
    }

    let mut all_acks: Vec<Ack> = Vec::new();
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        | 1;

    for iter in 0..iters {
        let mut child = Command::new(env!("CARGO_BIN_EXE_composctl"))
            .arg("--vault")
            .arg(&root)
            .arg("_torture-child")
            .arg("--docs")
            .arg(docs.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn torture child");

        let stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut acks = Vec::new();
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(rest) = line.strip_prefix("ACK ") {
                    let parts: Vec<&str> = rest.split(' ').collect();
                    if parts.len() == 3 {
                        acks.push((
                            parts[0].to_owned(),
                            parts[1].to_owned(),
                            parts[2].to_owned(),
                        ));
                    }
                }
            }
            acks
        });

        // 0–50 ms, biased low so kills land mid-transaction.
        let r = xorshift(&mut seed) % 50_000;
        std::thread::sleep(Duration::from_micros((r * r) / 50_000));

        child.kill().expect("kill -9");
        child.wait().expect("wait");
        all_acks.extend(reader.join().unwrap());

        if let Err(msg) = verify(&root, &all_acks, docs) {
            let kept = dir.keep();
            panic!(
                "torture iteration {iter} failed: {msg}\nvault kept for autopsy at {}",
                kept.display()
            );
        }
    }

    println!(
        "torture: {iters} kills survived, {} acknowledged saves verified",
        all_acks.len()
    );
}

/// Open the vault (running tail repair + reconciliation) and check every
/// invariant the save transaction promises.
fn verify(root: &Path, acks: &[Ack], docs: u32) -> Result<(), String> {
    let vault = Vault::open_write(root).map_err(|e| format!("open_write failed: {e}"))?;

    // Every seeded document must still exist — no acknowledged document
    // ever vanishes, whatever the kill did.
    if vault.index().len() != docs as usize {
        return Err(format!(
            "expected {docs} documents, found {}",
            vault.index().len()
        ));
    }

    // Every acknowledged save is in the journal (acked ⊆ journal; a record
    // fsynced but killed before its ack line is durable-but-unacked, legal).
    for (path, rev, object) in acks {
        let rid = RevisionId::from_string(rev.clone());
        if !vault.index().contains_rev(&rid) {
            return Err(format!(
                "acknowledged rev {rev} of {path} missing from journal"
            ));
        }
        if ObjectHash::parse(object).is_none() {
            return Err(format!("malformed acked object hash {object}"));
        }
    }

    // Heads: visible file matches the head object; objects verify.
    for (doc, head) in vault.index().iter_heads() {
        let visible = root.join("vault").join(&head.path);
        let bytes = std::fs::read(&visible)
            .map_err(|e| format!("visible file {} unreadable: {e}", head.path))?;
        if ObjectHash::of(&bytes) != head.object {
            return Err(format!(
                "visible {} does not match head object for {doc}",
                head.path
            ));
        }
        match vault.objects().verify(&head.object) {
            Ok(true) => {}
            Ok(false) => return Err(format!("object corrupt: {}", head.object)),
            Err(e) => return Err(format!("object {} unreadable: {e}", head.object)),
        }
    }

    // Reconciliation left no debris.
    if dir_count(&root.join("tmp")) > 0 {
        return Err("tmp/ not empty after reconciliation".into());
    }
    if dir_count(&root.join("intents")) > 0 {
        return Err("intents/ not empty after reconciliation".into());
    }
    if let Some(stray) = find_tmp_stray(&root.join("vault")) {
        return Err(format!("stray temp file under vault/: {stray}"));
    }
    Ok(())
}

fn dir_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
}

fn find_tmp_stray(dir: &Path) -> Option<String> {
    for entry in std::fs::read_dir(dir).ok()?.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_tmp_stray(&p) {
                return Some(found);
            }
        } else if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".compos-tmp-")
        {
            return Some(p.display().to_string());
        }
    }
    None
}

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}
