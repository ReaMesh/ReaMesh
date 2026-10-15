//! Localhost control server for the ReaMesh app.
//!
//! A tiny WebSocket server bound to 127.0.0.1 that outlives any session: the
//! app connects to it to remote-control the extension and mirror
//! its state. The C++ extension remains authoritative. Commands from the app
//! are queued here and drained by the extension's timer tick through
//! `reamesh_ctl_poll`, and the extension pushes its composed state JSON back
//! out through `reamesh_ctl_publish`, which this server broadcasts to every
//! connected app. Nothing here ever touches REAPER or the session directly.
//!
//! The only command handled server-side is `set_profile` (display name +
//! color): it updates the process-wide profile used by presence beacons, so
//! it works even while no session handle exists.
//!
//! Multiple REAPER instances on one machine (the documented dev setup) each
//! bind the first free port in `CTL_PORTS`; the app scans the whole range and
//! offers an instance picker when more than one answers.
//!
//! Every connection must present this run's secret: once a port is bound the
//! server writes a fresh random token to `<temp dir>/reamesh-ctl-<port>.token`
//! (owner-only permissions; the temp dir is per-user on macOS and Windows),
//! and a client offers it as the WebSocket subprotocol `reamesh.<token>`,
//! which the server echoes back. The ReaMesh app reads the file through its
//! backend (`ctl_token`). Nothing is accepted from a connection without the
//! right token, whatever its Origin, so neither a web page nor another user's
//! process can drive the extension or read its state; a process running as
//! the same user can read the file, which is the boundary the OS gives us
//! anyway. A wrong token is refused at the handshake. A connection that
//! offers no token at all (an app from before the token, or one that found no
//! token file) only gets the `hello` greeting, marked `"auth": false`, so the
//! app can still tell a version mismatch apart from a missing extension; its
//! commands are dropped and it never sees a state frame.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc as std_mpsc};
use std::time::{Duration, Instant};

use libp2p::futures::{SinkExt, StreamExt};
use serde_json::{Value as JsonValue, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// Ports tried in order; one per running REAPER instance.
pub const CTL_PORTS: [u16; 4] = [47415, 47416, 47417, 47418];
/// Commands queued but not yet drained by the extension tick. Bounded so a
/// stuck extension can't accumulate unbounded input.
const INBOUND_CAP: usize = 64;
/// How long to wait between bind retries when every port was taken. A freed
/// port (a REAPER instance quit, a squatting program closed) is picked up
/// automatically: the extension never has to be reloaded to recover.
const BIND_RETRY: Duration = Duration::from_secs(5);
/// Largest frame/message accepted from an app. Commands are small JSON
/// objects; nothing legitimate comes near this.
const MAX_INBOUND_BYTES: usize = 64 * 1024;
/// Outbound frames buffered per connected app. A client that stops reading
/// (or reads slower than the state changes) is dropped once this fills
/// instead of growing memory without bound.
const OUTBOUND_CAP: usize = 256;
/// Concurrent connections (open sockets, handshaking or upgraded). The app
/// keeps one per window; the rest is headroom for a restart's overlap.
const MAX_CLIENTS: usize = 16;
/// A handshake that hasn't completed by then is dropped, so a socket that
/// never speaks can't hold a connection slot.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Subprotocol prefix a client puts in front of the run's token.
const TOKEN_PROTOCOL_PREFIX: &str = "reamesh.";

/// Marker for a loaded extension that could not bind any control port
/// (47415-47418). Refresh on each failed attempt and remove after binding.
/// The app ignores stale markers so crashes cannot leave a permanent warning.
fn noport_marker_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("reamesh-noport-{}.json", std::process::id()))
}

fn write_noport_marker() {
    let body = json!({
        "pid": std::process::id(),
        "ports": CTL_PORTS,
    })
    .to_string();
    if let Err(e) = std::fs::write(noport_marker_path(), body) {
        eprintln!("[reamesh] ctl: could not write no-port marker: {e}");
    }
}

fn clear_noport_marker() {
    let _ = std::fs::remove_file(noport_marker_path());
}

/// Where the token of the server on `port` lives. The ReaMesh app's backend
/// reads the same path (`ctl_token` in app/src-tauri/src/lib.rs).
pub fn token_path(port: u16) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("reamesh-ctl-{port}.token"))
}

/// A fresh 256-bit token as hex, or None when the OS RNG is unavailable (the
/// server then refuses every connection rather than use a guessable secret).
fn new_token() -> Option<String> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).ok()?;
    Some(crate::hex_encode(&buf))
}

