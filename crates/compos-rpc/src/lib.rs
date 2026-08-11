//! compos-rpc: the composd API boundary (ARCHITECTURE.md §6, §13).
//!
//! One protocol, two listeners, every profile: newline-delimited JSON-RPC
//! 2.0 over a Unix domain socket for services (peer-cred auth, same-UID
//! convention on dev profiles), and the same payloads over WebSocket at
//! `ws://127.0.0.1:<port>/rpc` for the shell (localhost bind + startup token
//! in a 0600 file). Deliberately a small hand-rolled crate, not a JSON-RPC
//! framework — the dual-listener symmetry and per-role capability caps fight
//! framework assumptions.

pub mod events;
pub mod proto;
pub mod session;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use compos_core::{CommandRegistry, Vault};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use events::{Event, EventBus};
use session::{Session, Transport, event_notification};

/// Shared server state: the single write-mode vault behind a mutex (the
/// in-process face of constitutional rule 1), the command registry, the
/// event bus, and the WebSocket startup token.
pub struct AppState {
    pub vault: Mutex<Vault>,
    pub registry: CommandRegistry,
    pub events: EventBus,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct RpcConfig {
    /// UDS path. Kept short — macOS caps sun_path around 104 bytes.
    pub socket_path: PathBuf,
    /// WebSocket bind address; use port 0 to let the OS choose (tests).
    pub ws_addr: SocketAddr,
    /// Where the 0600 startup token file is written.
    pub token_file: PathBuf,
}

pub struct RpcHandle {
    pub state: Arc<AppState>,
    pub socket_path: PathBuf,
    pub ws_addr: SocketAddr,
    tasks: Vec<JoinHandle<()>>,
}

impl RpcHandle {
    /// Stop accepting and drop listeners. Existing connections are aborted.
    pub fn shutdown(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Drop for RpcHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start both listeners over an already-open write-mode vault.
pub async fn start(
    vault: Vault,
    registry: CommandRegistry,
    config: RpcConfig,
) -> std::io::Result<RpcHandle> {
    let token = generate_token();
    write_token_file(&config.token_file, &token)?;

    let state = Arc::new(AppState {
        vault: Mutex::new(vault),
        registry,
        events: EventBus::new(),
        token,
    });

    // The vault flock guarantees a single composd per vault, so a leftover
    // socket file can only be a crash remnant — safe to clear.
    if let Some(dir) = config.socket_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&config.socket_path);
    let uds = UnixListener::bind(&config.socket_path)?;

    let tcp = TcpListener::bind(config.ws_addr).await?;
    let ws_addr = tcp.local_addr()?;

    let uds_task = tokio::spawn(accept_uds(uds, state.clone()));
    let ws_task = tokio::spawn(accept_ws(tcp, state.clone()));

    Ok(RpcHandle {
        state,
        socket_path: config.socket_path,
        ws_addr,
        tasks: vec![uds_task, ws_task],
    })
}

async fn accept_uds(listener: UnixListener, state: Arc<AppState>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        // Same-UID convention (§6): reject foreign peers at accept.
        let ours = unsafe { libc::getuid() };
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == ours => {}
            _ => continue,
        }
        tokio::spawn(serve_uds(stream, state.clone()));
    }
}

async fn serve_uds(stream: UnixStream, state: Arc<AppState>) {
    let (read, mut write) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let writer = tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if write.write_all(line.as_bytes()).await.is_err()
                || write.write_all(b"\n").await.is_err()
            {
                break;
            }
        }
    });

    let mut session = Session::new(state.clone(), Transport::Uds);
    let mut events = state.events.subscribe();
    let mut lines = BufReader::new(read).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(raw)) => {
                    if let Some(resp) = session.handle(&raw)
                        && out_tx.send(resp).await.is_err()
                    {
                        break;
                    }
                }
                _ => break,
            },
            ev = events.recv() => {
                if !forward_event(&session, ev, &out_tx).await {
                    break;
                }
            }
        }
    }
    writer.abort();
}

async fn accept_ws(listener: TcpListener, state: Arc<AppState>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        tokio::spawn(serve_ws(stream, state.clone()));
    }
}

async fn serve_ws(stream: tokio::net::TcpStream, state: Arc<AppState>) {
    // The shell connects to /rpc; anything else is a client bug. The
    // callback's Err type is dictated by tungstenite's Callback trait, so
    // its size is not ours to shrink.
    #[allow(clippy::result_large_err)]
    let callback =
        |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
         resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
            if req.uri().path() == "/rpc" {
                Ok(resp)
            } else {
                Err(
                    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some(
                        "not found (use /rpc)".into(),
                    )),
                )
            }
        };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
        return;
    };
    let (mut sink, mut source) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut session = Session::new(state.clone(), Transport::WebSocket);
    let mut events = state.events.subscribe();
    loop {
        tokio::select! {
            msg = source.next() => match msg {
                Some(Ok(Message::Text(raw))) => {
                    if let Some(resp) = session.handle(raw.as_str())
                        && out_tx.send(resp).await.is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {} // ping/pong handled by tungstenite
            },
            ev = events.recv() => {
                if !forward_event(&session, ev, &out_tx).await {
                    break;
                }
            }
        }
    }
    writer.abort();
}

async fn forward_event(
    session: &Session,
    ev: Result<Event, broadcast::error::RecvError>,
    out_tx: &mpsc::Sender<String>,
) -> bool {
    match ev {
        Ok(ev) => {
            if session.wants(&ev) {
                return out_tx.send(event_notification(&ev)).await.is_ok();
            }
            true
        }
        // Lagged receivers skip events; clients detect the seq gap and
        // resync — exactly why seq exists (§6).
        Err(broadcast::error::RecvError::Lagged(_)) => true,
        Err(broadcast::error::RecvError::Closed) => false,
    }
}

fn generate_token() -> String {
    // Two UUIDv7s ≈ 148 bits of randomness; ample for a localhost token.
    format!(
        "{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    )
}

fn write_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(token.as_bytes())?;
    Ok(())
}
