//! ReaMesh core: yrs CRDT document + libp2p LAN swarm behind a C ABI.
//!
//! v2 model: **owner-authoritative sessions**. One peer (the Owner) hosts a
//! session for its open project; other peers (Clients) discover it on a
//! LAN-wide lobby topic, join, download the full project directory over a
//! request-response transfer protocol, then collaborate live.
//!
//! Authority: the Owner's yrs doc is the single source of truth. Owner-local
//! edits apply to the doc directly and are broadcast as authoritative updates
//! (`'A'` + yrs update bytes) on the session topic. Client-local edits are
//! *proposals* (`'P'` + changeset JSON): only the Owner consumes them, applies
//! the admitted fields to the doc and re-broadcasts the result. Fields the
//! Owner itself edited within the last `OWNER_PRIORITY_TTL` are dropped from
//! proposals; the owner wins live read/write conflicts. Clients keep their
//! optimistic local value while a proposal is in flight and revert to the
//! authoritative value if it is not confirmed within `PROPOSAL_TIMEOUT`.
//!
//! Threading model (unchanged from v1): the host (the REAPER extension) calls
//! every FFI function from REAPER's main thread; all networking runs on a
//! private Tokio runtime owned by `Reamesh`. The two sides meet only at
//! thread-safe queues/flags and a mutex around the yrs document.
//!
//! Document model (unchanged from v1): one root `Map` named "tracks", keyed
//! by REAPER track GUID; each value is a nested `Map` of scalar fields.
//! Deleted tracks are tombstoned with `present: false`, never removed.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use libp2p::futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::SwarmEvent;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::{Multiaddr, PeerId, StreamProtocol, Swarm, gossipsub, mdns, noise, tcp, yamux};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use yrs::updates::decoder::Decode;
use yrs::{Any, Doc, Map, MapRef, Out, ReadTxn, StateVector, Transact, Update};

const TRACKS_ROOT: &str = "tracks";
const LOBBY_TOPIC: &str = "reamesh/v2/lobby";
const XFER_PROTOCOL: &str = "/reamesh/xfer/1";
/// Max payload per file chunk over the transfer protocol.
const CHUNK_SIZE: u64 = 1024 * 1024;
/// Fields the owner edited this recently beat any client proposal.
const OWNER_PRIORITY_TTL: Duration = Duration::from_secs(3);
/// How long a client keeps its optimistic value before reverting to the
/// authoritative one when a proposal is never confirmed (i.e. was dropped).
const PROPOSAL_TIMEOUT: Duration = Duration::from_secs(3);
/// Lobby announcements older than this are considered gone.
const SESSION_STALE: Duration = Duration::from_secs(10);

// Session-topic message tags.
const TAG_AUTHORITATIVE: u8 = b'A';
const TAG_PROPOSAL: u8 = b'P';

type TrackFields = JsonMap<String, JsonValue>;
type StateMap = HashMap<String, TrackFields>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Owner,
    Client,
}

// ---------- file transfer protocol types ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileMeta {
    /// Path relative to the project directory, '/'-separated.
    path: String,
    size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