/// Write the token file readable by this user only. Written to a fresh
/// sibling (create_new, so a planted file or symlink is never followed) and
/// renamed over the final name, so a reader never sees a partial token and a
/// previous run's file is replaced atomically.
fn write_token_file(port: u16, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    let path = token_path(port);
    let tmp = path.with_extension(format!("token.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    owner_only(&mut opts);
    let mut f = opts.open(&tmp)?;
    f.write_all(token.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Owner-only permissions where the platform has mode bits. On Windows the
/// file inherits the per-user temp folder's ACL (the user, SYSTEM and
/// administrators), which is the equivalent.
#[cfg(unix)]
fn owner_only(opts: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    opts.mode(0o600);
}
#[cfg(not(unix))]
fn owner_only(_opts: &mut std::fs::OpenOptions) {}

/// The `reamesh.<token>` entry of a `Sec-WebSocket-Protocol` header (a comma
/// separated list) when it carries `expected`, compared in constant time.
fn offered_token_protocol<'a>(header: Option<&'a str>, expected: &str) -> Option<&'a str> {
    header?.split(',').map(str::trim).find(|p| {
        p.strip_prefix(TOKEN_PROTOCOL_PREFIX)
            .is_some_and(|t| crate::ct_eq(t.as_bytes(), expected.as_bytes()))
    })
}

/// Allowed WebSocket origins for the ReaMesh app and development server.
/// Browser JavaScript cannot forge Origin; checking it keeps websites from
/// reaching the control server at all. The run token is what authenticates
/// (see the module docs): it is required from every client, with or without
/// an Origin, so this list is a second fence rather than the gate. That is
/// why the dev-server origins can stay: a page on localhost:1420/1421 that
/// isn't the ReaMesh app has no way to read the token file. Local non-browser
/// clients (the app backend's quit-time `leave`, tests) send no Origin and
/// pass here, then need the token like everyone else.
///
/// Include http://tauri.localhost for Windows, where Tauri uses HTTP unless
/// useHttpsScheme is enabled.
fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => matches!(
            o,
            "tauri://localhost"          // macOS / Linux bundled app
                | "https://tauri.localhost" // Windows bundled app, useHttpsScheme = true
                | "http://tauri.localhost"  // Windows bundled app, Tauri v2 default
                | "http://localhost:1420"   // `tauri dev` / `bun run dev:a`
                | "http://127.0.0.1:1420"
                | "http://localhost:1421"   // `bun run dev:b` (second dev instance)
                | "http://127.0.0.1:1421"
        ),
    }
}

struct CtlShared {
    inbound: Mutex<VecDeque<String>>,
    clients: Mutex<Vec<ClientTx>>,
    /// Last state published by the extension; replayed to newly-connected
    /// apps so they render immediately instead of waiting for the next tick.
    last_state: Mutex<String>,
    /// Last time the extension's timer tick beat (`heartbeat`). REAPER's main
    /// thread stops ticking while a modal dialog waits for an answer (or a
    /// long blocking call runs); this server thread keeps running, so it can
    /// tell the app "REAPER isn't responding": see the monitor task.
    last_beat: Mutex<Option<Instant>>,
    port: AtomicU16,
    /// This run's control secret; None until a port is bound (or when the OS
    /// RNG failed, which leaves the server refusing everyone).
    token: Mutex<Option<String>>,
    /// Open sockets, for `MAX_CLIENTS`.
    open: AtomicUsize,
}

type ClientTx = tokio::sync::mpsc::Sender<String>;

/// Queue `msg` for every client; a client whose buffer is full or whose
/// connection ended is dropped (its socket task then sees the channel close).
fn broadcast(clients: &Mutex<Vec<ClientTx>>, msg: &str) {
    clients
        .lock()
        .unwrap()
        .retain(|tx| tx.try_send(msg.to_string()).is_ok());
}

static CTL: OnceLock<Arc<CtlShared>> = OnceLock::new();

