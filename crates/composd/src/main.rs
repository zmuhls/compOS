//! composd: the canonical document authority daemon (ARCHITECTURE.md §5–§6).
//! Opens the vault write-mode (taking the exclusive lock — rule 1), serves
//! the JSON-RPC boundary on UDS + WebSocket, and watches `vault/` so
//! out-of-band edits become `external` revisions promptly.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use compos_core::{CommandRegistry, HostProfile, Vault, VaultError};
use compos_rpc::AppState;
use notify::Watcher;
use serde_json::json;

#[derive(Parser)]
#[command(name = "composd", version, about = "CompOS document authority daemon")]
struct Cli {
    /// Vault root. Defaults to $COMPOS_VAULT, then the host profile location.
    #[arg(long)]
    vault: Option<PathBuf>,

    /// WebSocket port for the shell (0 = OS-assigned).
    #[arg(long, default_value_t = 7411)]
    ws_port: u16,

    /// Unix socket path override.
    #[arg(long)]
    socket: Option<PathBuf>,

    /// Run the phase-0 self-check and exit.
    #[arg(long)]
    self_check: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    println!(
        "composd {} (vault format {})",
        compos_core::VERSION,
        compos_core::VAULT_FORMAT
    );
    if cli.self_check {
        return self_check();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let root = match cli.vault {
        Some(v) => v,
        None => match std::env::var_os("COMPOS_VAULT") {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => HostProfile::resolve()
                .default_vault_root()
                .context("cannot determine a default vault root; pass --vault")?,
        },
    };

    let vault = match Vault::open_write(&root) {
        Ok(v) => v,
        Err(VaultError::VaultBusy) => {
            anyhow::bail!("vault at {} is held by another process", root.display())
        }
        Err(VaultError::NotAVault(_)) => {
            anyhow::bail!(
                "no vault at {} — create one with `composctl --vault <path> init`",
                root.display()
            )
        }
        Err(e) => return Err(e.into()),
    };
    for w in vault.warnings() {
        eprintln!("warning: {w}");
    }
    let vault_dir = root.join("vault");

    let config = compos_rpc::RpcConfig {
        socket_path: cli
            .socket
            .or_else(default_socket_path)
            .context("cannot determine a default socket path; pass --socket")?,
        ws_addr: format!("127.0.0.1:{}", cli.ws_port).parse()?,
        token_file: root.join("state").join("rpc-token"),
    };
    let token_file = config.token_file.clone();
    let mut handle = compos_rpc::start(vault, CommandRegistry::with_builtins(), config)
        .await
        .context("starting rpc listeners")?;

    let _watcher = spawn_watcher(handle.state.clone(), vault_dir)?;

    println!("listening");
    println!("  uds:   {}", handle.socket_path.display());
    println!("  ws:    ws://{}/rpc", handle.ws_addr);
    println!("  token: {}", token_file.display());
    std::io::stdout().flush().ok();

    tokio::signal::ctrl_c().await?;
    println!("shutting down");
    handle.shutdown();
    Ok(())
}

/// §6 socket placement: `$XDG_RUNTIME_DIR/compos/composd.sock` on Linux, the
/// app-support equivalent on macOS. Kept short — sun_path is ~104 bytes.
fn default_socket_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("compos")
                .join("composd.sock"),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
            && !dir.is_empty()
        {
            return Some(PathBuf::from(dir).join("compos").join("composd.sock"));
        }
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("compos")
                .join("composd.sock"),
        )
    }
}

/// Watch `vault/` and convert out-of-band edits into `external` revisions.
/// Bursts are coalesced with a quiet window; the scan itself is a no-op for
/// composd's own writes (the bytes already match the heads), so the daemon
/// never feeds back into itself.
fn spawn_watcher(
    state: Arc<AppState>,
    vault_dir: PathBuf,
) -> anyhow::Result<notify::RecommendedWatcher> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })?;
    watcher.watch(&vault_dir, notify::RecursiveMode::Recursive)?;

    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            // Coalesce the burst until 300 ms of quiet.
            while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
            let scan = {
                let mut vault = state.vault.lock().unwrap();
                vault.scan_external()
            };
            match scan {
                Ok(scan) => {
                    for out in &scan.converted {
                        let payload = json!({
                            "doc": out.doc, "path": out.path,
                            "rev": out.rev, "object": out.object,
                        });
                        state.events.publish("doc.external_change", payload.clone());
                        let mut with_origin = payload;
                        with_origin["origin"] = json!("external");
                        state.events.publish("revision.committed", with_origin);
                        compos_rpc::publish_stale_proposals(&state, out.doc.as_str(), &out.path);
                        println!("external revision: {} -> {}", out.path, out.rev);
                    }
                    for path in &scan.missing {
                        eprintln!("warning: '{path}' has revisions but no visible file");
                    }
                    std::io::stdout().flush().ok();
                }
                Err(e) => eprintln!("watcher scan failed: {e}"),
            }
        }
    });
    Ok(watcher)
}

/// The Phase-0 exit gate, kept honest: basic file I/O in an unpredictable
/// temp dir, then exit 0.
fn self_check() -> anyhow::Result<()> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("composd-selfcheck-{}-{nonce}", std::process::id()));
    // create_dir (not create_dir_all): fails if the path already exists, so a
    // pre-created directory or symlink at this path aborts the self-check
    // instead of being followed.
    std::fs::create_dir(&dir)?;
    let probe = dir.join("probe.md");
    std::fs::write(&probe, b"self-check")?;
    let read_back = std::fs::read(&probe)?;
    anyhow::ensure!(read_back == b"self-check", "self-check read-back mismatch");
    std::fs::remove_dir_all(&dir)?;
    println!("self-check ok");
    Ok(())
}