enum XferRequest {
    Manifest,
    Chunk { file: u32, offset: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
enum XferResponse {
    Manifest {
        session_id: String,
        session_name: String,
        project_file: String,
        files: Vec<FileMeta>,
    },
    Chunk {
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Error {
        message: String,
    },
}

// ---------- shared state between FFI thread and network task ----------

#[derive(Clone, Debug)]
struct SessionInfo {
    session_id: String,
    session_name: String,
    project_file: String,
    owner: String, // peer id string
    last_seen: Instant,
}

#[derive(Clone, Debug)]
struct StatusState {
    /// "hosting" | "browsing" | "connecting" | "downloading" | "ready" | "failed"
    state: String,
    progress: f64,
    project_path: Option<String>,
    session_id: Option<String>,
    session_name: Option<String>,
    message: Option<String>,
}

impl StatusState {
    fn to_json(&self) -> String {
        json!({
            "state": self.state,
            "progress": self.progress,
            "project_path": self.project_path,
            "session_id": self.session_id,
            "session_name": self.session_name,
            "message": self.message,
        })
        .to_string()
    }
}

struct DocState {
    doc: Doc,
    /// Snapshot of the doc as of the last moment the host was in sync with it
    /// (after the previous push_local or drain_remote on the owner; after the
    /// previous drain_remote on a client). drain_remote diffs the doc against
    /// this, which is what keeps already-seen changes from echoing back out.
    last_state: StateMap,
    /// Owner only: (guid, field) -> when the owner last edited it locally.
    recent_edits: HashMap<(String, String), Instant>,
    /// Client only: (guid, field) -> (proposed value, when proposed).
    pending: HashMap<(String, String), (JsonValue, Instant)>,
}

struct Shared {
    doc: Mutex<DocState>,
    /// Authoritative yrs updates received from the owner, queued for drain.
    inbound: Mutex<Vec<Vec<u8>>>,
    /// Set when the network task changed the doc directly (owner admitting a
    /// proposal), so drain_remote knows to diff even with an empty queue.
    dirty: AtomicBool,
    listen_addrs: Mutex<Vec<Multiaddr>>,
    /// Peers currently subscribed to this session's gossipsub topic.
    topic_peers: AtomicUsize,
    /// Sessions currently announced on the lobby (client side).
    sessions: Mutex<HashMap<String, SessionInfo>>,
    status: Mutex<StatusState>,
}

enum Command {
    PublishAuthoritative(Vec<u8>),
    PublishProposal(Vec<u8>),
    JoinSession(String),
    Dial(Multiaddr),
    Shutdown,
}

/// Owner-side session description handed to the network task.
struct Hosting {
    session_id: String,
    session_name: String,
    project_dir: PathBuf,
    project_file: String,
    files: Vec<FileMeta>,
}

struct NetConfig {
    role: Role,
    hosting: Option<Hosting>,
    staging_dir: Option<PathBuf>,
}

pub struct Reamesh {
    shared: Arc<Shared>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    runtime: Option<tokio::runtime::Runtime>,
    local_peer_id: String,
    role: Role,
}

impl Reamesh {
    /// Start hosting a session for the project at `project_dir/project_file`.
    /// Everything under `project_dir` (except hidden files, backups and peak
    /// caches) is offered to joining clients.
    pub fn host(
        session_name: &str,
        project_dir: &str,
        project_file: &str,
    ) -> anyhow::Result<Box<Self>> {
        let dir = PathBuf::from(project_dir);
        if !dir.is_dir() {
            anyhow::bail!("project directory not found: {project_dir}");
        }
        let files = build_manifest(&dir)?;
        let session_id = new_session_id();
        let hosting = Hosting {
            session_id: session_id.clone(),
            session_name: session_name.to_string(),
            project_dir: dir,
            project_file: project_file.to_string(),
            files,
        };
        let status = StatusState {
            state: "hosting".into(),
            progress: 1.0,
            project_path: None,
            session_id: Some(session_id),
            session_name: Some(session_name.to_string()),
            message: None,
        };
        Self::start(
            Role::Owner,
            NetConfig {
                role: Role::Owner,
                hosting: Some(hosting),
                staging_dir: None,
            },
            status,
        )
    }

    /// Start in browsing mode: listen for session announcements on the lobby.
    /// Joined sessions are downloaded into a fresh subdirectory of
    /// `staging_dir`.
    pub fn join(staging_dir: &str) -> anyhow::Result<Box<Self>> {
        let status = StatusState {
            state: "browsing".into(),
            progress: 0.0,
            project_path: None,
            session_id: None,
            session_name: None,
            message: None,
        };
        Self::start(
            Role::Client,
            NetConfig {
                role: Role::Client,
                hosting: None,
                staging_dir: Some(PathBuf::from(staging_dir)),
            },
            status,
        )
    }

    fn start(role: Role, cfg: NetConfig, status: StatusState) -> anyhow::Result<Box<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("reamesh-net")
            .enable_all()
            .build()?;

        let doc = Doc::new();
        // Root types must be registered before remote updates referencing
        // them are applied.
        doc.get_or_insert_map(TRACKS_ROOT);

        let shared = Arc::new(Shared {
            doc: Mutex::new(DocState {
                doc,
                last_state: StateMap::new(),
                recent_edits: HashMap::new(),
                pending: HashMap::new(),
            }),
            inbound: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(false),
            listen_addrs: Mutex::new(Vec::new()),
            topic_peers: AtomicUsize::new(0),
            sessions: Mutex::new(HashMap::new()),
            status: Mutex::new(status),
        });
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (init_tx, init_rx) = std_mpsc::channel();

        {
            let shared = shared.clone();
            runtime.spawn(async move {
                let swarm = match build_swarm() {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = init_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = init_tx.send(Ok(swarm.local_peer_id().to_string()));
                run_network(swarm, cfg, shared, cmd_rx).await;
            });
        }

        let local_peer_id = init_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| anyhow::anyhow!("network task did not start"))?
            .map_err(|e| anyhow::anyhow!("swarm init failed: {e}"))?;

        eprintln!("[reamesh] started as {role:?}, peer id {local_peer_id}");
        Ok(Box::new(Reamesh {
            shared,
            cmd_tx,
            runtime: Some(runtime),
            local_peer_id,
            role,
        }))
    }

    /// Apply a locally-observed changeset. Owner: apply to the authoritative
    /// doc and broadcast. Client: record as pending and send a proposal to
    /// the owner (the doc is only ever written by authoritative updates).
    /// Non-blocking.
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

        match self.role {
            Role::Owner => {
                let mut ds = self.shared.doc.lock().unwrap();
                let now = Instant::now();
                for (guid, fields) in tracks {
                    if let Some(fields) = fields.as_object() {
                        for key in fields.keys() {
                            ds.recent_edits.insert((guid.clone(), key.clone()), now);
                        }
                    }
                }
                ds.recent_edits
                    .retain(|_, t| now.duration_since(*t) < OWNER_PRIORITY_TTL * 4);
                let Some(update) = apply_changeset_to_doc(&mut ds, tracks) else {
                    return;
                };
                // Re-baseline so this local change never re-surfaces from
                // drain_remote.
                ds.last_state = snapshot(&ds.doc);
                drop(ds);
                let _ = self.cmd_tx.send(Command::PublishAuthoritative(update));
            }
            Role::Client => {
                let mut ds = self.shared.doc.lock().unwrap();
                let now = Instant::now();
                for (guid, fields) in tracks {
                    if let Some(fields) = fields.as_object() {
                        for (key, value) in fields {
                            // Normalize through the same Any round-trip the
                            // doc snapshot uses so confirmation compares hold.
                            let norm = any_to_json(&json_to_any(value));
                            ds.pending.insert((guid.clone(), key.clone()), (norm, now));
                        }
                    }
                }
                drop(ds);
                let _ = self
                    .cmd_tx
                    .send(Command::PublishProposal(changeset_json.as_bytes().to_vec()));
            }
        }
    }

    /// Merge authoritative updates received since the last call into the doc
    /// and return the net effect as a JSON array of changesets ("[]" when
    /// nothing changed). On clients this also reconciles in-flight proposals:
    /// confirmed ones are dropped silently, rejected ones come back as a
    /// revert to the owner's value.
    pub fn drain_remote(&self) -> String {
        let updates: Vec<Vec<u8>> = std::mem::take(&mut *self.shared.inbound.lock().unwrap());
        let dirty = self.shared.dirty.swap(false, Ordering::SeqCst);
        let mut ds = self.shared.doc.lock().unwrap();
        if updates.is_empty() && !dirty && ds.pending.is_empty() {
            return "[]".to_string();
        }
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
        let mut diff = diff_states(&ds.last_state, &new_state);

        // Client-side proposal reconciliation (pending is always empty on the
        // owner, so this is a no-op there).
        let now = Instant::now();
        let pending = &mut ds.pending;
        pending.retain(|(guid, field), (proposed, when)| {
            let doc_val = new_state.get(guid).and_then(|f| f.get(field));
            if doc_val == Some(proposed) {
                // Confirmed by the owner; suppress the no-op echo.
                if let Some(JsonValue::Object(m)) = diff.get_mut(guid) {
                    m.remove(field);
                }
                false
            } else if now.duration_since(*when) > PROPOSAL_TIMEOUT {
                // Never confirmed: the owner dropped it (or overrode it).
                // Revert the host to the authoritative value.
                if let Some(dv) = doc_val {
                    diff.entry(guid.clone())
                        .or_insert_with(|| JsonValue::Object(JsonMap::new()))
                        .as_object_mut()
                        .unwrap()
                        .insert(field.clone(), dv.clone());
                }
                false
            } else {
                // Still in flight: keep the optimistic local value for now.
                if let Some(JsonValue::Object(m)) = diff.get_mut(guid) {
                    m.remove(field);
                }
                true
            }
        });
        diff.retain(|_, v| v.as_object().is_none_or(|m| !m.is_empty()));

        ds.last_state = new_state;
        if diff.is_empty() {
            "[]".to_string()
        } else {
            json!([{ TRACKS_ROOT: diff }]).to_string()
        }
    }

    /// Sessions currently announced on the LAN lobby, as a JSON array.
    pub fn sessions_json(&self) -> String {
        let mut sessions = self.shared.sessions.lock().unwrap();
        let now = Instant::now();
        sessions.retain(|_, s| now.duration_since(s.last_seen) < SESSION_STALE);
        let mut list: Vec<&SessionInfo> = sessions.values().collect();
        list.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        let arr: Vec<JsonValue> = list
            .iter()
            .map(|s| {
                json!({
                    "session_id": s.session_id,
                    "session_name": s.session_name,
                    "project_file": s.project_file,
                    "owner": s.owner,
                })
            })
            .collect();
        json!(arr).to_string()
    }

    /// Join an announced session (client only): subscribe to its topic and
    /// download the project files. Progress is reported via `status_json`.
    pub fn join_session(&self, session_id: &str) -> bool {
        if self.role != Role::Client {
            return false;
        }
        let known = self
            .shared
            .sessions
            .lock()
            .unwrap()
            .contains_key(session_id);
        if !known {
            return false;
        }
        self.cmd_tx
            .send(Command::JoinSession(session_id.to_string()))
            .is_ok()
    }

    /// Current session status as JSON:
    /// `{"state","progress","project_path","session_id","session_name","message"}`.
    pub fn status_json(&self) -> String {
        self.shared.status.lock().unwrap().to_json()
    }

    /// Full current doc state in changeset shape. For tests and debugging.
    pub fn snapshot_json(&self) -> String {
        let ds = self.shared.doc.lock().unwrap();
        let state = snapshot(&ds.doc);
        let ordered: std::collections::BTreeMap<&String, &TrackFields> = state.iter().collect();
        json!({ TRACKS_ROOT: ordered }).to_string()
    }

    pub fn local_peer_id(&self) -> &str {
        &self.local_peer_id
    }

    /// Number of peers currently subscribed to this session's topic.
    pub fn connected_peers(&self) -> usize {
        self.shared.topic_peers.load(Ordering::Relaxed)
    }

    pub fn listen_addrs(&self) -> Vec<String> {
        self.shared
            .listen_addrs
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

/// Apply a parsed changeset into the doc. Returns the encoded yrs update when
/// something actually changed. Does NOT touch `last_state`; callers decide
/// whether the change should surface from drain_remote.
fn apply_changeset_to_doc(
    ds: &mut DocState,
    tracks: &JsonMap<String, JsonValue>,
) -> Option<Vec<u8>> {
    let tracks_root = ds.doc.get_or_insert_map(TRACKS_ROOT);
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
    changed.then(|| txn.encode_update_v1())
}

// ---------- session id / manifest helpers ----------

fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Mix in some address-space entropy so two hosts starting in the same
    // nanosecond (or with a skewed clock) still diverge.
    let stack_probe = &nanos as *const _ as usize;
    format!("{nanos:x}-{:x}", stack_probe & 0xffff)
}

fn skip_in_manifest(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with(".rpp-bak")
        || name.ends_with(".RPP-bak")
        || name.ends_with(".reapeaks")
}

/// All shareable files under `dir`, as project-relative '/'-separated paths.
fn build_manifest(dir: &Path) -> anyhow::Result<Vec<FileMeta>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in fs::read_dir(dir.join(&rel))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if skip_in_manifest(&name) {
                continue;
            }
            let rel_child = rel.join(&name);
            let meta = entry.metadata()?;
            if meta.is_dir() {
                stack.push(rel_child);
            } else if meta.is_file() {
                let path = rel_child
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(FileMeta { path, size: meta.len() });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn sanitize_dir_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.');
    if trimmed.is_empty() { "session".into() } else { trimmed.to_string() }
}

/// Reject manifest paths that could escape the staging directory.
fn safe_relative_path(p: &str) -> Option<PathBuf> {
    let path = PathBuf::from(p);
    if path
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        Some(path)
    } else {
        None
    }
}

// ---------- networking ----------

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    xfer: request_response::cbor::Behaviour<XferRequest, XferResponse>,
}

