//! ReaMesh core: yrs CRDT document + libp2p LAN swarm behind a C ABI.
//!
//! Threading model: the host (the REAPER extension) calls every FFI function
//! from a single thread (REAPER's main thread). All networking runs on a
//! private Tokio runtime owned by `Reamesh`. The two sides meet only at
//! thread-safe queues and a mutex around the yrs document.
//!
//! Document model: one root `Map` named "tracks", keyed by REAPER track GUID.
//! Each value is a nested `Map` of scalar fields (present/index/name/vol/pan/
//! mute/solo). yrs map semantics give deterministic last-writer-wins per
//! field. Deleted tracks are tombstoned with `present: false` rather than
//! removed, so identity survives concurrent edits.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::time::Duration;

use libp2p::futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::SwarmEvent;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::{Multiaddr, Swarm, gossipsub, mdns, noise, tcp, yamux};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use yrs::updates::decoder::Decode;
use yrs::{Any, Doc, Map, MapRef, Out, ReadTxn, StateVector, Transact, Update};

const TRACKS_ROOT: &str = "tracks";

type TrackFields = JsonMap<String, JsonValue>;
type StateMap = HashMap<String, TrackFields>;

struct DocState {
    doc: Doc,
    /// Snapshot of the doc as of the last moment the host was in sync with it
    /// (after the previous push_local or drain_remote). drain_remote diffs the
    /// doc against this, which is what keeps locally-pushed changes from
    /// echoing back out to the host.
    last_state: StateMap,
}

enum Command {
    Publish(Vec<u8>),
    Dial(Multiaddr),
    Shutdown,
}

pub struct Reamesh {
    doc: Arc<Mutex<DocState>>,
    inbound: Arc<Mutex<Vec<Vec<u8>>>>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    listen_addrs: Arc<Mutex<Vec<Multiaddr>>>,
    /// Peers currently subscribed to this project's gossipsub topic.
    topic_peers: Arc<AtomicUsize>,
    runtime: Option<tokio::runtime::Runtime>,
    local_peer_id: String,
}

impl Reamesh {
    pub fn new(project_id: &str) -> anyhow::Result<Box<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("reamesh-net")
            .enable_all()
            .build()?;

        let doc = Doc::new();
        // Root types must be registered before remote updates referencing
        // them are applied.
        doc.get_or_insert_map(TRACKS_ROOT);

        let doc = Arc::new(Mutex::new(DocState {
            doc,
            last_state: StateMap::new(),
        }));
        let inbound: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let listen_addrs: Arc<Mutex<Vec<Multiaddr>>> = Arc::new(Mutex::new(Vec::new()));
        let topic_peers = Arc::new(AtomicUsize::new(0));
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (init_tx, init_rx) = std_mpsc::channel();

        let project_id = project_id.to_string();
        {
            let doc = doc.clone();
            let inbound = inbound.clone();
            let listen_addrs = listen_addrs.clone();
            let topic_peers = topic_peers.clone();
            runtime.spawn(async move {
                let (swarm, topic) = match build_swarm(&project_id) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = init_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = init_tx.send(Ok(swarm.local_peer_id().to_string()));
                run_network(swarm, topic, doc, inbound, listen_addrs, topic_peers, cmd_rx).await;
            });
        }

