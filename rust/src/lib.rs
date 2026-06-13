//! ReaMesh core: yrs CRDT document + libp2p LAN swarm behind a C ABI.
//!
//! v2 model: **owner-authoritative sessions**. One peer (the Owner) hosts a
//! session for its open project; other peers (Clients) discover it on a
//! LAN-wide lobby topic, join, download the full project directory over a
//! request-response transfer protocol, then collaborate live.
//!
//! Authority: the owner's yrs document is the single source of truth and the
//! sole lock arbiter. The owner is not necessarily the active editor.
//! Owner-local edits apply to the doc directly and are broadcast as
//! authoritative updates (`'A'` + yrs update bytes) on the session topic.
//! Client-local edits are *proposals* (`'P'` + changeset JSON): only the Owner
//! consumes them, applies the admitted fields to the doc and re-broadcasts the
//! result. Admission uses a **soft editing lock** (`LOCK_HOLD`): whoever first
//! edits a field holds it exclusively until `LOCK_HOLD` after their last touch;
//! anyone else's edit to that field, including the owner's, is rejected
//! and snapped back. The owner republishes the live lock table (`'L'`) so peers
//! snap rejected edits back at once and can name the holder. Clients keep their
//! optimistic local value while a proposal is in flight and revert to the
//! authoritative value when the lock table shows someone else holds the field
//! (or, as a fallback, after `PROPOSAL_TIMEOUT`).
//!
//! Threading model (unchanged from v1): the host (the REAPER extension) calls
//! every FFI function from REAPER's main thread; all networking runs on a
//! private Tokio runtime owned by `Reamesh`. The two sides meet only at
//! thread-safe queues/flags and a mutex around the yrs document.
//!
//! Document model: root `Map`s named "tracks" and "items" (the *sections*),
//! keyed by REAPER GUID; each value is a nested `Map` of scalar fields.
//! Deleted entities are tombstoned with `present: false`, never removed.
//! Authority, proposals and reconciliation are per (section, id, field).

mod ctl;

use std::collections::{HashMap, VecDeque};
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
const ITEMS_ROOT: &str = "items";
/// Project-wide state that is neither a track nor an item: tempo map and
/// markers/regions, synced as a single entity under a well-known id.
const PROJECT_ROOT: &str = "project";
/// Root maps of the shared doc; every changeset is shaped
/// `{ "<section>": { "<id>": { fields... } } }` with any subset of sections.
const SECTIONS: [&str; 3] = [TRACKS_ROOT, ITEMS_ROOT, PROJECT_ROOT];
const LOBBY_TOPIC: &str = "reamesh/v2/lobby";
const XFER_PROTOCOL: &str = "/reamesh/xfer/1";
/// Max payload per file chunk over the transfer protocol.
const CHUNK_SIZE: u64 = 1024 * 1024;
/// How many chunk requests a downloader keeps in flight at once. Pipelining:
/// the link stays full instead of idling one round-trip per chunk.
const DOWNLOAD_WINDOW: usize = 12;
/// Soft editing lock (Word/Docs style): whoever edits a field holds it
/// exclusively for this long after their LAST touch. While a field is held,
/// edits to it from anyone else are rejected and snap back with a "X is editing
/// this" notice. The hold refreshes on each touch by the holder and
/// lapses this long after they stop, freeing the field for the next editor. It
/// is symmetric: a client locks the owner out of a field it is editing exactly
/// as the owner locks out a client (the owner is the arbiter, not a privileged
/// editor). The owner is the single serialization point, so a genuine
/// dead-simultaneous first touch resolves to whoever the owner processes first
/// (its own local edits beat an inbound proposal by the network delay).
const LOCK_HOLD: Duration = Duration::from_millis(2000);
/// Fallback for a client's optimistic value when a proposal is never confirmed
/// and no lock broadcast arrives to explain it (lost in transit). The owner's
/// lock broadcast normally snaps a rejected edit back within a tick, well
/// inside this; it only bounds the worst case.
const PROPOSAL_TIMEOUT: Duration = Duration::from_millis(2500);
/// Lobby announcements older than this are considered gone.
const SESSION_STALE: Duration = Duration::from_secs(10);
/// Cap on authoritative updates queued for the host to drain. Far above any
/// legitimate burst (a full-state sync is one message); a flood past this is
/// dropped rather than allowed to exhaust memory.
const MAX_PENDING_UPDATES: usize = 4096;
/// Companion byte cap on the same queue. The per-message `max_transmit_size` is
/// 64 MB, so bounding only the *count* would still let ~256 GB accumulate;
/// bounding total bytes keeps the worst case to a fixed, modest ceiling.
const MAX_PENDING_BYTES: usize = 256 * 1024 * 1024;

// Session-topic message tags.
const TAG_AUTHORITATIVE: u8 = b'A';
const TAG_PROPOSAL: u8 = b'P';
/// Lock table: `'L'` + JSON `{"locks":[[section,id,field,holder]...]}`, owner
/// -> all. The owner is the sole lock arbiter; it republishes the live set
/// whenever it changes so every peer knows who is editing what (to snap a
/// rejected edit back at once, name the holder, and (later) show it).
const TAG_LOCKS: u8 = b'L';
/// Presence beacon: `'N'` + JSON `{"name","color"}`, published by every
/// session participant every ~1s so peers can show who is in the session.
const TAG_PRESENCE: u8 = b'N';
/// Presence entries not refreshed within this window are dropped.
const PRESENCE_STALE: Duration = Duration::from_secs(8);
/// Session farewell: published by the owner right before it stops hosting so
/// clients end cleanly instead of waiting out a silence.
const TAG_SESSION_END: u8 = b'E';
/// Client fallback: if the owner has been gone from the session topic this
/// long (crash, network drop: no farewell seen), treat the session as ended.
const OWNER_GONE_TIMEOUT: Duration = Duration::from_secs(12);
/// How long a joining client waits for the owner to answer the manifest
/// request before the join fails (covers an owner that died between the
/// announcement and the join: the lobby entry stays cached for a while).
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Post-manifest safety net: a download that has not written a single new
/// byte for this long is declared dead instead of requeueing pieces forever.
/// The owner-gone fallback usually fires first; this covers an owner that is
/// still on the topic but no longer serving.
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(60);

type Fields = JsonMap<String, JsonValue>;
type SectionState = HashMap<String, Fields>;
/// section -> id -> fields
type StateMap = HashMap<String, SectionState>;
/// (section, id, field)
type FieldKey = (String, String, String);

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
    /// blake3 of the file's (uncompressed) contents, hex. Lets a downloader
    /// skip files it already has byte-for-byte (resume/dedup) and verify each
    /// completed file.
    #[serde(default)]
    hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
enum XferRequest {
    /// Initial project manifest. `auth` is the session token (empty for open
    /// sessions); the owner refuses to serve without the right one.
    Manifest { auth: String },
    /// Fetch of a single project-relative file at `offset`. Served by every
    /// session peer (not just the owner) from its project copy: this is what
    /// both the windowed initial download and mid-session media fetch ride.
    File { path: String, offset: u64, auth: String },
}

