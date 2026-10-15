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

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString, c_char};
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering, AtomicU64};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use libp2p::futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::SwarmEvent;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::{Multiaddr, PeerId, StreamProtocol, Swarm, gossipsub, mdns, noise, ping, tcp, yamux};
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
/// Compatibility number shared by everything a ReaMesh build talks to: the
/// session wire format (gossip, transfer, changeset JSON) and the control
/// server the ReaMesh app drives. Bump it on *any* change one side could
/// misread. Peers only join sessions announcing the same number, and the app
/// only drives an extension reporting it, so mismatched builds are refused
/// instead of tested. Mirrored in the app (`app/src/lib/version.ts`,
/// `app/src-tauri/src/lib.rs`); app/src/lib/version.test.ts checks the copies
/// agree.
pub const PROTO: u32 = 1;
/// This build's version, the same number across the core, the extension and
/// the ReaMesh app (they release together).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const XFER_PROTOCOL: &str = "/reamesh/xfer/1";
/// Max payload per file chunk over the transfer protocol. Each chunk is one
/// request-response round trip on its own substream, so bigger chunks mean
/// fewer per-request stalls (stream setup, CBOR framing, scheduling); the
/// libp2p CBOR codec caps responses at 10 MiB, so 4 MiB + framing fits.
const CHUNK_SIZE: u64 = 4 * 1024 * 1024;
/// How many chunk requests a downloader keeps in flight at once. Pipelining:
/// the link stays full instead of idling one round-trip per chunk. With 4 MiB
/// chunks this bounds in-flight buffers at 64 MiB.
const DOWNLOAD_WINDOW: usize = 16;
/// Where the window starts (and its floor after a timeout). In-flight responses
/// share one connection and complete together, so a window sized for the LAN
/// would make every piece time out on a slower link; it grows toward
/// DOWNLOAD_WINDOW as pieces prove to come back quickly.
const DOWNLOAD_WINDOW_MIN: usize = 4;
/// Progressive join: once the project file and every non-media file have
/// landed, and at least this much media is still outstanding, the download
/// reports `ready` early (status `background: true`) so the extension can open
/// the project with placeholder items while the media keeps streaming in
/// behind it. Below this the whole download finishes first, as before: a
/// sub-second flash of grey placeholders would only look like a glitch.
const PROGRESSIVE_MIN_BYTES: u64 = 16 * 1024 * 1024;
/// Minimum gap between two full-state broadcasts. A subscription to the
/// (publicly announced) session topic needs no passphrase, so a peer flapping
/// its subscription must not be able to make the owner rebroadcast the whole
/// doc every tick.
const FULL_SYNC_MIN_GAP: Duration = Duration::from_secs(3);
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
/// A second byte cap on the same queue. The per-message `max_transmit_size` is
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
/// session participant every `PRESENCE_INTERVAL` so peers can show who is in
/// the session (every `PRESENCE_FAST_INTERVAL` while someone follows us).
const TAG_PRESENCE: u8 = b'N';
/// Presence entries not refreshed within this window are dropped.
const PRESENCE_STALE: Duration = Duration::from_secs(8);
/// Beacon cadence when nobody is following this peer: rosters and
/// click-to-follow only need a coarse "where are they".
const PRESENCE_INTERVAL: Duration = Duration::from_secs(1);
/// Beacon cadence while at least one peer's beacon says it *follows us*
/// (`LivePresence::follow` == our peer id). Follow mode mirrors our transport
/// from these beacons, so a play start/stop or a cursor move reaches the
/// follower within ~100ms instead of up to a second. Only the followed peer
/// speeds up: the follower's own beacon is nobody's transport source, and the
/// rest of the session keeps the 1s cadence. Ten ~300-byte messages a second
/// on a mesh of a few peers is negligible even over a VPN.
const PRESENCE_FAST_INTERVAL: Duration = Duration::from_millis(100);
/// Ping (wire name: shared marker): `'M'` + JSON `{"name","pos","by","color","mac"}`,
/// published once when a user pings a timeline spot from the roster bar. Every
/// receiver shows it as a short notice and writes nothing into its project
/// (builds before pings dropped a local, non-synced "⚑" marker instead).
/// Authenticated exactly like a presence beacon (token-keyed MAC over the
/// sender's peer id), so open-topic bystanders can't inject markers.
const TAG_MARKER: u8 = b'M';
/// If the extension's main-thread presence feed (`set_presence`, called every
/// timer tick) has been silent this long while the network thread still runs,
/// REAPER's main thread is stuck (modal dialog, plugin hang): the beacon
/// carries that as `frozen` so peers' rosters can say so.
const MAIN_THREAD_STALE: Duration = Duration::from_secs(4);

/// How often each connected peer is pinged for a round-trip time. The
/// app turns a sustained high RTT into a "slow connection" notice;
/// ping cost is one tiny stream per interval, so a short interval is cheap.
const PING_INTERVAL: Duration = Duration::from_secs(5);
/// A ping that takes this long counts as a (very) slow link, not a dead one:
/// liveness is the topic subscription's business, so a timeout records a
/// pessimistic sample instead of dropping the peer.
const PING_TIMEOUT: Duration = Duration::from_secs(10);
/// Smoothed round-trip samples older than this are forgotten (the peer's
/// connection went quiet or away).
const RTT_STALE: Duration = Duration::from_secs(60);
/// Session farewell: published by the owner right before it stops hosting so
/// clients end cleanly instead of waiting out a silence.
const TAG_SESSION_END: u8 = b'E';
/// Moderation: `'K'` + JSON `{"peer","ban"}`, owner -> all. The named peer
/// ends its session (`end_reason` "kicked" or "banned"); everyone else drops
/// it from their roster at once. A kick is a polite "leave now": the peer may
/// rejoin immediately. A ban also puts the peer id on the ban table below.
const TAG_KICK: u8 = b'K';
/// Ban table: `'B'` + JSON `{"bans":[peer id...]}`, owner -> all, republished
/// whenever the live set changes (a ban, a revoke, an expiry) and to every
/// (re)joining peer. Every session member refuses file requests from, drops
/// gossip of, and blacklists the listed peers, so a banned peer that ignores
/// its 'K' still gets nothing: not the project (every peer serves files), not
/// a roster entry, not an admitted edit.
const TAG_BANS: u8 = b'B';
/// How long a ban keeps a peer id (the lasting per-install identity, i.e.
/// "this device") out. Process-global on the host, so it survives a re-host
/// within the window; not persisted across a REAPER restart.
pub const BAN_DURATION: Duration = Duration::from_secs(30 * 60);
/// Sealed envelope: `'X'` + 24-byte nonce + XChaCha20-Poly1305 ciphertext of a
/// whole tagged message (tag byte included, so even message *types* are
/// hidden). In a passphrase-protected session every session-topic message is
/// sealed with a key derived from the session token, and unsealed messages are
/// dropped: a LAN peer without the passphrase can subscribe to the (publicly
/// announced) topic but can neither read the gossip nor lift the session token
/// out of a proposal envelope. Open sessions stay plaintext (no secret to
/// derive a key from). The sender's peer id is bound as AAD, so a captured
/// ciphertext replayed from a different (transport-authenticated) peer id
/// fails to open.
const TAG_SEALED: u8 = b'X';
/// XChaCha20 nonce length: 24 bytes, safe to draw at random per message.
const SEAL_NONCE_LEN: usize = 24;
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
/// How often the owner re-walks the served project to keep the manifest
/// honest. A joiner plans its whole download from the manifest, including piece
/// offsets, chunk lengths, and the blake3 hash. A file the host
/// changes mid-session (a Ctrl+S rewriting the .rpp, above all) has to be
/// picked up, or every later join stalls on chunk lengths that no longer match.
const MANIFEST_REFRESH: Duration = Duration::from_secs(5);
/// Age after which an unused staging copy of a downloaded project is deleted.
/// Every join downloads a full copy into its own directory and nothing else
/// ever removes them, so without this a week of sessions leaves tens of
/// gigabytes behind that no UI can clear.
const STAGING_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        /// Host-reported session meta JSON ("" when the host never set one):
        /// track count + the plugin list parsed from the served .rpp. Shown in
        /// the joiner's trust dialog and checked against the actually-received
        /// project before it opens. `default` keeps older peers decodable.
        #[serde(default)]
        meta: String,
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
    /// The host's *self-reported* display name and color, carried on the public
    /// lobby announce so the lobby can say who is hosting before a join (there
    /// is no presence beacon until after the manifest is accepted). Bound to
    /// the authenticated signer like everything else in the announce, but the
    /// values themselves are chosen by the host: the UI must present them as a
    /// claimed label, never as a verified identity.
    owner_name: String,
    owner_color: String,
    /// Whether the session is passphrase-protected, and the per-session salt a
    /// client mixes with the passphrase to reproduce the access token.
    auth: bool,
    salt: String,
    /// Project size as announced (shareable file count + total bytes), so the
    /// lobby can show what a join would download before anyone commits.
    files: u64,
    bytes: u64,
    last_seen: Instant,
    /// Set when a *different* peer announced this same session id while the
    /// first-seen owner's entry was still live. The first-seen owner keeps the
    /// entry (see the announce handler), and the lobby greys the session out:
    /// a session id is public, so an impostor can otherwise re-point a card at
    /// itself simply by announcing faster than the real host.
    contested: bool,
    /// The host build's `PROTO` and version, from its announce (0 / "" from a
    /// build that predates them). A different `proto` can't be joined.
    proto: u32,
    version: String,
}

#[derive(Clone, Debug)]
struct StatusState {
    /// "hosting" | "browsing" | "connecting" | "downloading" | "ready" |
    /// "failed" | "ended" (the owner stopped the session)
    state: String,
    progress: f64,
    /// Byte counters behind `progress` while downloading (0 otherwise): the
    /// app shows "412 MB / 1.3 GB" and a transfer rate from these.
    done_bytes: u64,
    total_bytes: u64,
    /// File counters for the same download ("busy with file 3 of 41").
    files_done: u64,
    files_total: u64,
    /// Relative path of the file whose chunk arrived most recently ("" outside
    /// a download): the ReaMesh app's download screen shows its name.
    current_file: String,
    /// True while a progressive join is `ready` with media still downloading
    /// in the background (see PROGRESSIVE_MIN_BYTES). The byte/file counters
    /// keep moving meanwhile; it drops to false when the last file lands.
    background: bool,
    project_path: Option<String>,
    /// The folder a join downloads into (set when it starts), so the app
    /// can tell that copy is in use before the project is open.
    dir: Option<String>,
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
            "done_bytes": self.done_bytes,
            "total_bytes": self.total_bytes,
            "files_done": self.files_done,
            "files_total": self.files_total,
            "current_file": self.current_file,
            "background": self.background,
            "project_path": self.project_path,
            "dir": self.dir,
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
    /// Proposed value, when it was sent, and how often it has been re-sent.
    pending: HashMap<FieldKey, (JsonValue, Instant, u8)>,
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
    /// Sessions currently announced on the lobby (client side).
    sessions: Mutex<HashMap<String, SessionInfo>>,
    status: Mutex<StatusState>,
    /// Directory this peer serves (and fetches) session files from: the
    /// project dir on the owner, the downloaded copy on clients (set once the
    /// initial transfer completes).
    serve_dir: Mutex<Option<PathBuf>>,
    /// Peer ids currently subscribed to the session topic (fetch candidates;
    /// its length is the peer count).
    session_peers: Mutex<Vec<PeerId>>,
    /// Access token every peer in this session shares (empty = open session).
    /// Requests carry it; serving peers refuse mismatches. Set on the owner at
    /// host time and on a client when it joins (derived from the salt in the
    /// announcement + the passphrase the user supplied).
    auth_token: Mutex<String>,
    /// Latest *authenticated* presence beacon per peer id string (display
    /// name + color). Beacons whose MAC doesn't verify never land here.
    peers_info: Mutex<HashMap<String, PeerPresence>>,
    /// Smoothed round-trip time (ms) per connected peer id string, from the
    /// libp2p ping behaviour, plus when the last sample landed. Surfaced per
    /// roster entry as `rtt` so the app can say "slow connection to X".
    peer_rtt: Mutex<HashMap<String, (f64, Instant)>>,
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
    /// When `peer_locks` was last replaced by an owner broadcast (client side).
    ///
    /// A lock table that arrived *before* we made an edit says nothing about
    /// that edit's fate: the holder it names may well have lapsed in the
    /// meantime, which is precisely the moment a new editor takes the field.
    /// Snapping back on such a table produces a visible glitch (the edit
    /// reverts, then the owner admits it and it re-applies) and made the
    /// lock-lapse handover test flaky under load. Only a table received after
    /// the proposal is evidence; otherwise the `PROPOSAL_TIMEOUT` fallback
    /// decides.
    peer_locks_at: Mutex<Option<Instant>>,
    /// Owner only: fields whose owner-local edit was rejected because a client
    /// holds the lock. drain_remote re-emits the authoritative (holder's) value
    /// for these so the owner's own REAPER snaps back: the lock binds the owner
    /// too. Carries the holder id for the "X is editing this" notice.
    force_revert: Mutex<Vec<(FieldKey, String)>>,
    /// Result of the latest passphrase pre-check (`check_passphrase`):
    /// `(session_id, Some(ok), preview)` once it resolves, `(session_id, None,
    /// Null)` while in flight, `None` when no probe is active. Lets the app
    /// validate a passphrase against the owner *before* it shows the trust
    /// dialog or starts the download, so a wrong passphrase shows an inline
    /// error in the input instead of flashing the whole join animation. On
    /// success `preview` carries what a join would mean, straight from the
    /// owner's manifest: file count, total bytes, project file name, and the
    /// host-reported session meta (track count + plugin list): everything the
    /// trust dialog shows. All of it is *host-reported*: the UI labels it as a
    /// claim, and the post-download verify checks the plugin part for real.
    probe_result: Mutex<Option<(String, Option<bool>, JsonValue, &'static str)>>,
    /// Client: sequence of the last proposal sent (see next_proposal_seq).
    proposal_seq: AtomicU64,
    /// Owner: the last admitted sequence per proposer: anything not newer is
    /// a replay.
    proposal_seqs: Mutex<HashMap<String, u64>>,
    /// The session meta blob for the current session: what this owner set via
    /// `set_session_meta`, or (on a client) what the owner's manifest claimed.
    /// The extension reads it back after the download to verify the received
    /// project against the claim before opening it.
    session_meta: Mutex<String>,
    /// Shared markers received from peers ('M' messages), queued for the
    /// extension to drain on its main-thread tick and drop into the project as
    /// local, non-synced markers.
    marker_inbox: Mutex<Vec<JsonValue>>,
    /// Peer ids currently banned from this session: on the owner a mirror of
    /// the process-global ban table (refreshed every tick; the tick's diff
    /// against it is what triggers the blacklist + 'B' broadcast), on a
    /// client what the owner last broadcast ('B'). Consulted by gossip
    /// dispatch, the roster, and (on clients) file serving; the owner serves
    /// against the live table itself.
    banned: Mutex<HashSet<String>>,
}

#[derive(Clone, Debug)]
struct PeerPresence {
    name: String,
    color: String,
    /// GUID of the track this peer is currently focused on ("" if none).
    focus: String,
    /// Where this peer is in the timeline (project seconds): play cursor while
    /// rolling, else edit cursor. Used by the roster's click-to-follow to centre
    /// the view on them; not drawn as a live playhead.
    play: f64,
    /// This peer's arrange-view span (project seconds); click-to-follow adopts
    /// it so the follower lands with the same zoom + scroll. 0/0 = unknown.
    v0: f64,
    v1: f64,
    /// Whether this peer's transport is rolling (drives follow mode).
    playing: bool,
    /// Peer id this peer is following ("" = nobody): the followed person's
    /// roster shows "someone is following you" from this.
    follow: String,
    /// GUIDs of the media items this peer has selected (capped, see
    /// `PRESENCE_SEL_MAX`); a follower mirrors the selection.
    sel: Vec<String>,
    /// The FX window this peer is editing in (see `LivePresence::fx`), Null
    /// when none is open; a follower opens the same one.
    fx: JsonValue,
    /// This peer's REAPER main thread has stopped feeding presence while its
    /// network thread still beacons: i.e. their REAPER isn't responding.
    frozen: bool,
    last_seen: Instant,
    /// Verified beacons received from this peer so far (tests read the rate
    /// through `Reamesh::beacon_count`; nothing on the wire depends on it).
    beacons: u64,
}

/// Process-wide display profile (name, color hex), set by the ReaMesh app
/// through the control server (or `reamesh_set_profile`). Global rather than
/// per-session: it describes the user, not the session, and must survive
/// host/join/leave cycles.
static PROFILE: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

/// One banned peer: the display name it had when banned (for the host's ban
/// list; "" if it never beaconed one) and when the ban lapses, both as a
/// monotonic deadline (for pruning) and as wall-clock unix ms (for the UI: an
/// absolute stamp keeps the composed app state from churning every tick).
#[derive(Clone, Debug)]
struct Ban {
    name: String,
    /// Identity color at ban time (the roster's, so the host recognises the
    /// avatar in the ban list; the id-derived default if it never sent one).
    color: String,
    until: Instant,
    until_unix_ms: u64,
}

/// Process-wide ban table, peer id -> ban. Global like PROFILE: a ban is about
/// the *device* (its lasting peer id), not the session, and it must outlive a
/// stop-hosting / re-host cycle inside its window. Expired entries are pruned
/// on every read.
static BANS: std::sync::LazyLock<Mutex<HashMap<String, Ban>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// The live ban table (expired entries pruned), as `(peer id, ban)` pairs.
fn live_bans() -> Vec<(String, Ban)> {
    let mut bans = BANS.lock().unwrap();
    let now = Instant::now();
    bans.retain(|_, b| b.until > now);
    let mut out: Vec<(String, Ban)> = bans.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    out.sort_by(|a, b| a.1.until.cmp(&b.1.until));
    out
}

/// Peer ids currently banned (see `live_bans`).
fn banned_ids() -> HashSet<String> {
    live_bans().into_iter().map(|(id, _)| id).collect()
}

/// The live ban list for the ReaMesh app, as a JSON array of
/// `{"id","name","color","expires_at"}` (`expires_at` = unix ms), soonest to lapse
/// first. Independent of any session handle: the table is process-global.
pub fn bans_json() -> String {
    let arr: Vec<JsonValue> = live_bans()
        .into_iter()
        .map(|(id, b)| {
            json!({ "id": id, "name": b.name, "color": b.color, "expires_at": b.until_unix_ms })
        })
        .collect();
    json!(arr).to_string()
}

/// Revoke a ban early. Returns whether the peer was banned. The hosting
/// network task notices the table change on its next tick and republishes it
/// (so every session member un-blacklists the peer); nothing to do when not
/// hosting: the entry is simply gone.
pub fn unban(peer_id: &str) -> bool {
    BANS.lock().unwrap().remove(peer_id).is_some()
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn set_profile(name: &str, color: &str) {
    let mut p = PROFILE.lock().unwrap();
    p.0 = name.chars().take(64).collect();
    p.1 = color.chars().take(16).collect();
}

/// The local user's live "where am I" state, set by the extension each tick and
/// gossiped on the presence beacon so peers can show where each person is: which
/// track they're focused on (the roster track number) and where they sit in the
/// timeline (so click-to-follow can centre the view on them). Like PROFILE it's
/// process-global: it describes the user, not the session. GUIDs (never
/// indices) are the shared identity; peers resolve them against their own
/// project.
#[derive(Clone)]
struct LivePresence {
    focus: String,
    /// Timeline position (project seconds): play cursor while rolling, else edit
    /// cursor. Drives the roster's click-to-follow, not a live playhead.
    play: f64,
    /// Arrange-view span (project seconds, start/end) so click-to-follow can
    /// reproduce this peer's zoom + scroll, not just centre on their cursor.
    v0: f64,
    v1: f64,
    /// Whether the transport is rolling: follow mode mirrors the followed
    /// peer's transport, so "playing" has to ride the beacon, not be inferred.
    playing: bool,
    /// Peer id this user is currently *following* ("" = nobody). Broadcast so
    /// the followed person's roster can show "someone is following you".
    follow: String,
    /// GUIDs of the selected media items, so a follower sees the same items
    /// selected as the person they follow (what they are about to edit).
    sel: Vec<String>,
    /// The FX window this user is editing in: `{"track": <track guid>,
    /// "fx": <index>, "float": bool}` for a track FX, plus `"item": <item
    /// guid>, "take": <index>` for a take FX; Null when no FX window is open.
    /// A follower opens (and switches) the same window. The extension composes
    /// it from REAPER's focused-FX state; `sanitize_fx` bounds what is kept.
    fx: JsonValue,
}

static LIVE: Mutex<LivePresence> = Mutex::new(LivePresence {
    focus: String::new(),
    play: 0.0,
    v0: 0.0,
    v1: 0.0,
    playing: false,
    follow: String::new(),
    sel: Vec::new(),
    fx: JsonValue::Null,
});

/// Most selected-item GUIDs a presence beacon carries. A followed peer beacons
/// ten times a second, so the list is bounded; a bigger selection mirrors as
/// its first entries. Applied both to what the extension feeds us and to what
/// arrives from a peer (untrusted).
const PRESENCE_SEL_MAX: usize = 256;

/// Bound a beacon's `sel` field: at most `PRESENCE_SEL_MAX` GUID-sized strings.
fn sanitize_sel(v: Option<&JsonValue>) -> Vec<String> {
    v.and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|g| g.as_str())
                .filter(|g| !g.is_empty())
                .take(PRESENCE_SEL_MAX)
                .map(|g| g.chars().take(64).collect())
                .collect()
        })
        .unwrap_or_default()
}