        let local_peer_id = init_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| anyhow::anyhow!("network task did not start"))?
            .map_err(|e| anyhow::anyhow!("swarm init failed: {e}"))?;

        eprintln!("[reamesh] started, peer id {local_peer_id}");
        Ok(Box::new(Reamesh {
            doc,
            inbound,
            cmd_tx,
            listen_addrs,
            topic_peers,
            runtime: Some(runtime),
            local_peer_id,
        }))
    }

    /// Apply a locally-observed changeset to the shared doc and broadcast the
    /// resulting CRDT update to peers. Non-blocking.
    pub fn push_local(&self, changeset_json: &str) {
        let parsed: JsonValue = match serde_json::from_str(changeset_json) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[reamesh] push_local: bad changeset JSON: {e}");
                return;
            }
        };
        let Some(tracks) = parsed.get(TRACKS_ROOT).and_then(|t| t.as_object()) else {
            return;
        };

        let mut ds = self.doc.lock().unwrap();
        let tracks_root = ds.doc.get_or_insert_map(TRACKS_ROOT);
        let update = {
            let mut txn = ds.doc.transact_mut();
            let mut changed = false;
            for (guid, fields) in tracks {
                let Some(fields) = fields.as_object() else { continue };
                let track: MapRef = tracks_root.get_or_init(&mut txn, guid.as_str());
                for (key, value) in fields {
                    if track.try_update(&mut txn, key.as_str(), json_to_any(value)) {
                        changed = true;
                    }
                }
            }
            if !changed {
                return;
            }
            txn.encode_update_v1()
        };
        // Re-baseline so this local change never re-surfaces from drain_remote.
        ds.last_state = snapshot(&ds.doc);
        drop(ds);
        let _ = self.cmd_tx.send(Command::Publish(update));
    }

    /// Merge updates received from peers into the doc and return the net
    /// effect as a JSON array of changesets ("[]" when nothing arrived).
    pub fn drain_remote(&self) -> String {
        let updates: Vec<Vec<u8>> = std::mem::take(&mut *self.inbound.lock().unwrap());
        if updates.is_empty() {
            return "[]".to_string();
        }
        let mut ds = self.doc.lock().unwrap();
        {
            let mut txn = ds.doc.transact_mut();
            for bytes in &updates {
                match Update::decode_v1(bytes) {
                    Ok(u) => {
                        if let Err(e) = txn.apply_update(u) {
                            eprintln!("[reamesh] drain_remote: apply_update failed: {e}");
                        }
                    }
                    Err(e) => eprintln!("[reamesh] drain_remote: undecodable update: {e}"),
                }
            }
        }
        let new_state = snapshot(&ds.doc);
        let diff = diff_states(&ds.last_state, &new_state);
        ds.last_state = new_state;
        if diff.is_empty() {
            "[]".to_string()
        } else {
            json!([{ TRACKS_ROOT: diff }]).to_string()
        }
    }

    /// Full current doc state in changeset shape. For tests and debugging.
    pub fn snapshot_json(&self) -> String {
        let ds = self.doc.lock().unwrap();
        let state = snapshot(&ds.doc);
        let ordered: std::collections::BTreeMap<&String, &TrackFields> = state.iter().collect();
        json!({ TRACKS_ROOT: ordered }).to_string()
    }

    pub fn local_peer_id(&self) -> &str {
        &self.local_peer_id
    }

    /// Number of peers currently subscribed to this project's topic.
    pub fn connected_peers(&self) -> usize {
        self.topic_peers.load(Ordering::Relaxed)
    }

    pub fn listen_addrs(&self) -> Vec<String> {
        self.listen_addrs
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.to_string())
            .collect()
    }

    /// Explicitly dial a peer multiaddr. Production discovery is mDNS; this
    /// exists for tests and as an escape hatch on networks without multicast.
    pub fn dial(&self, addr: &str) -> bool {
        match addr.parse::<Multiaddr>() {
            Ok(a) => self.cmd_tx.send(Command::Dial(a)).is_ok(),
            Err(_) => false,
        }
    }
}

impl Drop for Reamesh {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

// ---------- networking ----------

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
}

fn build_swarm(project_id: &str) -> anyhow::Result<(Swarm<Behaviour>, gossipsub::IdentTopic)> {
    let topic = gossipsub::IdentTopic::new(format!("reamesh/{project_id}"));
    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map_err(|e| anyhow::anyhow!("tcp transport: {e}"))?
        .with_behaviour(|key| {
            let gs_config = gossipsub::ConfigBuilder::default()
                .max_transmit_size(8 * 1024 * 1024)
                .build()
                .map_err(std::io::Error::other)?;
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gs_config,
            )
            .map_err(std::io::Error::other)?;
            // mDNS needs a multicast socket; in sandboxed environments that
            // can fail, in which case we still run (peers must dial directly).
            let mdns = match mdns::tokio::Behaviour::new(
                mdns::Config::default(),
                key.public().to_peer_id(),
            ) {
                Ok(b) => Toggle::from(Some(b)),
                Err(e) => {
                    eprintln!("[reamesh] mDNS unavailable ({e}); discovery disabled");
                    Toggle::from(None)
                }
            };
            Ok(Behaviour { gossipsub, mdns })
        })
        .map_err(|e| anyhow::anyhow!("behaviour: {e}"))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(3600)))
        .build();

    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    Ok((swarm, topic))
}