#[derive(Debug, Serialize, Deserialize)]
enum XferResponse {
    Manifest {
        session_id: String,
        session_name: String,
        project_file: String,
        files: Vec<FileMeta>,
    },
    /// Response to `File`: `size` is the file's full *uncompressed* length so
    /// the requester knows when it is done; `data` is one chunk, zstd-deflated
    /// when `compressed` (project text files only: audio is left raw).
    FileChunk {
        size: u64,
        compressed: bool,
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
    /// Whether the session is passphrase-protected, and the per-session salt a
    /// client mixes with the passphrase to reproduce the access token.
    auth: bool,
    salt: String,
    last_seen: Instant,
}

#[derive(Clone, Debug)]
struct StatusState {
    /// "hosting" | "browsing" | "connecting" | "downloading" | "ready" |
    /// "failed" | "ended" (the owner stopped the session)
    state: String,
    progress: f64,
    project_path: Option<String>,
    session_id: Option<String>,
    session_name: Option<String>,
    message: Option<String>,
    /// Why state is "ended": "host_ended" (deliberate farewell) or
    /// "host_lost" (owner vanished: a reconnect attempt is worthwhile).
    end_reason: Option<String>,
}

impl StatusState {
    fn to_json(&self, reverts: usize, reverted: Vec<JsonValue>, probe: JsonValue) -> String {
        json!({
            "state": self.state,
            "progress": self.progress,
            "project_path": self.project_path,
            "session_id": self.session_id,
            "session_name": self.session_name,
            "message": self.message,
            "end_reason": self.end_reason,
            "reverts": reverts,
            // Tail of recent (section,id,by) snap-backs, most recent last. The
            // cumulative `reverts` counter tells the consumer how many are new,
            // so it can name *what* snapped back and *who* holds it.
            "reverted": reverted,
            // Passphrase pre-check outcome: {session_id, ok|null} or null.
            "probe": probe,
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
    /// Owner only: (section, id, field) -> who holds the soft editing lock and
    /// when they last touched it. A lock is "live" for `LOCK_HOLD` after its
    /// last touch; see `lock_holder`. The holder is a peer id string (the
    /// owner's own id for owner-held fields).
    locks: HashMap<FieldKey, Lock>,
    /// Client only: (section, id, field) -> (proposed value, when proposed).
    pending: HashMap<FieldKey, (JsonValue, Instant)>,
}

#[derive(Clone, Debug)]
struct Lock {
    holder: String,
    touched: Instant,
}

/// The current holder of a field's lock, or None if unlocked / lapsed. Prunes
/// the entry when it has lapsed so the map stays bounded and so a later edit by
/// the same peer is treated as a fresh acquisition (not a refresh of a dead
/// lock).
fn lock_holder(locks: &mut HashMap<FieldKey, Lock>, key: &FieldKey, now: Instant) -> Option<String> {
    match locks.get(key) {
        Some(l) if now.duration_since(l.touched) < LOCK_HOLD => Some(l.holder.clone()),
        Some(_) => {
            locks.remove(key);
            None
        }
        None => None,
    }
}

/// The live lock table as `[[section, id, field, holder] ...]`, pruning lapsed
/// entries. Used by the owner to broadcast who is editing what.
fn live_locks(locks: &mut HashMap<FieldKey, Lock>, now: Instant) -> Vec<(FieldKey, String)> {
    locks.retain(|_, l| now.duration_since(l.touched) < LOCK_HOLD);
    let mut out: Vec<(FieldKey, String)> = locks
        .iter()
        .map(|(k, l)| (k.clone(), l.holder.clone()))
        .collect();
    out.sort();
    out
}

/// Serialize a live-lock list to the `{"locks":[[section,id,field,holder]...]}`
/// wire shape (TAG_LOCKS payload).
fn lock_table_json(locks: &[(FieldKey, String)]) -> Vec<u8> {
    let arr: Vec<JsonValue> = locks
        .iter()
        .map(|((s, id, f), h)| json!([s, id, f, h]))
        .collect();
    json!({ "locks": arr }).to_string().into_bytes()
}

/// Parse a TAG_LOCKS payload back into a `(section,id,field) -> holder` map.
fn parse_lock_table(v: &JsonValue) -> HashMap<FieldKey, String> {
    let mut map = HashMap::new();
    if let Some(arr) = v.get("locks").and_then(|x| x.as_array()) {
        for e in arr {
            if let Some(t) = e.as_array()
                && t.len() == 4
                && let (Some(s), Some(id), Some(f), Some(h)) =
                    (t[0].as_str(), t[1].as_str(), t[2].as_str(), t[3].as_str())
            {
                map.insert((s.to_string(), id.to_string(), f.to_string()), h.to_string());
            }
        }
    }
    map
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
    /// Directory this peer serves (and fetches) session files from: the
    /// project dir on the owner, the downloaded copy on clients (set once the
    /// initial transfer completes).
    serve_dir: Mutex<Option<PathBuf>>,
    /// Peer ids currently subscribed to the session topic (fetch candidates).
    session_peers: Mutex<Vec<PeerId>>,
    /// Access token every peer in this session shares (empty = open session).
    /// Requests carry it; serving peers refuse mismatches. Set on the owner at
    /// host time and on a client when it joins (derived from the salt in the
    /// announcement + the passphrase the user supplied).
    auth_token: Mutex<String>,
    /// Latest *authenticated* presence beacon per peer id string (display
    /// name + color). Beacons whose MAC doesn't verify never land here.
    peers_info: Mutex<HashMap<String, PeerPresence>>,
    /// Peer id string of the session owner (self when hosting).
    owner_peer: Mutex<String>,
    /// Client only: how many of this peer's edits were snapped back because
    /// someone else held the field's lock (cumulative). Surfaced via status so
    /// the UI can tell the user their edit didn't stick rather than letting it
    /// silently revert.
    reverts: AtomicUsize,
    /// Client only: (section, id, holder) of recently snapped-back fields, most
    /// recent last, bounded. Pairs with `reverts` so the UI can name *what* was
    /// reverted (e.g. the track) and *who* was editing it.
    reverts_detail: Mutex<VecDeque<(String, String, String)>>,
    /// The live editing-lock table as last seen: the owner stores what it last
    /// broadcast (to detect changes worth resending); a client stores what the
    /// owner last sent. Keyed (section, id, field) -> holder peer id. Lets a
    /// client snap a rejected edit back at once and name who holds the field.
    peer_locks: Mutex<HashMap<FieldKey, String>>,
    /// Owner only: fields whose owner-local edit was rejected because a client
    /// holds the lock. drain_remote re-emits the authoritative (holder's) value
    /// for these so the owner's own REAPER snaps back: the lock binds the owner
    /// too. Carries the holder id for the "X is editing this" notice.
    force_revert: Mutex<Vec<(FieldKey, String)>>,
    /// Result of the latest passphrase pre-check (`check_passphrase`):
    /// `(session_id, Some(ok))` once it resolves, `(session_id, None)` while in
    /// flight, `None` when no probe is active. Lets the app validate a
    /// passphrase against the owner *before* it shows the trust dialog or starts
    /// the download, so a wrong passphrase shows an inline error in the input
    /// instead of flashing the whole join animation.
    probe_result: Mutex<Option<(String, Option<bool>)>>,
}

#[derive(Clone, Debug)]
struct PeerPresence {
    name: String,
    color: String,
    /// GUID of the track this peer is currently focused on ("" if none).
    focus: String,
    /// Play-cursor position in project seconds (sweeps while `playing`).
    play: f64,
    /// True while this peer's transport is rolling (the playhead is sweeping).
    playing: bool,
    /// Edit-cursor position in project seconds, drawn as a static caret.
    cursor: f64,
    /// Time selection / loop range (project seconds); `tsa == tsb` means none.
    tsa: f64,
    tsb: f64,
    /// GUIDs of the tracks and items this peer has selected: the "Alice is
    /// holding these" set the overlay draws as tinted ghosts.
    seltracks: Vec<String>,
    selitems: Vec<String>,
    last_seen: Instant,
}

/// Selection arrays are capped before they ride the ~1s beacon so a
/// select-all can't balloon presence traffic.
const PRESENCE_SEL_CAP: usize = 128;

/// Process-wide display profile (name, color hex), set by the companion app
/// through the control server (or `reamesh_set_profile`). Global rather than
/// per-session: it describes the user, not the session, and must survive
/// host/join/leave cycles.
static PROFILE: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

pub fn set_profile(name: &str, color: &str) {
    let mut p = PROFILE.lock().unwrap();
    p.0 = name.chars().take(64).collect();
    p.1 = color.chars().take(16).collect();
}

/// The local user's live "where am I" state, set by the extension each tick and
/// gossiped on the presence beacon so peers can show where each person is:
/// which track they're focused on, where their playhead sits, and what they
/// have selected. Like PROFILE it's process-global: it describes the user, not
/// the session. GUIDs (never indices) are the shared identity; peers resolve
/// them against their own project.
#[derive(Clone)]
struct LivePresence {
    focus: String,
    play: f64,
    playing: bool,
    /// Edit-cursor position (project seconds), distinct from `play`, which is
    /// the play cursor while rolling. Drawn as a static caret.
    cursor: f64,
    /// Time selection / loop range (project seconds); `tsa == tsb` means none.
    tsa: f64,
    tsb: f64,
    seltracks: Vec<String>,
    selitems: Vec<String>,
}

static LIVE: Mutex<LivePresence> = Mutex::new(LivePresence {
    focus: String::new(),
    play: 0.0,
    playing: false,
    cursor: 0.0,
    tsa: 0.0,
    tsb: 0.0,
    seltracks: Vec::new(),
    selitems: Vec::new(),
});

pub fn set_focus(guid: &str) {
    LIVE.lock().unwrap().focus = guid.chars().take(64).collect();
}

/// Replace the live presence from a JSON blob the extension composes each tick:
/// `{"focus","play","playing","seltr":[guid…],"seli":[guid…]}`. Any missing
/// field resets to its empty/zero default; selection arrays are capped.
pub fn set_presence(json: &str) {
    let Ok(v) = serde_json::from_str::<JsonValue>(json) else {
        return;
    };
    let guids = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|g| g.as_str().map(|s| s.chars().take(64).collect()))
                    .take(PRESENCE_SEL_CAP)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut p = LIVE.lock().unwrap();
    p.focus = v
        .get("focus")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .chars()
        .take(64)
        .collect();
    p.play = v.get("play").and_then(|x| x.as_f64()).unwrap_or(0.0);
    p.playing = v.get("playing").and_then(|x| x.as_bool()).unwrap_or(false);
    p.cursor = v.get("cursor").and_then(|x| x.as_f64()).unwrap_or(0.0);
    p.tsa = v.get("tsa").and_then(|x| x.as_f64()).unwrap_or(0.0);
    p.tsb = v.get("tsb").and_then(|x| x.as_f64()).unwrap_or(0.0);
    p.seltracks = guids("seltr");
    p.selitems = guids("seli");
}

/// Stable fallback identity color for a peer that never told us one: a
/// saturated hue picked from the peer id, so every UI derives the same color
/// for the same person without coordination.
fn default_color(peer_id: &str) -> String {
    let h = blake3::hash(peer_id.as_bytes());
    let hue = (u16::from_le_bytes([h.as_bytes()[0], h.as_bytes()[1]]) % 360) as f64;
    hsl_to_hex(hue, 0.62, 0.58)
}

fn hsl_to_hex(h: f64, s: f64, l: f64) -> String {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8
    )
}

/// Token a peer must present to download from / propose to a session.
/// Derived from the public per-session salt and the secret passphrase, so only
/// holders of the passphrase can produce it. Empty passphrase = open session.
///
/// Uses Argon2id (memory-hard) rather than a plain hash: the salt is broadcast
/// in the clear on the lobby, so a fast hash would let a LAN eavesdropper
/// brute-force a weak passphrase offline. Both peers derive the same token from
/// the same (salt, passphrase), so the KDF must be deterministic: hence the
/// low-level `hash_password_into` with the public salt rather than a random
/// PHC salt.
fn auth_token(salt: &str, passphrase: &str) -> String {
    if passphrase.is_empty() {
        return String::new();
    }
    // Domain-separate and guarantee the salt meets Argon2's 8-byte minimum.
    let salted = format!("reamesh/v3/auth\0{salt}");
    let argon = argon2::Argon2::default();
    let mut out = [0u8; 32];
    match argon.hash_password_into(passphrase.as_bytes(), salted.as_bytes(), &mut out) {
        Ok(()) => hex_encode(&out),
        Err(e) => {
            eprintln!("[reamesh] auth_token derivation failed: {e}");
            String::new()
        }
    }
}

/// MAC carried by every presence beacon, binding it to the session token and
/// the sending peer id. Peers only list roster entries with a verifying MAC,
/// so a LAN peer that merely subscribes to the (publicly-announced) session
/// topic can neither appear in anyone's roster nor spoof another member's
/// name/color: producing a valid MAC requires the token, and using someone
/// else's MAC would require publishing as their (transport-authenticated)
/// peer id. Open sessions have an empty token: every participant derives the
/// same key, so the check degrades to "knows the ReaMesh presence protocol".
fn presence_mac(auth_token: &str, peer_id: &str) -> String {
    let key = blake3::derive_key("reamesh/v3/presence", auth_token.as_bytes());
    blake3::keyed_hash(&key, peer_id.as_bytes()).to_hex().to_string()
}

/// Lower-case hex of a byte slice (no external dep for this one use).
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// Cryptographically-random lower-case hex string of `n_bytes` bytes.
fn random_hex(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    if let Err(e) = getrandom::getrandom(&mut buf) {
        // getrandom only fails if the OS RNG is unavailable; fall back to a
        // time/address probe rather than panicking. (Salt secrecy is not the
        // security property (the passphrase is) but uniqueness still matters.)
        eprintln!("[reamesh] getrandom failed ({e}); using weak salt fallback");
        return new_session_id();
    }
    hex_encode(&buf)
}

/// Constant-time byte-slice equality. The length difference is allowed to leak
/// (token length is fixed and not secret); the contents are not compared with
/// an early-exit, so a timing side-channel can't recover the token byte by
/// byte.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

enum Command {
    PublishAuthoritative(Vec<u8>),
    PublishProposal(Vec<u8>),
    JoinSession { session_id: String, passphrase: String },
    /// Pre-check a passphrase against the owner without joining (a lone manifest
    /// request whose accept/reject answers "is this passphrase right?").
    ProbeAuth { session_id: String, passphrase: String },
    FetchFile(String),
    Dial(Multiaddr),
    Shutdown,
}

/// Owner-side session description handed to the network task.
struct Hosting {
    session_id: String,
    session_name: String,
    project_file: String,
    files: Vec<FileMeta>,
    /// Public per-session salt announced to clients, and whether a passphrase
    /// is required at all. The secret token itself lives in `Shared::auth_token`.
    salt: String,
    auth: bool,
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
    /// Signalled by the network task right before it exits. Drop waits on it
    /// (bounded) so the owner's session-end farewell is flushed to the wire
    /// before the runtime tear-down cancels the task mid-send.
    net_done: std_mpsc::Receiver<()>,
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
        passphrase: &str,
    ) -> anyhow::Result<Box<Self>> {
        let dir = PathBuf::from(project_dir);
        if !dir.is_dir() {
            anyhow::bail!("project directory not found: {project_dir}");
        }
        let files = build_manifest(&dir)?;
        let session_id = new_session_id();
        let salt = random_hex(16); // fresh CSPRNG public salt, distinct from the id
        let token = auth_token(&salt, passphrase);
        let hosting = Hosting {
            session_id: session_id.clone(),
            session_name: session_name.to_string(),
            project_file: project_file.to_string(),
            files,
            salt,
            auth: !passphrase.is_empty(),
        };
        let status = StatusState {
            state: "hosting".into(),
            progress: 1.0,
            project_path: None,
            session_id: Some(session_id),
            session_name: Some(session_name.to_string()),
            message: None,
            end_reason: None,
        };
        let r = Self::start(
            Role::Owner,
            NetConfig {
                role: Role::Owner,
                hosting: Some(hosting),
                staging_dir: None,
            },
            status,
        )?;
        *r.shared.serve_dir.lock().unwrap() = Some(dir);
        *r.shared.auth_token.lock().unwrap() = token;
        *r.shared.owner_peer.lock().unwrap() = r.local_peer_id.clone();
        Ok(r)
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
            end_reason: None,
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
        for section in SECTIONS {
            doc.get_or_insert_map(section);
        }

        let shared = Arc::new(Shared {
            doc: Mutex::new(DocState {
                doc,
                last_state: StateMap::new(),
                locks: HashMap::new(),
                pending: HashMap::new(),
            }),
            inbound: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(false),
            listen_addrs: Mutex::new(Vec::new()),
            topic_peers: AtomicUsize::new(0),
            sessions: Mutex::new(HashMap::new()),
            status: Mutex::new(status),
            serve_dir: Mutex::new(None),
            session_peers: Mutex::new(Vec::new()),
            auth_token: Mutex::new(String::new()),
            peers_info: Mutex::new(HashMap::new()),
            owner_peer: Mutex::new(String::new()),
            reverts: AtomicUsize::new(0),
            reverts_detail: Mutex::new(VecDeque::new()),
            peer_locks: Mutex::new(HashMap::new()),
            force_revert: Mutex::new(Vec::new()),
            probe_result: Mutex::new(None),
        });
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (init_tx, init_rx) = std_mpsc::channel();
        let (done_tx, done_rx) = std_mpsc::channel();

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
                let _ = done_tx.send(());
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
            net_done: done_rx,
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
        if !SECTIONS
            .iter()
            .any(|s| parsed.get(s).and_then(|v| v.as_object()).is_some())
        {
            return;
        }

        match self.role {
            Role::Owner => {
                let owner_id = self.local_peer_id.to_string();
                let mut ds = self.shared.doc.lock().unwrap();
                let now = Instant::now();
                // Split by the soft editing lock. The owner keeps a field only
                // if it's unlocked or already owner-held: acquiring/refreshing
                // the lock; a field a *client* holds is rejected and queued for
                // a snap-back (the lock binds the owner too, so the active
                // editor wins even against the host).
                let mut kept = JsonMap::new();
                let mut rejected: Vec<((String, String, String), String)> = Vec::new();
                for section in SECTIONS {
                    let Some(entities) = parsed.get(section).and_then(|t| t.as_object()) else {
                        continue;
                    };
                    let mut sec_out = JsonMap::new();
                    for (id, fields) in entities {
                        let Some(fields) = fields.as_object() else { continue };
                        let mut keep = Fields::new();
                        for (key, value) in fields {
                            let fk = (section.to_string(), id.clone(), key.clone());
                            match lock_holder(&mut ds.locks, &fk, now) {
                                Some(h) if h != owner_id => rejected.push((fk, h)),
                                _ => {
                                    ds.locks.insert(
                                        fk,
                                        Lock { holder: owner_id.clone(), touched: now },
                                    );
                                    keep.insert(key.clone(), value.clone());
                                }
                            }
                        }
                        if !keep.is_empty() {
                            sec_out.insert(id.clone(), JsonValue::Object(keep));
                        }
                    }
                    if !sec_out.is_empty() {
                        kept.insert(section.to_string(), JsonValue::Object(sec_out));
                    }
                }
                if !rejected.is_empty() {
                    let mut fr = self.shared.force_revert.lock().unwrap();
                    fr.extend(rejected);
                    // The owner's REAPER already moved; make drain_remote run so
                    // it snaps back even if nothing else changed.
                    self.shared.dirty.store(true, Ordering::SeqCst);
                }
                if !kept.is_empty() {
                    if let Some(update) =
                        apply_changeset_to_doc(&mut ds, &JsonValue::Object(kept))
                    {
                        // Re-baseline so this local change never re-surfaces from
                        // drain_remote.
                        ds.last_state = snapshot(&ds.doc);
                        drop(ds);
                        let _ = self.cmd_tx.send(Command::PublishAuthoritative(update));
                    }
                }
            }
            Role::Client => {
                let mut ds = self.shared.doc.lock().unwrap();
                let now = Instant::now();
                for section in SECTIONS {
                    let Some(entities) = parsed.get(section).and_then(|t| t.as_object()) else {
                        continue;
                    };
                    for (id, fields) in entities {
                        if let Some(fields) = fields.as_object() {
                            for (key, value) in fields {
                                // Normalize through the same Any round-trip the
                                // doc snapshot uses so confirmation compares hold.
                                let norm = any_to_json(&json_to_any(value));
                                ds.pending.insert(
                                    (section.into(), id.clone(), key.clone()),
                                    (norm, now),
                                );
                            }
                        }
                    }
                }
                drop(ds);
                // Wrap the changeset in an authenticated envelope: the owner
                // only admits proposals carrying the session token, so topic
                // membership alone can't write to the doc. `age_ms` is how long
                // ago (on this clock) the edit was made; the owner re-anchors it
                // to its own clock for skew-free tie-breaking. Proposals are
                // sent the instant an edit is captured, so it is ~0 here, but
                // carrying it keeps the comparison correct under any delay.
                let envelope = json!({
                    "auth": self.shared.auth_token.lock().unwrap().clone(),
                    "cs": parsed,
                    "age_ms": now.elapsed().as_millis() as u64,
                });
                let _ = self
                    .cmd_tx
                    .send(Command::PublishProposal(envelope.to_string().into_bytes()));
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
        let now = Instant::now();
        let own_id = self.local_peer_id.to_string();

        // Helpers: add/remove one field from the diff being returned to REAPER.
        // `diff` is the changeset object {section -> {id -> {field -> value}}}.
        type Diff = JsonMap<String, JsonValue>;
        let put = |diff: &mut Diff, section: &str, id: &str, field: &str, v: JsonValue| {
            diff.entry(section.to_string())
                .or_insert_with(|| JsonValue::Object(JsonMap::new()))
                .as_object_mut()
                .unwrap()
                .entry(id.to_string())
                .or_insert_with(|| JsonValue::Object(JsonMap::new()))
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), v);
        };
        let drop_field = |diff: &mut Diff, section: &str, id: &str, field: &str| {
            if let Some(m) = diff
                .get_mut(section)
                .and_then(|s| s.get_mut(id))
                .and_then(|v| v.as_object_mut())
            {
                m.remove(field);
            }
        };
        let count_revert = |section: &str, id: &str, holder: &str| {
            self.shared.reverts.fetch_add(1, Ordering::Relaxed);
            let mut d = self.shared.reverts_detail.lock().unwrap();
            d.push_back((section.to_string(), id.to_string(), holder.to_string()));
            while d.len() > 32 {
                d.pop_front();
            }
        };

        // Owner: snap back the owner's own edits that a client's lock rejected
        // (push_local queued them in force_revert). Re-emit the authoritative
        // (holder's) value so the owner's REAPER reverts, and surface it like a
        // client revert so the owner is told who is editing that field: the
        // lock binds the host too.
        if self.role == Role::Owner {
            let queued =
                std::mem::take(&mut *self.shared.force_revert.lock().unwrap());
            for ((section, id, field), holder) in queued {
                if let Some(dv) = new_state
                    .get(&section)
                    .and_then(|s| s.get(&id))
                    .and_then(|f| f.get(&field))
                    .cloned()
                {
                    count_revert(&section, &id, &holder);
                    put(&mut diff, &section, &id, &field, dv);
                }
            }
        }

        // Client-side proposal reconciliation (pending is always empty on the
        // owner). A field someone else now holds is snapped back at once (the
        // owner's lock broadcast names the holder); an unexplained miss falls
        // back to the proposal timeout.
        let locks = self.shared.peer_locks.lock().unwrap();
        let pending = &mut ds.pending;
        pending.retain(|(section, id, field), (proposed, when)| {
            let doc_val = new_state
                .get(section)
                .and_then(|s| s.get(id))
                .and_then(|f| f.get(field));
            if doc_val == Some(proposed) {
                // Confirmed by the owner; suppress the no-op echo.
                drop_field(&mut diff, section, id, field);
                return false;
            }
            let holder = locks.get(&(section.clone(), id.clone(), field.clone()));
            let held_by_other = holder.is_some_and(|h| *h != own_id);
            if held_by_other || now.duration_since(*when) > PROPOSAL_TIMEOUT {
                // Lost the field: snap REAPER back to the authoritative value
                // and record who holds it (when known) so the UI can name them.
                if let Some(dv) = doc_val {
                    let who = holder.filter(|h| **h != own_id).cloned().unwrap_or_default();
                    count_revert(section, id, &who);
                    put(&mut diff, section, id, field, dv.clone());
                }
                false
            } else {
                // Still in flight (held by us, or no word yet): keep optimistic.
                drop_field(&mut diff, section, id, field);
                true
            }
        });
        drop(locks);
        // Drop empty entities, then empty sections.
        for (_, section) in diff.iter_mut() {
            if let Some(s) = section.as_object_mut() {
                s.retain(|_, v| v.as_object().is_none_or(|m| !m.is_empty()));
            }
        }
        diff.retain(|_, s| s.as_object().is_none_or(|m| !m.is_empty()));

        ds.last_state = new_state;
        if diff.is_empty() {
            "[]".to_string()
        } else {
            json!([diff]).to_string()
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
                    "auth": s.auth,
                })
            })
            .collect();
        json!(arr).to_string()
    }

    /// Join an announced session (client only): subscribe to its topic and
    /// download the project files. Progress is reported via `status_json`.
    pub fn join_session(&self, session_id: &str, passphrase: &str) -> bool {
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
            .send(Command::JoinSession {
                session_id: session_id.to_string(),
                passphrase: passphrase.to_string(),
            })
            .is_ok()
    }

    /// Pre-check a passphrase against the owner without joining (client only).
    /// Fires a lone manifest request; the owner's accept/reject answers "is this
    /// passphrase right?". The outcome surfaces in `status_json` under `probe`.
    /// The app calls this when the user clicks Join so a wrong passphrase shows
    /// an inline error instead of starting the whole download animation.
    pub fn check_passphrase(&self, session_id: &str, passphrase: &str) -> bool {
        if self.role != Role::Client {
            return false;
        }
        if !self.shared.sessions.lock().unwrap().contains_key(session_id) {
            return false;
        }
        // Mark the probe in flight so the app can show "checking…".
        *self.shared.probe_result.lock().unwrap() = Some((session_id.to_string(), None));
        self.cmd_tx
            .send(Command::ProbeAuth {
                session_id: session_id.to_string(),
                passphrase: passphrase.to_string(),
            })
            .is_ok()
    }

    /// Current session status as JSON:
    /// `{"state","progress","project_path","session_id","session_name",
    ///   "message","end_reason","reverts","reverted"}`.
    pub fn status_json(&self) -> String {
        let reverts = self.shared.reverts.load(Ordering::Relaxed);
        let reverted: Vec<JsonValue> = self
            .shared
            .reverts_detail
            .lock()
            .unwrap()
            .iter()
            .map(|(s, i, by)| json!({ "section": s, "id": i, "by": by }))
            .collect();
        let probe = match &*self.shared.probe_result.lock().unwrap() {
            Some((sid, ok)) => json!({ "session_id": sid, "ok": ok }),
            None => JsonValue::Null,
        };
        self.shared
            .status
            .lock()
            .unwrap()
            .to_json(reverts, reverted, probe)
    }

    /// Full current doc state in changeset shape. For tests and debugging.
    pub fn snapshot_json(&self) -> String {
        let ds = self.shared.doc.lock().unwrap();
        let state = snapshot(&ds.doc);
        let mut out = JsonMap::new();
        for section in SECTIONS {
            let ordered: std::collections::BTreeMap<&String, &Fields> = state
                .get(section)
                .map(|s| s.iter().collect())
                .unwrap_or_default();
            out.insert(section.to_string(), json!(ordered));
        }
        JsonValue::Object(out).to_string()
    }

    /// Fetch a project-relative file from the session peers into this peer's
    /// project copy (no-op if already present or already being fetched). The
    /// file appears atomically: it exists on disk only once it is complete.
    pub fn fetch_file(&self, rel_path: &str) -> bool {
        if safe_relative_path(rel_path).is_none() {
            return false;
        }
        self.cmd_tx
            .send(Command::FetchFile(rel_path.to_string()))
            .is_ok()
    }

    pub fn local_peer_id(&self) -> &str {
        &self.local_peer_id
    }

    /// Everyone visibly in the session: self first, then the session-topic
    /// peers with an *authenticated* presence beacon, as a JSON array of
    /// `{"id","name","color","is_self","is_owner"}`. A topic subscriber that
    /// never produced a verifying beacon (no session token) is not listed:
    /// topic membership is public knowledge, roster membership is not. A
    /// legitimate joiner appears within one beacon interval (~1s).
    pub fn peers_json(&self) -> String {
        let owner = self.shared.owner_peer.lock().unwrap().clone();
        let (my_name, my_color) = PROFILE.lock().unwrap().clone();
        let me = LIVE.lock().unwrap().clone();
        #[allow(clippy::too_many_arguments)]
        let entry = |id: &str,
                     name: &str,
                     color: &str,
                     focus: &str,
                     play: f64,
                     playing: bool,
                     cursor: f64,
                     tsa: f64,
                     tsb: f64,
                     seltracks: &[String],
                     selitems: &[String],
                     is_self: bool| {
            json!({
                "id": id,
                "name": name,
                "color": if color.is_empty() { default_color(id) } else { color.to_string() },
                "focus": focus,
                "play": play,
                "playing": playing,
                "cursor": cursor,
                "tsa": tsa,
                "tsb": tsb,
                "seltr": seltracks,
                "seli": selitems,
                "is_self": is_self,
                "is_owner": id == owner,
            })
        };
        let mut arr = vec![entry(
            &self.local_peer_id,
            &my_name,
            &my_color,
            &me.focus,
            me.play,
            me.playing,
            me.cursor,
            me.tsa,
            me.tsb,
            &me.seltracks,
            &me.selitems,
            true,
        )];
        let mut infos = self.shared.peers_info.lock().unwrap();
        let now = Instant::now();
        infos.retain(|_, p| now.duration_since(p.last_seen) < PRESENCE_STALE);
        for p in self.shared.session_peers.lock().unwrap().iter() {
            let id = p.to_string();
            if let Some(info) = infos.get(&id) {
                arr.push(entry(
                    &id,
                    &info.name,
                    &info.color,
                    &info.focus,
                    info.play,
                    info.playing,
                    info.cursor,
                    info.tsa,
                    info.tsb,
                    &info.seltracks,
                    &info.selitems,
                    false,
                ));
            }
        }
        json!(arr).to_string()
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
        // Wait (bounded) for the network task to exit on its own: tearing the
        // runtime down would cancel it at its next await, before the owner's
        // session-end farewell ever reaches the socket. Clients exit
        // immediately; owners after their ~250ms farewell flush.
        let _ = self.net_done.recv_timeout(Duration::from_secs(1));
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

/// Apply a parsed changeset (any subset of sections) into the doc. Returns
/// the encoded yrs update when something actually changed. Does NOT touch
/// `last_state`: callers decide whether the change should surface from
/// drain_remote.
fn apply_changeset_to_doc(ds: &mut DocState, changeset: &JsonValue) -> Option<Vec<u8>> {
    let roots: Vec<(&str, MapRef)> = SECTIONS
        .iter()
        .map(|s| (*s, ds.doc.get_or_insert_map(*s)))
        .collect();
    let mut txn = ds.doc.transact_mut();
    let mut changed = false;
    for (section, root) in roots {
        let Some(entities) = changeset.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        for (id, fields) in entities {
            let Some(fields) = fields.as_object() else { continue };
            let entity: MapRef = root.get_or_init(&mut txn, id.as_str());
            for (key, value) in fields {
                if entity.try_update(&mut txn, key.as_str(), json_to_any(value)) {
                    changed = true;
                }
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
    // Dotfiles also cover in-progress ".reamesh-part-*" fetch temp files.
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
                let hash = blake3_file(&dir.join(&rel_child)).unwrap_or_default();
                out.push(FileMeta { path, size: meta.len(), hash });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// blake3 of a file's contents as a hex string, streamed so large media never
/// loads fully into memory.
fn blake3_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Whether a project-relative file is worth compressing: text-shaped project
/// data (.rpp and friends) shrinks ~10x; audio is already dense, so it is left
/// raw to avoid burning CPU for nothing.
fn is_compressible(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".rpp", ".rtracktemplate", ".rfxchain", ".txt", ".reapeaks", ".xml", ".csv"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Decompress a zstd stream, refusing to allocate more than `limit` bytes.
/// A compressed chunk arrives from an arbitrary session-topic peer (no auth is
/// required to answer a File request), so an unbounded `decode_all` would let a
/// crafted ~1 MB stream inflate to gigabytes and OOM the receiver. Every
/// legitimate chunk is at most `CHUNK_SIZE`, so anything larger is hostile.
fn zstd_decode_bounded(data: &[u8], limit: usize) -> std::io::Result<Vec<u8>> {
    let dec = zstd::stream::Decoder::new(data)?;
    let mut out = Vec::new();
    // Read one past the limit so an over-long stream is detected, not truncated.
    dec.take(limit as u64 + 1).read_to_end(&mut out)?;
    if out.len() > limit {
        return Err(std::io::Error::other("decompressed chunk exceeds limit"));
    }
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
        // QUIC (UDP) connects in fewer round-trips than TCP and avoids
        // head-of-line blocking between concurrent file chunks; libp2p picks
        // whichever transport a peer's advertised address uses, falling back
        // to TCP. Same identity keypair as the TCP transport.
        .with_quic()
        .with_behaviour(|key| {
            // Item/FX-chain state chunks ride gossipsub; plugin state blobs
            // (samplers etc.) can be tens of MB.
            let gs_config = gossipsub::ConfigBuilder::default()
                .max_transmit_size(64 * 1024 * 1024)
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
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    Ok(swarm)
}

/// One file in an in-progress download. Written to a hidden temp at chunk
/// offsets (out of order is fine) and renamed to `dest` when whole.
struct FileDl {
    meta: FileMeta,
    tmp: PathBuf, // equals `dest` for files already present (never requested)
    dest: PathBuf,
    file: Option<fs::File>,
    remaining: u64, // chunks not yet written
    tries: u32,     // hash-mismatch re-downloads
}

/// One outstanding chunk: which file index, at what byte offset.
#[derive(Clone, Copy, Debug)]
struct Piece {
    file: usize,
    offset: u64,
}

/// Client-side state of an in-progress project download. Pieces are pulled
/// from `queue` into a sliding window of in-flight requests (pipelining), each
/// sent to a peer chosen round-robin from `peers` (swarming: other clients
/// that already have the project serve too); files already present
/// byte-for-byte are skipped (dedup/resume). Each file lands atomically.
struct Download {
    owner: PeerId,
    /// Joined session id: the session topic is subscribed only once the owner
    /// accepts our token (manifest served), never at request time.
    session_id: String,
    dir: PathBuf,
    project_file: String,
    auth: String,
    got_manifest: bool,
    manifest_req: Option<request_response::OutboundRequestId>,
    files: Vec<FileDl>,
    queue: VecDeque<Piece>,
    inflight: HashMap<request_response::OutboundRequestId, Piece>,
    peers: Vec<PeerId>,
    next_peer: usize,
    total: u64,
    done: u64,
    last_activity: Instant,
    /// When the join started: bounds how long we keep re-requesting a
    /// manifest from an owner that never answers.
    started: Instant,
    /// Last time a new byte actually landed (or the manifest arrived):
    /// bounds how long pieces are requeued without any forward progress.
    last_progress: Instant,
}

/// Chunk offsets for a file of `size` bytes.
fn file_pieces(file: usize, size: u64) -> Vec<Piece> {
    let mut v = Vec::new();
    let mut off = 0;
    while off < size {
        v.push(Piece { file, offset: off });
        off += CHUNK_SIZE;
    }
    v
}

impl Download {
    fn request(&self, p: Piece) -> XferRequest {
        XferRequest::File {
            path: self.files[p.file].meta.path.clone(),
            offset: p.offset,
            auth: self.auth.clone(),
        }
    }
}

/// Mid-session fetch of one project-relative file from the session peers.
/// Written to a hidden ".part" temp name and renamed when complete, so the
/// destination path existing means the file is whole.
struct Fetch {
    rel: String,
    candidates: Vec<PeerId>,
    idx: usize,
    tmp: PathBuf,
    dest: PathBuf,
    out: Option<fs::File>,
    written: u64,
    last_activity: Instant,
    attempts: u32,
    auth: String,
}

impl Fetch {
    fn request(&self) -> XferRequest {
        XferRequest::File {
            path: self.rel.clone(),
            offset: self.written,
            auth: self.auth.clone(),
        }
    }

    fn peer(&self) -> Option<PeerId> {
        self.candidates.get(self.idx).copied()
    }
}

type FetchMap = HashMap<String, Fetch>;
type FetchReqs = HashMap<request_response::OutboundRequestId, String>;

/// Passphrase pre-check, matched by request id. A manifest confirms access;
/// errors distinguish rejection, a ban, and an unreachable host.
struct Probe {
    req: request_response::OutboundRequestId,
    session_id: String,
    started: Instant,
}

/// Deadline for a passphrase pre-check. Expiry reports a timeout,
/// not an incorrect passphrase.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

fn fetch_send(
    swarm: &mut Swarm<Behaviour>,
    fetch_reqs: &mut FetchReqs,
    fetch: &mut Fetch,
) -> bool {
    let Some(peer) = fetch.peer() else { return false };
    fetch.last_activity = Instant::now();
    let id = swarm.behaviour_mut().xfer.send_request(&peer, fetch.request());
    fetch_reqs.insert(id, fetch.rel.clone());
    true
}

fn start_fetch(
    swarm: &mut Swarm<Behaviour>,
    shared: &Shared,
    fetches: &mut FetchMap,
    fetch_reqs: &mut FetchReqs,
    rel: String,
) {
    if fetches.contains_key(&rel) {
        return;
    }
    let Some(dir) = shared.serve_dir.lock().unwrap().clone() else { return };
    let Some(rel_path) = safe_relative_path(&rel) else { return };
    let dest = dir.join(&rel_path);
    if dest.exists() {
        return;
    }
    let candidates = shared.session_peers.lock().unwrap().clone();
    if candidates.is_empty() {
        return; // the host re-requests on its tick once peers are around
    }
    let tmp = dest.with_file_name(format!(
        ".reamesh-part-{}",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut fetch = Fetch {
        rel,
        candidates,
        idx: 0,
        tmp,
        dest,
        out: None,
        written: 0,
        last_activity: Instant::now(),
        attempts: 0,
        auth: shared.auth_token.lock().unwrap().clone(),
    };
    if fetch_send(swarm, fetch_reqs, &mut fetch) {
        fetches.insert(fetch.rel.clone(), fetch);
    }
}

/// Feed a response (or failure: `response = None`) for a fetch request id.
fn handle_fetch_response(
    swarm: &mut Swarm<Behaviour>,
    fetches: &mut FetchMap,
    fetch_reqs: &mut FetchReqs,
    rel: &str,
    response: Option<XferResponse>,
) {
    let Some(fetch) = fetches.get_mut(rel) else { return };

    match response {
        Some(XferResponse::FileChunk { size, compressed, data }) => {
            fetch.attempts = 0;
            let chunk = if compressed {
                match zstd_decode_bounded(&data, CHUNK_SIZE as usize) {
                    Ok(d) => d,
                    Err(_) => return, // corrupt/oversized; in-order request stays in flight
                }
            } else {
                data
            };
            let expected = CHUNK_SIZE.min(size.saturating_sub(fetch.written)) as usize;
            if chunk.len() != expected {
                return; // stale retry; the in-order request stays in flight
            }
            let mut write = || -> std::io::Result<bool> {
                if fetch.out.is_none() {
                    if let Some(parent) = fetch.tmp.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fetch.out = Some(fs::File::create(&fetch.tmp)?);
                    fetch.written = 0;
                }
                fetch.out.as_mut().unwrap().write_all(&chunk)?;
                fetch.written += chunk.len() as u64;
                if fetch.written >= size {
                    fetch.out = None; // close before rename
                    fs::rename(&fetch.tmp, &fetch.dest)?;
                    return Ok(true);
                }
                Ok(false)
            };
            match write() {
                Ok(true) => {
                    eprintln!("[reamesh] fetched {} ({size} bytes)", fetch.rel);
                    fetches.remove(rel);
                }
                Ok(false) => {
                    let mut fetch = fetches.remove(rel).unwrap();
                    fetch_send(swarm, fetch_reqs, &mut fetch);
                    fetches.insert(rel.to_string(), fetch);
                }
                Err(e) => {
                    eprintln!("[reamesh] fetch {} failed: {e}", fetch.rel);
                    let f = fetches.remove(rel);
                    if let Some(f) = f {
                        let _ = fs::remove_file(&f.tmp);
                    }
                }
            }
        }
        _ => {
            // Any other response (peer cannot serve the file) or an outright
            // request failure (None): start over with the next candidate; out
            // of candidates, give up: the host re-requests while the file is
            // missing.
            fetch.idx += 1;
            fetch.out = None;
            fetch.written = 0;
            fetch.attempts = 0;
            let _ = fs::remove_file(&fetch.tmp);
            if fetch.peer().is_some() {
                let mut fetch = fetches.remove(rel).unwrap();
                fetch_send(swarm, fetch_reqs, &mut fetch);
                fetches.insert(rel.to_string(), fetch);
            } else {
                fetches.remove(rel);
            }
        }
    }
}

fn set_status(shared: &Shared, f: impl FnOnce(&mut StatusState)) {
    f(&mut shared.status.lock().unwrap());
}

/// Handler for transfer requests. Every request carries the session access
/// token, checked against `auth_token` (empty = open session) before anything
/// is served. `Manifest` is owner-only; `File` is served by every peer that
/// has a project copy (which powers both the windowed initial download and
/// mid-session media fetch). Project text chunks are returned zstd-compressed.
fn serve_xfer(
    hosting: Option<&Hosting>,
    serve_dir: Option<&Path>,
    auth_token: &str,
    req: &XferRequest,
) -> XferResponse {
    let presented = match req {
        XferRequest::Manifest { auth } | XferRequest::File { auth, .. } => auth.as_str(),
    };
    if !ct_eq(presented.as_bytes(), auth_token.as_bytes()) {
        return XferResponse::Error {
            message: "wrong or missing session passphrase".into(),
        };
    }
    match req {
        XferRequest::File { path, offset, .. } => {
            let Some(dir) = serve_dir else {
                return XferResponse::Error {
                    message: "no project copy to serve from".into(),
                };
            };
            let Some(rel) = safe_relative_path(path) else {
                return XferResponse::Error {
                    message: format!("unsafe path: {path}"),
                };
            };
            let full = dir.join(rel);
            let compress = is_compressible(path);
            let read = || -> std::io::Result<(u64, bool, Vec<u8>)> {
                let mut f = fs::File::open(&full)?;
                let size = f.metadata()?.len();
                f.seek(SeekFrom::Start(*offset))?;
                let want = CHUNK_SIZE.min(size.saturating_sub(*offset)) as usize;
                let mut buf = vec![0u8; want];
                f.read_exact(&mut buf)?;
                if compress {
                    if let Ok(z) = zstd::encode_all(&buf[..], 3) {
                        return Ok((size, true, z));
                    }
                }
                Ok((size, false, buf))
            };
            match read() {
                Ok((size, compressed, data)) => XferResponse::FileChunk { size, compressed, data },
                Err(e) => XferResponse::Error {
                    message: format!("read {path}: {e}"),
                },
            }
        }
        XferRequest::Manifest { .. } => match hosting {
            Some(h) => XferResponse::Manifest {
                session_id: h.session_id.clone(),
                session_name: h.session_name.clone(),
                project_file: h.project_file.clone(),
                files: h.files.clone(),
            },
            None => XferResponse::Error {
                message: "not hosting".into(),
            },
        },
    }
}

/// Owner-side proposal admission under the soft editing lock. A field is
/// admitted (and `proposer` becomes/refreshes its lock holder) unless someone
/// else currently holds it: i.e. the proposer is the active editor, or the
/// field is free. A field a different peer holds is rejected; the proposer's
/// own lock broadcast will snap it back. `proposer` is the authenticated
/// gossipsub source. Returns the update to re-broadcast.
fn admit_proposal(shared: &Shared, payload: &[u8], proposer: &str) -> Option<Vec<u8>> {
    let envelope: JsonValue = serde_json::from_slice(payload).ok()?;
    // Gate live edits on the session token, exactly as file serving is gated in
    // serve_xfer: subscribing to the (publicly-announced) session topic must
    // not by itself authorize writing to the authoritative doc.
    let presented = envelope.get("auth").and_then(|v| v.as_str()).unwrap_or("");
    let expected = shared.auth_token.lock().unwrap().clone();
    if !ct_eq(presented.as_bytes(), expected.as_bytes()) {
        eprintln!("[reamesh] proposal rejected: wrong or missing session token");
        return None;
    }
    let parsed = envelope.get("cs")?;

    let mut ds = shared.doc.lock().unwrap();
    let now = Instant::now();
    let mut admitted = JsonMap::new();
    for section in SECTIONS {
        let Some(entities) = parsed.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        let mut sec_out = JsonMap::new();
        for (id, fields) in entities {
            let Some(fields) = fields.as_object() else { continue };
            let mut keep = Fields::new();
            for (key, value) in fields {
                let fk = (section.to_string(), id.clone(), key.clone());
                match lock_holder(&mut ds.locks, &fk, now) {
                    Some(h) if h != proposer => {
                        eprintln!(
                            "[reamesh] proposal for {section}/{id}.{key} rejected (held by {h})"
                        );
                    }
                    _ => {
                        // Free or already the proposer's: grant + refresh.
                        ds.locks.insert(
                            fk,
                            Lock { holder: proposer.to_string(), touched: now },
                        );
                        keep.insert(key.clone(), value.clone());
                    }
                }
            }
            if !keep.is_empty() {
                sec_out.insert(id.clone(), JsonValue::Object(keep));
            }
        }
        if !sec_out.is_empty() {
            admitted.insert(section.to_string(), JsonValue::Object(sec_out));
        }
    }
    if admitted.is_empty() {
        return None;
    }
    let update = apply_changeset_to_doc(&mut ds, &JsonValue::Object(admitted))?;
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
            "auth": h.auth,
            "salt": h.salt,
        })
        .to_string()
        .into_bytes()
    });

    let mut download: Option<Download> = None;
    // Mid-session file fetches, keyed by project-relative path, plus the
    // outstanding request ids that belong to them (request-response gives us
    // back the id, which is how fetch responses are told apart from the
    // initial project download).
    let mut fetches: FetchMap = HashMap::new();
    let mut fetch_reqs: FetchReqs = HashMap::new();
    // In-flight passphrase pre-check (at most one at a time).
    let mut probe: Option<Probe> = None;

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
    // Client: last time the session owner was seen on the session topic (for
    // the owner-vanished fallback below).
    let mut last_owner_seen = Instant::now();

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
                Some(Command::ProbeAuth { session_id, passphrase }) => {
                    // Lone manifest request: the owner accepts iff the derived
                    // token is right, so the response answers "is this passphrase
                    // correct?" without any download or topic subscription.
                    let info = shared.sessions.lock().unwrap().get(&session_id).cloned();
                    match info.and_then(|i| i.owner.parse::<PeerId>().ok().map(|o| (o, i.salt))) {
                        Some((owner, salt)) => {
                            let token = auth_token(&salt, &passphrase);
                            let req = swarm
                                .behaviour_mut()
                                .xfer
                                .send_request(&owner, XferRequest::Manifest { auth: token });
                            probe = Some(Probe { req, session_id, started: Instant::now() });
                        }
                        None => {
                            // Session gone or unparseable owner -> can't verify.
                            *shared.probe_result.lock().unwrap() = Some((session_id, Some(false)));
                        }
                    }
                }
                Some(Command::JoinSession { session_id, passphrase }) => {
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
                    // Derive (and remember) this session's access token so we
                    // both present it and can serve other peers later.
                    let token = auth_token(&info.salt, &passphrase);
                    *shared.auth_token.lock().unwrap() = token.clone();
                    *shared.owner_peer.lock().unwrap() = info.owner.clone();
                    // Deliberately NOT subscribing to the session topic yet:
                    // joining the topic is what makes a peer visible in the
                    // session (peer lists, presence). That visibility must be
                    // gated on the owner accepting our token; otherwise a
                    // wrong-passphrase joiner flashes up as a ghost peer on
                    // everyone's roster until its download fails. The
                    // subscribe happens when the manifest arrives.
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
                        s.end_reason = None;
                    });
                    shared.reverts.store(0, Ordering::Relaxed);
                    shared.reverts_detail.lock().unwrap().clear();
                    shared.peer_locks.lock().unwrap().clear();
                    shared.force_revert.lock().unwrap().clear();
                    // Drop any proposals left in flight from a previous session
                    // (e.g. before an auto-rejoin): their stale timestamps would
                    // otherwise time out and surface as phantom snap-backs the
                    // moment we reconnect.
                    shared.doc.lock().unwrap().pending.clear();
                    let req = swarm
                        .behaviour_mut()
                        .xfer
                        .send_request(&owner, XferRequest::Manifest { auth: token.clone() });
                    download = Some(Download {
                        owner,
                        session_id: session_id.clone(),
                        dir,
                        project_file: info.project_file.clone(),
                        auth: token,
                        got_manifest: false,
                        manifest_req: Some(req),
                        files: Vec::new(),
                        queue: VecDeque::new(),
                        inflight: HashMap::new(),
                        peers: vec![owner],
                        next_peer: 0,
                        total: 0,
                        done: 0,
                        last_activity: Instant::now(),
                        started: Instant::now(),
                        last_progress: Instant::now(),
                    });
                }
                Some(Command::FetchFile(rel)) => {
                    start_fetch(&mut swarm, &shared, &mut fetches, &mut fetch_reqs, rel);
                }
                Some(Command::Dial(addr)) => {
                    if let Err(e) = swarm.dial(addr) {
                        eprintln!("[reamesh] dial failed: {e}");
                    }
                }
                Some(Command::Shutdown) | None => {
                    // Farewell, then keep driving the swarm briefly so it
                    // actually leaves the socket before the runtime is torn
                    // down. Owner: a session-end broadcast (clients that miss
                    // it are covered by their OWNER_GONE_TIMEOUT fallback).
                    // Client: a topic unsubscribe, so the other peers drop it
                    // from their rosters immediately instead of waiting out
                    // the presence staleness window.
                    if let Some(t) = &sess_topic {
                        if hosting.is_some() {
                            let _ = swarm.behaviour_mut().gossipsub.publish(t.clone(), vec![TAG_SESSION_END]);
                        } else {
                            swarm.behaviour_mut().gossipsub.unsubscribe(t);
                        }
                        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
                        loop {
                            tokio::select! {
                                _ = tokio::time::sleep_until(deadline) => break,
                                _ = swarm.select_next_some() => {}
                            }
                        }
                    }
                    break;
                }
            },
            _ = tick.tick() => {
                tick_count += 1;
                // End an unanswered passphrase check with a timeout result.
                if probe.as_ref().is_some_and(|p| p.started.elapsed() > PROBE_TIMEOUT) {
                    let p = probe.take().unwrap();
                    *shared.probe_result.lock().unwrap() = Some((p.session_id, Some(false)));
                }
                // Recompute session-topic peers from gossipsub's view. (Event
                // counting misses subscriptions learned before we joined the
                // topic ourselves.)
                if let Some(t) = &sess_topic {
                    let hash = t.hash();
                    let peers: Vec<PeerId> = swarm.behaviour().gossipsub.all_peers()
                        .filter(|(_, topics)| topics.iter().any(|th| **th == hash))
                        .map(|(p, _)| *p)
                        .collect();
                    shared.topic_peers.store(peers.len(), Ordering::Relaxed);
                    *shared.session_peers.lock().unwrap() = peers;
                }
                // Client fallback: the owner vanished from the session topic
                // (crash, quit without the farewell reaching us): end the
                // session instead of leaving the user stranded in a dead one.
                // Covers an in-progress download too (the topic is subscribed
                // from the manifest on), so a host that dies mid-transfer
                // doesn't leave the client requeueing pieces forever.
                if cfg.role == Role::Client && sess_topic.is_some() {
                    let owner = shared.owner_peer.lock().unwrap().clone();
                    let owner_present = !owner.is_empty()
                        && shared
                            .session_peers
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|p| p.to_string() == owner);
                    if owner_present {
                        last_owner_seen = Instant::now();
                    } else if last_owner_seen.elapsed() > OWNER_GONE_TIMEOUT {
                        let st = shared.status.lock().unwrap().state.clone();
                        if st == "ready" || st == "downloading" {
                            eprintln!("[reamesh] the owner disappeared; ending the session");
                            download_cleanup(&mut download);
                            set_status(&shared, |s| {
                                s.state = "ended".into();
                                s.message = Some("lost the connection to the host".into());
                                s.end_reason = Some("host_lost".into());
                            });
                        }
                    }
                }
                // Owner: announce the session on the lobby every ~1s.
                if let (Some(bytes), true) = (&announce, tick_count % 2 == 0) {
                    let _ = swarm.behaviour_mut().gossipsub.publish(lobby.clone(), bytes.clone());
                }
                // Everyone: presence beacon on the session topic every ~1s so
                // peers can show who is here: name, color, focused track,
                // playhead and selection.
                if tick_count % 2 == 0 && let Some(t) = &sess_topic {
                    let (name, color) = PROFILE.lock().unwrap().clone();
                    let live = LIVE.lock().unwrap().clone();
                    // The MAC proves token possession; peers only roster
                    // beacons that verify (see presence_mac).
                    let mac = presence_mac(
                        &shared.auth_token.lock().unwrap().clone(),
                        &swarm.local_peer_id().to_string(),
                    );
                    let mut data = vec![TAG_PRESENCE];
                    data.extend_from_slice(
                        json!({
                            "name": name,
                            "color": color,
                            "focus": live.focus,
                            "play": live.play,
                            "playing": live.playing,
                            "cursor": live.cursor,
                            "tsa": live.tsa,
                            "tsb": live.tsb,
                            "seltr": live.seltracks,
                            "seli": live.selitems,
                            "mac": mac,
                        })
                        .to_string()
                        .as_bytes(),
                    );
                    let _ = swarm.behaviour_mut().gossipsub.publish(t.clone(), data);
                }
                // Owner: republish the editing-lock table whenever the live set
                // changes (an acquisition or a lapse), so peers can snap a
                // rejected edit back at once and name who holds each field.
                // `peer_locks` doubles as the owner's last-broadcast cache (the
                // owner never receives a lock table; only clients do).
                if hosting.is_some() && let Some(t) = &sess_topic {
                    let live = {
                        let mut ds = shared.doc.lock().unwrap();
                        live_locks(&mut ds.locks, Instant::now())
                    };
                    let current: HashMap<FieldKey, String> = live.iter().cloned().collect();
                    let changed = {
                        let mut last = shared.peer_locks.lock().unwrap();
                        if *last != current {
                            *last = current;
                            true
                        } else {
                            false
                        }
                    };
                    if changed {
                        let mut data = vec![TAG_LOCKS];
                        data.extend_from_slice(&lock_table_json(&live));
                        let _ = swarm.behaviour_mut().gossipsub.publish(t.clone(), data);
                    }
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
                // Nudge stalled mid-session file fetches.
                let stalled: Vec<String> = fetches
                    .values()
                    .filter(|f| f.last_activity.elapsed() > Duration::from_secs(3))
                    .map(|f| f.rel.clone())
                    .collect();
                for rel in stalled {
                    let mut fetch = fetches.remove(&rel).unwrap();
                    fetch.attempts += 1;
                    if fetch.attempts > 5 {
                        // Restart on the next candidate from scratch.
                        fetch.idx += 1;
                        fetch.out = None;
                        fetch.written = 0;
                        fetch.attempts = 0;
                        let _ = fs::remove_file(&fetch.tmp);
                    }
                    if fetch.peer().is_some() && fetch_send(&mut swarm, &mut fetch_reqs, &mut fetch) {
                        fetches.insert(rel, fetch);
                    } else {
                        let _ = fs::remove_file(&fetch.tmp);
                    }
                }
                if fetch_reqs.len() > 1024 {
                    fetch_reqs.retain(|_, rel| fetches.contains_key(rel));
                }
                // Client: keep the download's request window full, fold in any
                // newly-arrived peers (swarming), and recover from stalls.
                download_tick(&mut swarm, &shared, &mut download);
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
                            // Bind the announcement to its authenticated signer:
                            // the claimed `owner` peer id must match the message
                            // source. Otherwise any LAN peer could announce a
                            // session naming someone else as owner, steering a
                            // joining client's manifest request (and trust) at an
                            // attacker-chosen peer.
                            let owner = get("owner");
                            let signer_is_owner = message
                                .source
                                .is_some_and(|src| src.to_string() == owner);
                            let info = SessionInfo {
                                session_id: get("session_id"),
                                session_name: get("session_name"),
                                project_file: get("project_file"),
                                owner,
                                auth: v.get("auth").and_then(|x| x.as_bool()).unwrap_or(false),
                                salt: get("salt"),
                                last_seen: Instant::now(),
                            };
                            if signer_is_owner && !info.session_id.is_empty() {
                                shared.sessions.lock().unwrap().insert(info.session_id.clone(), info);
                            }
                        }
                    } else if sess_topic.as_ref().is_some_and(|st| message.topic == st.hash()) {
                        match (message.data.first(), cfg.role) {
                            (Some(&TAG_PRESENCE), _) => {
                                // Signed gossipsub messages carry the author.
                                if let Some(src) = message.source
                                    && let Ok(v) = serde_json::from_slice::<JsonValue>(&message.data[1..])
                                {
                                    let get = |k: &str| {
                                        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
                                    };
                                    let get_guids = |k: &str| -> Vec<String> {
                                        v.get(k)
                                            .and_then(|x| x.as_array())
                                            .map(|a| {
                                                a.iter()
                                                    .filter_map(|g| g.as_str().map(String::from))
                                                    .take(PRESENCE_SEL_CAP)
                                                    .collect()
                                            })
                                            .unwrap_or_default()
                                    };
                                    // Only token holders may appear in the
                                    // roster: verify the beacon's MAC against
                                    // the sender's authenticated peer id.
                                    let expected = presence_mac(
                                        &shared.auth_token.lock().unwrap().clone(),
                                        &src.to_string(),
                                    );
                                    if !ct_eq(get("mac").as_bytes(), expected.as_bytes()) {
                                        eprintln!(
                                            "[reamesh] dropped unauthenticated presence beacon from {src}"
                                        );
                                        continue;
                                    }
                                    shared.peers_info.lock().unwrap().insert(
                                        src.to_string(),
                                        PeerPresence {
                                            name: get("name"),
                                            color: get("color"),
                                            focus: get("focus"),
                                            play: v.get("play").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                            playing: v
                                                .get("playing")
                                                .and_then(|x| x.as_bool())
                                                .unwrap_or(false),
                                            cursor: v.get("cursor").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                            tsa: v.get("tsa").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                            tsb: v.get("tsb").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                            seltracks: get_guids("seltr"),
                                            selitems: get_guids("seli"),
                                            last_seen: Instant::now(),
                                        },
                                    );
                                }
                            }
                            (Some(&TAG_AUTHORITATIVE), Role::Client) => {
                                // Authority is the owner's alone. gossipsub lets
                                // *any* topic member publish, so without this
                                // check a non-owner LAN peer (who only needs the
                                // publicly-announced session id, not the
                                // passphrase) could inject authoritative updates,
                                // including item or FX chunks that load arbitrary
                                // plugin code, into every client. Signed
                                // messages carry an authenticated source, so we
                                // accept 'A' only from the known owner peer.
                                let from_owner = message.source.is_some_and(|src| {
                                    src.to_string() == *shared.owner_peer.lock().unwrap()
                                });
                                if !from_owner {
                                    eprintln!(
                                        "[reamesh] dropped authoritative update from non-owner {:?}",
                                        message.source
                                    );
                                    continue;
                                }
                                // Bound the queue (by count and by total bytes) so
                                // a compromised owner can't pin unbounded memory by
                                // publishing faster than the host drains. yrs
                                // updates are idempotent and the owner re-sends full
                                // state on (re)subscribe, so dropping under a flood
                                // is safe.
                                let mut q = shared.inbound.lock().unwrap();
                                let queued_bytes: usize = q.iter().map(|u| u.len()).sum();
                                if q.len() < MAX_PENDING_UPDATES
                                    && queued_bytes + message.data.len() <= MAX_PENDING_BYTES
                                {
                                    q.push(message.data[1..].to_vec());
                                    shared.dirty.store(true, Ordering::SeqCst);
                                }
                            }
                            (Some(&TAG_SESSION_END), Role::Client) => {
                                // Only the owner may end the session (any topic
                                // member can publish; signed messages carry the
                                // authenticated source).
                                let from_owner = message.source.is_some_and(|src| {
                                    src.to_string() == *shared.owner_peer.lock().unwrap()
                                });
                                if from_owner {
                                    eprintln!("[reamesh] the owner ended the session");
                                    // A farewell can also land mid-download;
                                    // abort it rather than pumping requests at
                                    // a host that is gone.
                                    download_cleanup(&mut download);
                                    set_status(&shared, |s| {
                                        s.state = "ended".into();
                                        s.message = Some("the host ended the session".into());
                                        s.end_reason = Some("host_ended".into());
                                    });
                                }
                            }
                            (Some(&TAG_PROPOSAL), Role::Owner) => {
                                // The lock holder is the authenticated source.
                                let proposer =
                                    message.source.map(|p| p.to_string()).unwrap_or_default();
                                if let Some(update) =
                                    admit_proposal(&shared, &message.data[1..], &proposer)
                                {
                                    let mut data = vec![TAG_AUTHORITATIVE];
                                    data.extend_from_slice(&update);
                                    if let Some(t) = &sess_topic
                                        && swarm.behaviour_mut().gossipsub.publish(t.clone(), data).is_err() {
                                        need_full_sync = true;
                                    }
                                }
                            }
                            (Some(&TAG_LOCKS), Role::Client) => {
                                // The owner is the sole lock arbiter; ignore any
                                // table from a non-owner topic member.
                                let from_owner = message.source.is_some_and(|src| {
                                    src.to_string() == *shared.owner_peer.lock().unwrap()
                                });
                                if from_owner
                                    && let Ok(v) =
                                        serde_json::from_slice::<JsonValue>(&message.data[1..])
                                {
                                    *shared.peer_locks.lock().unwrap() = parse_lock_table(&v);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Xfer(request_response::Event::Message { message, .. })) => {
                    match message {
                        request_response::Message::Request { request, channel, .. } => {
                            let serve_dir = shared.serve_dir.lock().unwrap().clone();
                            let token = shared.auth_token.lock().unwrap().clone();
                            let resp = serve_xfer(hosting.as_ref(), serve_dir.as_deref(), &token, &request);
                            let _ = swarm.behaviour_mut().xfer.send_response(channel, resp);
                        }
                        request_response::Message::Response { request_id, response } => {
                            if probe.as_ref().is_some_and(|p| p.req == request_id) {
                                // Passphrase pre-check answer: Manifest = right,
                                // Error = wrong. No download, no subscription.
                                let p = probe.take().unwrap();
                                let ok = matches!(response, XferResponse::Manifest { .. });
                                *shared.probe_result.lock().unwrap() = Some((p.session_id, Some(ok)));
                            } else if download.as_ref().is_some_and(|dl| dl.manifest_req == Some(request_id)) {
                                // A manifest only arrives once the owner accepted
                                // our session token: this is the moment we become
                                // a legitimate session member, so join the topic
                                // now (an Error response never subscribes; the
                                // wrong-passphrase peer stays invisible).
                                if matches!(response, XferResponse::Manifest { .. }) && sess_topic.is_none()
                                    && let Some(dl) = &download
                                {
                                    let t = session_topic(&dl.session_id);
                                    if let Err(e) = swarm.behaviour_mut().gossipsub.subscribe(&t) {
                                        eprintln!("[reamesh] subscribe to session topic failed: {e}");
                                    }
                                    sess_topic = Some(t);
                                    // Fresh anchor for the owner-gone fallback;
                                    // the pre-join idle time must not count.
                                    last_owner_seen = Instant::now();
                                }
                                handle_download_manifest(&mut swarm, &shared, &mut download, response);
                            } else if download.as_ref().is_some_and(|dl| dl.inflight.contains_key(&request_id)) {
                                handle_download_piece(&mut swarm, &shared, &mut download, request_id, Some(response));
                            } else if let Some(rel) = fetch_reqs.remove(&request_id) {
                                handle_fetch_response(&mut swarm, &mut fetches, &mut fetch_reqs, &rel, Some(response));
                            }
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Xfer(request_response::Event::OutboundFailure { peer, error, request_id, .. })) => {
                    eprintln!("[reamesh] transfer request to {peer} failed: {error}");
                    if probe.as_ref().is_some_and(|p| p.req == request_id) {
                        // Report an unreachable host separately from a rejected passphrase.
                        let p = probe.take().unwrap();
                        *shared.probe_result.lock().unwrap() = Some((p.session_id, Some(false)));
                    } else if download.as_ref().is_some_and(|dl| dl.manifest_req == Some(request_id)) {
                        // Manifest never arrived; re-request on the next tick.
                        if let Some(dl) = &mut download {
                            dl.manifest_req = None;
                            dl.last_activity = Instant::now() - Duration::from_secs(10);
                        }
                    } else if download.as_ref().is_some_and(|dl| dl.inflight.contains_key(&request_id)) {
                        handle_download_piece(&mut swarm, &shared, &mut download, request_id, None);
                    } else if let Some(rel) = fetch_reqs.remove(&request_id) {
                        handle_fetch_response(&mut swarm, &mut fetches, &mut fetch_reqs, &rel, None);
                    }
                }
                _ => {}
            },
        }
    }
}

impl Download {
    /// Next serving peer in the round-robin (owner + other session peers).
    fn next_peer(&mut self) -> Option<PeerId> {
        if self.peers.is_empty() {
            return None;
        }
        let p = self.peers[self.next_peer % self.peers.len()];
        self.next_peer = self.next_peer.wrapping_add(1);
        Some(p)
    }
}

/// Existing files under the staging root, indexed by size (cheap: no
/// hashing). Same-size candidates are hashed lazily when a manifest entry
/// might match one, so a re-download reuses bytes already on disk.
fn build_reuse_index(root: &Path) -> HashMap<u64, Vec<PathBuf>> {
    let mut idx: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    let mut budget = 50_000u32; // bound the scan on huge staging roots
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            if budget == 0 {
                return idx;
            }
            budget -= 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() && !name.starts_with(".reamesh-part-") {
                idx.entry(meta.len()).or_default().push(entry.path());
            }
        }
    }
    idx
}

/// Whether `path` already holds exactly the file `meta` describes.
fn file_matches(path: &Path, meta: &FileMeta) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() == meta.size => {
            meta.hash.is_empty() || blake3_file(path).ok().as_deref() == Some(meta.hash.as_str())
        }
        _ => false,
    }
}