/// Bound a beacon's `fx` field to the known keys with their expected types
/// (`track`/`item` GUID strings, `take`/`fx` integers, `float` bool). Anything
/// else, or a missing track, collapses to Null (= no FX window).
fn sanitize_fx(v: Option<&JsonValue>) -> JsonValue {
    let Some(o) = v.and_then(|x| x.as_object()) else {
        return JsonValue::Null;
    };
    let s = |k: &str| -> Option<String> {
        o.get(k)
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .map(|x| x.chars().take(64).collect())
    };
    let Some(track) = s("track") else {
        return JsonValue::Null;
    };
    let n = |k: &str| o.get(k).and_then(|x| x.as_i64()).unwrap_or(0).clamp(0, i32::MAX as i64);
    let mut out = json!({
        "track": track,
        "fx": n("fx"),
        "float": o.get("float").and_then(|x| x.as_bool()).unwrap_or(false),
    });
    if let Some(item) = s("item") {
        out["item"] = json!(item);
        out["take"] = json!(n("take"));
    }
    out
}

/// Last time the extension's main-thread tick fed us presence. The network
/// thread keeps beaconing while REAPER's main thread is stuck, so the gap
/// between "beacons still flowing" and "presence feed silent" is exactly the
/// "REAPER isn't responding" signal peers show in their rosters.
static PRESENCE_TOUCH: Mutex<Option<Instant>> = Mutex::new(None);

/// Whether the extension's main-thread presence feed has gone quiet (see
/// `MAIN_THREAD_STALE`). `false` until the first tick ever lands: an extension
/// that hasn't started feeding presence isn't frozen, just early.
fn main_thread_stale() -> bool {
    PRESENCE_TOUCH
        .lock()
        .unwrap()
        .is_some_and(|t| t.elapsed() > MAIN_THREAD_STALE)
}

pub fn set_focus(guid: &str) {
    LIVE.lock().unwrap().focus = guid.chars().take(64).collect();
}

/// Replace the live presence from a JSON blob the extension composes each tick:
/// `{"focus","play"}`. Any missing field resets to its empty/zero default.
pub fn set_presence(json: &str) {
    // Touch even on unparseable input: any call proves the main thread ticks.
    *PRESENCE_TOUCH.lock().unwrap() = Some(Instant::now());
    let Ok(v) = serde_json::from_str::<JsonValue>(json) else {
        return;
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
    p.v0 = v.get("v0").and_then(|x| x.as_f64()).unwrap_or(0.0);
    p.v1 = v.get("v1").and_then(|x| x.as_f64()).unwrap_or(0.0);
    p.playing = v.get("playing").and_then(|x| x.as_bool()).unwrap_or(false);
    p.follow = v
        .get("follow")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .chars()
        .take(128)
        .collect();
    p.sel = sanitize_sel(v.get("sel"));
    p.fx = sanitize_fx(v.get("fx"));
}

// ---------- persistent network identity ----------
//
// The extension registers a keypair path under the REAPER resource directory.
// Swarm starts reuse that identity so hosts can be recognized across sessions.
// Registration is optional: in-process tests need independent ephemeral peers.
// Settings can delete the stored key; the next session creates a new identity.

static IDENTITY_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Held for the life of the process: the advisory lock marking our identity
/// slot as in use (see `claim_identity_slot`).
static IDENTITY_LOCK: Mutex<Option<fs::File>> = Mutex::new(None);
/// How many concurrent instances sharing one resource dir get distinct
/// identities before the last one falls back to the base path unlocked.
const IDENTITY_SLOTS: usize = 8;

pub fn set_identity_path(path: &str) {
    if path.is_empty() {
        return;
    }
    let (slot, lock) = claim_identity_slot(Path::new(path));
    *IDENTITY_LOCK.lock().unwrap() = lock;
    *IDENTITY_PATH.lock().unwrap() = Some(slot);
}

/// The identity file the n-th concurrent instance uses: `base` itself for the
/// first, `<stem>.2<ext>`, `<stem>.3<ext>`, … for the others.
fn identity_slot_path(base: &Path, n: usize) -> PathBuf {
    if n <= 1 {
        return base.to_path_buf();
    }
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("reamesh-identity");
    let ext = base
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    base.with_file_name(format!("{stem}.{n}{ext}"))
}

/// Claim an identity slot for this process. REAPER instances sharing a
/// resource directory need distinct keypairs because libp2p cannot connect
/// a peer to itself. Each slot has a separate .lock file held for the process
/// lifetime; keeping the key file unlocked permits Windows reads.
/// The first instance uses the base path, then .2, .3, and so on. If locking
/// is unsupported or all slots are busy, fall back to the unlocked base path.
fn claim_identity_slot(base: &Path) -> (PathBuf, Option<fs::File>) {
    if let Some(parent) = base.parent() {
        let _ = fs::create_dir_all(parent);
    }
    for n in 1..=IDENTITY_SLOTS {
        let slot = identity_slot_path(base, n);
        let mut lock_name = slot
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        lock_name.push(".lock");
        let lock_path = slot.with_file_name(lock_name);
        let file = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "[reamesh] could not open identity lock {}: {e}; using {} unlocked",
                    lock_path.display(),
                    base.display()
                );
                return (base.to_path_buf(), None);
            }
        };
        match file.try_lock() {
            Ok(()) => {
                if n > 1 {
                    eprintln!(
                        "[reamesh] identity slot {n}: another instance holds the primary identity, using {}",
                        slot.display()
                    );
                }
                return (slot, Some(file));
            }
            Err(std::fs::TryLockError::WouldBlock) => continue,
            Err(std::fs::TryLockError::Error(e)) => {
                eprintln!(
                    "[reamesh] identity lock unsupported at {}: {e}; using {} unlocked",
                    lock_path.display(),
                    base.display()
                );
                return (base.to_path_buf(), None);
            }
        }
    }
    eprintln!(
        "[reamesh] all {IDENTITY_SLOTS} identity slots are in use; using {} unlocked",
        base.display()
    );
    (base.to_path_buf(), None)
}

/// The keypair every swarm starts with: the persisted per-install identity
/// when a path is registered (created on first use), else a fresh ephemeral
/// one (tests, or an extension that never registered a path).
fn identity_keypair() -> libp2p::identity::Keypair {
    let path = IDENTITY_PATH.lock().unwrap().clone();
    match path {
        Some(p) => load_or_create_identity_at(&p),
        None => libp2p::identity::Keypair::generate_ed25519(),
    }
}

/// Load the keypair stored at `path`, or mint + persist a fresh one there.
/// Kept free of the `IDENTITY_PATH` global so it is unit-testable without
/// contaminating the in-process network tests (which must each get their own
/// ephemeral identity).
fn load_or_create_identity_at(path: &Path) -> libp2p::identity::Keypair {
    if let Ok(bytes) = fs::read(path)
        && let Ok(kp) = libp2p::identity::Keypair::from_protobuf_encoding(&bytes)
    {
        return kp;
    }
    let kp = libp2p::identity::Keypair::generate_ed25519();
    let persist = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = kp
            .to_protobuf_encoding()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        fs::write(path, bytes)?;
        // The file is this install's network identity; keep it out of other
        // users' reach where the OS supports it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    };
    if let Err(e) = persist() {
        eprintln!("[reamesh] could not persist identity to {}: {e}", path.display());
    } else {
        eprintln!("[reamesh] new persistent identity at {}", path.display());
    }
    kp
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

/// Symmetric key sealing this session's gossip, derived from the session
/// token. `None` for open sessions (empty token: nothing secret to derive
/// from), whose gossip stays plaintext. Deriving per message is a single
/// blake3 hash: cheap enough to skip caching and its invalidation questions.
fn gossip_key(auth_token: &str) -> Option<chacha20poly1305::Key> {
    if auth_token.is_empty() {
        return None;
    }
    Some(chacha20poly1305::Key::from(blake3::derive_key(
        "reamesh/v3/gossip",
        auth_token.as_bytes(),
    )))
}

/// Seal one tagged session-topic message (see `TAG_SEALED`). `sender` is the
/// publishing peer's id, bound as AAD: receivers open against the gossipsub
/// message's authenticated source, so a replay under any other identity fails.
/// Returns `None` only if the OS RNG fails (the message is then not sent).
fn seal_session_msg(key: &chacha20poly1305::Key, tagged: &[u8], sender: &str) -> Option<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, Payload};
    use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
    let mut nonce = [0u8; SEAL_NONCE_LEN];
    if let Err(e) = getrandom::fill(&mut nonce) {
        eprintln!("[reamesh] seal: getrandom failed ({e}); message not sent");
        return None;
    }
    let ct = XChaCha20Poly1305::new(key)
        .encrypt(&XNonce::from(nonce), Payload { msg: tagged, aad: sender.as_bytes() })
        .ok()?;
    let mut out = Vec::with_capacity(1 + SEAL_NONCE_LEN + ct.len());
    out.push(TAG_SEALED);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Some(out)
}

/// Open a sealed session-topic message against the sender's authenticated peer
/// id. `None` on anything that doesn't verify: wrong key (no/other passphrase),
/// tampering, truncation, or a ciphertext replayed from a different peer id.
fn open_session_msg(key: &chacha20poly1305::Key, data: &[u8], sender: &str) -> Option<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, Payload};
    use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
    if data.first() != Some(&TAG_SEALED) || data.len() < 1 + SEAL_NONCE_LEN + 16 {
        return None;
    }
    let nonce = XNonce::try_from(&data[1..1 + SEAL_NONCE_LEN]).ok()?;
    XChaCha20Poly1305::new(key)
        .decrypt(
            &nonce,
            Payload { msg: &data[1 + SEAL_NONCE_LEN..], aad: sender.as_bytes() },
        )
        .ok()
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

/// Validate a peer-supplied session id before using it in paths or lobby entries.
/// Accept only a bounded ASCII identifier.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn random_hex(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    if let Err(e) = getrandom::fill(&mut buf) {
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

// ---------- join-by-ticket ----------
//
// Discovery is normally mDNS + the lobby topic, which never leaves the local
// subnet. An invite *ticket* is a `reamesh://` link that carries everything a
// client needs to reach the host directly instead: the host's peer id and
// where it listens, plus the session id/name and whether it takes a
// passphrase. Over a VPN (Tailscale etc.) or a multicast-blocking LAN, the
// link stands in for the announcement being overheard: the client dials the
// addresses, the (signed) lobby announce then arrives over that direct
// connection, and the join proceeds exactly like a discovered one: same
// manifest auth, same download, same session topic. The link is only valid
// while that hosting run lives (id and ports change on re-host), like a
// meeting link.

/// Ticket wire format, a deep link the ReaMesh app registers as its own:
///
/// `reamesh://join/<sid>?n=<name>&s=1&p=<peer>&a=<ip>,<ip>&t=<tcp>&q=<quic>`
///
/// - `n` session name, percent-encoded (absent when empty)
/// - `s` `1` when the session takes a passphrase (absent otherwise); read by
///   the ReaMesh app to ask for one before joining, not by this side
/// - `p` the host's peer id (base58)
/// - `a` the host's dialable IPs, comma-separated
/// - `t` the host's TCP port, `q` its QUIC (UDP) port; at least one
///
/// The swarm listens on `0.0.0.0` once per transport, so every interface
/// shares the same two ports: naming the IPs and the ports separately is what
/// keeps the link short (the previous base64-JSON form spelled out every
/// `/ip4/../tcp/..` multiaddr and ran to several hundred characters). It stays
/// a plain readable URL, not binary, so the ReaMesh app can parse it in the
/// webview to show the session name and passphrase flag before anything
/// touches the network.
const TICKET_PREFIX: &str = "reamesh://join/";
/// Hard cap on pasted-ticket length before any parsing.
const TICKET_MAX_LEN: usize = 8 * 1024;
/// Cap on the IPs a ticket may carry (a host has a handful of interfaces;
/// hundreds would be a dial-storm request, not an invite). Each IP expands to
/// at most two dial addresses (TCP + QUIC).
const TICKET_MAX_IPS: usize = 8;
/// How long a ticket join keeps dialing + waiting for the host's announce
/// before it fails. Longer than MANIFEST_TIMEOUT: this leg spans WAN/VPN
/// connection setup, not just one request round-trip.
const TICKET_JOIN_TIMEOUT: Duration = Duration::from_secs(30);
/// Fast-fail for a ticket whose host is unreachable: if we never even
/// established a connection to the pinned peer within this window, the host is
/// gone (stopped hosting, wrong network): fail now instead of making the user
/// stare at "contacting the host" for the full TICKET_JOIN_TIMEOUT. A reachable
/// host completes the transport handshake in well under this even over a VPN,
/// so a slow-but-alive host still gets the full window to send its announce.
const TICKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(12);

/// Percent-encode everything but the RFC 3986 unreserved characters, so a
/// session name survives inside a URL query (and never contains whitespace,
/// which chat clients would wrap on).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-decode; `None` on a malformed escape or invalid UTF-8. A `+` is
/// left alone (this is a URI, not a form body), matching the app's
/// `decodeURIComponent`.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Everything a "join by ticket" needs, decoded from the pasted link.
#[derive(Clone, Debug)]
struct TicketInfo {
    session_id: String,
    session_name: String,
    /// The host's peer id. The join is *pinned* to it: only an announce whose
    /// authenticated signer is this peer is accepted, so a LAN peer announcing
    /// the same session id can't hijack a ticket join.
    peer: PeerId,
    addrs: Vec<Multiaddr>,
}

/// An address worth putting in a ticket: loopback and unspecified are
/// meaningless off-machine (a same-host client re-derives loopback itself, see
/// `dial_ticket`), and link-local (169.254/16, fe80::/10) can't be dialed
/// without a scope the link doesn't carry.
fn dialable_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local(),
        IpAddr::V6(v6) => {
            !v6.is_loopback() && !v6.is_unspecified() && (v6.segments()[0] & 0xffc0) != 0xfe80
        }
    }
}

/// What a ticket says about where the host listens, from the swarm's listen
/// addresses: the dialable IPs plus the TCP and QUIC ports (0 = that transport
/// isn't listening). Both transports listen on `0.0.0.0`, so every interface
/// reports the same port per transport; the first seen is taken.
fn ticket_endpoints(listen: &[Multiaddr]) -> (Vec<IpAddr>, u16, u16) {
    let mut ips: Vec<IpAddr> = Vec::new();
    let (mut tcp, mut quic) = (0u16, 0u16);
    for addr in listen {
        let Some(ip) = addr_ip(addr) else { continue };
        if !dialable_ip(&ip) {
            continue;
        }
        let is_quic = addr.iter().any(|p| matches!(p, Protocol::QuicV1));
        for p in addr.iter() {
            match p {
                Protocol::Tcp(port) if tcp == 0 => tcp = port,
                Protocol::Udp(port) if quic == 0 && is_quic => quic = port,
                _ => {}
            }
        }
        if !ips.contains(&ip) && ips.len() < TICKET_MAX_IPS {
            ips.push(ip);
        }
    }
    (ips, tcp, quic)
}

/// Build a ticket from its parts (see `TICKET_PREFIX` for the format).
/// `peer` is the host's peer id in its base58 display form.
fn encode_ticket(
    sid: &str,
    name: &str,
    peer: &str,
    ips: &[IpAddr],
    tcp: u16,
    quic: u16,
    auth: bool,
) -> String {
    let mut params: Vec<String> = Vec::with_capacity(6);
    if !name.is_empty() {
        params.push(format!("n={}", percent_encode(name)));
    }
    if auth {
        params.push("s=1".into());
    }
    params.push(format!("p={peer}"));
    let ips: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
    params.push(format!("a={}", ips.join(",")));
    if tcp != 0 {
        params.push(format!("t={tcp}"));
    }
    if quic != 0 {
        params.push(format!("q={quic}"));
    }
    format!("{TICKET_PREFIX}{sid}?{}", params.join("&"))
}

fn parse_ticket(ticket: &str) -> Option<TicketInfo> {
    let t = ticket.trim();
    if t.len() > TICKET_MAX_LEN {
        return None;
    }
    // The scheme may come back capitalised from a chat client or the OS; the
    // rest is case-sensitive (peer ids are base58).
    if !t.get(..TICKET_PREFIX.len())?.eq_ignore_ascii_case(TICKET_PREFIX) {
        return None;
    }
    let rest = t.get(TICKET_PREFIX.len()..)?;
    let (sid, query) = rest.split_once('?').unwrap_or((rest, ""));
    let sid = sid.trim_end_matches('/');
    if !valid_session_id(sid) {
        return None;
    }
    let mut name = String::new();
    let mut peer: Option<PeerId> = None;
    let mut ips: Vec<IpAddr> = Vec::new();
    let (mut tcp, mut quic) = (0u16, 0u16);
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            // A garbled name is cosmetic: show nothing rather than refuse.
            "n" => name = percent_decode(v).unwrap_or_default(),
            "p" => peer = Some(v.parse().ok()?),
            "a" => {
                ips = v
                    .split(',')
                    .filter_map(|ip| ip.parse().ok())
                    .take(TICKET_MAX_IPS)
                    .collect()
            }
            "t" => tcp = v.parse().ok().filter(|p| *p != 0)?,
            "q" => quic = v.parse().ok().filter(|p| *p != 0)?,
            // `s` (the auth flag) is the ReaMesh app's business; anything a
            // later version adds is ignored rather than refused.
            _ => {}
        }
    }
    let peer = peer?;
    if ips.is_empty() || (tcp == 0 && quic == 0) {
        return None;
    }
    let mut addrs = Vec::with_capacity(ips.len() * 2);
    for ip in ips {
        let base = Multiaddr::from(ip);
        if tcp != 0 {
            addrs.push(base.clone().with(Protocol::Tcp(tcp)));
        }
        if quic != 0 {
            addrs.push(base.clone().with(Protocol::Udp(quic)).with(Protocol::QuicV1));
        }
    }
    Some(TicketInfo { session_id: sid.to_string(), session_name: name, peer, addrs })
}

/// The first IP protocol component of a multiaddr, if any.
fn addr_ip(addr: &Multiaddr) -> Option<std::net::IpAddr> {
    addr.iter().find_map(|p| match p {
        Protocol::Ip4(ip) => Some(std::net::IpAddr::from(ip)),
        Protocol::Ip6(ip) => Some(std::net::IpAddr::from(ip)),
        _ => None,
    })
}

