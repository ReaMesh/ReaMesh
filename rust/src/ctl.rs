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
use std::time::Duration;

use libp2p::futures::{SinkExt, StreamExt};
use serde_json::{Value as JsonValue, json};
use tokio_tungstenite::tungstenite::Message;

/// Ports tried in order; one per running REAPER instance.
pub const CTL_PORTS: [u16; 4] = [47415, 47416, 47417, 47418];
/// Commands queued but not yet drained by the extension tick. Bounded so a
/// stuck extension can't accumulate unbounded input.
const INBOUND_CAP: usize = 64;

struct CtlShared {
    inbound: Mutex<VecDeque<String>>,
    clients: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<String>>>,
    /// Last state published by the extension; replayed to newly-connected
    /// apps so they render immediately instead of waiting for the next tick.
    last_state: Mutex<String>,
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
                    let Some((listener, port)) = bound else {
                        eprintln!("[reamesh] ctl server: no free port in {CTL_PORTS:?}");
                        let _ = port_tx.send(0u16);
                        return;
                    };
                    shared.port.store(port, Ordering::Relaxed);
                    let _ = port_tx.send(port);
                    eprintln!("[reamesh] ctl server on 127.0.0.1:{port}");
                    accept_loop(listener, shared).await;
                });
            });
        if spawned.is_err() {
            return 0;
        }
    }
    let port = port_rx.recv_timeout(Duration::from_secs(3)).unwrap_or(0);
    if port != 0 {
        let _ = CTL.set(shared);
    }
    port
}

/// Next queued command from a connected app as JSON, or None.
pub fn poll() -> Option<String> {
    CTL.get()?.inbound.lock().unwrap().pop_front()
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
            let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
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
