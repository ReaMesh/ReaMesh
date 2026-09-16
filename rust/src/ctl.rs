//! Localhost control server for the ReaMesh companion app.
//!
//! A tiny WebSocket server bound to 127.0.0.1 that outlives any session: the
//! companion app connects to it to remote-control the extension and mirror
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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc as std_mpsc};
use std::time::{Duration, Instant};

use libp2p::futures::{SinkExt, StreamExt};
use serde_json::{Value as JsonValue, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

/// Ports tried in order; one per running REAPER instance.
pub const CTL_PORTS: [u16; 4] = [47415, 47416, 47417, 47418];
/// Commands queued but not yet drained by the extension tick. Bounded so a
/// stuck extension can't accumulate unbounded input.
const INBOUND_CAP: usize = 64;
/// How long to wait between bind retries when every port was taken. A freed
/// port (a REAPER instance quit, a squatting program closed) is picked up
/// automatically: the extension never has to be reloaded to recover.
const BIND_RETRY: Duration = Duration::from_secs(5);

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

/// Allowed WebSocket origins for the companion app and development server.
/// Browser JavaScript cannot forge Origin; checking it prevents websites from
/// sending commands to the local control server. Local non-browser clients
/// may omit Origin because they already run as the user.
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
    clients: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<String>>>,
    /// Last state published by the extension; replayed to newly-connected
    /// apps so they render immediately instead of waiting for the next tick.
    last_state: Mutex<String>,
    /// Last time the extension's timer tick beat (`heartbeat`). REAPER's main
    /// thread stops ticking while a modal dialog waits for an answer (or a
    /// long blocking call runs); this server thread keeps running, so it can
    /// tell the app "REAPER isn't responding": see the monitor task.
    last_beat: Mutex<Option<Instant>>,
    port: AtomicU16,
}

static CTL: OnceLock<Arc<CtlShared>> = OnceLock::new();

/// Start the server (idempotent). Returns the bound port, 0 on failure.
pub fn start() -> u16 {
    if let Some(ctl) = CTL.get() {
        return ctl.port.load(Ordering::Relaxed);
    }
    let shared = Arc::new(CtlShared {
        inbound: Mutex::new(VecDeque::new()),
        clients: Mutex::new(Vec::new()),
        last_state: Mutex::new(String::new()),
        last_beat: Mutex::new(None),
        port: AtomicU16::new(0),
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
                                    shared
                                        .clients
                                        .lock()
                                        .unwrap()
                                        .retain(|tx| tx.send(msg.clone()).is_ok());
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
    ctl.clients
        .lock()
        .unwrap()
        .retain(|tx| tx.send(state.to_string()).is_ok());
}

async fn accept_loop(listener: tokio::net::TcpListener, shared: Arc<CtlShared>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            // Reject cross-origin handshakes (CSWSH) before upgrading.
            let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                let origin = req
                    .headers()
                    .get("origin")
                    .and_then(|v| v.to_str().ok());
                if origin_allowed(origin) {
                    Ok(resp)
                } else {
                    eprintln!("[reamesh] ctl: rejected connection from origin {origin:?}");
                    let mut err = ErrorResponse::new(Some("origin not allowed".to_string()));
                    *err.status_mut() = StatusCode::FORBIDDEN;
                    Err(err)
                }
            };
            let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, check).await else {
                return;
            };
            let (mut ws_tx, mut ws_rx) = ws.split();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            // Greet, then replay the last known state so the app paints now.
            let _ = tx.send(
                json!({"type": "hello", "pid": std::process::id()}).to_string(),
            );
            {
                let last = shared.last_state.lock().unwrap().clone();
                if !last.is_empty() {
                    let _ = tx.send(last);
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
    use super::origin_allowed;

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