fn session_topic(session_id: &str) -> gossipsub::IdentTopic {
    gossipsub::IdentTopic::new(format!("reamesh/v2/session/{session_id}"))
}

fn build_swarm() -> anyhow::Result<Swarm<Behaviour>> {
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
            let xfer = request_response::cbor::Behaviour::new(
                [(StreamProtocol::new(XFER_PROTOCOL), ProtocolSupport::Full)],
                request_response::Config::default()
                    .with_request_timeout(Duration::from_secs(30)),
            );
            Ok(Behaviour { gossipsub, mdns, xfer })
        })
        .map_err(|e| anyhow::anyhow!("behaviour: {e}"))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(3600)))
        .build();

    swarm
        .behaviour_mut()
        .gossipsub
        .subscribe(&gossipsub::IdentTopic::new(LOBBY_TOPIC))?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    Ok(swarm)
}

/// Client-side state of an in-progress project download.
struct Download {
    owner: PeerId,
    dir: PathBuf,
    files: Vec<FileMeta>,
    project_file: String,
    got_manifest: bool,
    cur: usize,
    out: Option<fs::File>,
    written: u64,
    total: u64,
    done: u64,
    last_activity: Instant,
    attempts: u32,
}

impl Download {
    /// The request that should currently be in flight (for retries).
    fn current_request(&self) -> XferRequest {
        if !self.got_manifest {
            XferRequest::Manifest
        } else {
            XferRequest::Chunk {
                file: self.cur as u32,
                offset: self.written,
            }
        }
    }