/// Turn a manifest into the per-file download plan: skip files already present
/// byte-for-byte (resume), copy in identical files found elsewhere under the
/// staging root (dedup), and queue chunk requests for the rest.
fn download_build(dl: &mut Download, files: Vec<FileMeta>) -> std::io::Result<()> {
    let reuse = build_reuse_index(dl.dir.parent().unwrap_or(&dl.dir));
    for (idx, meta) in files.into_iter().enumerate() {
        let Some(rel) = safe_relative_path(&meta.path) else {
            return Err(std::io::Error::other(format!(
                "unsafe path in manifest: {}",
                meta.path
            )));
        };
        let dest = dl.dir.join(&rel);
        dl.total += meta.size;

        // Already on disk (a prior attempt, or another session's copy)?
        let reused = file_matches(&dest, &meta)
            || (!meta.hash.is_empty()
                && reuse
                    .get(&meta.size)
                    .and_then(|cands| {
                        cands.iter().find(|p| {
                            p.as_path() != dest
                                && blake3_file(p).ok().as_deref() == Some(meta.hash.as_str())
                        })
                    })
                    .map(|src| {
                        dest.parent().map(fs::create_dir_all).transpose().is_ok()
                            && fs::copy(src, &dest).is_ok()
                    })
                    .unwrap_or(false));
        if reused {
            dl.done += meta.size;
            dl.files.push(FileDl { meta, tmp: dest.clone(), dest, file: None, remaining: 0, tries: 0 });
            continue;
        }

        // Need it: write chunks (possibly out of order) into a sparse temp.
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = dest.with_file_name(format!(
            ".reamesh-part-{}",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
        let f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        f.set_len(meta.size)?;
        let pieces = file_pieces(idx, meta.size);
        let remaining = pieces.len() as u64;
        if remaining == 0 {
            drop(f); // empty file: complete immediately
            fs::rename(&tmp, &dest)?;
            dl.files.push(FileDl { meta, tmp, dest, file: None, remaining: 0, tries: 0 });
            continue;
        }
        for p in pieces {
            dl.queue.push_back(p);
        }
        dl.files.push(FileDl { meta, tmp, dest, file: Some(f), remaining, tries: 0 });
    }
    Ok(())
}

/// Fill the in-flight window: pull pending pieces and dispatch each to the
/// next serving peer in the rotation.
fn download_pump(swarm: &mut Swarm<Behaviour>, dl: &mut Download) {
    while dl.inflight.len() < DOWNLOAD_WINDOW {
        let Some(piece) = dl.queue.pop_front() else { break };
        let Some(peer) = dl.next_peer() else {
            dl.queue.push_front(piece);
            break;
        };
        let req = dl.request(piece);
        let id = swarm.behaviour_mut().xfer.send_request(&peer, req);
        dl.inflight.insert(id, piece);
    }
}

/// Drop an in-progress download (if any), removing its partial temp files.
/// Status is the caller's business: this is shared by the failure path and
/// the session-ended paths.
fn download_cleanup(download: &mut Option<Download>) {
    if let Some(dl) = download {
        for f in &dl.files {
            if f.tmp != f.dest {
                let _ = fs::remove_file(&f.tmp);
            }
        }
    }
    *download = None;
}

fn download_fail(download: &mut Option<Download>, shared: &Shared, msg: String) {
    eprintln!("[reamesh] download failed: {msg}");
    download_cleanup(download);
    set_status(shared, |s| {
        s.state = "failed".into();
        s.message = Some(msg);
    });
}

fn download_finalize(shared: &Shared, download: &mut Option<Download>) {
    let Some(dl) = download.as_ref() else { return };
    let path = dl.dir.join(&dl.project_file);
    eprintln!("[reamesh] download complete: {}", path.display());
    // This copy is now what we serve to (and fetch from) other peers.
    *shared.serve_dir.lock().unwrap() = Some(dl.dir.clone());
    set_status(shared, |s| {
        s.state = "ready".into();
        s.progress = 1.0;
        s.project_path = Some(path.to_string_lossy().into_owned());
    });
    *download = None;
}

/// Per-tick download upkeep: refresh the serving-peer set (swarming), recover
/// from stalls by requeueing lost in-flight pieces, and keep the window full.
fn download_tick(swarm: &mut Swarm<Behaviour>, shared: &Shared, download: &mut Option<Download>) {
    let Some(dl) = download.as_mut() else { return };
    let mut peers = vec![dl.owner];
    for p in shared.session_peers.lock().unwrap().iter() {
        if !peers.contains(p) {
            peers.push(*p);
        }
    }
    dl.peers = peers;

    if !dl.got_manifest {
        // The lobby announcement outlives its host by SESSION_STALE, so a
        // join can be aimed at an owner that just died: give up instead of
        // re-requesting the manifest forever.
        if dl.started.elapsed() > MANIFEST_TIMEOUT {
            return download_fail(download, shared, "the host did not respond".into());
        }
        if dl.manifest_req.is_none() {
            let id = swarm
                .behaviour_mut()
                .xfer
                .send_request(&dl.owner, XferRequest::Manifest { auth: dl.auth.clone() });
            dl.manifest_req = Some(id);
            dl.last_activity = Instant::now();
        }
        return;
    }
    // Forward-progress watchdog: requeue-and-retry must not loop forever
    // against peers that accept requests but never deliver.
    if dl.last_progress.elapsed() > DOWNLOAD_STALL_TIMEOUT {
        return download_fail(download, shared, "the download stalled".into());
    }
    if dl.last_activity.elapsed() > Duration::from_secs(5) && !dl.inflight.is_empty() {
        let lost: Vec<Piece> = dl.inflight.drain().map(|(_, p)| p).collect();
        for p in lost {
            dl.queue.push_back(p);
        }
        dl.last_activity = Instant::now();
    }
    download_pump(swarm, dl);
}

/// Client-side: the manifest arrived. Plan the download and start pumping.
fn handle_download_manifest(
    swarm: &mut Swarm<Behaviour>,
    shared: &Shared,
    download: &mut Option<Download>,
    response: XferResponse,
) {
    {
        let Some(dl) = download.as_mut() else { return };
        dl.manifest_req = None;
        dl.last_activity = Instant::now();
        match response {
            XferResponse::Manifest { files, project_file, .. } => {
                if dl.got_manifest {
                    return; // duplicate (a retry raced the response)
                }
                // The owner-supplied project file name is later joined onto the
                // staging dir and handed to Main_openProject; validate it like
                // every other manifest path so a malicious owner can't point the
                // open outside the staging directory.
                if safe_relative_path(&project_file).is_none() {
                    return download_fail(
                        download,
                        shared,
                        format!("unsafe project file in manifest: {project_file}"),
                    );
                }
                if let Err(e) = fs::create_dir_all(&dl.dir) {
                    return download_fail(download, shared, format!("create staging dir: {e}"));
                }
                dl.got_manifest = true;
                dl.last_progress = Instant::now();
                dl.project_file = project_file;
                if let Err(e) = download_build(dl, files) {
                    return download_fail(download, shared, e.to_string());
                }
                eprintln!(
                    "[reamesh] downloading into {} ({} of {} bytes already present)",
                    dl.dir.display(),
                    dl.done,
                    dl.total
                );
                let progress = if dl.total == 0 { 1.0 } else { dl.done as f64 / dl.total as f64 };
                set_status(shared, |s| {
                    s.state = "downloading".into();
                    s.progress = progress;
                });
                download_pump(swarm, dl);
            }
            XferResponse::Error { message } => {
                return download_fail(download, shared, format!("owner refused: {message}"));
            }
            _ => {}
        }
    }
    // Everything may already have been present locally.
    if let Some(dl) = download.as_ref() {
        if dl.got_manifest && dl.queue.is_empty() && dl.inflight.is_empty() {
            download_finalize(shared, download);
        }
    }
}

/// Client-side: a chunk arrived (or a piece request failed: `response` None).
fn handle_download_piece(
    swarm: &mut Swarm<Behaviour>,
    shared: &Shared,
    download: &mut Option<Download>,
    request_id: request_response::OutboundRequestId,
    response: Option<XferResponse>,
) {
    let Some(dl) = download.as_mut() else { return };
    let Some(piece) = dl.inflight.remove(&request_id) else { return };
    dl.last_activity = Instant::now();

    // Decode/validate. On anything wrong (peer can't serve it, corrupt,
    // stale), requeue the piece and keep the window full.
    let chunk: Option<Vec<u8>> = match response {
        Some(XferResponse::FileChunk { compressed, data, .. }) => {
            let bytes = if compressed {
                zstd_decode_bounded(&data, CHUNK_SIZE as usize).ok()
            } else {
                Some(data)
            };
            bytes.filter(|b| {
                let expected =
                    CHUNK_SIZE.min(dl.files[piece.file].meta.size.saturating_sub(piece.offset))
                        as usize;
                b.len() == expected
            })
        }
        _ => None,
    };
    let Some(chunk) = chunk else {
        dl.queue.push_back(piece);
        download_pump(swarm, dl);
        return;
    };

    let fd = &mut dl.files[piece.file];
    let wrote = fd.file.as_mut().map(|f| {
        f.seek(SeekFrom::Start(piece.offset))
            .and_then(|_| f.write_all(&chunk))
    });
    match wrote {
        Some(Ok(())) => {}
        Some(Err(e)) => {
            let dest = fd.dest.display().to_string();
            return download_fail(download, shared, format!("write {dest}: {e}"));
        }
        None => {
            // File already finished (a duplicate/late chunk); ignore.
            download_pump(swarm, dl);
            return;
        }
    }
    fd.remaining -= 1;
    dl.done += chunk.len() as u64;
    dl.last_progress = Instant::now();
    let progress = if dl.total == 0 { 1.0 } else { dl.done as f64 / dl.total as f64 };
    set_status(shared, |s| s.progress = progress);

    if dl.files[piece.file].remaining == 0 {
        let fd = &mut dl.files[piece.file];
        fd.file = None; // close before verifying/renaming
        let ok = fd.meta.hash.is_empty()
            || blake3_file(&fd.tmp).ok().as_deref() == Some(fd.meta.hash.as_str());
        if ok {
            if let Err(e) = fs::rename(&fd.tmp, &fd.dest) {
                let dest = fd.dest.display().to_string();
                return download_fail(download, shared, format!("finalize {dest}: {e}"));
            }
        } else {
            fd.tries += 1;
            if fd.tries > 3 {
                let p = fd.meta.path.clone();
                return download_fail(download, shared, format!("checksum mismatch for {p}"));
            }
            eprintln!("[reamesh] checksum mismatch for {}, re-downloading", fd.meta.path);
            match fs::OpenOptions::new().read(true).write(true).truncate(true).open(&fd.tmp) {
                Ok(f) => {
                    let _ = f.set_len(fd.meta.size);
                    fd.file = Some(f);
                }
                Err(e) => {
                    let tmp = fd.tmp.display().to_string();
                    return download_fail(download, shared, format!("reopen {tmp}: {e}"));
                }
            }
            let size = fd.meta.size;
            let pieces = file_pieces(piece.file, size);
            fd.remaining = pieces.len() as u64;
            dl.done = dl.done.saturating_sub(size);
            for p in pieces {
                dl.queue.push_back(p);
            }
        }
    }

    if dl.queue.is_empty() && dl.inflight.is_empty() {
        return download_finalize(shared, download);
    }
    download_pump(swarm, dl);
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
    let roots: Vec<(&str, MapRef)> = SECTIONS
        .iter()
        .map(|s| (*s, doc.get_or_insert_map(*s)))
        .collect();
    let txn = doc.transact();
    let mut out = StateMap::new();
    for (section, root) in roots {
        let entities = out.entry(section.to_string()).or_default();
        for (id, value) in root.iter(&txn) {
            if let Out::YMap(entity) = value {
                let mut fields = Fields::new();
                for (key, v) in entity.iter(&txn) {
                    if let Out::Any(any) = v {
                        fields.insert(key.to_string(), any_to_json(&any));
                    }
                }
                entities.insert(id.to_string(), fields);
            }
        }
    }
    out
}

/// Diff two doc snapshots into changeset shape: `{section: {id: {changed
/// fields}}}`, omitting empty sections.
fn diff_states(old: &StateMap, new: &StateMap) -> JsonMap<String, JsonValue> {
    let empty = SectionState::new();
    let mut out = JsonMap::new();
    for section in SECTIONS {
        let old_s = old.get(section).unwrap_or(&empty);
        let new_s = new.get(section).unwrap_or(&empty);
        let mut sec_out = JsonMap::new();
        for (id, new_fields) in new_s {
            match old_s.get(id) {
                None => {
                    sec_out.insert(id.clone(), JsonValue::Object(new_fields.clone()));
                }
                Some(old_fields) => {
                    let mut changed = Fields::new();
                    for (key, value) in new_fields {
                        if old_fields.get(key) != Some(value) {
                            changed.insert(key.clone(), value.clone());
                        }
                    }
                    if !changed.is_empty() {
                        sec_out.insert(id.clone(), JsonValue::Object(changed));
                    }
                }
            }
        }
        // Entities are tombstoned (present:false), not removed, so this
        // branch is defensive only.
        for id in old_s.keys() {
            if !new_s.contains_key(id) {
                sec_out.insert(id.clone(), json!({ "present": false }));
            }
        }
        if !sec_out.is_empty() {
            out.insert(section.to_string(), JsonValue::Object(sec_out));
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
/// `passphrase` gates who may download and edit (empty/null = open session).
/// Returns null on failure. Release with `reamesh_shutdown`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_host(
    session_name: *const c_char,
    project_dir: *const c_char,
    project_file: *const c_char,
    passphrase: *const c_char,
) -> *mut Reamesh {
    let (Some(name), Some(dir), Some(file)) = (
        cstr_arg(session_name),
        cstr_arg(project_dir),
        cstr_arg(project_file),
    ) else {
        return std::ptr::null_mut();
    };
    // Null/absent passphrase = open session.
    let pass = cstr_arg(passphrase).unwrap_or_default();
    std::panic::catch_unwind(|| match Reamesh::host(&name, &dir, &file, &pass) {
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

/// Join an announced session by id (client handles only). `passphrase` must
/// match the host's if the session is protected (empty/null otherwise).
/// Returns true if the join was started; watch `reamesh_status` for progress
/// and the final project path.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_join_session(
    handle: *mut Reamesh,
    session_id: *const c_char,
    passphrase: *const c_char,
) -> bool {
    let (false, Some(sid)) = (handle.is_null(), cstr_arg(session_id)) else {
        return false;
    };
    let pass = cstr_arg(passphrase).unwrap_or_default();
    let r = unsafe { &*handle };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.join_session(&sid, &pass)))
        .unwrap_or(false)
}

/// Pre-check a passphrase against an announced session's owner without joining.
/// Returns true if the check was started; watch `reamesh_status`'s `probe`
/// field (`{session_id, ok|null}`) for the result. Used so a wrong passphrase
/// shows an inline error before the join animation / trust dialog.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_check_passphrase(
    handle: *mut Reamesh,
    session_id: *const c_char,
    passphrase: *const c_char,
) -> bool {
    let (false, Some(sid)) = (handle.is_null(), cstr_arg(session_id)) else {
        return false;
    };
    let pass = cstr_arg(passphrase).unwrap_or_default();
    let r = unsafe { &*handle };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.check_passphrase(&sid, &pass)))
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

/// Request a project-relative file ('/'-separated) from the session peers.
/// Non-blocking and idempotent: safe to call every tick while the file is
/// missing. The file appears at its final path only once fully transferred,
/// so "the path exists" is the completion signal.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_fetch_file(handle: *mut Reamesh, rel_path: *const c_char) -> bool {
    let (false, Some(rel)) = (handle.is_null(), cstr_arg(rel_path)) else {
        return false;
    };
    let r = unsafe { &*handle };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.fetch_file(&rel))).unwrap_or(false)
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