async fn run_network(
    mut swarm: Swarm<Behaviour>,
    topic: gossipsub::IdentTopic,
    doc: Arc<Mutex<DocState>>,
    inbound: Arc<Mutex<Vec<Vec<u8>>>>,
    listen_addrs: Arc<Mutex<Vec<Multiaddr>>>,
    topic_peers: Arc<AtomicUsize>,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
) {
    // IPs this host listens on; used to recognize same-host peers (see below).
    let mut local_ips: std::collections::HashSet<std::net::IpAddr> =
        std::collections::HashSet::new();

    // Anti-entropy: whenever a peer (re)appears on the topic, or a publish
    // fails because nobody was listening, we owe the network a full-state
    // update. yrs updates are idempotent, so over-sending is harmless.
    let mut need_full_sync = false;
    let mut sync_tick = tokio::time::interval(Duration::from_millis(500));
    sync_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::Publish(bytes)) => {
                    if swarm.behaviour_mut().gossipsub.publish(topic.clone(), bytes).is_err() {
                        need_full_sync = true;
                    }
                }
                Some(Command::Dial(addr)) => {
                    if let Err(e) = swarm.dial(addr) {
                        eprintln!("[reamesh] dial failed: {e}");
                    }
                }
                Some(Command::Shutdown) | None => break,
            },
            _ = sync_tick.tick() => {
                if need_full_sync && topic_peers.load(Ordering::Relaxed) > 0 {
                    let bytes = {
                        let ds = doc.lock().unwrap();
                        ds.doc.transact().encode_state_as_update_v1(&StateVector::default())
                    };
                    if swarm.behaviour_mut().gossipsub.publish(topic.clone(), bytes).is_ok() {
                        need_full_sync = false;
                    }
                }
            },
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    eprintln!("[reamesh] listening on {address}");
                    for proto in address.iter() {
                        match proto {
                            Protocol::Ip4(ip) => { local_ips.insert(ip.into()); }
                            Protocol::Ip6(ip) => { local_ips.insert(ip.into()); }
                            _ => {}
                        }
                    }
                    listen_addrs.lock().unwrap().push(address);
                }
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    eprintln!("[reamesh] connection established with {peer_id} via {endpoint:?}");
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    eprintln!("[reamesh] outgoing connection error (peer {peer_id:?}): {error}");
                }
                SwarmEvent::IncomingConnectionError { error, .. } => {
                    eprintln!("[reamesh] incoming connection error: {error}");
                }
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
                    for (peer_id, addr) in peers {
                        eprintln!("[reamesh] mDNS discovered {peer_id} at {addr}");
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        // mDNS advertises LAN-interface addresses only. When the
                        // peer is another instance on this same machine (the
                        // documented dev setup), traffic to our own LAN IP can be
                        // dropped by firewalls/filters, so also try loopback.
                        let mut addresses = Vec::new();
                        if let Some(lo) = loopback_variant(&addr, &local_ips) {
                            addresses.push(lo);
                        }
                        addresses.push(addr);
                        // An ephemeral source port avoids the TCP simultaneous-open
                        // race when two peers that discover each other at the same
                        // moment dial from their (reused) listen ports.
                        let _ = swarm.dial(
                            DialOpts::peer_id(peer_id)
                                .addresses(addresses)
                                .allocate_new_port()
                                .build(),
                        );
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
                    for (peer_id, _) in peers {
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic: t })) => {
                    if t == topic.hash() {
                        eprintln!("[reamesh] peer {peer_id} joined the session");
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        topic_peers.fetch_add(1, Ordering::Relaxed);
                        // Late joiners missed earlier gossip; send everything.
                        need_full_sync = true;
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Unsubscribed { peer_id, topic: t })) => {
                    if t == topic.hash() {
                        eprintln!("[reamesh] peer {peer_id} left the session");
                        let _ = topic_peers.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message { message, .. })) => {
                    if message.topic == topic.hash() {
                        inbound.lock().unwrap().push(message.data);
                    }
                }
                _ => {}
            },
        }
    }
}

/// If `addr` points at one of this host's own non-loopback IPs (i.e. the peer
/// is another instance on this machine), return the same address rewritten to
/// loopback. Returns None for genuinely remote peers.
fn loopback_variant(
    addr: &Multiaddr,
    local_ips: &std::collections::HashSet<std::net::IpAddr>,
) -> Option<Multiaddr> {
    let mut rewrote = false;
    let rewritten: Multiaddr = addr
        .iter()
        .map(|p| match p {
            Protocol::Ip4(ip) if !ip.is_loopback() && local_ips.contains(&ip.into()) => {
                rewrote = true;
                Protocol::Ip4(std::net::Ipv4Addr::LOCALHOST)
            }
            Protocol::Ip6(ip) if !ip.is_loopback() && local_ips.contains(&ip.into()) => {
                rewrote = true;
                Protocol::Ip6(std::net::Ipv6Addr::LOCALHOST)
            }
            other => other,
        })
        .collect();
    rewrote.then_some(rewritten)
}