    /// Ensure the current output file is open, skipping completed and
    /// zero-byte files. Returns true when every file is finished.
    fn advance(&mut self) -> std::io::Result<bool> {
        loop {
            if self.cur >= self.files.len() {
                self.out = None;
                return Ok(true);
            }
            let meta = &self.files[self.cur];
            if self.out.is_none() {
                let Some(rel) = safe_relative_path(&meta.path) else {
                    return Err(std::io::Error::other(format!(
                        "unsafe path in manifest: {}",
                        meta.path
                    )));
                };
                let dest = self.dir.join(rel);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                self.out = Some(fs::File::create(dest)?);
                self.written = 0;
            }
            if self.written >= self.files[self.cur].size {
                self.out = None;
                self.cur += 1;
                continue;
            }
            return Ok(false);
        }
    }
}

fn set_status(shared: &Shared, f: impl FnOnce(&mut StatusState)) {
    f(&mut shared.status.lock().unwrap());
}

/// Owner-side handler for transfer requests.
fn serve_xfer(hosting: Option<&Hosting>, req: &XferRequest) -> XferResponse {
    let Some(h) = hosting else {
        return XferResponse::Error {
            message: "not hosting".into(),
        };
    };
    match req {
        XferRequest::Manifest => XferResponse::Manifest {
            session_id: h.session_id.clone(),
            session_name: h.session_name.clone(),
            project_file: h.project_file.clone(),
            files: h.files.clone(),
        },
        XferRequest::Chunk { file, offset } => {
            let Some(meta) = h.files.get(*file as usize) else {
                return XferResponse::Error {
                    message: format!("no such file index {file}"),
                };
            };
            let read = || -> std::io::Result<Vec<u8>> {
                let mut f = fs::File::open(h.project_dir.join(&meta.path))?;
                f.seek(SeekFrom::Start(*offset))?;
                let want = CHUNK_SIZE.min(meta.size.saturating_sub(*offset)) as usize;
                let mut buf = vec![0u8; want];
                f.read_exact(&mut buf)?;
                Ok(buf)
            };
            match read() {
                Ok(data) => XferResponse::Chunk { data },
                Err(e) => XferResponse::Error {
                    message: format!("read {}: {e}", meta.path),
                },
            }
        }
    }
}