/// Everyone visibly in the session, self first, as a JSON array of
/// `{"id","name","color","is_self","is_owner"}`. Names and colors come from
/// the presence beacons peers broadcast; a peer without a profile has an
/// empty name and a color derived from its peer id. Free the result with
/// `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_peers(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        "[]".to_string()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.peers_json()))
            .unwrap_or_else(|_| "[]".to_string())
    };
    into_c_string(out)
}

/// Set the process-wide display profile (name + color hex) included in this
/// peer's presence beacons. Survives host/join/leave; usually set by the
/// companion app through the control server rather than called directly.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_set_profile(name: *const c_char, color: *const c_char) {
    set_profile(
        &cstr_arg(name).unwrap_or_default(),
        &cstr_arg(color).unwrap_or_default(),
    );
}

/// Set the local user's currently-focused track GUID, gossiped on the presence
/// beacon so session peers can show which track this user is on. Pass an empty
/// string to clear it. Called by the extension each tick.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_set_focus(guid: *const c_char) {
    set_focus(&cstr_arg(guid).unwrap_or_default());
}

/// Set the local user's full live presence from a JSON blob
/// `{"focus","play","playing","seltr":[guid…],"seli":[guid…]}`, gossiped on the
/// presence beacon so peers can show where each person is (focused track,
/// playhead, and selected tracks/items). Supersedes `reamesh_set_focus`; the
/// extension calls it each tick. Missing fields reset to empty/zero.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_set_presence(json: *const c_char) {
    set_presence(&cstr_arg(json).unwrap_or_default());
}