enum Command {
    PublishAuthoritative(Vec<u8>),
    PublishProposal(Vec<u8>),
    JoinSession { session_id: String, passphrase: String },
    /// Join (or, with `probe_only`, just passphrase-check) a session reached
    /// through a pasted ticket instead of LAN discovery: dial the ticket's
    /// addresses, wait for the pinned owner's announce, then proceed exactly
    /// like a discovered session.
    JoinTicket { info: TicketInfo, passphrase: String, probe_only: bool },
    /// Pre-check a passphrase against the owner without joining (a lone manifest
    /// request whose accept/reject answers "is this passphrase right?").
    ProbeAuth { session_id: String, passphrase: String },
    FetchFile(String),
    Dial(Multiaddr),
    /// Owner-side: the background hashing pass over the project finished;
    /// replace the served manifest with this hash-complete one. Hosting starts
    /// with an unhashed manifest so `host()` returns instantly (hashing a
    /// multi-GB project would otherwise freeze REAPER's main thread).
    ManifestReady(Vec<FileMeta>),
    /// Owner-side: replace the manifest's session-meta blob (track count +
    /// plugin list). Computed by the extension when hosting starts and again
    /// when the served project file changes on disk.
    SetMeta(String),
    /// Share a flag marker with every session peer (one 'M' publish). The
    /// sharer's display name and color are stamped on from the live profile.
    ShareMarker { name: String, pos: f64 },
    /// Owner-side moderation: tell `peer` to leave (one 'K' publish), and with
    /// `ban` also keep it out for BAN_DURATION (the table itself is updated
    /// synchronously by `Reamesh::moderate`; the task publishes the change).
    Moderate { peer: String, ban: bool },
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
    /// Host-computed session meta JSON (track count + plugin list, parsed from
    /// the served .rpp by the extension) carried on the manifest so a joiner's
    /// trust dialog can say what a join loads *before* any download. "" until
    /// the extension supplies it.
    meta: String,
    /// Every path any manifest of this session has listed. Only these (and
    /// media the live doc references) are served: a file in the project
    /// folder the project never used stays private, and a joiner that planned
    /// its download against an older manifest can still finish it.
    ever_listed: HashSet<String>,
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
    /// The project file and the files under `project_dir` the project uses
    /// (see `referenced_files`) are offered to joining clients; anything else
    /// in the folder stays private.
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
        // Walk only: no hashes yet. This runs on REAPER's main thread (the
        // user just clicked "Host"), and blake3 over a multi-GB project takes
        // long enough to look like a hang. Hashes are filled in by a background
        // thread below; a client that requests the manifest before that
        // finishes simply downloads without resume-dedup/verify (empty hashes
        // are legal throughout the transfer path).
        let files = referenced_files(build_manifest(&dir)?, &dir, project_file, "");
        let files_for_hash = files.clone();
        let session_id = new_session_id();
        let salt = random_hex(16); // fresh CSPRNG public salt, distinct from the id
        let token = auth_token(&salt, passphrase);
        let hosting = Hosting {
            session_id: session_id.clone(),
            session_name: session_name.to_string(),
            project_file: project_file.to_string(),
            ever_listed: files.iter().map(|f| f.path.clone()).collect(),
            files,
            salt,
            auth: !passphrase.is_empty(),
            meta: String::new(),
        };
        let status = StatusState {
            state: "hosting".into(),
            progress: 1.0,
            done_bytes: 0,
            total_bytes: 0,
            files_done: 0,
            files_total: 0,
            current_file: String::new(),
            background: false,
            project_path: None,
            dir: None,
            session_id: Some(session_id),
            session_name: Some(session_name.to_string()),
            message: None,
            end_reason: None,
        };
        let r = Self::start(
            NetConfig {
                role: Role::Owner,
                hosting: Some(hosting),
                staging_dir: None,
            },
            status,
        )?;
        *r.shared.serve_dir.lock().unwrap() = Some(dir.clone());
        *r.shared.auth_token.lock().unwrap() = token;
        *r.shared.owner_peer.lock().unwrap() = r.local_peer_id.clone();
        {
            let files = files_for_hash;
            let cmd_tx = r.cmd_tx.clone();
            let shared = Arc::downgrade(&r.shared);
            let project_file = project_file.to_string();
            std::thread::Builder::new()
                .name("reamesh-hash".into())
                .spawn(move || manifest_worker(dir, project_file, shared, files, cmd_tx))
                .ok();
        }
        Ok(r)
    }

    /// Start in browsing mode: listen for session announcements on the lobby.
    /// Joined sessions are downloaded into a fresh subdirectory of
    /// `staging_dir`.
    pub fn join(staging_dir: &str) -> anyhow::Result<Box<Self>> {
        Self::join_keeping(staging_dir, &[])
    }

    /// `join`, telling the staging sweep which project files are open in this
    /// REAPER: a downloaded copy that is still open is never swept, however
    /// long it has sat untouched.
    pub fn join_keeping(staging_dir: &str, open_projects: &[PathBuf]) -> anyhow::Result<Box<Self>> {
        // Sweep stale project copies off disk before another one lands. This
        // runs on REAPER's main thread, and removing gigabytes of media is not
        // something to block it on, so it goes to its own thread.
        {
            let root = PathBuf::from(staging_dir);
            let keep = open_projects.to_vec();
            std::thread::Builder::new()
                .name("reamesh-prune".into())
                .spawn(move || prune_staging(&root, &keep))
                .ok();
        }
        let status = StatusState {
            state: "browsing".into(),
            progress: 0.0,
            done_bytes: 0,
            total_bytes: 0,
            files_done: 0,
            files_total: 0,
            current_file: String::new(),
            background: false,
            project_path: None,
            dir: None,
            session_id: None,
            session_name: None,
            message: None,
            end_reason: None,
        };
        Self::start(
            NetConfig {
                role: Role::Client,
                hosting: None,
                staging_dir: Some(PathBuf::from(staging_dir)),
            },
            status,
        )
    }

    fn start(cfg: NetConfig, status: StatusState) -> anyhow::Result<Box<Self>> {
        let role = cfg.role;
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
            sessions: Mutex::new(HashMap::new()),
            status: Mutex::new(status),
            serve_dir: Mutex::new(None),
            session_peers: Mutex::new(Vec::new()),
            auth_token: Mutex::new(String::new()),
            peers_info: Mutex::new(HashMap::new()),
            peer_rtt: Mutex::new(HashMap::new()),
            owner_peer: Mutex::new(String::new()),
            reverts: AtomicUsize::new(0),
            reverts_detail: Mutex::new(VecDeque::new()),
            peer_locks: Mutex::new(HashMap::new()),
            peer_locks_at: Mutex::new(None),
            banned: Mutex::new(HashSet::new()),
            force_revert: Mutex::new(Vec::new()),
            probe_result: Mutex::new(None),
            proposal_seq: AtomicU64::new(0),
            proposal_seqs: Mutex::new(HashMap::new()),
            session_meta: Mutex::new(String::new()),
            marker_inbox: Mutex::new(Vec::new()),
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
                    let kept = JsonValue::Object(kept);
                    if let Some(update) = apply_changeset_to_doc(&mut ds, &kept) {
                        // Re-baseline ONLY the fields this push wrote, so they
                        // never re-surface from drain_remote, and nothing else.
                        // The doc may also hold client proposals the network
                        // task admitted since the last drain (the extension
                        // captures before it drains on every tick); a full
                        // snapshot here folded those into last_state too, so
                        // the owner's own REAPER never applied an edit every
                        // other peer (and the doc) already had.
                        let after = snapshot(&ds.doc);
                        rebaseline_fields(&mut ds.last_state, &after, &kept);
                        drop(ds);
                        let _ = self.cmd_tx.send(Command::PublishAuthoritative(update));
                    }
                }
            }
            Role::Client => {
                let mut ds = self.shared.doc.lock().unwrap();
                let now = Instant::now();
                // Never propose an item chunk that would blank a file-backed
                // source (a progressive-join placeholder leaking out). The
                // extension already keeps placeholders out of its diff; this
                // is the second line, judged against our view of the doc.
                let mut parsed = parsed;
                {
                    let last = &ds.last_state;
                    reject_media_blanking(&mut parsed, |id| {
                        last.get(ITEMS_ROOT).and_then(|s| s.get(id)).and_then(|f| f.get("chunk")).cloned()
                    }, "local");
                }
                if !SECTIONS.iter().any(|s| parsed.get(s).is_some()) {
                    return;
                }
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
                                    (norm, now, 0),
                                );
                            }
                        }
                    }
                }
                drop(ds);
                self.publish_proposal(&parsed);
            }
        }
    }

    /// Send a changeset to the owner as a proposal, wrapped in an authenticated
    /// envelope: the owner only admits proposals carrying the session token, so
    /// topic membership alone can't write to the doc.
    fn publish_proposal(&self, cs: &JsonValue) {
        let envelope = json!({
            "auth": self.shared.auth_token.lock().unwrap().clone(),
            "cs": cs,
            "seq": self.next_proposal_seq(),
        });
        let _ = self
            .cmd_tx
            .send(Command::PublishProposal(envelope.to_string().into_bytes()));
    }

    /// Per-proposal sequence number: strictly increasing within this process
    /// and anchored to wall-clock microseconds so it keeps increasing across a
    /// rejoin. The owner refuses anything not newer than the last it admitted
    /// from us, which is what stops a topic member from replaying a captured
    /// proposal once gossipsub's duplicate cache has forgotten it (the
    /// envelope, sealed or not, is otherwise byte-for-byte replayable).
    fn next_proposal_seq(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        let prev = self.shared.proposal_seq.load(Ordering::Relaxed);
        let next = now.max(prev + 1);
        self.shared.proposal_seq.store(next, Ordering::Relaxed);
        next
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
        let locks_at = *self.shared.peer_locks_at.lock().unwrap();
        let pending = &mut ds.pending;
        let mut resend = JsonMap::new();
        pending.retain(|(section, id, field), (proposed, when, retries)| {
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
            // Only a lock table that arrived AFTER this edit can say the edit
            // lost the field. An older table is about a world before the edit
            // existed, and the holder it names may have lapsed since, which is
            // exactly when someone else legitimately takes over. Judging by it
            // snaps the edit back and then re-applies it a moment later when
            // the owner admits it. The owner re-broadcasts the table every tick
            // while any lock is live, so a genuine rejection still arrives fast.
            let fresh = locks_at.is_some_and(|t| t > *when);
            let held_by_other = fresh && holder.is_some_and(|h| *h != own_id);
            if held_by_other || now.duration_since(*when) > PROPOSAL_TIMEOUT {
                // Lost the field: snap REAPER back to the authoritative value
                // and record who holds it (when known) so the UI can name them.
                if let Some(dv) = doc_val {
                    let who = holder.filter(|h| **h != own_id).cloned().unwrap_or_default();
                    count_revert(section, id, &who);
                    put(&mut diff, section, id, field, dv.clone());
                } else if !held_by_other && *retries < 2 {
                    // Nothing in the doc to snap back to: the proposal itself
                    // was lost on the way (a gossip drop, a mesh the owner had
                    // not joined yet), not refused. Re-send it rather than
                    // quietly keeping an entity no other peer will ever see.
                    *retries += 1;
                    *when = now;
                    put(&mut resend, section, id, field, proposed.clone());
                    return true;
                }
                false
            } else {
                // Still in flight (held by us, or no word yet): keep optimistic.
                drop_field(&mut diff, section, id, field);
                true
            }
        });
        drop(locks);
        if !resend.is_empty() {
            self.publish_proposal(&JsonValue::Object(resend));
        }
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
                    // Self-reported by the host: a label, not an identity.
                    "owner_name": s.owner_name,
                    "owner_color": s.owner_color,
                    "auth": s.auth,
                    "files": s.files,
                    "bytes": s.bytes,
                    "contested": s.contested,
                    "proto": s.proto,
                    "version": s.version,
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

    /// Invite ticket for the session this peer is hosting: a `reamesh://` link
    /// carrying the host's peer id + dialable IPs/ports and the session id/
    /// name/auth flag, so a peer that can't overhear the LAN announcement
    /// (VPN, multicast-blocked network) can still reach it. Empty until at
    /// least one dialable listen address is known, and always empty on clients.
    pub fn session_ticket(&self) -> String {
        if self.role != Role::Owner {
            return String::new();
        }
        let (sid, name) = {
            let st = self.shared.status.lock().unwrap();
            (st.session_id.clone(), st.session_name.clone())
        };
        let (Some(sid), Some(name)) = (sid, name) else {
            return String::new();
        };
        let (ips, tcp, quic) = ticket_endpoints(&self.shared.listen_addrs.lock().unwrap());
        if ips.is_empty() || (tcp == 0 && quic == 0) {
            return String::new();
        }
        let auth = !self.shared.auth_token.lock().unwrap().is_empty();
        encode_ticket(&sid, &name, &self.local_peer_id, &ips, tcp, quic, auth)
    }

    /// Join a session through a pasted ticket (client only): dial the ticket's
    /// addresses directly, wait for the (pinned) owner's announce, then join as
    /// if it had been discovered. Returns whether the attempt was started;
    /// watch `status_json` for connecting/downloading/failed.
    pub fn join_ticket(&self, ticket: &str, passphrase: &str) -> bool {
        if self.role != Role::Client {
            return false;
        }
        let Some(info) = parse_ticket(ticket) else {
            return false;
        };
        self.cmd_tx
            .send(Command::JoinTicket {
                info,
                passphrase: passphrase.to_string(),
                probe_only: false,
            })
            .is_ok()
    }

    /// Pre-check a passphrase against a ticket's host without joining (the
    /// ticket analogue of `check_passphrase`; same `probe` reporting). An
    /// unreachable host reports `ok: false` too: the caller gets a definite
    /// answer either way.
    pub fn check_ticket(&self, ticket: &str, passphrase: &str) -> bool {
        if self.role != Role::Client {
            return false;
        }
        let Some(info) = parse_ticket(ticket) else {
            return false;
        };
        *self.shared.probe_result.lock().unwrap() =
            Some((info.session_id.clone(), None, JsonValue::Null, ""));
        self.cmd_tx
            .send(Command::JoinTicket {
                info,
                passphrase: passphrase.to_string(),
                probe_only: true,
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
        *self.shared.probe_result.lock().unwrap() =
            Some((session_id.to_string(), None, JsonValue::Null, ""));
        self.cmd_tx
            .send(Command::ProbeAuth {
                session_id: session_id.to_string(),
                passphrase: passphrase.to_string(),
            })
            .is_ok()
    }

    /// Set the session-meta blob the manifest carries (owner only, but stored
    /// unconditionally so `session_meta_json` reads back what was set).
    pub fn set_session_meta(&self, meta: &str) {
        let clipped: String = meta.chars().take(256 * 1024).collect();
        *self.shared.session_meta.lock().unwrap() = clipped.clone();
        let _ = self.cmd_tx.send(Command::SetMeta(clipped));
    }

    /// The current session's meta blob: what this owner set, or what the
    /// owner's manifest claimed (client). "" when none is known.
    pub fn session_meta_json(&self) -> String {
        self.shared.session_meta.lock().unwrap().clone()
    }

    /// Ping a timeline spot to every session peer: `{"name","pos"}` (a
    /// "shared marker" on the wire; receivers show it as a notice). The
    /// sharer's profile name/color are stamped on by the network task. The
    /// caller shows its own copy itself (gossipsub does not loop a publish
    /// back to its sender).
    pub fn share_marker(&self, marker_json: &str) -> bool {
        let Ok(v) = serde_json::from_str::<JsonValue>(marker_json) else {
            return false;
        };
        let name: String = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .chars()
            .take(256)
            .collect();
        let pos = v.get("pos").and_then(|x| x.as_f64()).unwrap_or(0.0);
        if name.is_empty() {
            return false;
        }
        self.cmd_tx.send(Command::ShareMarker { name, pos }).is_ok()
    }

    /// Owner-side moderation: make `peer_id` leave the session now (`ban` =
    /// false: a kick, it may rejoin at once) or keep it out for BAN_DURATION
    /// (`ban` = true). The ban lands in the process-global table immediately
    /// (so `bans_json` and file serving reflect it before the network task
    /// runs); the task publishes the 'K' notice and the updated ban table.
    /// Returns false on a client, for ourselves, or for an empty id.
    pub fn moderate(&self, peer_id: &str, ban: bool) -> bool {
        if self.role != Role::Owner || peer_id.is_empty() || peer_id == self.local_peer_id {
            return false;
        }
        if peer_id.parse::<PeerId>().is_err() {
            return false;
        }
        if ban {
            let (name, color) = self
                .shared
                .peers_info
                .lock()
                .unwrap()
                .get(peer_id)
                .map(|p| (p.name.clone(), p.color.clone()))
                .unwrap_or_default();
            BANS.lock().unwrap().insert(
                peer_id.to_string(),
                Ban {
                    name,
                    color: if color.is_empty() { default_color(peer_id) } else { color },
                    until: Instant::now() + BAN_DURATION,
                    until_unix_ms: unix_ms_now() + BAN_DURATION.as_millis() as u64,
                },
            );
            // Deliberately NOT mirrored into `shared.banned` here: the task's
            // tick detects the table change against that mirror and that is
            // what triggers the blacklist + 'B' broadcast. The owner's file
            // serving reads the global table directly, so the refusal is
            // immediate regardless.
        }
        // Gone from our roster right away, whatever it does about the notice.
        self.shared.peers_info.lock().unwrap().remove(peer_id);
        self.cmd_tx
            .send(Command::Moderate { peer: peer_id.to_string(), ban })
            .is_ok()
    }

    /// Pings (shared markers) received from peers since the last call, as a JSON array
    /// of `{"name","pos","by","color"}` ("[]" when none).
    pub fn drain_markers(&self) -> String {
        let drained: Vec<JsonValue> =
            std::mem::take(&mut *self.shared.marker_inbox.lock().unwrap());
        json!(drained).to_string()
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
            // `reason` says why a probe came back false: "rejected" (wrong
            // passphrase: the only one the app may call that), "timeout",
            // "unreachable" or "gone" (the session vanished from the lobby),
            // or "version" (the host runs a build with another PROTO).
            Some((sid, ok, preview, reason)) => {
                json!({ "session_id": sid, "ok": ok, "preview": preview, "reason": reason })
            }
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
    /// `{"id","name","color",…,"age","is_self","is_owner"}`. A topic subscriber
    /// that never produced a verifying beacon (no session token) is not listed:
    /// topic membership is public knowledge, roster membership is not. A
    /// legitimate joiner appears within one beacon interval (~1s). `age` is
    /// the milliseconds since that peer's latest beacon landed here (0 for
    /// self): follow mode adds it (plus half the RTT) to the beaconed play
    /// position to estimate where the followed transport is *now*.
    pub fn peers_json(&self) -> String {
        let owner = self.shared.owner_peer.lock().unwrap().clone();
        let (my_name, my_color) = PROFILE.lock().unwrap().clone();
        let me = LIVE.lock().unwrap().clone();
        let entry = |id: &str,
                     name: &str,
                     color: &str,
                     focus: &str,
                     play: f64,
                     view: (f64, f64),
                     playing: bool,
                     follow: &str,
                     sel: &[String],
                     fx: &JsonValue,
                     frozen: bool,
                     rtt: u64,
                     age: u64,
                     is_self: bool| {
            json!({
                "id": id,
                "name": name,
                "color": if color.is_empty() { default_color(id) } else { color.to_string() },
                "focus": focus,
                "play": play,
                "v0": view.0,
                "v1": view.1,
                "playing": playing,
                "follow": follow,
                "sel": sel,
                "fx": fx,
                "frozen": frozen,
                // Smoothed round trip to this peer in whole ms (rounded up so a
                // real sub-ms LAN sample is 1, never the "unknown" 0).
                "rtt": rtt,
                "age": age,
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
            (me.v0, me.v1),
            me.playing,
            &me.follow,
            &me.sel,
            &me.fx,
            false, // if our own main thread were frozen, nobody would be asking
            0,     // no round trip to ourselves
            0,     // our own presence is never stale
            true,
        )];
        let mut rtts = self.shared.peer_rtt.lock().unwrap();
        rtts.retain(|_, (_, at)| at.elapsed() < RTT_STALE);
        let mut infos = self.shared.peers_info.lock().unwrap();
        let now = Instant::now();
        infos.retain(|_, p| now.duration_since(p.last_seen) < PRESENCE_STALE);
        // Everyone with a fresh authenticated beacon, directly connected or
        // not: where peers can't discover each other (multicast blocked, a
        // guest network, a ticket join), two guests are each connected only
        // to the host, and their beacons reach each other through the host's
        // gossip relay. Requiring a direct connection hid them from each
        // other. A direct member that leaves is dropped from `peers_info` by
        // the tick right away; a relayed one ages out after PRESENCE_STALE.
        // Sorted for a deterministic roster order, so the composed app state
        // doesn't "change" every tick and rows don't shuffle.
        let mut others: Vec<String> = infos
            .keys()
            .filter(|id| id.as_str() != self.local_peer_id)
            .cloned()
            .collect();
        others.sort();
        others.dedup();
        let banned = self.shared.banned.lock().unwrap();
        for id in others {
            if banned.contains(&id) {
                continue;
            }
            if let Some(info) = infos.get(&id) {
                arr.push(entry(
                    &id,
                    &info.name,
                    &info.color,
                    &info.focus,
                    info.play,
                    (info.v0, info.v1),
                    info.playing,
                    &info.follow,
                    &info.sel,
                    &info.fx,
                    info.frozen,
                    rtts.get(&id).map(|(ms, _)| ms.ceil() as u64).unwrap_or(0),
                    now.duration_since(info.last_seen).as_millis() as u64,
                    false,
                ));
            }
        }
        json!(arr).to_string()
    }

    /// Peer ids this peer currently treats as banned (a client: the owner's
    /// last 'B' table; the owner: its tick mirror of the global table), sorted.
    /// A test hook for the ban broadcast; the app reads `bans_json` instead.
    pub fn banned_peers(&self) -> Vec<String> {
        let mut v: Vec<String> = self.shared.banned.lock().unwrap().iter().cloned().collect();
        v.sort();
        v
    }

    /// Number of verified presence beacons received from `peer_id` so far
    /// (0 for an unknown peer). A test hook for the beacon cadence: the
    /// roster JSON deliberately carries no counter, since a value that moves
    /// on every beacon would churn the composed app state.
    pub fn beacon_count(&self, peer_id: &str) -> u64 {
        self.shared
            .peers_info
            .lock()
            .unwrap()
            .get(peer_id)
            .map(|p| p.beacons)
            .unwrap_or(0)
    }

    /// The live editing locks currently held by a peer *other than us*, as a
    /// JSON array of `{"section","id","field","holder"}`. Read from the live
    /// lock table (`peer_locks`: what the owner broadcasts and a client last
    /// received), so it only ever lists locks still inside `LOCK_HOLD`; the
    /// owner's own held fields and a client's own held fields are filtered out
    /// (holder == us). The extension uses this purely to tint media items
    /// another peer is editing: it is a local view hint, never project state
    /// and never anything that rides back out on the wire.
    pub fn locked_entities_json(&self) -> String {
        let me = self.local_peer_id.as_str();
        let locks = self.shared.peer_locks.lock().unwrap();
        let arr: Vec<JsonValue> = locks
            .iter()
            .filter(|(_, holder)| holder.as_str() != me)
            .map(|((section, id, field), holder)| {
                json!({ "section": section, "id": id, "field": field, "holder": holder })
            })
            .collect();
        json!(arr).to_string()
    }

    /// Number of peers currently subscribed to this session's topic.
    pub fn connected_peers(&self) -> usize {
        self.shared.session_peers.lock().unwrap().len()
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

/// Read one field from the document without taking a full snapshot.
fn doc_field(doc: &Doc, section: &str, id: &str, field: &str) -> Option<JsonValue> {
    let root = doc.get_or_insert_map(section);
    let txn = doc.transact();
    match root.get(&txn, id)? {
        Out::YMap(entity) => match entity.get(&txn, field)? {
            Out::Any(any) => Some(any_to_json(&any)),
            _ => None,
        },
        _ => None,
    }
}

/// Would replacing `old_chunk` by `new_chunk` turn a file-backed media item
/// into an empty one? That is what a progressive-join placeholder (`<SOURCE
/// EMPTY>` standing in for media still downloading) looks like if it ever
/// escaped onto the wire, and there is no editing gesture in REAPER that
/// produces it: a take keeps its source until the item goes. MIDI/section/
/// empty items never had a FILE line and are unaffected.
fn blanks_media_source(old_chunk: &str, new_chunk: &str) -> bool {
    let has_file = |c: &str| c.lines().any(|l| l.trim_start().starts_with("FILE "));
    let has_empty = |c: &str| c.lines().any(|l| l.trim_start().starts_with("<SOURCE EMPTY"));
    has_file(old_chunk) && has_empty(new_chunk) && !has_file(new_chunk)
}

/// Strip every item `chunk` field from `changeset` that would blank a media
/// source (see `blanks_media_source`), judged against `current(id)`, the chunk
/// the item has now. Entities and sections left empty are removed too.
fn reject_media_blanking(
    changeset: &mut JsonValue,
    current: impl Fn(&str) -> Option<JsonValue>,
    who: &str,
) {
    let Some(items) = changeset.get_mut(ITEMS_ROOT).and_then(|v| v.as_object_mut()) else {
        return;
    };
    let mut dropped = Vec::new();
    for (id, fields) in items.iter_mut() {
        let Some(fields) = fields.as_object_mut() else { continue };
        let Some(new_chunk) = fields.get("chunk").and_then(|v| v.as_str()) else { continue };
        let Some(old) = current(id) else { continue };
        let Some(old_chunk) = old.as_str() else { continue };
        if blanks_media_source(old_chunk, new_chunk) {
            eprintln!("[reamesh] refused item {id} from {who}: chunk would blank its media source");
            fields.remove("chunk");
            if fields.is_empty() {
                dropped.push(id.clone());
            }
        }
    }
    for id in dropped {
        items.remove(&id);
    }
    if items.is_empty() {
        changeset.as_object_mut().map(|o| o.remove(ITEMS_ROOT));
    }
}

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

/// Copy just the fields named by `changeset` (any subset of sections) from
/// `from` into `into`, leaving every other field of `into` untouched. This is
/// how the owner re-baselines its own push: the fields it wrote are marked
/// seen, while anything else that changed in the doc meanwhile (an admitted
/// client proposal) still surfaces from the next drain_remote.
fn rebaseline_fields(into: &mut StateMap, from: &StateMap, changeset: &JsonValue) {
    for section in SECTIONS {
        let Some(entities) = changeset.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        for (id, fields) in entities {
            let Some(fields) = fields.as_object() else { continue };
            for key in fields.keys() {
                let Some(v) = from
                    .get(section)
                    .and_then(|s| s.get(id))
                    .and_then(|f| f.get(key))
                else {
                    continue;
                };
                into.entry(section.to_string())
                    .or_default()
                    .entry(id.clone())
                    .or_default()
                    .insert(key.clone(), v.clone());
            }
        }
    }
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
                // Hash deliberately left empty: see `hash_manifest`.
                out.push(FileMeta { path, size: meta.len(), hash: String::new() });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Narrow a folder walk to the files the project actually uses: the project
/// file itself plus every file whose name appears in the project text, the
/// `.rpp` on disk and `live_text` (the live doc on a running session). The
/// text is searched with plugin state decoded too, so a sampler or
/// convolution plugin keeps the sample/impulse it loads from the project
/// folder; a referenced sub-project (`.rpp`) brings in what it uses in turn.
///
/// A file whose name is unique in the folder matches on its name alone (the
/// project may store it absolute, relative, or with Windows separators); two
/// files sharing a name match on their project-relative path. Erring towards
/// sharing a same-named file is harmless; missing one the project needs is
/// not, which is why the match is a plain case-insensitive substring search.
fn referenced_files(files: Vec<FileMeta>, dir: &Path, project_file: &str, live_text: &str) -> Vec<FileMeta> {
    use aho_corasick::{AhoCorasick, MatchKind};

    let lower = |p: &str| p.to_ascii_lowercase();
    let base_of = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let mut name_count: HashMap<String, usize> = HashMap::new();
    for f in &files {
        *name_count.entry(lower(&base_of(&f.path))).or_default() += 1;
    }
    let patterns: Vec<String> = files
        .iter()
        .map(|f| {
            let base = base_of(&f.path);
            if name_count.get(&lower(&base)).copied().unwrap_or(0) > 1 { f.path.clone() } else { base }
        })
        .collect();
    let Ok(ac) = AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .match_kind(MatchKind::Standard)
        .build(&patterns)
    else {
        return files; // can't search: share the walk as before rather than nothing
    };

    let mut keep: Vec<bool> = files.iter().map(|f| f.path == project_file).collect();
    let mut read_texts: HashSet<String> = HashSet::new();
    let mut texts: Vec<Vec<u8>> = vec![live_text.as_bytes().to_vec()];
    let mut to_read: Vec<String> = vec![project_file.to_string()];
    to_read.retain(|p| !p.is_empty());
    // Sub-projects nest a few levels at most; the bound only guards a cycle.
    for _ in 0..8 {
        for rel in to_read.drain(..) {
            if read_texts.insert(rel.clone())
                && let Ok(t) = fs::read(dir.join(&rel))
            {
                texts.push(t);
            }
        }
        for text in texts.drain(..) {
            let hay = expand_haystack(&text);
            for m in ac.find_overlapping_iter(&hay) {
                let i = m.pattern().as_usize();
                if !keep[i] {
                    keep[i] = true;
                    let path = &files[i].path;
                    if path.to_ascii_lowercase().ends_with(".rpp") && !read_texts.contains(path) {
                        to_read.push(path.clone());
                    }
                }
            }
        }
        if to_read.is_empty() {
            break;
        }
    }
    files.into_iter().zip(keep).filter_map(|(f, k)| k.then_some(f)).collect()
}

/// Project text prepared for the name search in `referenced_files`: the text
/// itself with Windows separators turned into '/', followed by every run of
/// base64 lines (binary plugin state) decoded, the same way.
fn expand_haystack(text: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = text.iter().map(|&b| if b == b'\\' { b'/' } else { b }).collect();
    let mut decoded = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for line in text.split(|&b| b == b'\n') {
        let t = line.trim_ascii();
        let is_b64 = t.len() >= 4
            && t.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=');
        if !is_b64 {
            if bits != 0 || decoded.last().is_some_and(|&b| b != b'\n') {
                decoded.push(b'\n');
            }
            acc = 0;
            bits = 0;
            continue;
        }
        for &c in t {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => {
                    // Padding ends an encoded segment; the next one starts fresh.
                    acc = 0;
                    bits = 0;
                    continue;
                }
            };
            acc = (acc << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                decoded.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
    }
    out.push(b'\n');
    out.extend(decoded.into_iter().map(|b| if b == b'\\' { b'/' } else { b }));
    out
}

/// Every string field of every live (non-deleted) track and item in the doc,
/// newline-joined: what the session currently references, for
/// `referenced_files` and `doc_references`.
fn doc_text(doc: &Doc) -> String {
    let roots: Vec<MapRef> = [TRACKS_ROOT, ITEMS_ROOT].iter().map(|s| doc.get_or_insert_map(*s)).collect();
    let txn = doc.transact();
    let mut out = String::new();
    for root in roots {
        for (_, value) in root.iter(&txn) {
            let Out::YMap(entity) = value else { continue };
            if let Some(Out::Any(Any::Bool(false))) = entity.get(&txn, "present") {
                continue;
            }
            for (_, v) in entity.iter(&txn) {
                if let Out::Any(Any::String(text)) = v {
                    out.push_str(&text);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Owner-side serving check for a path no manifest has listed yet: whether
/// the live doc references it (media added since the last manifest refresh).
fn doc_references(doc: &Doc, path: &str) -> bool {
    let probe = FileMeta { path: path.to_string(), size: 0, hash: String::new() };
    !referenced_files(vec![probe], Path::new(""), "", &doc_text(doc)).is_empty()
}

/// Owner-side hash cache: project-relative path -> (size, mtime, blake3) as of
/// the last time we hashed it. Never crosses the wire; it exists so a manifest
/// refresh only re-hashes the files that actually changed.
type HashCache = HashMap<String, (u64, Option<SystemTime>, String)>;

/// Fill in the blake3 hash of every manifest entry, reusing `cache` for files
/// whose size and mtime are unchanged. Runs on a background thread (never on
/// the REAPER main thread); the result replaces the served manifest via
/// `Command::ManifestReady`. A file that changed size since the walk (the user
/// is still editing) keeps an empty hash: the transfer path treats that as
/// "download unconditionally, skip resume and verify". `cancelled` is polled
/// between files so a session that ends mid-pass (minutes, on a multi-GB
/// project) stops the pass instead of letting the thread outlive the session.
fn hash_manifest(
    dir: &Path,
    mut files: Vec<FileMeta>,
    cache: &mut HashCache,
    cancelled: &dyn Fn() -> bool,
) -> Vec<FileMeta> {
    for meta in &mut files {
        if cancelled() {
            break;
        }
        let full = dir.join(&meta.path);
        let Ok(m) = fs::metadata(&full) else { continue };
        if m.len() != meta.size {
            continue; // changed under the walk: leave it unhashed
        }
        let mtime = m.modified().ok();
        // (size, mtime) can't tell two same-length writes apart when they land
        // inside the filesystem's mtime granularity (a second on HFS+/ext3, two
        // on FAT): a stale hash there would fail every later joiner's verify
        // for the rest of the session. So the cache is only trusted for files
        // that have sat still for a couple of refresh intervals; whatever the
        // host is actively saving is simply re-hashed.
        let settled = mtime
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= 2 * MANIFEST_REFRESH);
        if settled
            && let Some((size, stamp, hash)) = cache.get(&meta.path)
            && *size == meta.size
            && *stamp == mtime
            && !hash.is_empty()
        {
            meta.hash = hash.clone();
            continue;
        }
        meta.hash = blake3_file(&full).unwrap_or_default();
        cache.insert(meta.path.clone(), (meta.size, mtime, meta.hash.clone()));
    }
    files
}

/// Owner-side background worker keeping the served manifest in step with the
/// project on disk. Hosting starts with an unhashed manifest so `host()`
/// returns instantly, and the manifest was historically hashed once and then
/// frozen for the whole session, but a joiner plans its entire download
/// against it (piece offsets, per-chunk length checks, the blake3 it verifies
/// against), so any file the host rewrites mid-session: a Ctrl+S on the .rpp
/// above all: left every later join requeueing a chunk whose length no longer
/// matched until it timed out. Re-walking is cheap (one stat per file) and
/// re-hashing is limited to entries whose size or mtime moved.
///
/// Exits when the network task's command receiver is gone (the session ended).
fn manifest_worker(
    dir: PathBuf,
    project_file: String,
    shared: std::sync::Weak<Shared>,
    files: Vec<FileMeta>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
) {
    let mut cache = HashCache::new();
    let mut walked = files;
    let mut last_sent: Option<Vec<FileMeta>> = None;
    loop {
        let hashed = hash_manifest(&dir, walked.clone(), &mut cache, &|| cmd_tx.is_closed());
        if cmd_tx.is_closed() {
            return;
        }
        if last_sent.as_ref() != Some(&hashed) {
            if cmd_tx.send(Command::ManifestReady(hashed.clone())).is_err() {
                return;
            }
            last_sent = Some(hashed);
        }
        // Sleep in slices so a session that ends between refreshes doesn't
        // leave this thread lingering for the whole interval.
        let mut waited = Duration::ZERO;
        while waited < MANIFEST_REFRESH {
            if cmd_tx.is_closed() {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
            waited += Duration::from_millis(500);
        }
        // A momentarily unreadable project folder keeps the previous walk
        // rather than publishing an empty manifest. The live doc counts as
        // project text too: media the host added since the last save is
        // listed for a late joiner without waiting for a Ctrl+S.
        if let Ok(v) = build_manifest(&dir) {
            let live = shared.upgrade().map(|sh| doc_text(&sh.doc.lock().unwrap().doc)).unwrap_or_default();
            walked = referenced_files(v, &dir, &project_file, &live);
        }
    }
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
    // No .reapeaks here: peak caches are deliberately never transferred
    // (skip_in_manifest drops them, and a mid-session fetch only ever asks for
    // media paths parsed out of item chunks), so listing them would only
    // suggest otherwise.
    [".rpp", ".rtracktemplate", ".rfxchain", ".txt", ".xml", ".csv"]
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

/// Entry budget of a bounded directory walk: enough for any real project
/// tree, small enough that a huge staging root can't turn a sweep into a
/// full-disk walk.
const WALK_BUDGET: u32 = 50_000;

/// Depth-first walk of everything under `root`, calling `visit` with each
/// entry and its metadata (unreadable directories and entries are skipped).
/// Returns `false` when the entry budget ran out: the walk was partial.
fn walk_bounded(root: &Path, mut visit: impl FnMut(&fs::DirEntry, &fs::Metadata)) -> bool {
    let mut stack = vec![root.to_path_buf()];
    let mut budget = WALK_BUDGET;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            if budget == 0 {
                return false;
            }
            budget -= 1;
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            }
            visit(&entry, &meta);
        }
    }
    true
}

/// Newest modification time anywhere under `dir` (the directory's own stamp
/// when it is empty or unreadable). A tree too big to walk within the budget
/// reports `None` ("unknown"), never an age derived from a partial walk that
/// could brand a live project copy stale.
fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut newest = fs::metadata(dir).and_then(|m| m.modified()).ok();
    let complete = walk_bounded(dir, |_, meta| {
        if let Ok(t) = meta.modified()
            && newest.is_none_or(|n| t > n)
        {
            newest = Some(t);
        }
    });
    if complete { newest } else { None }
}

/// Delete staging subdirectories nothing has touched for `STAGING_MAX_AGE`.
///
/// Each join downloads a full copy of the host's project into its own
/// directory, and a session id is minted fresh per hosting run, so rejoining
/// the same project tomorrow never reuses today's directory, and the dedup path
/// *copies* the matching bytes across rather than sharing them. Nothing else
/// ever removes any of it, so a few evenings of sessions used to leave tens of
/// gigabytes behind with no way for the user to find or clear them.
///
/// Age is the newest mtime found anywhere inside. Never remove a live session,
/// a directory served by another local REAPER instance, or a project in `keep`.
fn prune_staging(root: &Path, keep: &[PathBuf]) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for entry in rd.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if keep.iter().any(|k| k.starts_with(&path)) {
            continue;
        }
        let stale = newest_mtime(&path)
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STAGING_MAX_AGE);
        if stale {
            eprintln!("[reamesh] pruning stale staging copy {}", path.display());
            let _ = fs::remove_dir_all(&path);
        }
    }
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
    /// Round-trip probe to every connected peer: the only latency signal the
    /// app has (gossip carries no timestamps), see `Shared::peer_rtt`.
    ping: ping::Behaviour,
}

fn session_topic(session_id: &str) -> gossipsub::IdentTopic {
    gossipsub::IdentTopic::new(format!("reamesh/v2/session/{session_id}"))
}

fn build_swarm() -> anyhow::Result<Swarm<Behaviour>> {
    // The per-install persisted keypair when the extension registered one
    // (lasting identity: remembered hosts, tickets that survive a restart), a
    // fresh ephemeral one otherwise (tests spin many peers per process).
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(identity_keypair())
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
            let ping = ping::Behaviour::new(
                ping::Config::new()
                    .with_interval(PING_INTERVAL)
                    .with_timeout(PING_TIMEOUT),
            );
            Ok(Behaviour { gossipsub, mdns, xfer, ping })
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
    /// Set after a checksum mismatch: pieces of this file go to the owner only.
    owner_only: bool,
    /// Whole and verified at `dest` (dedup'd, empty, or renamed into place).
    complete: bool,
    /// Audio/video by extension: held back behind the project file and the
    /// small files, then streamed in timeline order (progressive join).
    media: bool,
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
    inflight: HashMap<request_response::OutboundRequestId, (Piece, Instant)>,
    /// Current pipelining depth (see DOWNLOAD_WINDOW_MIN).
    window: usize,
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
    /// Completed files whose blake3 verification is still running on a
    /// blocking task (hashing a GB file would stall the event loop). The
    /// download only finalizes once this drops to zero.
    verifying: usize,
    /// Media pieces held back until the project file is on disk, so they can
    /// be queued in timeline order (see `release_media`).
    media: Vec<Piece>,
    media_released: bool,
    /// Progressive join: `ready` was already reported with media outstanding.
    early_ready: bool,
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
    /// The request currently in flight. A response to any other request (a
    /// stall retry that was superseded) is dropped: appended at the current
    /// offset it would silently corrupt the file.
    req: Option<request_response::OutboundRequestId>,
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

/// Fire the lone manifest request a passphrase pre-check consists of. The
/// owner answers `Manifest` iff the token derived from its salt and this
/// passphrase is right (`Error` otherwise). The reply is matched by request id
/// and verifies the passphrase without any download or
/// topic subscription. Shared by the discovered-session and the ticket check.
fn send_probe(
    swarm: &mut Swarm<Behaviour>,
    owner: &PeerId,
    salt: &str,
    session_id: String,
    passphrase: &str,
) -> Probe {
    let req = swarm.behaviour_mut().xfer.send_request(
        owner,
        XferRequest::Manifest { auth: auth_token(salt, passphrase) },
    );
    Probe { req, session_id, started: Instant::now() }
}

fn fetch_send(
    swarm: &mut Swarm<Behaviour>,
    fetch_reqs: &mut FetchReqs,
    fetch: &mut Fetch,
) -> bool {
    let Some(peer) = fetch.peer() else { return false };
    fetch.last_activity = Instant::now();
    let id = swarm.behaviour_mut().xfer.send_request(&peer, fetch.request());
    fetch.req = Some(id);
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
        req: None,
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
    request_id: request_response::OutboundRequestId,
    response: Option<XferResponse>,
) {
    let Some(fetch) = fetches.get_mut(rel) else { return };
    if fetch.req != Some(request_id) {
        return; // a superseded retry answering late; only the live request counts
    }

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

/// Publish our presence beacon on the session topic: name, color, focused
/// track, timeline position, view span, transport state, who we follow, the
/// selected items and the open FX window (the last two are what a follower
/// mirrors to see what this user is editing).
/// Called from the network thread's beacon timer (see `PRESENCE_INTERVAL` /
/// `PRESENCE_FAST_INTERVAL` for the cadence).
fn publish_presence(swarm: &mut Swarm<Behaviour>, shared: &Shared, topic: &gossipsub::IdentTopic) {
    let (name, color) = PROFILE.lock().unwrap().clone();
    let live = LIVE.lock().unwrap().clone();
    // The MAC proves token possession; peers only roster beacons that verify
    // (see presence_mac).
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
            "v0": live.v0,
            "v1": live.v1,
            "playing": live.playing,
            "follow": live.follow,
            "sel": live.sel,
            "fx": live.fx,
            // This beacon is sent by the *network* thread; the main-thread
            // presence feed going quiet while it still runs is exactly
            // "REAPER isn't responding".
            "frozen": main_thread_stale(),
            "mac": mac,
        })
        .to_string()
        .as_bytes(),
    );
    let _ = publish_session(swarm, shared, topic, &data);
}

fn set_status(shared: &Shared, f: impl FnOnce(&mut StatusState)) {
    f(&mut shared.status.lock().unwrap());
}

/// Handler for transfer requests. Every request carries the session access
/// token, checked against `auth_token` (empty = open session) before anything
/// is served. `Manifest` is owner-only; `File` is served by every peer that
/// has a project copy (which powers both the windowed initial download and
/// mid-session media fetch), limited to the paths `may_serve` admits.
/// Project text chunks are returned zstd-compressed.
fn serve_xfer(
    hosting: Option<&Hosting>,
    serve_dir: Option<&Path>,
    auth_token: &str,
    banned: bool,
    req: &XferRequest,
    may_serve: &dyn Fn(&str) -> bool,
) -> XferResponse {
    // A banned peer gets a distinct answer (a probe/join reports "banned",
    // not "wrong password"), and it gets it whether or not it still holds
    // the right token: the ban outranks the passphrase.
    if banned {
        return XferResponse::Error {
            message: BANNED_MESSAGE.into(),
        };
    }
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
            if !may_serve(path) {
                return XferResponse::Error {
                    message: format!("not part of this session: {path}"),
                };
            }
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
                    // Level 1: this runs synchronously in the serving peer's
                    // network event loop, once per chunk: throughput over
                    // ratio (.rpp text still shrinks ~8x).
                    if let Ok(z) = zstd::encode_all(&buf[..], 1) {
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
                meta: h.meta.clone(),
            },
            None => XferResponse::Error {
                message: "not hosting".into(),
            },
        },
    }
}

/// The refusal a banned peer gets from every serving peer, and the status
/// message its own join fails with. The extension maps it to a stable code;
/// a probe maps it to reason "banned".
const BANNED_MESSAGE: &str = "banned from this session by the host";

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
    let mut parsed = envelope.get("cs")?.clone();
    // Replay guard: a proposal must be newer than the last one admitted from
    // this proposer. gossipsub's duplicate cache forgets a message after a
    // couple of minutes, after which any topic member could re-forward a
    // captured (still validly signed and sealed) proposal as that client's edit.
    let seq = envelope.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
    {
        let mut seqs = shared.proposal_seqs.lock().unwrap();
        let last = seqs.get(proposer).copied().unwrap_or(0);
        if seq <= last {
            eprintln!("[reamesh] proposal from {proposer} rejected: sequence {seq} not after {last}");
            return None;
        }
        seqs.insert(proposer.to_string(), seq);
    }

    let mut ds = shared.doc.lock().unwrap();
    // Third line of the placeholder guard (after the extension's diff pinning
    // and the client's push_local check): a chunk that would blank a media
    // source out of an item is refused here too, so no client build, however
    // broken, can ever strip media from the host's project.
    reject_media_blanking(&mut parsed, |id| doc_field(&ds.doc, ITEMS_ROOT, id, "chunk"), proposer);
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

/// Publish one tagged message on the session topic, sealing it first when the
/// session is passphrase-protected (see `TAG_SEALED`). Every session-topic
/// publish goes through here so no message class can leak plaintext by
/// accident. Returns whether the publish succeeded: callers use a failure to
/// queue a retry / full sync exactly as they did with a raw publish. Callers
/// keep (and retry with) the *plaintext* tagged message; each attempt is
/// sealed fresh with a new nonce.
fn publish_session(
    swarm: &mut Swarm<Behaviour>,
    shared: &Shared,
    topic: &gossipsub::IdentTopic,
    tagged: &[u8],
) -> bool {
    let data = match gossip_key(&shared.auth_token.lock().unwrap()) {
        Some(key) => {
            let sender = swarm.local_peer_id().to_string();
            match seal_session_msg(&key, tagged, &sender) {
                Some(sealed) => sealed,
                None => return false,
            }
        }
        None => tagged.to_vec(),
    };
    swarm
        .behaviour_mut()
        .gossipsub
        .publish(topic.clone(), data)
        .is_ok()
}

/// The 'B' ban-table message for `live` (peer id strings).
fn ban_table_msg(live: &HashSet<String>) -> Vec<u8> {
    let mut ids: Vec<&String> = live.iter().collect();
    ids.sort();
    let mut data = vec![TAG_BANS];
    data.extend_from_slice(json!({ "bans": ids }).to_string().as_bytes());
    data
}

/// Bring gossipsub's blacklist in line with `want`: newly banned peers are
/// blacklisted (their messages rejected, their future connections ignored by
/// gossipsub), peers no longer banned are released. `current` tracks what is
/// blacklisted now.
fn sync_blacklist(
    swarm: &mut Swarm<Behaviour>,
    current: &mut HashSet<PeerId>,
    want: &HashSet<String>,
) {
    let want: HashSet<PeerId> = want.iter().filter_map(|s| s.parse().ok()).collect();
    for p in current.difference(&want) {
        swarm.behaviour_mut().gossipsub.remove_blacklisted_peer(p);
    }
    for p in want.difference(current) {
        swarm.behaviour_mut().gossipsub.blacklist_peer(p);
    }
    *current = want;
}

/// An in-flight ticket join/check: dialing + waiting for the pinned owner's
/// announce (see Command::JoinTicket and the tick handling).
struct PendingJoin {
    info: TicketInfo,
    passphrase: String,
    probe_only: bool,
    started: Instant,
    last_dial: Instant,
    /// Set once the transport handshake to the pinned host completes. Until it
    /// does, TICKET_CONNECT_TIMEOUT fast-fails an unreachable host; after it
    /// does, the host is alive and gets the full TICKET_JOIN_TIMEOUT to
    /// announce.
    connected: bool,
}

/// Dial a ticket's host: remember its addresses (request-response dials by
/// peer id), mark it an explicit gossipsub peer, and connect. When one of the
/// ticket's addresses is actually one of *our own* IPs (two instances on one
/// machine: the documented dev setup), also try the loopback rewrite, for the
/// same reason the mDNS path does.
fn dial_ticket(
    swarm: &mut Swarm<Behaviour>,
    info: &TicketInfo,
    local_ips: &std::collections::HashSet<std::net::IpAddr>,
) {
    swarm.behaviour_mut().gossipsub.add_explicit_peer(&info.peer);
    let mut addresses = Vec::new();
    for addr in &info.addrs {
        if let Some(lo) = loopback_variant(addr, local_ips) {
            swarm.add_peer_address(info.peer, lo.clone());
            addresses.push(lo);
        }
        swarm.add_peer_address(info.peer, addr.clone());
        addresses.push(addr.clone());
    }
    if let Err(e) = swarm.dial(
        DialOpts::peer_id(info.peer)
            .addresses(addresses)
            .allocate_new_port()
            .build(),
    ) {
        // Includes the benign "already connected/dialing" conditions.
        eprintln!("[reamesh] ticket dial: {e}");
    }
}

/// Begin joining an announced session: derive + remember the token, reset the
/// per-session client state, and fire the manifest request. Returns the
/// in-progress download, or None with status already set to "failed". Shared
/// by discovered joins (Command::JoinSession) and ticket joins once the
/// ticket's owner announce arrived.
fn start_join(
    swarm: &mut Swarm<Behaviour>,
    shared: &Shared,
    staging_dir: Option<&Path>,
    session_id: &str,
    passphrase: &str,
) -> Option<Download> {
    let info = shared.sessions.lock().unwrap().get(session_id).cloned();
    let Some(info) = info else {
        set_status(shared, |s| {
            s.state = "failed".into();
            s.message = Some(format!("session {session_id} is no longer announced"));
        });
        return None;
    };
    if info.proto != PROTO {
        // The app greys such a session out; this covers a ticket join (whose
        // link carries no version) and anything that got past the lobby.
        set_status(shared, |s| {
            s.state = "failed".into();
            s.message = Some(format!(
                "version mismatch: host {} (proto {}), here {VERSION} (proto {PROTO})",
                if info.version.is_empty() { "?" } else { &info.version },
                info.proto
            ));
        });
        return None;
    }
    let Ok(owner) = info.owner.parse::<PeerId>() else {
        set_status(shared, |s| {
            s.state = "failed".into();
            s.message = Some("bad owner peer id in announcement".into());
        });
        return None;
    };
    // Derive (and remember) this session's access token so we
    // both present it and can serve other peers later.
    let token = auth_token(&info.salt, passphrase);
    *shared.auth_token.lock().unwrap() = token.clone();
    *shared.owner_peer.lock().unwrap() = info.owner.clone();
    // Deliberately NOT subscribing to the session topic yet:
    // joining the topic is what makes a peer visible in the
    // session (peer lists, presence). That visibility must be
    // gated on the owner accepting our token: otherwise a
    // wrong-passphrase joiner flashes up as a ghost peer on
    // everyone's roster until its download fails. The
    // subscribe happens when the manifest arrives.
    let staging = staging_dir.map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
    let dir = staging.join(format!(
        "{}-{}",
        sanitize_dir_name(&info.session_name),
        session_id.chars().take(8).collect::<String>()
    ));
    let sid = session_id.to_string();
    let name = info.session_name.clone();
    let dir_str = dir.to_string_lossy().into_owned();
    set_status(shared, |s| {
        s.state = "connecting".into();
        s.progress = 0.0;
        s.dir = Some(dir_str);
        s.session_id = Some(sid);
        s.session_name = Some(name);
        s.message = None;
        s.end_reason = None;
    });
    shared.reverts.store(0, Ordering::Relaxed);
    shared.reverts_detail.lock().unwrap().clear();
    shared.peer_locks.lock().unwrap().clear();
    *shared.peer_locks_at.lock().unwrap() = None;
    shared.force_revert.lock().unwrap().clear();
    // Per-session extras from any previous join on this handle. The roster
    // lists every fresh beacon, so the previous session's people go too.
    shared.session_meta.lock().unwrap().clear();
    shared.peers_info.lock().unwrap().clear();
    shared.marker_inbox.lock().unwrap().clear();
    // Drop any proposals left in flight from a previous session
    // (e.g. before an auto-rejoin): their stale timestamps would
    // otherwise time out and surface as phantom snap-backs the
    // moment we reconnect.
    shared.doc.lock().unwrap().pending.clear();
    let req = swarm
        .behaviour_mut()
        .xfer
        .send_request(&owner, XferRequest::Manifest { auth: token.clone() });
    Some(Download {
        owner,
        session_id: session_id.to_string(),
        dir,
        project_file: info.project_file.clone(),
        auth: token,
        got_manifest: false,
        manifest_req: Some(req),
        files: Vec::new(),
        queue: VecDeque::new(),
        inflight: HashMap::new(),
        window: DOWNLOAD_WINDOW_MIN,
        peers: vec![owner],
        next_peer: 0,
        total: 0,
        done: 0,
        last_activity: Instant::now(),
        started: Instant::now(),
        last_progress: Instant::now(),
        verifying: 0,
        media: Vec::new(),
        media_released: false,
        early_ready: false,
    })
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
    let mut hosting = cfg.hosting;

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
    // Everything in the announce except the host's profile and the project
    // sizes, both of which can change mid-session: that half is stamped on at
    // publish time below.
    let announce_base: Option<JsonValue> = hosting.as_ref().map(|h| {
        json!({
            "kind": "announce",
            "session_id": h.session_id,
            "session_name": h.session_name,
            "project_file": h.project_file,
            "owner": swarm.local_peer_id().to_string(),
            "auth": h.auth,
            "salt": h.salt,
            "proto": PROTO,
            "ver": VERSION,
        })
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
    // In-flight ticket join/check: dialing the ticket's addresses and waiting
    // for the pinned owner's announce to arrive over the direct connection.
    let mut pending_join: Option<PendingJoin> = None;

    // Anti-entropy (owner): whenever a peer (re)appears on the session topic,
    // or a publish fails because nobody was listening, we owe the network a
    // full-state update. yrs updates are idempotent, so over-sending is
    // harmless.
    let mut need_full_sync = false;
    let mut last_full_sync: Option<Instant> = None;
    // Session messages that failed to publish (nobody connected yet); retried
    // on the tick. Bounded so a long-dead session can't grow without limit.
    let mut retry_pubs: Vec<Vec<u8>> = Vec::new();
    // Peers currently blacklisted in gossipsub (both roles: mirrors
    // `shared.banned`), and banned peers whose connections are to be dropped
    // once their 'K' notice has had a moment to leave the socket.
    let mut blacklisted: HashSet<PeerId> = HashSet::new();
    let mut ban_disconnects: Vec<(PeerId, Instant)> = Vec::new();
    // Lock-table heartbeat (owner): the lock broadcast is normally edge-driven
    // (only on change), so a single dropped "lock released" packet would strand
    // a stale lock on a client, and its grey item tint with it. To self-heal,
    // re-broadcast every tick while any lock is live, and for a short tail after
    // the set empties (covers a dropped clear). Counts down to 0 when idle, so a
    // session with nobody editing stays silent.
    let mut lock_resend: i32 = 0;

    // Results of off-loop blake3 verification of completed download files.
    let (verify_tx, mut verify_rx) =
        tokio::sync::mpsc::unbounded_channel::<(PathBuf, bool)>();

    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tick_count: u64 = 0;
    // Presence beacon timer. Runs at the *fast* cadence and decides per tick
    // whether a beacon is due: always while a peer follows us (their beacon
    // says `follow` == our id), else once per PRESENCE_INTERVAL. A single
    // publish path keeps both cadences on the same message.
    let mut beacon_tick = tokio::time::interval(PRESENCE_FAST_INTERVAL);
    beacon_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_beacon: Option<Instant> = None;
    // Client: last time the session owner was seen on the session topic (for
    // the owner-vanished fallback below).
    let mut last_owner_seen = Instant::now();

    loop {
        tokio::select! {
            Some((tmp, ok)) = verify_rx.recv() => {
                handle_verify_result(&mut swarm, &shared, &mut download, tmp, ok);
            }
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::PublishAuthoritative(update)) => {
                    let mut data = vec![TAG_AUTHORITATIVE];
                    data.extend_from_slice(&update);
                    if let Some(t) = &sess_topic {
                        if !publish_session(&mut swarm, &shared, t, &data) {
                            need_full_sync = true;
                        }
                    }
                }
                Some(Command::PublishProposal(payload)) => {
                    let mut data = vec![TAG_PROPOSAL];
                    data.extend_from_slice(&payload);
                    match sess_topic.clone() {
                        Some(t) => {
                            if !publish_session(&mut swarm, &shared, &t, &data)
                                && retry_pubs.len() < 256 {
                                retry_pubs.push(data);
                            }
                        }
                        None => eprintln!("[reamesh] dropped proposal: not in a session"),
                    }
                }
                Some(Command::ProbeAuth { session_id, passphrase }) => {
                    let info = shared.sessions.lock().unwrap().get(&session_id).cloned();
                    match info.and_then(|i| i.owner.parse::<PeerId>().ok().map(|o| (o, i.salt, i.proto))) {
                        Some((_, _, proto)) if proto != PROTO => {
                            *shared.probe_result.lock().unwrap() =
                                Some((session_id, Some(false), JsonValue::Null, "version"));
                        }
                        Some((owner, salt, _)) => {
                            probe = Some(send_probe(&mut swarm, &owner, &salt, session_id, &passphrase));
                        }
                        None => {
                            // Session gone or unparseable owner -> can't verify.
                            *shared.probe_result.lock().unwrap() =
                                Some((session_id, Some(false), JsonValue::Null, "gone"));
                        }
                    }
                }
                Some(Command::JoinSession { session_id, passphrase }) => {
                    pending_join = None;
                    if let Some(dl) = start_join(
                        &mut swarm,
                        &shared,
                        cfg.staging_dir.as_deref(),
                        &session_id,
                        &passphrase,
                    ) {
                        download = Some(dl);
                    }
                }
                Some(Command::JoinTicket { info, passphrase, probe_only }) => {
                    eprintln!(
                        "[reamesh] ticket {}: dialing {} at {} address(es)",
                        if probe_only { "check" } else { "join" },
                        info.peer,
                        info.addrs.len()
                    );
                    dial_ticket(&mut swarm, &info, &local_ips);
                    if !probe_only {
                        // Show "connecting" (with the ticket's own name) while
                        // we wait for the host's announce to arrive over the
                        // fresh connection.
                        let (sid, name) = (info.session_id.clone(), info.session_name.clone());
                        set_status(&shared, |s| {
                            s.state = "connecting".into();
                            s.progress = 0.0;
                            s.session_id = Some(sid);
                            s.session_name = Some(name);
                            s.message = None;
                            s.end_reason = None;
                        });
                    }
                    pending_join = Some(PendingJoin {
                        info,
                        passphrase,
                        probe_only,
                        started: Instant::now(),
                        last_dial: Instant::now(),
                        connected: false,
                    });
                }
                Some(Command::FetchFile(rel)) => {
                    // A file the running (progressive) download will deliver
                    // anyway is not fetched a second time on the side.
                    let in_download = download
                        .as_ref()
                        .is_some_and(|dl| dl.files.iter().any(|f| f.meta.path == rel && !f.complete));
                    if !in_download {
                        start_fetch(&mut swarm, &shared, &mut fetches, &mut fetch_reqs, rel);
                    }
                }
                Some(Command::ManifestReady(files)) => {
                    if let Some(h) = hosting.as_mut() {
                        eprintln!("[reamesh] manifest updated ({} files)", files.len());
                        h.ever_listed.extend(files.iter().map(|f| f.path.clone()));
                        h.files = files;
                    }
                }
                Some(Command::SetMeta(meta)) => {
                    if let Some(h) = hosting.as_mut() {
                        h.meta = meta;
                    }
                }
                Some(Command::ShareMarker { name, pos }) => {
                    let (by, color) = PROFILE.lock().unwrap().clone();
                    let mac = presence_mac(
                        &shared.auth_token.lock().unwrap().clone(),
                        &swarm.local_peer_id().to_string(),
                    );
                    let mut data = vec![TAG_MARKER];
                    data.extend_from_slice(
                        json!({ "name": name, "pos": pos, "by": by, "color": color, "mac": mac })
                            .to_string()
                            .as_bytes(),
                    );
                    match sess_topic.clone() {
                        Some(t) => {
                            // Retry like a proposal if the mesh isn't up yet:
                            // a marker shared right after joining must not
                            // vanish into a not-yet-grafted mesh.
                            if !publish_session(&mut swarm, &shared, &t, &data)
                                && retry_pubs.len() < 256
                            {
                                retry_pubs.push(data);
                            }
                        }
                        None => eprintln!("[reamesh] dropped shared marker: not in a session"),
                    }
                }
                Some(Command::Moderate { peer, ban }) => {
                    if hosting.is_none() {
                        eprintln!("[reamesh] moderation ignored: not hosting");
                    } else if let Some(t) = sess_topic.clone() {
                        let mut data = vec![TAG_KICK];
                        data.extend_from_slice(
                            json!({ "peer": peer, "ban": ban }).to_string().as_bytes(),
                        );
                        if !publish_session(&mut swarm, &shared, &t, &data) && retry_pubs.len() < 256 {
                            retry_pubs.push(data);
                        }
                        eprintln!("[reamesh] {} {peer}", if ban { "banned" } else { "kicked" });
                        if ban && let Ok(pid) = peer.parse::<PeerId>() {
                            // The tick mirrors the table into the blacklist
                            // at once; the connection itself goes a moment
                            // later so the notice above isn't cut off.
                            ban_disconnects.push((pid, Instant::now() + Duration::from_millis(1500)));
                        }
                    }
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
                    if let Some(t) = sess_topic.clone() {
                        if hosting.is_some() {
                            let _ = publish_session(&mut swarm, &shared, &t, &[TAG_SESSION_END]);
                        } else {
                            swarm.behaviour_mut().gossipsub.unsubscribe(&t);
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
            _ = beacon_tick.tick() => {
                if let Some(t) = sess_topic.clone() {
                    let me = swarm.local_peer_id().to_string();
                    let followed = shared
                        .peers_info
                        .lock()
                        .unwrap()
                        .values()
                        .any(|p| p.follow == me && p.last_seen.elapsed() < PRESENCE_STALE);
                    let due = followed
                        || last_beacon.is_none_or(|at| at.elapsed() >= PRESENCE_INTERVAL);
                    if due {
                        last_beacon = Some(Instant::now());
                        publish_presence(&mut swarm, &shared, &t);
                    }
                }
            }
            _ = tick.tick() => {
                tick_count += 1;
                // End an unanswered passphrase check with a timeout result.
                if probe.as_ref().is_some_and(|p| p.started.elapsed() > PROBE_TIMEOUT) {
                    let p = probe.take().unwrap();
                    *shared.probe_result.lock().unwrap() =
                        Some((p.session_id, Some(false), JsonValue::Null, "timeout"));
                }
                // Ticket join/check in flight: once the *pinned* owner's
                // announce lands in the session table, proceed exactly like a
                // discovered session; keep re-dialing until then; give up
                // after TICKET_JOIN_TIMEOUT. An announce for the same session
                // id signed by anyone else is ignored: fail closed rather
                // than join an impostor.
                if let Some(pj) = &mut pending_join {
                    // The salt from the pinned owner's own announce, once it
                    // has landed; an announce signed by anyone else never
                    // matches, so it can't be a stand-in.
                    let pinned_salt = shared
                        .sessions
                        .lock()
                        .unwrap()
                        .get(&pj.info.session_id)
                        .filter(|i| i.owner == pj.info.peer.to_string())
                        .map(|i| (i.salt.clone(), i.proto));
                    if let Some((salt, proto)) = pinned_salt {
                        let pj = pending_join.take().unwrap();
                        if pj.probe_only && proto != PROTO {
                            *shared.probe_result.lock().unwrap() =
                                Some((pj.info.session_id, Some(false), JsonValue::Null, "version"));
                        } else if pj.probe_only {
                            probe = Some(send_probe(
                                &mut swarm,
                                &pj.info.peer,
                                &salt,
                                pj.info.session_id,
                                &pj.passphrase,
                            ));
                        } else if let Some(dl) = start_join(
                            &mut swarm,
                            &shared,
                            cfg.staging_dir.as_deref(),
                            &pj.info.session_id,
                            &pj.passphrase,
                        ) {
                            download = Some(dl);
                        }
                    } else if (!pj.connected && pj.started.elapsed() > TICKET_CONNECT_TIMEOUT)
                        || pj.started.elapsed() > TICKET_JOIN_TIMEOUT
                    {
                        // Two ways to give up: never reached the host at all
                        // (fast-fail: it's dead/on another network), or reached
                        // it but it never announced within the full window.
                        let pj = pending_join.take().unwrap();
                        eprintln!(
                            "[reamesh] ticket {} gave up ({}, connected={})",
                            if pj.probe_only { "check" } else { "join" },
                            pj.info.peer,
                            pj.connected
                        );
                        if pj.probe_only {
                            // Unreachable reports false, like an unanswered
                            // probe: a definite answer beats a hang.
                            *shared.probe_result.lock().unwrap() =
                                Some((pj.info.session_id, Some(false), JsonValue::Null, "unreachable"));
                        } else {
                            set_status(&shared, |s| {
                                s.state = "failed".into();
                                // Distinct message so the extension can localize
                                // "the invited host isn't reachable" cleanly.
                                s.message = Some("ticket host unreachable".into());
                            });
                        }
                    } else if pj.last_dial.elapsed() >= Duration::from_secs(3) {
                        pj.last_dial = Instant::now();
                        dial_ticket(&mut swarm, &pj.info, &local_ips);
                    }
                }
                // Recompute session-topic peers from gossipsub's view. (Event
                // counting misses subscriptions learned before we joined the
                // topic ourselves.)
                if let Some(t) = &sess_topic {
                    let hash = t.hash();
                    // A banned peer that keeps its subscription is neither a
                    // fetch candidate nor a counted member.
                    let peers: Vec<PeerId> = swarm.behaviour().gossipsub.all_peers()
                        .filter(|(_, topics)| topics.iter().any(|th| **th == hash))
                        .map(|(p, _)| *p)
                        .filter(|p| !blacklisted.contains(p))
                        .collect();
                    // A direct member that just left the topic (unsubscribed,
                    // disconnected) leaves the roster now, not after its last
                    // beacon goes stale. If it is still in the session behind
                    // the host, its next relayed beacon brings it back.
                    let prev = std::mem::replace(&mut *shared.session_peers.lock().unwrap(), peers.clone());
                    let gone: Vec<String> = prev
                        .iter()
                        .filter(|p| !peers.contains(p))
                        .map(|p| p.to_string())
                        .collect();
                    if !gone.is_empty() {
                        let mut infos = shared.peers_info.lock().unwrap();
                        for id in &gone {
                            infos.remove(id);
                        }
                    }
                }
                // Owner: mirror the process-global ban table (a new ban, a
                // revoke, an expiry) into the shared set, the gossipsub
                // blacklist, and a 'B' broadcast whenever it changes.
                if hosting.is_some() {
                    let live = banned_ids();
                    let changed = {
                        let mut cur = shared.banned.lock().unwrap();
                        if *cur != live {
                            *cur = live.clone();
                            true
                        } else {
                            false
                        }
                    };
                    if changed {
                        sync_blacklist(&mut swarm, &mut blacklisted, &live);
                        let mut infos = shared.peers_info.lock().unwrap();
                        infos.retain(|id, _| !live.contains(id));
                        drop(infos);
                        if let Some(t) = sess_topic.clone() {
                            let data = ban_table_msg(&live);
                            if !publish_session(&mut swarm, &shared, &t, &data) && retry_pubs.len() < 256 {
                                retry_pubs.push(data);
                            }
                        }
                    }
                }
                // Drop a banned peer's connections once its notice is out:
                // gossipsub ignores a blacklisted peer from the next connect
                // on, so this is what takes it out of the mesh for good.
                if !ban_disconnects.is_empty() {
                    let now = Instant::now();
                    let (due, later): (Vec<_>, Vec<_>) =
                        ban_disconnects.drain(..).partition(|(_, at)| *at <= now);
                    ban_disconnects = later;
                    for (pid, _) in due {
                        let _ = swarm.disconnect_peer_id(pid);
                    }
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
                // Owner: announce the session on the lobby every ~1s. The
                // profile half is stamped on per publish so a name/color change
                // mid-session reaches the lobby without a re-host.
                if let (Some(base), true) = (&announce_base, tick_count % 2 == 0) {
                    let (name, color) = PROFILE.lock().unwrap().clone();
                    let mut v = base.clone();
                    v["owner_name"] = json!(name);
                    v["owner_color"] = json!(color);
                    // Sizes ride along per publish: the manifest follows the
                    // project on disk (see manifest_worker), so a precomputed
                    // pair would drift the moment the host saves or records.
                    if let Some(h) = &hosting {
                        v["files"] = json!(h.files.len() as u64);
                        v["bytes"] = json!(h.files.iter().map(|f| f.size).sum::<u64>());
                    }
                    let _ = swarm
                        .behaviour_mut()
                        .gossipsub
                        .publish(lobby.clone(), v.to_string().into_bytes());
                }
                // (The presence beacon has its own timer below: it needs a
                // finer cadence than this tick while someone follows us.)
                // Owner: republish the editing-lock table whenever the live set
                // changes (an acquisition or a lapse), so peers can snap a
                // rejected edit back at once and name who holds each field.
                // `peer_locks` doubles as the owner's last-broadcast cache (the
                // owner never receives a lock table: only clients do).
                if hosting.is_some() && let Some(t) = sess_topic.clone() {
                    let live = {
                        let mut ds = shared.doc.lock().unwrap();
                        live_locks(&mut ds.locks, Instant::now())
                    };
                    let current: HashMap<FieldKey, String> = live.iter().cloned().collect();
                    let current_empty = current.is_empty();
                    let changed = {
                        let mut last = shared.peer_locks.lock().unwrap();
                        if *last != current {
                            *last = current;
                            true
                        } else {
                            false
                        }
                    };
                    // Heartbeat: keep resending while locks are live (refreshes
                    // clients every tick so a dropped packet self-heals), and for
                    // a few ticks after the set empties (so a dropped "released"
                    // can't leave a tint stuck). Idle (no locks) goes quiet.
                    if !current_empty {
                        lock_resend = 4;
                    } else {
                        lock_resend = lock_resend.saturating_sub(1);
                    }
                    if changed || lock_resend > 0 {
                        let mut data = vec![TAG_LOCKS];
                        data.extend_from_slice(&lock_table_json(&live));
                        let _ = publish_session(&mut swarm, &shared, &t, &data);
                    }
                }
                // Owner: full-state anti-entropy.
                if need_full_sync
                    && last_full_sync.is_none_or(|t| t.elapsed() >= FULL_SYNC_MIN_GAP)
                    && !shared.session_peers.lock().unwrap().is_empty()
                {
                    if let Some(t) = sess_topic.clone() {
                        let mut data = vec![TAG_AUTHORITATIVE];
                        {
                            let ds = shared.doc.lock().unwrap();
                            data.extend_from_slice(
                                &ds.doc.transact().encode_state_as_update_v1(&StateVector::default()),
                            );
                        }
                        if publish_session(&mut swarm, &shared, &t, &data) {
                            need_full_sync = false;
                            last_full_sync = Some(Instant::now());
                        }
                    }
                }
                // Retry queued session publishes (proposals from before the
                // mesh was up).
                if !retry_pubs.is_empty() {
                    if let Some(t) = sess_topic.clone() {
                        let queued = std::mem::take(&mut retry_pubs);
                        for data in queued {
                            if !publish_session(&mut swarm, &shared, &t, &data) {
                                retry_pubs.push(data);
                            }
                        }
                    }
                }
                // Nudge stalled mid-session file fetches.
                let stalled: Vec<String> = fetches
                    .values()
                    // Generous: a 4 MiB chunk over a VPN can legitimately take
                    // seconds, and a retry that overtakes it is only wasted
                    // bandwidth now (responses are keyed to the live request).
                    .filter(|f| f.last_activity.elapsed() > Duration::from_secs(10))
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
                    // A reachable ticket host: stop the fast-fail clock and let
                    // it have the full window to send its announce.
                    if let Some(pj) = &mut pending_join
                        && peer_id == pj.info.peer {
                        pj.connected = true;
                    }
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    eprintln!("[reamesh] outgoing connection error (peer {peer_id:?}): {error}");
                }
                SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                    // Last connection to this peer gone: its round trip is no
                    // longer a fact about anything.
                    shared.peer_rtt.lock().unwrap().remove(&peer_id.to_string());
                }
                SwarmEvent::Behaviour(BehaviourEvent::Ping(ping::Event { peer, result, .. })) => {
                    let sample_ms = match result {
                        Ok(rtt) => Some(rtt.as_secs_f64() * 1000.0),
                        // A timed-out ping is a pessimistic sample, not a
                        // vanished peer (liveness is the topic's business).
                        Err(ping::Failure::Timeout) => Some(PING_TIMEOUT.as_secs_f64() * 1000.0),
                        // Unsupported / stream errors: nothing to learn.
                        Err(_) => None,
                    };
                    if let Some(ms) = sample_ms {
                        let mut rtts = shared.peer_rtt.lock().unwrap();
                        let e = rtts.entry(peer.to_string()).or_insert((ms, Instant::now()));
                        // EWMA so a single slow sample doesn't flash a notice and a
                        // single fast one doesn't clear a genuinely slow link.
                        e.0 = e.0 * 0.6 + ms * 0.4;
                        e.1 = Instant::now();
                    }
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
                            // A (re)joining peer starts its proposal sequence
                            // afresh: its own clock anchors it, and a clock
                            // that went backwards must not lock it out.
                            shared.proposal_seqs.lock().unwrap().remove(&peer_id.to_string());
                            // ...and the current ban table, so it refuses the
                            // same peers everyone else does.
                            let live = shared.banned.lock().unwrap().clone();
                            if !live.is_empty() && retry_pubs.len() < 256 {
                                retry_pubs.push(ban_table_msg(&live));
                            }
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
                            let mut info = SessionInfo {
                                session_id: get("session_id"),
                                session_name: get("session_name"),
                                project_file: get("project_file"),
                                owner,
                                owner_name: get("owner_name"),
                                owner_color: get("owner_color"),
                                auth: v.get("auth").and_then(|x| x.as_bool()).unwrap_or(false),
                                salt: get("salt"),
                                files: v.get("files").and_then(|x| x.as_u64()).unwrap_or(0),
                                bytes: v.get("bytes").and_then(|x| x.as_u64()).unwrap_or(0),
                                last_seen: Instant::now(),
                                contested: false,
                                proto: v.get("proto").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                                version: get("ver"),
                            };
                            if signer_is_owner && valid_session_id(&info.session_id) {
                                let mut sessions = shared.sessions.lock().unwrap();
                                match sessions.get_mut(&info.session_id) {
                                    // Pin the first-seen owner. Binding the
                                    // announce to its signer (above) stops a
                                    // peer naming *someone else* as owner, but
                                    // not from naming *itself* under a session
                                    // id it overheard: the ids are public and
                                    // last-write-wins would hand the card, and
                                    // with it the manifest request and the
                                    // user's trust, to whoever announces last.
                                    // A re-host always mints a fresh id, so a
                                    // second owner on a live id is never
                                    // legitimate: keep the incumbent, flag the
                                    // clash, and let the lobby say so.
                                    Some(existing) if existing.owner != info.owner => {
                                        existing.contested = true;
                                        existing.last_seen = Instant::now();
                                        eprintln!(
                                            "[reamesh] session {} announced by a second peer ({}); keeping the first-seen owner {}",
                                            info.session_id, info.owner, existing.owner
                                        );
                                    }
                                    Some(existing) => {
                                        // Same owner: refresh, but a clash once
                                        // seen stays sticky for the entry's life
                                        // so the warning can't be raced away.
                                        info.contested = existing.contested;
                                        *existing = info;
                                    }
                                    None => {
                                        sessions.insert(info.session_id.clone(), info);
                                    }
                                }
                            }
                        }
                    } else if sess_topic.as_ref().is_some_and(|st| message.topic == st.hash()) {
                        // All session gossip is signed (MessageAuthenticity::
                        // Signed), so every legitimate message carries its
                        // authenticated author; one without a source can't be
                        // checked against anything below: drop it.
                        let Some(src) = message.source else { continue };
                        // A banned peer's gossip is nothing to anyone (the
                        // gossipsub blacklist drops most of it before it gets
                        // here; this covers the tick between ban and mirror).
                        if shared.banned.lock().unwrap().contains(&src.to_string()) {
                            continue;
                        }
                        // Confidentiality gate: in a passphrase-protected
                        // session every message must arrive sealed (and open
                        // against the sender's peer id). Drop plaintext from a
                        // peer without the passphrase and sealed data sent to
                        // an open session before dispatch.
                        let plain: Vec<u8> = match (
                            gossip_key(&shared.auth_token.lock().unwrap()),
                            message.data.first(),
                        ) {
                            (Some(key), Some(&TAG_SEALED)) => {
                                match open_session_msg(&key, &message.data, &src.to_string()) {
                                    Some(pt) => pt,
                                    None => {
                                        eprintln!(
                                            "[reamesh] dropped sealed session message from {src}: did not verify"
                                        );
                                        continue;
                                    }
                                }
                            }
                            (Some(_), _) => {
                                eprintln!(
                                    "[reamesh] dropped unsealed session message from {src} (protected session)"
                                );
                                continue;
                            }
                            (None, Some(&TAG_SEALED)) => continue, // sealed, but we have no key
                            (None, _) => message.data.clone(),
                        };
                        match (plain.first(), cfg.role) {
                            (Some(&TAG_PRESENCE), _) => {
                                if let Ok(v) = serde_json::from_slice::<JsonValue>(&plain[1..]) {
                                    let get = |k: &str| {
                                        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
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
                                    let mut infos = shared.peers_info.lock().unwrap();
                                    let beacons =
                                        infos.get(&src.to_string()).map(|p| p.beacons).unwrap_or(0) + 1;
                                    infos.insert(
                                        src.to_string(),
                                        PeerPresence {
                                            name: get("name"),
                                            color: get("color"),
                                            focus: get("focus"),
                                            play: v.get("play").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                            v0: v.get("v0").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                            v1: v.get("v1").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                            playing: v.get("playing").and_then(|x| x.as_bool()).unwrap_or(false),
                                            follow: get("follow"),
                                            sel: sanitize_sel(v.get("sel")),
                                            fx: sanitize_fx(v.get("fx")),
                                            frozen: v.get("frozen").and_then(|x| x.as_bool()).unwrap_or(false),
                                            last_seen: Instant::now(),
                                            beacons,
                                        },
                                    );
                                }
                            }
                            (Some(&TAG_MARKER), _) => {
                                // A shared flag marker. Authenticated like a
                                // presence beacon: only token holders may drop
                                // markers into anyone's project.
                                if let Ok(v) = serde_json::from_slice::<JsonValue>(&plain[1..]) {
                                    let get = |k: &str| {
                                        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
                                    };
                                    let expected = presence_mac(
                                        &shared.auth_token.lock().unwrap().clone(),
                                        &src.to_string(),
                                    );
                                    if !ct_eq(get("mac").as_bytes(), expected.as_bytes()) {
                                        eprintln!(
                                            "[reamesh] dropped unauthenticated shared marker from {src}"
                                        );
                                        continue;
                                    }
                                    let mut inbox = shared.marker_inbox.lock().unwrap();
                                    if inbox.len() < 64 {
                                        inbox.push(json!({
                                            "name": get("name").chars().take(256).collect::<String>(),
                                            "pos": v.get("pos").and_then(|x| x.as_f64()).unwrap_or(0.0),
                                            "by": get("by").chars().take(64).collect::<String>(),
                                            "color": get("color").chars().take(16).collect::<String>(),
                                        }));
                                    }
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
                                let from_owner =
                                    src.to_string() == *shared.owner_peer.lock().unwrap();
                                if !from_owner {
                                    eprintln!(
                                        "[reamesh] dropped authoritative update from non-owner {src}"
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
                                    && queued_bytes + plain.len() <= MAX_PENDING_BYTES
                                {
                                    q.push(plain[1..].to_vec());
                                    shared.dirty.store(true, Ordering::SeqCst);
                                }
                            }
                            (Some(&TAG_SESSION_END), Role::Client) => {
                                // Only the owner may end the session (any topic
                                // member can publish; signed messages carry the
                                // authenticated source).
                                let from_owner =
                                    src.to_string() == *shared.owner_peer.lock().unwrap();
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
                                let proposer = src.to_string();
                                if let Some(update) =
                                    admit_proposal(&shared, &plain[1..], &proposer)
                                {
                                    let mut data = vec![TAG_AUTHORITATIVE];
                                    data.extend_from_slice(&update);
                                    if let Some(t) = sess_topic.clone()
                                        && !publish_session(&mut swarm, &shared, &t, &data) {
                                        need_full_sync = true;
                                    }
                                }
                            }
                            (Some(&TAG_KICK), Role::Client) => {
                                // Only the owner moderates.
                                let from_owner =
                                    src.to_string() == *shared.owner_peer.lock().unwrap();
                                if from_owner
                                    && let Ok(v) = serde_json::from_slice::<JsonValue>(&plain[1..])
                                {
                                    let target = v.get("peer").and_then(|x| x.as_str()).unwrap_or("");
                                    let ban = v.get("ban").and_then(|x| x.as_bool()).unwrap_or(false);
                                    if target == swarm.local_peer_id().to_string() {
                                        eprintln!(
                                            "[reamesh] the host {} us",
                                            if ban { "banned" } else { "removed" }
                                        );
                                        download_cleanup(&mut download);
                                        // Leave the topic now (no more beacons,
                                        // off every roster at once) rather than
                                        // waiting for the extension's shutdown;
                                        // a later rejoin re-subscribes when its
                                        // manifest is accepted.
                                        if let Some(t) = sess_topic.take() {
                                            let _ = swarm.behaviour_mut().gossipsub.unsubscribe(&t);
                                        }
                                        set_status(&shared, |s| {
                                            s.state = "ended".into();
                                            s.message = Some(
                                                if ban { BANNED_MESSAGE } else { "removed from the session by the host" }
                                                    .into(),
                                            );
                                            s.end_reason = Some(if ban { "banned" } else { "kicked" }.into());
                                        });
                                    } else if !target.is_empty() {
                                        // Someone else: off our roster now,
                                        // not after the presence timeout.
                                        shared.peers_info.lock().unwrap().remove(target);
                                    }
                                }
                            }
                            (Some(&TAG_BANS), Role::Client) => {
                                let from_owner =
                                    src.to_string() == *shared.owner_peer.lock().unwrap();
                                if from_owner
                                    && let Ok(v) = serde_json::from_slice::<JsonValue>(&plain[1..])
                                {
                                    let live: HashSet<String> = v
                                        .get("bans")
                                        .and_then(|b| b.as_array())
                                        .map(|a| {
                                            a.iter()
                                                .filter_map(|x| x.as_str())
                                                .map(|s| s.to_string())
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    // Never let a table ban *us* out of the
                                    // roster maths: our own end comes via 'K'.
                                    let me = swarm.local_peer_id().to_string();
                                    let live: HashSet<String> =
                                        live.into_iter().filter(|id| *id != me).collect();
                                    let changed = {
                                        let mut cur = shared.banned.lock().unwrap();
                                        if *cur != live {
                                            *cur = live.clone();
                                            true
                                        } else {
                                            false
                                        }
                                    };
                                    if changed {
                                        sync_blacklist(&mut swarm, &mut blacklisted, &live);
                                        shared.peers_info.lock().unwrap().retain(|id, _| !live.contains(id));
                                    }
                                }
                            }
                            (Some(&TAG_LOCKS), Role::Client) => {
                                // The owner is the sole lock arbiter; ignore any
                                // table from a non-owner topic member.
                                let from_owner =
                                    src.to_string() == *shared.owner_peer.lock().unwrap();
                                if from_owner
                                    && let Ok(v) =
                                        serde_json::from_slice::<JsonValue>(&plain[1..])
                                {
                                    *shared.peer_locks.lock().unwrap() = parse_lock_table(&v);
                                    // Stamp the arrival: reconciliation only
                                    // trusts a table newer than the proposal
                                    // it is judging (see `peer_locks_at`).
                                    *shared.peer_locks_at.lock().unwrap() = Some(Instant::now());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Xfer(request_response::Event::Message { peer, message, .. })) => {
                    match message {
                        request_response::Message::Request { request, channel, .. } => {
                            let serve_dir = shared.serve_dir.lock().unwrap().clone();
                            let token = shared.auth_token.lock().unwrap().clone();
                            // The owner judges by the live table itself: its
                            // `shared.banned` mirror refreshes on the tick, and
                            // a revoke followed at once by a rejoin must not
                            // be refused by a half-second-stale copy.
                            let banned = if hosting.is_some() {
                                banned_ids().contains(&peer.to_string())
                            } else {
                                shared.banned.lock().unwrap().contains(&peer.to_string())
                            };
                            // The owner serves only what the session uses: files a
                            // manifest listed, or media the live doc references
                            // (added since the last manifest refresh). A client's
                            // copy holds nothing but session files anyway.
                            let may_serve = |p: &str| match hosting.as_ref() {
                                Some(h) => {
                                    h.ever_listed.contains(p)
                                        || doc_references(&shared.doc.lock().unwrap().doc, p)
                                }
                                None => true,
                            };
                            let resp = serve_xfer(hosting.as_ref(), serve_dir.as_deref(), &token, banned, &request, &may_serve);
                            // Remember an admitted path so its further pieces skip the doc scan.
                            if let (Some(h), XferRequest::File { path, .. }, XferResponse::FileChunk { .. }) =
                                (hosting.as_mut(), &request, &resp)
                            {
                                h.ever_listed.insert(path.clone());
                            }
                            let _ = swarm.behaviour_mut().xfer.send_response(channel, resp);
                        }
                        request_response::Message::Response { request_id, response } => {
                            if probe.as_ref().is_some_and(|p| p.req == request_id) {
                                // A successful pre-check supplies the join preview without downloading
                                // files or subscribing. Errors distinguish a ban from a rejected token.
                                let p = probe.take().unwrap();
                                let (ok, preview) = match &response {
                                    XferResponse::Manifest { files, project_file, meta, .. } => {
                                        let parsed_meta = serde_json::from_str::<JsonValue>(meta)
                                            .unwrap_or(JsonValue::Null);
                                        (
                                            true,
                                            json!({
                                                "files": files.len() as u64,
                                                "bytes": files.iter().map(|f| f.size).sum::<u64>(),
                                                "project_file": project_file,
                                                "meta": parsed_meta,
                                            }),
                                        )
                                    }
                                    _ => (false, JsonValue::Null),
                                };
                                let reason = match &response {
                                    _ if ok => "",
                                    XferResponse::Error { message } if message == BANNED_MESSAGE => "banned",
                                    _ => "rejected",
                                };
                                *shared.probe_result.lock().unwrap() =
                                    Some((p.session_id, Some(ok), preview, reason));
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
                                handle_download_piece(&mut swarm, &shared, &mut download, &verify_tx, request_id, Some(response));
                            } else if let Some(rel) = fetch_reqs.remove(&request_id) {
                                handle_fetch_response(&mut swarm, &mut fetches, &mut fetch_reqs, &rel, request_id, Some(response));
                            }
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Xfer(request_response::Event::OutboundFailure { peer, error, request_id, .. })) => {
                    eprintln!("[reamesh] transfer request to {peer} failed: {error}");
                    if probe.as_ref().is_some_and(|p| p.req == request_id) {
                        // Report an unreachable host separately from a rejected passphrase.
                        let p = probe.take().unwrap();
                        *shared.probe_result.lock().unwrap() =
                            Some((p.session_id, Some(false), JsonValue::Null, "unreachable"));
                    } else if download.as_ref().is_some_and(|dl| dl.manifest_req == Some(request_id)) {
                        // Manifest never arrived; re-request on the next tick.
                        if let Some(dl) = &mut download {
                            dl.manifest_req = None;
                            dl.last_activity = Instant::now() - Duration::from_secs(10);
                        }
                    } else if download.as_ref().is_some_and(|dl| dl.inflight.contains_key(&request_id)) {
                        handle_download_piece(&mut swarm, &shared, &mut download, &verify_tx, request_id, None);
                    } else if let Some(rel) = fetch_reqs.remove(&request_id) {
                        handle_fetch_response(&mut swarm, &mut fetches, &mut fetch_reqs, &rel, request_id, None);
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
    // A partial index (budget exhausted on a huge staging root) is still a
    // valid index: it just reuses less.
    walk_bounded(root, |entry, meta| {
        if meta.is_file() && !entry.file_name().to_string_lossy().starts_with(".reamesh-part-") {
            idx.entry(meta.len()).or_default().push(entry.path());
        }
    });
    idx
}

/// Files of this download already fully on disk (reused or completed).
fn files_done(dl: &Download) -> u64 {
    dl.files.iter().filter(|f| f.remaining == 0).count() as u64
}

/// Whether `path` already holds exactly the file `meta` describes.
///
/// A size match alone proves nothing: two takes of the same length, or a stem
/// re-rendered in place, collide constantly, so an entry the host has not
/// hashed yet is always re-downloaded rather than assumed identical. (Hashes
/// are empty only while the owner's first hashing pass is still running, or for
/// a file that changed under the walk; the dedup path below already refuses to
/// reuse bytes on an empty hash for the same reason.)
fn file_matches(path: &Path, meta: &FileMeta) -> bool {
    if meta.hash.is_empty() {
        return false;
    }
    match fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() == meta.size => {
            blake3_file(path).ok().as_deref() == Some(meta.hash.as_str())
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
        let media = is_media_path(&meta.path);
        if reused {
            dl.done += meta.size;
            dl.files.push(FileDl { meta, tmp: dest.clone(), dest, file: None, remaining: 0, tries: 0, owner_only: false, complete: true, media });
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
            dl.files.push(FileDl { meta, tmp, dest, file: None, remaining: 0, tries: 0, owner_only: false, complete: true, media });
            continue;
        }
        // Media waits for the project file (release_media orders it by
        // timeline position); everything else goes first, so the project can
        // open as soon as the small stuff is here.
        for p in pieces {
            if media {
                dl.media.push(p);
            } else {
                dl.queue.push_back(p);
            }
        }
        dl.files.push(FileDl { meta, tmp, dest, file: Some(f), remaining, tries: 0, owner_only: false, complete: false, media });
    }
    Ok(())
}

/// Fill the in-flight window: pull pending pieces and dispatch each to the
/// next serving peer in the rotation.
fn download_pump(swarm: &mut Swarm<Behaviour>, dl: &mut Download) {
    while dl.inflight.len() < dl.window {
        let Some(piece) = dl.queue.pop_front() else { break };
        // A file that failed verification is re-fetched from the owner alone:
        // another client may still be serving a copy the host has since
        // rewritten in place.
        let peer = if dl.files[piece.file].owner_only {
            dl.owner
        } else {
            match dl.next_peer() {
                Some(p) => p,
                None => {
                    dl.queue.push_front(piece);
                    break;
                }
            }
        };
        let req = dl.request(piece);
        let id = swarm.behaviour_mut().xfer.send_request(&peer, req);
        dl.inflight.insert(id, (piece, Instant::now()));
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
        s.current_file = String::new();
    });
}

fn download_finalize(shared: &Shared, download: &mut Option<Download>) {
    let Some(dl) = download.as_ref() else { return };
    let path = dl.dir.join(&dl.project_file);
    eprintln!("[reamesh] download complete: {}", path.display());
    // This copy is now what we serve to (and fetch from) other peers.
    *shared.serve_dir.lock().unwrap() = Some(dl.dir.clone());
    let total = dl.total;
    let files = dl.files.len() as u64;
    set_status(shared, |s| {
        s.state = "ready".into();
        s.progress = 1.0;
        s.done_bytes = total;
        s.total_bytes = total;
        s.files_done = files;
        s.files_total = files;
        s.current_file = String::new();
        s.background = false;
        s.project_path = Some(path.to_string_lossy().into_owned());
    });
    *download = None;
}

/// Audio/video by extension. Only decides *ordering* within the download
/// (project text and small files first, media behind them in timeline order)
/// and what counts toward the progressive-join threshold; an unknown extension
/// is simply treated as "small stuff" and downloaded up front.
fn is_media_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".wav", ".aif", ".aiff", ".aifc", ".flac", ".mp3", ".ogg", ".opus", ".m4a", ".aac",
        ".wv", ".w64", ".rf64", ".caf", ".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v",
        ".mpg", ".mpeg", ".wmv", ".wma", ".mid", ".midi",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// Earliest timeline position each media file is used at, parsed from RPP
/// text: every `FILE "…"` line is attributed to the most recent `POSITION`
/// line (items serialize POSITION before their takes' sources). Paths come
/// back '/'-separated so they compare equal to manifest paths. Files not
/// referenced by any item (or referenced by absolute path) are absent.
fn media_order_from_rpp(text: &str) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    let mut position = f64::MAX;
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("POSITION ") {
            if let Ok(p) = rest.trim().parse::<f64>() {
                position = p;
            }
        } else if let Some(rest) = t.strip_prefix("FILE ") {
            let rest = rest.trim_start();
            let Some(q) = rest.chars().next().filter(|c| matches!(c, '"' | '\'' | '`')) else {
                continue;
            };
            let body = &rest[1..];
            let Some(end) = body.find(q) else { continue };
            let path = body[..end].replace('\\', "/");
            let path = path.strip_prefix("./").unwrap_or(&path).to_string();
            let e = out.entry(path).or_insert(f64::MAX);
            if position < *e {
                *e = position;
            }
        }
    }
    out
}

/// Queue the held-back media pieces once the project file is on disk, in the
/// order the timeline uses them (earliest first), so the arrangement fills in
/// from the left and the start of the song is playable soonest. Media a
/// manifest lists but no item references goes last.
fn release_media(dl: &mut Download) {
    if dl.media_released {
        return;
    }
    let project_idx = dl.files.iter().position(|f| f.meta.path == dl.project_file);
    if let Some(i) = project_idx {
        if !dl.files[i].complete {
            return; // wait for the project file
        }
    }
    dl.media_released = true;
    let order = fs::read_to_string(dl.dir.join(&dl.project_file))
        .map(|t| media_order_from_rpp(&t))
        .unwrap_or_default();
    let mut media = std::mem::take(&mut dl.media);
    let key = |p: &Piece| {
        let pos = order.get(&dl.files[p.file].meta.path).copied().unwrap_or(f64::MAX);
        (pos, p.file, p.offset)
    };
    media.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));
    dl.queue.extend(media);
}

/// Nothing left to request, receive or verify: the download can finalize.
fn download_idle(dl: &Download) -> bool {
    dl.media_released && dl.media.is_empty() && dl.queue.is_empty() && dl.inflight.is_empty()
        && dl.verifying == 0
}

/// Bookkeeping after a file became whole (or right after the plan was built):
/// release the media queue behind the project file, and report the
/// progressive-join `ready` once everything but a substantial amount of media
/// is in place. Idempotent.
fn download_progress_check(shared: &Shared, dl: &mut Download) {
    release_media(dl);
    if dl.early_ready || !dl.media_released {
        return;
    }
    let small_done = dl.files.iter().all(|f| f.media || f.complete);
    let media_left: u64 = dl.files.iter().filter(|f| f.media && !f.complete).map(|f| f.meta.size).sum();
    if !small_done || media_left < PROGRESSIVE_MIN_BYTES {
        return;
    }
    dl.early_ready = true;
    let path = dl.dir.join(&dl.project_file);
    eprintln!(
        "[reamesh] project ready, {} bytes of media still downloading: {}",
        media_left,
        path.display()
    );
    // Serve (and fetch into) this copy from now on; a missing file simply
    // isn't served yet, requesters fall through to the other peers.
    *shared.serve_dir.lock().unwrap() = Some(dl.dir.clone());
    set_status(shared, |s| {
        s.state = "ready".into();
        s.background = true;
        s.project_path = Some(path.to_string_lossy().into_owned());
    });
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
    // No blanket "nothing for 5s → requeue everything": on a link where the
    // whole window takes longer than that, it discarded every response as
    // stale and never progressed. A request that really dies comes back as an
    // OutboundFailure (timeout / connection closed) and is requeued there.
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
            XferResponse::Manifest { files, project_file, meta, .. } => {
                if dl.got_manifest {
                    return; // duplicate (a retry raced the response)
                }
                // Remember what the host *claimed* the project loads; the
                // extension checks the received .rpp against this before
                // Main_openProject (a dishonest list must not survive).
                *shared.session_meta.lock().unwrap() = meta;
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
                let (done, total) = (dl.done, dl.total);
                let (fdone, ftotal) = (files_done(dl), dl.files.len() as u64);
                set_status(shared, |s| {
                    s.state = "downloading".into();
                    s.progress = progress;
                    s.done_bytes = done;
                    s.total_bytes = total;
                    s.files_done = fdone;
                    s.files_total = ftotal;
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
    if let Some(dl) = download.as_mut() {
        if dl.got_manifest {
            download_progress_check(shared, dl);
            download_pump(swarm, dl);
            if download_idle(dl) {
                download_finalize(shared, download);
            }
        }
    }
}

/// Client-side: a chunk arrived (or a piece request failed: `response` None).
fn handle_download_piece(
    swarm: &mut Swarm<Behaviour>,
    shared: &Shared,
    download: &mut Option<Download>,
    verify_tx: &VerifyTx,
    request_id: request_response::OutboundRequestId,
    response: Option<XferResponse>,
) {
    let Some(dl) = download.as_mut() else { return };
    let Some((piece, sent)) = dl.inflight.remove(&request_id) else { return };
    dl.last_activity = Instant::now();
    // Adapt the window to the link: a piece that came back quickly means there
    // is headroom for more in flight; a failed one (timeout, peer gone) means
    // the window may have been too wide for the link.
    if response.is_some() {
        if sent.elapsed() < Duration::from_secs(3) && dl.window < DOWNLOAD_WINDOW {
            dl.window += 1;
        }
    } else {
        dl.window = (dl.window / 2).max(DOWNLOAD_WINDOW_MIN);
    }

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
    let current = fd.meta.path.clone();
    dl.done += chunk.len() as u64;
    dl.last_progress = Instant::now();
    let progress = if dl.total == 0 { 1.0 } else { dl.done as f64 / dl.total as f64 };
    let (done, total) = (dl.done, dl.total);
    let (fdone, ftotal) = (files_done(dl), dl.files.len() as u64);
    set_status(shared, |s| {
        s.progress = progress;
        s.done_bytes = done;
        s.total_bytes = total;
        s.files_done = fdone;
        s.files_total = ftotal;
        s.current_file = current;
    });

    if dl.files[piece.file].remaining == 0 {
        let fd = &mut dl.files[piece.file];
        fd.file = None; // close before verifying/renaming
        if fd.meta.hash.is_empty() {
            // No hash to check (host was still hashing when we joined).
            if let Err(e) = fs::rename(&fd.tmp, &fd.dest) {
                let dest = fd.dest.display().to_string();
                return download_fail(download, shared, format!("finalize {dest}: {e}"));
            }
            fd.complete = true;
            download_progress_check(shared, dl);
        } else {
            // Hash on a blocking task: a multi-GB file would otherwise stall
            // every in-flight response while the event loop grinds blake3.
            // The result comes back through verify_rx (handle_verify_result).
            dl.verifying += 1;
            let tmp = fd.tmp.clone();
            let hash = fd.meta.hash.clone();
            let tx = verify_tx.clone();
            tokio::task::spawn_blocking(move || {
                let ok = blake3_file(&tmp).ok().as_deref() == Some(hash.as_str());
                let _ = tx.send((tmp, ok));
            });
        }
    }

    if download_idle(dl) {
        return download_finalize(shared, download);
    }
    download_pump(swarm, dl);
}

/// Sender half of the download-verification channel: (temp path, hash ok).
type VerifyTx = tokio::sync::mpsc::UnboundedSender<(PathBuf, bool)>;

/// Handle a completed client-side blake3 check. Match by temporary path so a
/// result from an earlier download cannot finalize a file after auto-rejoin.
/// Stale failures may only requeue data that would be downloaded again anyway.
fn handle_verify_result(
    swarm: &mut Swarm<Behaviour>,
    shared: &Shared,
    download: &mut Option<Download>,
    tmp: PathBuf,
    ok: bool,
) {
    let Some(dl) = download.as_mut() else { return };
    if dl.verifying == 0 {
        return;
    }
    let Some(idx) = dl
        .files
        .iter()
        .position(|f| f.tmp == tmp && f.tmp != f.dest && f.remaining == 0 && f.file.is_none())
    else {
        return;
    };
    dl.verifying -= 1;
    let fd = &mut dl.files[idx];
    if ok {
        if let Err(e) = fs::rename(&fd.tmp, &fd.dest) {
            let dest = fd.dest.display().to_string();
            return download_fail(download, shared, format!("finalize {dest}: {e}"));
        }
        fd.complete = true;
        download_progress_check(shared, dl);
    } else {
        fd.tries += 1;
        if fd.tries > 3 {
            let p = fd.meta.path.clone();
            return download_fail(download, shared, format!("checksum mismatch for {p}"));
        }
        eprintln!("[reamesh] checksum mismatch for {}, re-downloading", fd.meta.path);
        fd.owner_only = true;
        match fs::OpenOptions::new().read(true).write(true).truncate(true).open(&fd.tmp) {
            Ok(f) => {
                let _ = f.set_len(fd.meta.size);
                fd.file = Some(f);
            }
            Err(e) => {
                let t = fd.tmp.display().to_string();
                return download_fail(download, shared, format!("reopen {t}: {e}"));
            }
        }
        let size = fd.meta.size;
        let pieces = file_pieces(idx, size);
        fd.remaining = pieces.len() as u64;
        dl.done = dl.done.saturating_sub(size);
        for p in pieces {
            dl.queue.push_back(p);
        }
        let progress = if dl.total == 0 { 1.0 } else { dl.done as f64 / dl.total as f64 };
        let (done, total) = (dl.done, dl.total);
        let (fdone, ftotal) = (files_done(dl), dl.files.len() as u64);
        set_status(shared, |s| {
            s.progress = progress;
            s.done_bytes = done;
            s.total_bytes = total;
            s.files_done = fdone;
            s.files_total = ftotal;
        });
    }

    if download_idle(dl) {
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
pub extern "C" fn reamesh_join(
    staging_dir: *const c_char,
    open_projects: *const c_char,
) -> *mut Reamesh {
    let Some(dir) = cstr_arg(staging_dir) else {
        return std::ptr::null_mut();
    };
    // Newline-separated project files open in this REAPER (may be null): the
    // staging sweep leaves any copy holding one of them alone.
    let keep: Vec<PathBuf> = cstr_arg(open_projects)
        .map(|s| s.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect())
        .unwrap_or_default();
    std::panic::catch_unwind(|| match Reamesh::join_keeping(&dir, &keep) {
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

/// Invite ticket for the session this owner handle is hosting: a copyable
/// `reamesh://` link a remote peer opens or pastes to join without LAN
/// discovery (works across
/// VPNs like Tailscale and on multicast-blocking networks). Empty until the
/// host's listen addresses are known, and always empty on client handles.
/// Valid only for this hosting run. Free the result with
/// `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_session_ticket(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        String::new()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.session_ticket()))
            .unwrap_or_default()
    };
    into_c_string(out)
}

/// Join a session through a pasted invite ticket (client handles only):
/// dials the ticket's addresses directly, then joins once the host's announce
/// arrives over that connection. `passphrase` as in `reamesh_join_session`.
/// Returns true if the attempt was started; watch `reamesh_status` for
/// connecting/downloading/ready/failed.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_join_ticket(
    handle: *mut Reamesh,
    ticket: *const c_char,
    passphrase: *const c_char,
) -> bool {
    let (false, Some(t)) = (handle.is_null(), cstr_arg(ticket)) else {
        return false;
    };
    let pass = cstr_arg(passphrase).unwrap_or_default();
    let r = unsafe { &*handle };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.join_ticket(&t, &pass)))
        .unwrap_or(false)
}

/// Pre-check a passphrase against a ticket's host without joining: the ticket
/// analogue of `reamesh_check_passphrase`, reporting through the same `probe`
/// status field. An unreachable host reports ok=false rather than hanging.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_check_ticket(
    handle: *mut Reamesh,
    ticket: *const c_char,
    passphrase: *const c_char,
) -> bool {
    let (false, Some(t)) = (handle.is_null(), cstr_arg(ticket)) else {
        return false;
    };
    let pass = cstr_arg(passphrase).unwrap_or_default();
    let r = unsafe { &*handle };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.check_ticket(&t, &pass)))
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

/// Live editing locks held by a peer *other than us*, as a JSON array of
/// `{"section","id","field","holder"}`: the entities someone else is actively
/// editing right now (pruned to `LOCK_HOLD`; our own held fields excluded). The
/// extension tints locked media items locally from this; the tint is a pure
/// view hint and is never captured, proposed, synced or written to the project.
/// Free the result with `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_locked_entities(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        "[]".to_string()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.locked_entities_json()))
            .unwrap_or_else(|_| "[]".to_string())
    };
    into_c_string(out)
}

/// Set the process-wide display profile (name + color hex) included in this
/// peer's presence beacons. Survives host/join/leave; usually set by the
/// app through the control server rather than called directly.
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

/// Set the local user's live presence from a JSON blob `{"focus","play"}`,
/// gossiped on the presence beacon so peers can show where each person is (their
/// focused track for the roster, and timeline position for click-to-follow).
/// Supersedes `reamesh_set_focus`; the extension calls it each tick. Missing
/// fields reset to empty/zero.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_set_presence(json: *const c_char) {
    set_presence(&cstr_arg(json).unwrap_or_default());
}

/// Register the file this install's network identity is persisted in (usually
/// `<REAPER resource>/reamesh-identity.key`). Once set, every session start
/// reuses the same keypair: the peer id becomes a lasting, per-install
/// identity (remembered hosts, tickets that survive a restart). Without this
/// call every session start generates a fresh ephemeral identity (the old
/// behavior, kept for tests). Call once at plugin load, before any session.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_identity_init(path: *const c_char) {
    let Some(p) = cstr_arg(path) else { return };
    set_identity_path(&p);
}

/// Set the session-meta blob (JSON: track count + plugin list) the hosted
/// session's manifest carries, so joiners can see what a join loads before
/// downloading. Owner handles only (a no-op elsewhere beyond local storage).
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_set_session_meta(handle: *mut Reamesh, meta_json: *const c_char) {
    let (false, Some(meta)) = (handle.is_null(), cstr_arg(meta_json)) else {
        return;
    };
    let r = unsafe { &*handle };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.set_session_meta(&meta)));
}

/// The current session's meta blob: what this owner set via
/// `reamesh_set_session_meta`, or (on a client) what the owner's manifest
/// claimed. "" when none is known. Free with `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_session_meta(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        String::new()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.session_meta_json()))
            .unwrap_or_default()
    };
    into_c_string(out)
}

/// Share a flag marker (`{"name","pos"}`) with every session peer. Receivers
/// drop it into their project as a local, non-synced marker; the caller
/// inserts its own copy itself. Returns whether the share was queued.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_share_marker(handle: *mut Reamesh, marker_json: *const c_char) -> bool {
    let (false, Some(m)) = (handle.is_null(), cstr_arg(marker_json)) else {
        return false;
    };
    let r = unsafe { &*handle };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.share_marker(&m))).unwrap_or(false)
}

/// Shared markers received from peers since the last call, as a JSON array of
/// `{"name","pos","by","color"}` ("[]" when none). Free with
/// `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_drain_markers(handle: *mut Reamesh) -> *mut c_char {
    let out = if handle.is_null() {
        "[]".to_string()
    } else {
        let r = unsafe { &*handle };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.drain_markers()))
            .unwrap_or_else(|_| "[]".to_string())
    };
    into_c_string(out)
}

/// Owner-side moderation: make `peer_id` leave the session (`ban` false: a
/// kick, it may rejoin at once) or keep it out for 30 minutes (`ban` true).
/// Returns whether the request was accepted (owner only, not for self).
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_moderate(handle: *mut Reamesh, peer_id: *const c_char, ban: bool) -> bool {
    let (false, Some(p)) = (handle.is_null(), cstr_arg(peer_id)) else {
        return false;
    };
    let r = unsafe { &*handle };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.moderate(&p, ban))).unwrap_or(false)
}

/// Lift a ban early. Returns whether the peer was banned. Independent of any
/// session handle: the ban table is process-global (it outlives a re-host).
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_unban(peer_id: *const c_char) -> bool {
    let Some(p) = cstr_arg(peer_id) else { return false };
    std::panic::catch_unwind(|| unban(&p)).unwrap_or(false)
}

/// The live ban table as a JSON array of `{"id","name","color","expires_at"}`
/// (`expires_at` = unix ms), soonest to lapse first; "[]" when none. Free
/// with `reamesh_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_bans() -> *mut c_char {
    into_c_string(std::panic::catch_unwind(bans_json).unwrap_or_else(|_| "[]".to_string()))
}

/// Start the localhost control server the ReaMesh app connects to
/// (idempotent). Returns the bound port, 0 if no port was available.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_ctl_start() -> u16 {
    std::panic::catch_unwind(ctl::start).unwrap_or(0)
}