/// Owner-side proposal admission: drop fields the owner edited recently
/// (owner wins), apply the rest to the authoritative doc. Returns the update
/// to re-broadcast.
fn admit_proposal(shared: &Shared, payload: &[u8]) -> Option<Vec<u8>> {
    let parsed: JsonValue = serde_json::from_slice(payload).ok()?;
    let tracks = parsed.get(TRACKS_ROOT)?.as_object()?;

    let mut ds = shared.doc.lock().unwrap();
    let now = Instant::now();
    let mut admitted = JsonMap::new();
    for (guid, fields) in tracks {
        let Some(fields) = fields.as_object() else { continue };
        let mut keep = TrackFields::new();
        for (key, value) in fields {
            let owner_recent = ds
                .recent_edits
                .get(&(guid.clone(), key.clone()))
                .is_some_and(|t| now.duration_since(*t) < OWNER_PRIORITY_TTL);
            if owner_recent {
                eprintln!("[reamesh] proposal for {guid}.{key} dropped (owner priority)");
            } else {
                keep.insert(key.clone(), value.clone());
            }
        }
        if !keep.is_empty() {
            admitted.insert(guid.clone(), JsonValue::Object(keep));
        }
    }
    if admitted.is_empty() {
        return None;
    }
    let update = apply_changeset_to_doc(&mut ds, &admitted)?;
    // Deliberately no last_state re-baseline: the admitted change must
    // surface from the owner's next drain_remote so its REAPER applies it.
    shared.dirty.store(true, Ordering::SeqCst);
    Some(update)
}

