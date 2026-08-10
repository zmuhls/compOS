//! Phase 0 hello-world: report version, self-check basic file I/O in a
//! temporary directory, exit 0. The daemon (compos-rpc listeners) arrives
//! later in Phase 1.

use std::fs;

fn main() -> anyhow::Result<()> {
    println!(
        "composd {} (vault format {})",
        compos_core::VERSION,
        compos_core::VAULT_FORMAT
    );

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("composd-selfcheck-{}-{nonce}", std::process::id()));
    // create_dir (not create_dir_all): fails if the path already exists, so a
    // pre-created directory or symlink at this path aborts the self-check
    // instead of being followed.
    fs::create_dir(&dir)?;
    let probe = dir.join("probe.md");
    fs::write(&probe, b"self-check")?;
    let read_back = fs::read(&probe)?;
    anyhow::ensure!(read_back == b"self-check", "self-check read-back mismatch");
    fs::remove_dir_all(&dir)?;

    println!("self-check ok");
    Ok(())
}