/// Next command sent by a connected ReaMesh app, as a JSON string, or null
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

/// Broadcast the extension's composed state JSON to every connected ReaMesh
/// app (and replay it to apps that connect later).
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_ctl_publish(state_json: *const c_char) {
    let Some(s) = cstr_arg(state_json) else { return };
    let _ = std::panic::catch_unwind(|| ctl::publish(&s));
}

/// Mark the extension's main-thread timer alive. Call once per timer tick;
/// when the beats stop (a modal REAPER dialog, a long blocking call) the
/// control server tells connected apps the instance is busy.
#[unsafe(no_mangle)]
pub extern "C" fn reamesh_ctl_heartbeat() {
    let _ = std::panic::catch_unwind(ctl::heartbeat);
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

    fn metas(paths: &[&str]) -> Vec<FileMeta> {
        paths.iter().map(|p| FileMeta { path: p.to_string(), size: 1, hash: String::new() }).collect()
    }

    fn kept(files: Vec<FileMeta>) -> Vec<String> {
        files.into_iter().map(|f| f.path).collect()
    }

    fn b64(bytes: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in bytes.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn referenced_files_keeps_only_what_the_project_uses() {
        let dir = std::env::temp_dir().join(format!("reamesh-refs-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // A sampler's state names its sample in base64; a sub-project brings its own media.
        let sampler = b64(b"\x00\x01C:\\Users\\me\\Song\\Samples\\Hit.WAV\x00");
        let rpp = format!(
            "<REAPER_PROJECT\n<TRACK\n  <FXCHAIN\n    <VST \"VSTi: ReaSamplOmatic5000\" rs5k.vst 0\n      {sampler}\n      AAAAAA==\n    >\n  >\n  <ITEM\n    <SOURCE WAVE\n      FILE \"Audio\\kick.wav\"\n    >\n  >\n  <ITEM\n    <SOURCE RPP_PROJECT\n      FILE \"Sub/inner.rpp\"\n    >\n  >\n>\n>\n"
        );
        fs::write(dir.join("Song.rpp"), rpp).unwrap();
        fs::create_dir_all(dir.join("Sub")).unwrap();
        fs::write(dir.join("Sub/inner.rpp"), "<REAPER_PROJECT\n  FILE \"/abs/elsewhere/Sub/Audio/vox.wav\"\n>\n").unwrap();
        let files = metas(&[
            "Song.rpp",
            "Audio/kick.wav",
            "Old/kick.wav",
            "Samples/hit.wav",
            "Sub/inner.rpp",
            "Sub/Audio/vox.wav",
            "Unused/outtake.wav",
            "notes.txt",
        ]);
        let got = kept(referenced_files(files.clone(), &dir, "Song.rpp", ""));
        assert_eq!(
            got,
            ["Song.rpp", "Audio/kick.wav", "Samples/hit.wav", "Sub/inner.rpp", "Sub/Audio/vox.wav"],
            "same-named files match by path, plugin state and sub-projects count"
        );
        // The live doc references media the saved project doesn't know yet.
        let live = "<ITEM\n  <SOURCE WAVE\n    FILE \"Unused/outtake.wav\"\n  >\n>\n";
        assert!(kept(referenced_files(files, &dir, "Song.rpp", live)).contains(&"Unused/outtake.wav".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn doc_references_sees_live_item_media() {
        let mut ds = DocState {
            doc: Doc::new(),
            last_state: StateMap::new(),
            locks: HashMap::new(),
            pending: HashMap::new(),
        };
        apply_changeset_to_doc(
            &mut ds,
            &json!({ "items": {
                "{I1}": { "present": true, "chunk": "<ITEM\n  <SOURCE WAVE\n    FILE \"Audio/snare.wav\"\n  >\n>\n" },
                "{I2}": { "present": false, "chunk": "<ITEM\n  <SOURCE WAVE\n    FILE \"Audio/deleted.wav\"\n  >\n>\n" },
            }}),
        );
        let doc = &ds.doc;
        assert!(doc_references(doc, "Audio/snare.wav"));
        assert!(!doc_references(doc, "Audio/deleted.wav"), "a deleted item references nothing");
        assert!(!doc_references(doc, "Private/secret.txt"));
    }

    #[test]
    fn media_order_follows_item_positions() {
        let rpp = "<REAPER_PROJECT\n<TRACK\n  <ITEM\n    POSITION 12.5\n    <SOURCE WAVE\n      FILE \"Audio\\late.wav\"\n    >\n  >\n  <ITEM\n    POSITION 3\n    <SOURCE SECTION\n      <SOURCE WAVE\n        FILE 'Audio/early.wav'\n      >\n    >\n  >\n  <ITEM\n    POSITION 40\n    <SOURCE WAVE\n      FILE \"Audio/early.wav\"\n    >\n  >\n>\n>\n";
        let order = media_order_from_rpp(rpp);
        assert_eq!(order["Audio/early.wav"], 3.0); // earliest use wins
        assert_eq!(order["Audio/late.wav"], 12.5); // backslashes normalized
        assert!(!order.contains_key("Audio/unused.wav"));
    }

    #[test]
    fn media_blanking_is_refused_only_for_file_to_empty() {
        let wave = "<ITEM\n  POSITION 1\n  <SOURCE WAVE\n    FILE \"Audio/kick.wav\"\n  >\n>\n";
        let empty = "<ITEM\n  POSITION 1\n  <SOURCE EMPTY\n  >\n>\n";
        let midi = "<ITEM\n  POSITION 1\n  <SOURCE MIDI\n    E 0 90 3c 60\n  >\n>\n";
        let moved = "<ITEM\n  POSITION 2\n  <SOURCE WAVE\n    FILE \"Audio/kick.wav\"\n  >\n>\n";
        assert!(blanks_media_source(wave, empty));
        assert!(!blanks_media_source(wave, moved)); // ordinary edit
        assert!(!blanks_media_source(empty, wave)); // media arriving: the good direction
        assert!(!blanks_media_source(midi, empty)); // never had a file
        assert!(!blanks_media_source(empty, empty));

        let mut cs = json!({
            "items": {
                "a": {"chunk": empty, "track": "t"},
                "b": {"chunk": moved},
                "c": {"chunk": empty}
            },
            "tracks": {"t": {"name": "Drums"}}
        });
        let current = |id: &str| match id {
            "a" | "b" => Some(JsonValue::String(wave.to_string())),
            "c" => Some(JsonValue::String(midi.to_string())),
            _ => None,
        };
        reject_media_blanking(&mut cs, current, "test");
        assert_eq!(cs["items"]["a"], json!({"track": "t"})); // chunk gone, move kept
        assert_eq!(cs["items"]["b"]["chunk"], moved);
        assert_eq!(cs["items"]["c"]["chunk"], empty); // MIDI → empty is not media blanking
        assert_eq!(cs["tracks"]["t"]["name"], "Drums");

        // A changeset that only carried the blanking collapses to nothing.
        let mut lone = json!({"items": {"a": {"chunk": empty}}});
        reject_media_blanking(&mut lone, current, "test");
        assert!(lone.get("items").is_none());
    }

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
    fn presence_detail_is_bounded_and_typed() {
        // `sel`: only non-empty strings, GUID-sized, at most PRESENCE_SEL_MAX.
        let mut big: Vec<JsonValue> = (0..PRESENCE_SEL_MAX + 10)
            .map(|i| json!(format!("{{guid-{i}}}")))
            .collect();
        big.push(json!(7));
        big.push(json!(""));
        let sel = sanitize_sel(Some(&json!(big)));
        assert_eq!(sel.len(), PRESENCE_SEL_MAX);
        assert_eq!(sel[0], "{guid-0}");
        assert!(sanitize_sel(Some(&json!("not-an-array"))).is_empty());
        assert!(sanitize_sel(None).is_empty());

        // `fx`: a track FX keeps track/fx/float, a take FX adds item/take;
        // unknown keys are dropped, wrong types fall back, no track = Null.
        let track = sanitize_fx(Some(&json!({"track": "{t}", "fx": 3, "float": true, "junk": 1})));
        assert_eq!(track, json!({"track": "{t}", "fx": 3, "float": true}));
        let take = sanitize_fx(Some(&json!({"track": "{t}", "item": "{i}", "take": 1, "fx": "x"})));
        assert_eq!(take, json!({"track": "{t}", "item": "{i}", "take": 1, "fx": 0, "float": false}));
        assert_eq!(sanitize_fx(Some(&json!({"fx": 1}))), JsonValue::Null);
        assert_eq!(sanitize_fx(Some(&json!([1, 2]))), JsonValue::Null);
        assert_eq!(sanitize_fx(None), JsonValue::Null);
        // A negative index can't address anything: clamped to 0.
        assert_eq!(sanitize_fx(Some(&json!({"track": "{t}", "fx": -5})))["fx"], 0);
    }

    #[test]
    fn sealed_gossip_roundtrips_and_binds_key_and_sender() {
        let key = gossip_key("token-a").expect("protected session must derive a key");
        let msg = b"Ahello-world";
        let sealed = seal_session_msg(&key, msg, "12D3KooPeerOne").unwrap();
        assert_eq!(sealed[0], TAG_SEALED);
        assert!(sealed.len() > 1 + SEAL_NONCE_LEN + msg.len(), "must carry nonce + auth tag");
        // Round-trip against the same sender id.
        assert_eq!(open_session_msg(&key, &sealed, "12D3KooPeerOne").unwrap(), msg);
        // A replay under a different (transport-authenticated) peer id fails.
        assert!(open_session_msg(&key, &sealed, "12D3KooPeerTwo").is_none());
        // A different token (other passphrase / no passphrase) can't open it.
        let other = gossip_key("token-b").unwrap();
        assert!(open_session_msg(&other, &sealed, "12D3KooPeerOne").is_none());
        // Tampering is detected.
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open_session_msg(&key, &bad, "12D3KooPeerOne").is_none());
        // Truncation is rejected, not panicked on.
        assert!(open_session_msg(&key, &sealed[..10], "12D3KooPeerOne").is_none());
        assert!(open_session_msg(&key, &[], "12D3KooPeerOne").is_none());
        // Fresh nonce per seal: the same message never seals identically.
        let sealed2 = seal_session_msg(&key, msg, "12D3KooPeerOne").unwrap();
        assert_ne!(sealed, sealed2);
        // Open sessions have no key at all: gossip stays plaintext.
        assert!(gossip_key("").is_none());
    }

    #[test]
    fn auth_and_sealing_bytes_are_wire_stable() {
        // Known answers from argon2 0.5.3 + chacha20poly1305 0.10.1. Peers on
        // different builds must derive the same token and open each other's
        // sealed gossip, so a dependency bump that changes either output is a
        // wire change and needs a PROTO bump.
        let token = auth_token("salt-abc", "hunter2");
        assert_eq!(token, "3b7578c63371470c2a87e10ddcbfc8afe1154d61649ff26ec47b97665c3bbdbb");
        let hex = "5824242424242424242424242424242424242424242424242461e21698d50c45\
                   1221db05239abb2ae4ea0183f5410b671ef07f64db";
        let sealed: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let key = gossip_key(&token).unwrap();
        assert_eq!(open_session_msg(&key, &sealed, "12D3KooPeerOne").unwrap(), b"Ahello-world");
    }

    #[test]
    fn percent_coding_roundtrips() {
        let s = "Late Night Mix & friends / 100% ünïcödé ~._-";
        let enc = percent_encode(s);
        assert!(enc.chars().all(|c| c.is_ascii_alphanumeric() || "%-._~".contains(c)));
        assert_eq!(percent_decode(&enc).unwrap(), s);
        // `+` is not a space here (URI, not a form body).
        assert_eq!(percent_decode("a+b").unwrap(), "a+b");
        // Malformed escapes and invalid UTF-8 are refused, not mangled.
        assert!(percent_decode("%").is_none());
        assert!(percent_decode("%2").is_none());
        assert!(percent_decode("%zz").is_none());
        assert!(percent_decode("%ff%fe").is_none());
    }

    #[test]
    fn ticket_endpoints_skip_undialable_addresses() {
        let listen: Vec<Multiaddr> = [
            "/ip4/127.0.0.1/tcp/54001",
            "/ip4/0.0.0.0/tcp/54001",
            "/ip4/169.254.3.4/tcp/54001",
            "/ip6/fe80::1/tcp/54001",
            "/ip6/::1/tcp/54001",
            "/ip4/192.168.1.7/tcp/54001",
            "/ip4/127.0.0.1/udp/54002/quic-v1",
            "/ip4/192.168.1.7/udp/54002/quic-v1",
            "/ip4/100.101.102.103/udp/54002/quic-v1",
        ]
        .iter()
        .map(|a| a.parse().unwrap())
        .collect();
        let (ips, tcp, quic) = ticket_endpoints(&listen);
        assert_eq!(ips, vec![IpAddr::from([192, 168, 1, 7]), IpAddr::from([100, 101, 102, 103])]);
        assert_eq!((tcp, quic), (54001, 54002));
        // Nothing dialable at all: no ticket.
        let lo: Vec<Multiaddr> = vec!["/ip4/127.0.0.1/tcp/1".parse().unwrap()];
        assert!(ticket_endpoints(&lo).0.is_empty());
    }

    #[test]
    fn ticket_roundtrips_and_rejects_garbage() {
        let peer_str = "12D3KooWQYhTNQdmr3ArTeUHRYzFg94BKyTkoWBDWez9kSCVe2Xo";
        let peer: PeerId = peer_str.parse().unwrap();
        let ips = [IpAddr::from([192, 168, 1, 7]), IpAddr::from([100, 101, 102, 103])];
        let ticket = encode_ticket("abc123-9f", "Late Night Mix & more", peer_str, &ips, 54001, 54002, true);
        assert!(ticket.starts_with("reamesh://join/abc123-9f?"), "{ticket}");
        assert!(ticket.contains("n=Late%20Night%20Mix%20%26%20more"), "{ticket}");
        assert!(ticket.contains("&s=1&"), "{ticket}");
        assert!(ticket.contains("&a=192.168.1.7,100.101.102.103&"), "{ticket}");
        assert!(!ticket.contains(char::is_whitespace));
        let info = parse_ticket(&ticket).expect("well-formed ticket must parse");
        assert_eq!(info.session_id, "abc123-9f");
        assert_eq!(info.session_name, "Late Night Mix & more");
        assert_eq!(info.peer, peer);
        // Every IP expands to one address per transport.
        assert_eq!(info.addrs.len(), 4);
        assert!(info.addrs.contains(&"/ip4/192.168.1.7/tcp/54001".parse().unwrap()));
        assert!(info.addrs.contains(&"/ip4/100.101.102.103/udp/54002/quic-v1".parse().unwrap()));
        // Surrounding whitespace from a sloppy paste, a capitalised scheme and
        // a trailing slash after the id are tolerated.
        assert!(parse_ticket(&format!("  {ticket}\n")).is_some());
        assert!(parse_ticket(&ticket.replacen("reamesh", "ReaMesh", 1)).is_some());
        assert!(parse_ticket(&ticket.replacen("abc123-9f?", "abc123-9f/?", 1)).is_some());
        // Open session, TCP only, no name: the optional parts are absent.
        let open = encode_ticket("abc", "", peer_str, &ips[..1], 54001, 0, false);
        assert!(!open.contains("n=") && !open.contains("s=") && !open.contains("q="), "{open}");
        assert_eq!(parse_ticket(&open).unwrap().addrs.len(), 1);
        // An unknown parameter (a future version) is ignored, not refused.
        assert!(parse_ticket(&format!("{ticket}&x=1")).is_some());
        // Garbage in all its forms is rejected.
        assert!(parse_ticket("").is_none());
        assert!(parse_ticket("reamesh://join/").is_none());
        assert!(parse_ticket("reamesh://join/abc").is_none()); // no peer, no address
        assert!(parse_ticket(&ticket.replacen("reamesh://join/", "https://join/", 1)).is_none());
        assert!(parse_ticket(&ticket.replacen("reamesh://join/", "reamesh://host/", 1)).is_none());
        assert!(parse_ticket(&format!("reamesh://join/{}", "x".repeat(TICKET_MAX_LEN))).is_none());
        assert!(parse_ticket("reamesh://join/é").is_none()); // non-ASCII never panics
        assert!(parse_ticket(&ticket.replace("abc123-9f", "../etc")).is_none()); // hostile id
        assert!(parse_ticket(&ticket.replace(peer_str, "not-a-peer-id")).is_none());
        assert!(parse_ticket(&ticket.replace("a=192.168.1.7,100.101.102.103", "a=nope")).is_none());
        assert!(parse_ticket(&ticket.replace("&t=54001&q=54002", "")).is_none()); // no port
        assert!(parse_ticket(&ticket.replace("t=54001", "t=70000")).is_none());
        assert!(parse_ticket(&ticket.replace("t=54001", "t=0")).is_none());
    }

    #[test]
    fn concurrent_instances_claim_distinct_identity_slots() {
        let dir = std::env::temp_dir().join(format!(
            "reamesh-idslot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let base = dir.join("reamesh-identity.key");
        // Two "instances" (two independent lock handles) get two slots…
        let (p1, l1) = claim_identity_slot(&base);
        let (p2, l2) = claim_identity_slot(&base);
        assert_eq!(p1, base);
        assert_eq!(p2, dir.join("reamesh-identity.2.key"));
        assert!(l1.is_some() && l2.is_some());
        // …with distinct keypairs, so they are distinct peers.
        let k1 = load_or_create_identity_at(&p1);
        let k2 = load_or_create_identity_at(&p2);
        assert_ne!(k1.public().to_peer_id(), k2.public().to_peer_id());
        // Releasing the primary lock hands the primary identity to the next
        // instance (a restarted first REAPER keeps its old peer id).
        drop(l1);
        let (p3, _l3) = claim_identity_slot(&base);
        assert_eq!(p3, base);
        assert_eq!(
            load_or_create_identity_at(&p3).public().to_peer_id(),
            k1.public().to_peer_id()
        );
        drop(l2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn identity_persists_and_survives_corruption() {
        let dir = std::env::temp_dir().join(format!("reamesh-id-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("identity.key");
        // First use mints and persists.
        let a = load_or_create_identity_at(&path);
        assert!(path.is_file(), "identity must be persisted");
        // Reusing the path must load the same identity.
        let b = load_or_create_identity_at(&path);
        assert_eq!(
            a.public().to_peer_id(),
            b.public().to_peer_id(),
            "the persisted identity must be stable across loads"
        );
        // A corrupt file is replaced, not crashed on.
        fs::write(&path, b"not a keypair").unwrap();
        let c = load_or_create_identity_at(&path);
        assert_ne!(a.public().to_peer_id(), c.public().to_peer_id());
        // Reset = delete: the next load mints fresh (per-install, revocable).
        fs::remove_file(&path).unwrap();
        let d = load_or_create_identity_at(&path);
        assert_ne!(c.public().to_peer_id(), d.public().to_peer_id());
        let _ = fs::remove_dir_all(&dir);
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