/// Start the server (idempotent). Returns the bound port, 0 on failure.
pub fn start() -> u16 {
    // Two concurrent first calls must not start two servers.
    static STARTING: Mutex<()> = Mutex::new(());
    let _guard = STARTING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ctl) = CTL.get() {
        return ctl.port.load(Ordering::Relaxed);
    }
    let shared = Arc::new(CtlShared {
        inbound: Mutex::new(VecDeque::new()),
        clients: Mutex::new(Vec::new()),
        last_state: Mutex::new(String::new()),
        last_beat: Mutex::new(None),
        port: AtomicU16::new(0),
        token: Mutex::new(None),
        open: AtomicUsize::new(0),
    });
    let (port_tx, port_rx) = std_mpsc::channel();
    {
        let shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("reamesh-ctl".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("[reamesh] ctl runtime failed: {e}");
                        let _ = port_tx.send(0u16);
                        return;
                    }
                };
                rt.block_on(async move {
                    // Keep trying until a port frees up. Another REAPER or
                    // process may be using all four ports temporarily.
                    // A marker file tells the app what is going on meanwhile.
                    let mut reported = false;
                    let (listener, port) = loop {
                        let mut bound = None;
                        for port in CTL_PORTS {
                            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                                Ok(l) => {
                                    bound = Some((l, port));
                                    break;
                                }
                                Err(_) => continue,
                            }
                        }
                        if let Some(b) = bound {
                            break b;
                        }
                        eprintln!(
                            "[reamesh] ctl server: no free port in {CTL_PORTS:?}; retrying"
                        );
                        write_noport_marker();
                        if !reported {
                            reported = true;
                            let _ = port_tx.send(0u16);
                        }
                        tokio::time::sleep(BIND_RETRY).await;
                    };
                    clear_noport_marker();
                    match new_token() {
                        Some(token) => match write_token_file(port, &token) {
                            Ok(()) => *shared.token.lock().unwrap() = Some(token),
                            Err(e) => eprintln!(
                                "[reamesh] ctl: could not write the control token: {e}"
                            ),
                        },
                        None => {
                            eprintln!("[reamesh] ctl: no OS randomness for the control token")
                        }
                    }
                    shared.port.store(port, Ordering::Relaxed);
                    if !reported {
                        let _ = port_tx.send(port);
                    }
                    eprintln!("[reamesh] ctl server on 127.0.0.1:{port}");
                    // Main-thread watchdog: while the extension's timer beats,
                    // stay silent; once it goes quiet (a modal REAPER dialog,
                    // a long blocking call) tell every app, and repeat each
                    // second while it lasts so late-connecting apps hear it
                    // too. One `busy:false` marks recovery.
                    {
                        let shared = shared.clone();
                        tokio::spawn(async move {
                            let mut busy = false;
                            let mut iv = tokio::time::interval(Duration::from_secs(1));
                            loop {
                                iv.tick().await;
                                let stale = shared
                                    .last_beat
                                    .lock()
                                    .unwrap()
                                    .is_some_and(|t| t.elapsed() > Duration::from_secs(3));
                                if stale || busy != stale {
                                    busy = stale;
                                    let msg =
                                        json!({"type": "busy", "busy": busy}).to_string();
                                    broadcast(&shared.clients, &msg);
                                }
                            }
                        });
                    }
                    accept_loop(listener, shared).await;
                });
            });
        if spawned.is_err() {
            return 0;
        }
    }
    let port = port_rx.recv_timeout(Duration::from_secs(3)).unwrap_or(0);
    // Register the shared state even when no port bound yet: the server thread
    // keeps retrying, and heartbeat/publish/poll must be live the moment a
    // port frees up (they are harmless no-op buffers meanwhile).
    let _ = CTL.set(shared);
    port
}

/// Next queued command from a connected app as JSON, or None.
pub fn poll() -> Option<String> {
    CTL.get()?.inbound.lock().unwrap().pop_front()
}

/// Mark the extension's timer tick alive. Called every tick from REAPER's
/// main thread; the watchdog task reports "busy" to the apps when this stops.
pub fn heartbeat() {
    if let Some(ctl) = CTL.get() {
        *ctl.last_beat.lock().unwrap() = Some(Instant::now());
    }
}

/// Broadcast the extension's state JSON to every connected app and remember
/// it for late joiners.
pub fn publish(state: &str) {
    let Some(ctl) = CTL.get() else { return };
    *ctl.last_state.lock().unwrap() = state.to_string();
    broadcast(&ctl.clients, state);
}