async fn run_network(
    mut swarm: Swarm<Behaviour>,
    cfg: NetConfig,
    shared: Arc<Shared>,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
) {
    // IPs this host listens on; used to recognize same-host peers (see below).
    let mut local_ips: std::collections::HashSet<std::net::IpAddr> =
        std::collections::HashSet::new();

    let lobby = gossipsub::IdentTopic::new(LOBBY_TOPIC);
    let hosting = cfg.hosting;

    // The session topic this node is currently active on. Owners have one
    // from the start; clients get one when they join.
    let mut sess_topic: Option<gossipsub::IdentTopic> = None;
    if let Some(h) = &hosting {
        let t = session_topic(&h.session_id);
        if let Err(e) = swarm.behaviour_mut().gossipsub.subscribe(&t) {
            eprintln!("[reamesh] subscribe to session topic failed: {e}");
        }
        sess_topic = Some(t);
    }
    let announce: Option<Vec<u8>> = hosting.as_ref().map(|h| {
        json!({
            "kind": "announce",
            "session_id": h.session_id,
            "session_name": h.session_name,
            "project_file": h.project_file,
            "owner": swarm.local_peer_id().to_string(),
        })
        .to_string()
        .into_bytes()
    });

    let mut download: Option<Download> = None;

    // Anti-entropy (owner): whenever a peer (re)appears on the session topic,
    // or a publish fails because nobody was listening, we owe the network a
    // full-state update. yrs updates are idempotent, so over-sending is
    // harmless.
    let mut need_full_sync = false;
    // Session messages that failed to publish (nobody connected yet); retried
    // on the tick. Bounded so a long-dead session can't grow without limit.
    let mut retry_pubs: Vec<Vec<u8>> = Vec::new();

    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tick_count: u64 = 0;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::PublishAuthoritative(update)) => {
                    let mut data = vec![TAG_AUTHORITATIVE];
                    data.extend_from_slice(&update);
                    if let Some(t) = &sess_topic {
                        if swarm.behaviour_mut().gossipsub.publish(t.clone(), data).is_err() {
                            need_full_sync = true;
                        }
                    }
                }
                Some(Command::PublishProposal(payload)) => {
                    let mut data = vec![TAG_PROPOSAL];
                    data.extend_from_slice(&payload);
                    match &sess_topic {
                        Some(t) => {
                            if swarm.behaviour_mut().gossipsub.publish(t.clone(), data.clone()).is_err() {
                                if retry_pubs.len() < 256 { retry_pubs.push(data); }
                            }
                        }
                        None => eprintln!("[reamesh] dropped proposal: not in a session"),
                    }
                }
                Some(Command::JoinSession(session_id)) => {
                    let info = shared.sessions.lock().unwrap().get(&session_id).cloned();
                    let Some(info) = info else {
                        set_status(&shared, |s| {
                            s.state = "failed".into();
                            s.message = Some(format!("session {session_id} is no longer announced"));
                        });
                        continue;
                    };
                    let Ok(owner) = info.owner.parse::<PeerId>() else {
                        set_status(&shared, |s| {
                            s.state = "failed".into();
                            s.message = Some("bad owner peer id in announcement".into());
                        });
                        continue;
                    };
                    let t = session_topic(&session_id);
                    if let Err(e) = swarm.behaviour_mut().gossipsub.subscribe(&t) {
                        eprintln!("[reamesh] subscribe to session topic failed: {e}");
                    }
                    sess_topic = Some(t);
                    let staging = cfg.staging_dir.clone().unwrap_or_else(std::env::temp_dir);
                    let dir = staging.join(format!(
                        "{}-{}",
                        sanitize_dir_name(&info.session_name),
                        &session_id[..session_id.len().min(8)]
                    ));
                    set_status(&shared, |s| {
                        s.state = "connecting".into();
                        s.progress = 0.0;
                        s.session_id = Some(session_id.clone());
                        s.session_name = Some(info.session_name.clone());
                        s.message = None;
                    });
                    swarm.behaviour_mut().xfer.send_request(&owner, XferRequest::Manifest);
                    download = Some(Download {
                        owner,
                        project_file: info.project_file.clone(),
                        dir,
                        files: Vec::new(),
                        got_manifest: false,
                        cur: 0,
                        out: None,
                        written: 0,
                        total: 0,
                        done: 0,
                        last_activity: Instant::now(),
                        attempts: 0,
                    });
                }
                Some(Command::Dial(addr)) => {
                    if let Err(e) = swarm.dial(addr) {
                        eprintln!("[reamesh] dial failed: {e}");
                    }
                }
                Some(Command::Shutdown) | None => break,
            },
            _ = tick.tick() => {
                tick_count += 1;
                // Recompute session-topic peers from gossipsub's view. (Event
                // counting misses subscriptions learned before we joined the
                // topic ourselves.)
                if let Some(t) = &sess_topic {
                    let hash = t.hash();
                    let count = swarm.behaviour().gossipsub.all_peers()
                        .filter(|(_, topics)| topics.iter().any(|th| **th == hash))
                        .count();
                    shared.topic_peers.store(count, Ordering::Relaxed);
                }
                // Owner: announce the session on the lobby every ~1s.
                if let (Some(bytes), true) = (&announce, tick_count % 2 == 0) {
                    let _ = swarm.behaviour_mut().gossipsub.publish(lobby.clone(), bytes.clone());
                }
                // Owner: full-state anti-entropy.
                if need_full_sync && shared.topic_peers.load(Ordering::Relaxed) > 0 {
                    if let Some(t) = &sess_topic {
                        let mut data = vec![TAG_AUTHORITATIVE];
                        {
                            let ds = shared.doc.lock().unwrap();
                            data.extend_from_slice(
                                &ds.doc.transact().encode_state_as_update_v1(&StateVector::default()),
                            );
                        }
                        if swarm.behaviour_mut().gossipsub.publish(t.clone(), data).is_ok() {
                            need_full_sync = false;
                        }
                    }
                }
                // Retry queued session publishes (proposals from before the
                // mesh was up).
                if !retry_pubs.is_empty() {
                    if let Some(t) = sess_topic.clone() {
                        let queued = std::mem::take(&mut retry_pubs);
                        for data in queued {
                            if swarm.behaviour_mut().gossipsub.publish(t.clone(), data.clone()).is_err() {
                                retry_pubs.push(data);
                            }
                        }
                    }
                }
                // Client: nudge a stalled download (lost request, dropped
                // connection mid-transfer, ...).
                if let Some(dl) = &mut download {
                    if dl.last_activity.elapsed() > Duration::from_secs(3) {
                        dl.attempts += 1;
                        if dl.attempts > 20 {
                            eprintln!("[reamesh] download from {} failed (timeout)", dl.owner);
                            set_status(&shared, |s| {
                                s.state = "failed".into();
                                s.message = Some("project download timed out".into());
                            });
                            download = None;
                        } else {
                            let req = dl.current_request();
                            dl.last_activity = Instant::now();
                            swarm.behaviour_mut().xfer.send_request(&dl.owner, req);
                        }
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
                    shared.listen_addrs.lock().unwrap().push(address);
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
                            swarm.add_peer_address(peer_id, lo.clone());
                            addresses.push(lo);
                        }
                        // Remember the address so request-response can dial too.
                        swarm.add_peer_address(peer_id, addr.clone());
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
                    if sess_topic.as_ref().is_some_and(|st| t == st.hash()) {
                        eprintln!("[reamesh] peer {peer_id} joined the session");
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        // Late joiners missed earlier gossip; the owner sends
                        // everything.
                        if hosting.is_some() {
                            need_full_sync = true;
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Unsubscribed { peer_id, topic: t })) => {
                    if sess_topic.as_ref().is_some_and(|st| t == st.hash()) {
                        eprintln!("[reamesh] peer {peer_id} left the session");
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message { message, .. })) => {
                    if message.topic == lobby.hash() {
                        if cfg.role == Role::Client
                            && let Ok(v) = serde_json::from_slice::<JsonValue>(&message.data)
                            && v.get("kind").and_then(|k| k.as_str()) == Some("announce")
                        {
                            let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let info = SessionInfo {
                                session_id: get("session_id"),
                                session_name: get("session_name"),
                                project_file: get("project_file"),
                                owner: get("owner"),
                                last_seen: Instant::now(),
                            };
                            if !info.session_id.is_empty() && !info.owner.is_empty() {
                                shared.sessions.lock().unwrap().insert(info.session_id.clone(), info);
                            }
                        }
                    } else if sess_topic.as_ref().is_some_and(|st| message.topic == st.hash()) {
                        match (message.data.first(), cfg.role) {
                            (Some(&TAG_AUTHORITATIVE), Role::Client) => {
                                shared.inbound.lock().unwrap().push(message.data[1..].to_vec());
                                shared.dirty.store(true, Ordering::SeqCst);
                            }
                            (Some(&TAG_PROPOSAL), Role::Owner) => {
                                if let Some(update) = admit_proposal(&shared, &message.data[1..]) {
                                    let mut data = vec![TAG_AUTHORITATIVE];
                                    data.extend_from_slice(&update);
                                    if let Some(t) = &sess_topic
                                        && swarm.behaviour_mut().gossipsub.publish(t.clone(), data).is_err() {
                                        need_full_sync = true;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Xfer(request_response::Event::Message { peer, message, .. })) => {
                    match message {
                        request_response::Message::Request { request, channel, .. } => {
                            let resp = serve_xfer(hosting.as_ref(), &request);
                            let _ = swarm.behaviour_mut().xfer.send_response(channel, resp);
                        }
                        request_response::Message::Response { response, .. } => {
                            handle_xfer_response(&mut swarm, &shared, &mut download, peer, response);
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Xfer(request_response::Event::OutboundFailure { peer, error, .. })) => {
                    eprintln!("[reamesh] transfer request to {peer} failed: {error}");
                    // The tick-driven stall handler retries; just age the
                    // activity timestamp so it kicks in soon.
                    if let Some(dl) = &mut download
                        && dl.owner == peer {
                        dl.last_activity = Instant::now() - Duration::from_secs(10);
                    }
                }
                _ => {}
            },
        }
    }
}

/// Client-side: feed a transfer response into the download state machine.
fn handle_xfer_response(
    swarm: &mut Swarm<Behaviour>,
    shared: &Shared,
    download: &mut Option<Download>,
    peer: PeerId,
    response: XferResponse,
) {
    let Some(dl) = download.as_mut() else { return };
    if dl.owner != peer {
        return;
    }
    dl.last_activity = Instant::now();
    dl.attempts = 0;

    let fail = |download: &mut Option<Download>, shared: &Shared, msg: String| {
        eprintln!("[reamesh] download failed: {msg}");
        set_status(shared, |s| {
            s.state = "failed".into();
            s.message = Some(msg);
        });
        *download = None;
    };

    match response {
        XferResponse::Manifest { files, project_file, .. } => {
            if dl.got_manifest {
                return; // duplicate (retry raced the response)
            }
            if let Err(e) = fs::create_dir_all(&dl.dir) {
                return fail(download, shared, format!("create staging dir: {e}"));
            }
            dl.got_manifest = true;
            dl.total = files.iter().map(|f| f.size).sum();
            dl.files = files;
            dl.project_file = project_file;
            eprintln!(
                "[reamesh] downloading {} files ({} bytes) into {}",
                dl.files.len(),
                dl.total,
                dl.dir.display()
            );
            set_status(shared, |s| {
                s.state = "downloading".into();
                s.progress = 0.0;
            });
            step_download(swarm, shared, download);
        }
        XferResponse::Chunk { data } => {
            if !dl.got_manifest {
                return;
            }
            // A stale retry can deliver a chunk we already have; only accept
            // the bytes we are waiting for.
            let expected = CHUNK_SIZE.min(dl.files[dl.cur].size - dl.written) as usize;
            if data.len() != expected {
                eprintln!(
                    "[reamesh] ignoring unexpected chunk ({} bytes, wanted {expected})",
                    data.len()
                );
                return;
            }
            if let Some(out) = &mut dl.out {
                if let Err(e) = out.write_all(&data) {
                    return fail(download, shared, format!("write chunk: {e}"));
                }
            } else {
                return; // advance() opens the file before any chunk is requested
            }
            dl.written += data.len() as u64;
            dl.done += data.len() as u64;
            let progress = if dl.total == 0 { 1.0 } else { dl.done as f64 / dl.total as f64 };
            set_status(shared, |s| s.progress = progress);
            step_download(swarm, shared, download);
        }
        XferResponse::Error { message } => {
            fail(download, shared, format!("owner reported: {message}"));
        }
    }
}

/// Open the next pending file and request its next chunk, or finish.
fn step_download(swarm: &mut Swarm<Behaviour>, shared: &Shared, download: &mut Option<Download>) {
    let Some(dl) = download.as_mut() else { return };
    match dl.advance() {
        Err(e) => {
            eprintln!("[reamesh] download failed: {e}");
            set_status(shared, |s| {
                s.state = "failed".into();
                s.message = Some(e.to_string());
            });
            *download = None;
        }
        Ok(true) => {
            let path = dl.dir.join(&dl.project_file);
            eprintln!("[reamesh] download complete: {}", path.display());
            set_status(shared, |s| {
                s.state = "ready".into();
                s.progress = 1.0;
                s.project_path = Some(path.to_string_lossy().into_owned());
            });
            *download = None;
        }
        Ok(false) => {
            let req = dl.current_request();
            swarm.behaviour_mut().xfer.send_request(&dl.owner, req);
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

fn cstr_arg(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn into_c_string(s: String) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| CString::new("").unwrap())
        .into_raw()
}

/// Start hosting a session named `session_name` that shares the project at
/// `project_dir`/`project_file` (path + file name of the saved .rpp).
/// Returns null on failure. Release with `reamesh_shutdown`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_host(
    session_name: *const c_char,
    project_dir: *const c_char,
    project_file: *const c_char,
) -> *mut Reamesh {
    let (Some(name), Some(dir), Some(file)) = (
        cstr_arg(session_name),
        cstr_arg(project_dir),
        cstr_arg(project_file),
    ) else {
        return std::ptr::null_mut();
    };
    std::panic::catch_unwind(|| match Reamesh::host(&name, &dir, &file) {
        Ok(r) => Box::into_raw(r),
        Err(e) => {
            eprintln!("[reamesh] host failed: {e}");
            std::ptr::null_mut()
        }
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Start browsing for sessions on the LAN. Downloaded projects are written
/// into fresh subdirectories of `staging_dir` (created as needed). Returns
/// null on failure. Release with `reamesh_shutdown`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_join(staging_dir: *const c_char) -> *mut Reamesh {
    let Some(dir) = cstr_arg(staging_dir) else {
        return std::ptr::null_mut();
    };
    std::panic::catch_unwind(|| match Reamesh::join(&dir) {
        Ok(r) => Box::into_raw(r),
        Err(e) => {
            eprintln!("[reamesh] join failed: {e}");
            std::ptr::null_mut()
        }
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Sessions currently announced on the LAN, as a JSON array of
/// `{"session_id","session_name","project_file","owner"}`. Free the result
/// with `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_sessions(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        "[]".to_string()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.sessions_json()))
            .unwrap_or_else(|_| "[]".to_string())
    };
    into_c_string(out)
}

/// Join an announced session by id (client handles only). Returns true if the
/// join was started; watch `reamesh_status` for progress and the final
/// project path.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_join_session(handle: *mut Reamesh, session_id: *const c_char) -> bool {
    let (false, Some(sid)) = (handle.is_null(), cstr_arg(session_id)) else {
        return false;
    };
    let r = unsafe { &*handle };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.join_session(&sid)))
        .unwrap_or(false)
}

/// Current session status as JSON:
/// `{"state","progress","project_path","session_id","session_name","message"}`
/// where state is hosting|browsing|connecting|downloading|ready|failed.
/// Free the result with `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_status(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        "{}".to_string()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.status_json()))
            .unwrap_or_else(|_| "{}".to_string())
    };
    into_c_string(out)
}

/// Feed a locally-observed changeset (JSON, see schema in README) into the
/// session. On the owner this is authoritative; on clients it is a proposal
/// the owner may reject. Non-blocking; never calls back into the host.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_push_local(handle: *mut Reamesh, changeset_json: *const c_char) {
    if handle.is_null() || changeset_json.is_null() {
        return;
    }
    let r = unsafe { &*handle };
    let json = unsafe { CStr::from_ptr(changeset_json) }.to_string_lossy();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.push_local(&json)));
}

/// Return changesets the host must apply, as a JSON array ("[]" when none):
/// authoritative remote changes, plus (on clients) reverts of rejected
/// proposals. Free the result with `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_drain_remote(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        "[]".to_string()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.drain_remote()))
            .unwrap_or_else(|_| "[]".to_string())
    };
    into_c_string(out)
}

/// Number of peers currently in this session (for status display).
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_peer_count(handle: *mut Reamesh) -> u32 {
    if handle.is_null() {
        return 0;
    }
    let r = unsafe { &*handle };
    r.connected_peers() as u32
}

/// Free a string returned by any reamesh_* function.
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