/// Start the localhost control server the companion app connects to
/// (idempotent). Returns the bound port, 0 if no port was available.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_ctl_start() -> u16 {
    std::panic::catch_unwind(ctl::start).unwrap_or(0)
}

/// Next command sent by a connected companion app, as a JSON string, or null
/// when none is queued. The extension drains this on its timer tick and
/// dispatches the commands itself (the server never acts on the session).
/// Free non-null results with `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_ctl_poll() -> *mut c_char {
    match std::panic::catch_unwind(ctl::poll).unwrap_or(None) {
        Some(s) => into_c_string(s),
        None => std::ptr::null_mut(),
    }
}

/// Broadcast the extension's composed state JSON to every connected companion
/// app (and replay it to apps that connect later).
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_ctl_publish(state_json: *const c_char) {
    let Some(s) = cstr_arg(state_json) else { return };
    let _ = std::panic::catch_unwind(|| ctl::publish(&s));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_only_identical_slices() {
        assert!(ct_eq(b"", b""));
        assert!(ct_eq(b"deadbeef", b"deadbeef"));
        assert!(!ct_eq(b"deadbeef", b"deadbee0"));
        assert!(!ct_eq(b"short", b"longer"));
    }

    #[test]
    fn auth_token_is_deterministic_and_passphrase_bound() {
        // Empty passphrase => open session => empty token.
        assert_eq!(auth_token("salt", ""), "");
        // Same (salt, passphrase) => same token (both peers must agree).
        let a = auth_token("salt-abc", "hunter2");
        let b = auth_token("salt-abc", "hunter2");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // 32 bytes hex
        // A different passphrase or salt yields a different token.
        assert_ne!(a, auth_token("salt-abc", "hunter3"));
        assert_ne!(a, auth_token("salt-xyz", "hunter2"));
    }

    #[test]
    fn presence_mac_binds_token_and_peer() {
        let a = presence_mac("token-a", "12D3KooPeerOne");
        // Deterministic: both ends derive the same value.
        assert_eq!(a, presence_mac("token-a", "12D3KooPeerOne"));
        assert_eq!(a.len(), 64);
        // A different token or a different sender yields a different MAC: a
        // captured beacon can't be replayed under another identity, and a
        // peer without the token can't produce a verifying one.
        assert_ne!(a, presence_mac("token-b", "12D3KooPeerOne"));
        assert_ne!(a, presence_mac("token-a", "12D3KooPeerTwo"));
        // Open sessions (empty token) still MAC: deterministic too.
        assert_eq!(presence_mac("", "p"), presence_mac("", "p"));
    }

    #[test]
    fn random_hex_is_unique_and_correct_length() {
        let a = random_hex(16);
        let b = random_hex(16);
        assert_eq!(a.len(), 32);
        assert_ne!(a, b, "two CSPRNG salts must not collide");
    }

    #[test]
    fn zstd_decode_bounded_accepts_normal_chunk() {
        let plain = vec![7u8; 4096];
        let comp = zstd::encode_all(&plain[..], 3).unwrap();
        let out = zstd_decode_bounded(&comp, CHUNK_SIZE as usize).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn zstd_decode_bounded_rejects_decompression_bomb() {
        // Highly compressible 8 MB payload -> tiny compressed stream that would
        // inflate well past the 1 MB chunk limit.
        let bomb_plain = vec![0u8; 8 * 1024 * 1024];
        let comp = zstd::encode_all(&bomb_plain[..], 3).unwrap();
        assert!(comp.len() < CHUNK_SIZE as usize, "test setup: bomb must be small compressed");
        assert!(
            zstd_decode_bounded(&comp, CHUNK_SIZE as usize).is_err(),
            "decompression past the limit must be rejected, not allocated"
        );
    }
}