// ---------- doc <-> JSON helpers ----------

/// Read the full doc into plain JSON, normalizing all numbers to f64 so that
/// snapshot comparisons never flap on int-vs-float representation.
fn snapshot(doc: &Doc) -> StateMap {
    let tracks = doc.get_or_insert_map(TRACKS_ROOT);
    let txn = doc.transact();
    let mut out = StateMap::new();
    for (guid, value) in tracks.iter(&txn) {
        if let Out::YMap(track) = value {
            let mut fields = TrackFields::new();
            for (key, v) in track.iter(&txn) {
                if let Out::Any(any) = v {
                    fields.insert(key.to_string(), any_to_json(&any));
                }
            }
            out.insert(guid.to_string(), fields);
        }
    }
    out
}

fn diff_states(old: &StateMap, new: &StateMap) -> JsonMap<String, JsonValue> {
    let mut out = JsonMap::new();
    for (guid, new_fields) in new {
        match old.get(guid) {
            None => {
                out.insert(guid.clone(), JsonValue::Object(new_fields.clone()));
            }
            Some(old_fields) => {
                let mut changed = TrackFields::new();
                for (key, value) in new_fields {
                    if old_fields.get(key) != Some(value) {
                        changed.insert(key.clone(), value.clone());
                    }
                }
                if !changed.is_empty() {
                    out.insert(guid.clone(), JsonValue::Object(changed));
                }
            }
        }
    }
    // Tracks are tombstoned (present:false), not removed, so this branch is
    // defensive only.
    for guid in old.keys() {
        if !new.contains_key(guid) {
            out.insert(guid.clone(), json!({ "present": false }));
        }
    }
    out
}

fn json_to_any(v: &JsonValue) -> Any {
    match v {
        JsonValue::Null => Any::Null,
        JsonValue::Bool(b) => Any::Bool(*b),
        JsonValue::Number(n) => Any::Number(n.as_f64().unwrap_or(0.0)),
        JsonValue::String(s) => Any::String(s.as_str().into()),
        // Arrays/objects never appear as field values in the changeset schema.
        other => Any::String(other.to_string().into()),
    }
}

fn any_to_json(a: &Any) -> JsonValue {
    match a {
        Any::Null | Any::Undefined => JsonValue::Null,
        Any::Bool(b) => json!(*b),
        Any::Number(f) => json!(*f),
        Any::BigInt(i) => json!(*i as f64),
        Any::String(s) => json!(s.as_ref()),
        other => json!(other.to_string()),
    }
}

// ---------- C ABI ----------

/// Start the runtime, open the shared doc for `project_id`, start LAN
/// discovery. Returns null on failure. The handle must be released with
/// `reamesh_shutdown`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_init(project_id: *const c_char) -> *mut Reamesh {
    if project_id.is_null() {
        return std::ptr::null_mut();
    }
    let pid = unsafe { CStr::from_ptr(project_id) }
        .to_string_lossy()
        .into_owned();
    let result = std::panic::catch_unwind(|| match Reamesh::new(&pid) {
        Ok(r) => Box::into_raw(r),
        Err(e) => {
            eprintln!("[reamesh] init failed: {e}");
            std::ptr::null_mut()
        }
    });
    result.unwrap_or(std::ptr::null_mut())
}

/// Feed a locally-observed changeset (JSON, see schema in README) into the
/// shared document. Non-blocking; never calls back into the host.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_push_local(handle: *mut Reamesh, changeset_json: *const c_char) {
    if handle.is_null() || changeset_json.is_null() {
        return;
    }
    let r = unsafe { &*handle };
    let json = unsafe { CStr::from_ptr(changeset_json) }.to_string_lossy();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.push_local(&json)));
}

/// Return changesets that arrived from remote peers since the last call, as a
/// JSON array ("[]" when none). Free the result with `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_drain_remote(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        "[]".to_string()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.drain_remote()))
            .unwrap_or_else(|_| "[]".to_string())
    };
    CString::new(out)
        .unwrap_or_else(|_| CString::new("[]").unwrap())
        .into_raw()
}

/// Number of peers currently in this project's session (for status display).
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_peer_count(handle: *mut Reamesh) -> u32 {
    if handle.is_null() {
        return 0;
    }
    let r = unsafe { &*handle };
    r.connected_peers() as u32
}

/// Free a string returned by `reamesh_drain_remote`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)) };
    }
}

/// Stop networking, drop the document, release the handle.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_shutdown(handle: *mut Reamesh) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle)) };
    }
}