/// Releases a `MAX_CLIENTS` slot when a connection task ends, however it ends.
struct Slot(Arc<CtlShared>);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn accept_loop(listener: tokio::net::TcpListener, shared: Arc<CtlShared>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        if shared.open.fetch_add(1, Ordering::Relaxed) >= MAX_CLIENTS {
            shared.open.fetch_sub(1, Ordering::Relaxed);
            drop(stream);
            continue;
        }
        let slot = Slot(shared.clone());
        let shared = shared.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let token = shared.token.lock().unwrap().clone();
            let authed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let authed_in = authed.clone();
            // Refuse cross-origin handshakes (CSWSH) and a wrong token before
            // upgrading; note whether the right one came along.
            let check = move |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
                let refuse = |why: &str| {
                    let mut err = ErrorResponse::new(Some(why.to_string()));
                    *err.status_mut() = StatusCode::FORBIDDEN;
                    err
                };
                let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
                if !origin_allowed(origin) {
                    eprintln!("[reamesh] ctl: rejected connection from origin {origin:?}");
                    return Err(refuse("origin not allowed"));
                }
                let offered = req
                    .headers()
                    .get("sec-websocket-protocol")
                    .and_then(|v| v.to_str().ok());
                let offers_token = offered.is_some_and(|h| {
                    h.split(',').any(|p| p.trim().starts_with(TOKEN_PROTOCOL_PREFIX))
                });
                if !offers_token {
                    // Greeting only (see the module docs).
                    return Ok(resp);
                }
                let Some(proto) = token
                    .as_deref()
                    .and_then(|t| offered_token_protocol(offered, t))
                else {
                    eprintln!("[reamesh] ctl: rejected connection with a wrong token");
                    return Err(refuse("unauthorized"));
                };
                // Browsers fail a handshake whose offered subprotocol isn't
                // echoed back.
                let Ok(value) = HeaderValue::from_str(proto) else {
                    return Err(refuse("unauthorized"));
                };
                resp.headers_mut().insert("sec-websocket-protocol", value);
                authed_in.store(true, Ordering::Relaxed);
                Ok(resp)
            };
            let config = WebSocketConfig::default()
                .max_message_size(Some(MAX_INBOUND_BYTES))
                .max_frame_size(Some(MAX_INBOUND_BYTES));
            let Ok(Ok(ws)) = tokio::time::timeout(
                HANDSHAKE_TIMEOUT,
                tokio_tungstenite::accept_hdr_async_with_config(stream, check, Some(config)),
            )
            .await
            else {
                return;
            };
            let (mut ws_tx, mut ws_rx) = ws.split();
            let authed = authed.load(Ordering::Relaxed);
            // Greet (with this build's version + PROTO, which the app checks
            // against its own before driving anything), then replay the last
            // known state so the app paints now.
            let hello = json!({
                "type": "hello",
                "pid": std::process::id(),
                "version": crate::VERSION,
                "proto": crate::PROTO,
                "auth": authed,
            })
            .to_string();
            if !authed {
                // Greeting only: everything it sends is ignored until it
                // goes away.
                if ws_tx.send(Message::Text(hello.into())).await.is_err() {
                    return;
                }
                while let Some(Ok(msg)) = ws_rx.next().await {
                    if matches!(msg, Message::Close(_)) {
                        break;
                    }
                }
                return;
            }
            let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(OUTBOUND_CAP);
            let _ = tx.try_send(hello);
            {
                let last = shared.last_state.lock().unwrap().clone();
                if !last.is_empty() {
                    let _ = tx.try_send(last);
                }
            }
            shared.clients.lock().unwrap().push(tx);
            loop {
                tokio::select! {
                    out = rx.recv() => match out {
                        Some(s) => {
                            if ws_tx.send(Message::Text(s.into())).await.is_err() {
                                break;
                            }
                        }
                        // Dropped from the client list (its buffer filled up).
                        None => break,
                    },
                    inbound = ws_rx.next() => match inbound {
                        Some(Ok(Message::Text(t))) => handle_inbound(&shared, t.to_string()),
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        Some(Ok(_)) => {} // binary/ping/pong: ignored
                    },
                }
            }
            // Dead senders are pruned on the next publish.
        });
    }
}

fn handle_inbound(shared: &CtlShared, msg: String) {
    // set_profile is the one command resolved here: the profile is global and
    // must be settable before any session exists.
    if let Ok(v) = serde_json::from_str::<JsonValue>(&msg)
        && v.get("cmd").and_then(|c| c.as_str()) == Some("set_profile")
    {
        let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        crate::set_profile(get("name"), get("color"));
        return;
    }
    let mut q = shared.inbound.lock().unwrap();
    if q.len() < INBOUND_CAP {
        q.push_back(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::{offered_token_protocol, origin_allowed};

    #[test]
    fn token_protocol_must_match_exactly() {
        let t = "ab12";
        assert_eq!(offered_token_protocol(Some("reamesh.ab12"), t), Some("reamesh.ab12"));
        assert_eq!(
            offered_token_protocol(Some("chat, reamesh.ab12"), t),
            Some("reamesh.ab12")
        );
        assert_eq!(offered_token_protocol(None, t), None);
        assert_eq!(offered_token_protocol(Some("reamesh.ab13"), t), None);
        assert_eq!(offered_token_protocol(Some("reamesh.ab1"), t), None);
        assert_eq!(offered_token_protocol(Some("reamesh."), t), None);
        assert_eq!(offered_token_protocol(Some("ab12"), t), None);
    }

    #[test]
    fn allows_app_and_devserver_origins() {
        assert!(origin_allowed(None)); // non-browser client
        assert!(origin_allowed(Some("tauri://localhost")));
        assert!(origin_allowed(Some("https://tauri.localhost")));
        assert!(origin_allowed(Some("http://tauri.localhost")));
        assert!(origin_allowed(Some("http://localhost:1420")));
        assert!(origin_allowed(Some("http://localhost:1421")));
        assert!(origin_allowed(Some("http://127.0.0.1:1421")));
    }

    #[test]
    fn rejects_cross_site_origins() {
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("http://evil.example")));
        assert!(!origin_allowed(Some("null")));
        assert!(!origin_allowed(Some("http://localhost:1422")));
    }
}
