//! Relay session lists, the client's side (the contract is
//! `reamesh_relay_proto::sessions`).
//!
//! Two parts, both independent of the session table: entries never reach
//! `Shared::sessions`, so nothing on a list can touch the lobby's first-seen
//! owner rule. The app merges both sources for display.
//!
//! The **watcher** reads the lists while the app asks for them (Lobby and
//! Settings hold a lease with `relay_watch`). It runs on a swarm of its own:
//! the lobby's would tell the relay our LAN addresses through identify, and a
//! viewer has no reason to. It stops by itself once no lease is left, so a
//! closed app or a screen that went away never keeps a connection open.
//!
//! The **listing** is the hosting run's half: `HostListing` is the state per
//! relay, driven from `run_network`, which publishes on the main swarm because
//! that's the connection holding the reservation. It also long-polls the list,
//! to hear when the operator switches it on or off.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use libp2p::futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::request_response::{self, OutboundRequestId, ProtocolSupport};
use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{PeerId, StreamProtocol, noise, ping, tcp, yamux};
use reamesh_relay_proto::sessions::{
    PublishRequest, PublishResponse, REASON_DISABLED, REASON_NOT_RESERVED, RELAY_PUBLISH_PROTOCOL,
    RELAY_SESSIONS_PROTOCOL, SESSIONS_TIMEOUT_SECS, SessionEntry, SessionsRequest, SessionsResponse,
    open_entry, seal_entry,
};
use reamesh_relay_proto::{REASON_BAD_KEY, REASON_BANNED};
use serde_json::{Value as JsonValue, json};

use crate::limits;
use crate::relay::{self, RelayCode, auth_proof};

/// cbindgen:ignore
pub(crate) type SessionsBehaviour = request_response::cbor::Behaviour<SessionsRequest, SessionsResponse>;
/// cbindgen:ignore
pub(crate) type PublishBehaviour = request_response::cbor::Behaviour<PublishRequest, PublishResponse>;

pub(crate) fn sessions_behaviour() -> SessionsBehaviour {
    request_response::cbor::Behaviour::new(
        [(StreamProtocol::new(RELAY_SESSIONS_PROTOCOL), ProtocolSupport::Outbound)],
        request_response::Config::default().with_request_timeout(Duration::from_secs(SESSIONS_TIMEOUT_SECS)),
    )
}

pub(crate) fn publish_behaviour() -> PublishBehaviour {
    request_response::cbor::Behaviour::new(
        [(StreamProtocol::new(RELAY_PUBLISH_PROTOCOL), ProtocolSupport::Outbound)],
        request_response::Config::default().with_request_timeout(Duration::from_secs(20)),
    )
}

/// A host republishes a changed entry at most this often. Taking the session
/// off a list, or putting it back, isn't held back.
const REPUBLISH_EVERY: Duration = Duration::from_secs(30);
/// Between two list requests on one connection, whatever the answers say.
const QUERY_MIN_GAP: Duration = Duration::from_secs(1);
/// A relay that said no is asked again this much later: the operator may have
/// given the key back, or may have just been restarting.
const REFUSED_RETRY: Duration = Duration::from_secs(60);
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// A lease the app stopped renewing runs out after this.
const LEASE: Duration = Duration::from_secs(60);
const WATCH_TICK: Duration = Duration::from_millis(250);
/// Pings a relay may miss before the connection counts as dead.
const PING_MISSES: u32 = 3;
/// Most entries taken from one relay's answer.
const MAX_ENTRIES: usize = limits::MAX_SESSIONS;

// ---------- what the app sees and says (process-global, like the relays) ----------

/// One relay's list as the `relay` control message carries it.
#[derive(Clone, Debug, Default, PartialEq)]
struct ListState {
    /// `connecting` | `ok` | `down` | `refused` | `idle` (nobody is looking).
    state: &'static str,
    /// Why it refused: `bad_key` | `banned` | `expired` | `refused`.
    reason: String,
    /// Whether the relay keeps a list; `None` until it said.
    enabled: Option<bool>,
    budget_used: bool,
    /// How many sessions are on it.
    count: usize,
    entries: Vec<JsonValue>,
}

static LISTS: LazyLock<Mutex<HashMap<PeerId, ListState>>> = LazyLock::new(Default::default);
/// Relays this install's sessions stay off. `None` until the app said so: a
/// session is only ever listed on the app's word.
static HIDDEN: Mutex<Option<HashSet<PeerId>>> = Mutex::new(None);
/// Who wants the lists read, and until when (`relay_watch`).
static LEASES: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);
static WATCHING: AtomicBool = AtomicBool::new(false);

fn update(relay: PeerId, f: impl FnOnce(&mut ListState)) {
    let mut lists = LISTS.lock().unwrap();
    let state = lists.entry(relay).or_insert_with(|| ListState { state: "idle", ..Default::default() });
    let before = state.clone();
    f(state);
    if *state != before {
        relay::bump_generation();
    }
}

/// `{peer: {state, reason, enabled, budget_used, count, entries}}`, only for
/// configured relays. The entries are what the app shows and joins by; an open
/// session's ticket carries its secret, as an invite link does.
pub(crate) fn lists_json() -> JsonValue {
    let configured: HashSet<PeerId> = relay::configured_relays().iter().map(|c| c.addr.peer).collect();
    let lists = LISTS.lock().unwrap();
    let mut out = serde_json::Map::new();
    for (peer, l) in lists.iter().filter(|(p, _)| configured.contains(p)) {
        out.insert(
            peer.to_string(),
            json!({
                "state": l.state,
                "reason": l.reason,
                "enabled": l.enabled,
                "budget_used": l.budget_used,
                "count": l.count,
                "entries": l.entries,
            }),
        );
    }
    JsonValue::Object(out)
}

/// The app's choice: this install's sessions stay off these relays' lists.
/// Unparsable ids are dropped, so a stale one can't do anything.
pub fn set_listing(hidden: &[String]) {
    let set = hidden.iter().filter_map(|p| p.parse().ok()).collect();
    *HIDDEN.lock().unwrap() = Some(set);
}

fn listing_allowed(relay: &PeerId) -> bool {
    HIDDEN.lock().unwrap().as_ref().is_some_and(|h| !h.contains(relay))
}

/// Take or give up a lease on the lists. The first lease starts the watcher;
/// it stops on its own once none is left.
pub fn watch(id: &str, on: bool) {
    {
        let mut leases = LEASES.lock().unwrap();
        if on {
            if leases.len() < 8 || leases.contains_key(id) {
                leases.insert(id.to_owned(), Instant::now() + LEASE);
            }
        } else {
            leases.remove(id);
        }
    }
    if on && !WATCHING.swap(true, Ordering::SeqCst) {
        let spawned = std::thread::Builder::new()
            .name("reamesh-relay-lists".into())
            .spawn(run_watch_thread);
        if spawned.is_err() {
            WATCHING.store(false, Ordering::SeqCst);
        }
    }
}

fn lease_held() -> bool {
    let mut leases = LEASES.lock().unwrap();
    let now = Instant::now();
    leases.retain(|_, until| *until > now);
    !leases.is_empty()
}

fn run_watch_thread() {
    let finished = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => {
            rt.block_on(run_watch());
            true
        }
        Err(e) => {
            eprintln!("[reamesh] relay lists: no runtime: {e}");
            false
        }
    };
    // Nobody is looking anymore: what's on the lists is unknown from here on.
    // Whether a relay has a list stays known, the host card needs it.
    let mut lists = LISTS.lock().unwrap();
    for l in lists.values_mut() {
        *l = ListState { state: "idle", enabled: l.enabled, ..Default::default() };
    }
    drop(lists);
    relay::bump_generation();
    WATCHING.store(false, Ordering::SeqCst);
    // A lease may have come in while this was winding down.
    if finished && lease_held() && !WATCHING.swap(true, Ordering::SeqCst) {
        let _ = std::thread::Builder::new()
            .name("reamesh-relay-lists".into())
            .spawn(run_watch_thread);
    }
}

// ---------- reading an answer ----------

/// The answer's entries as the app gets them: each one checked, its ticket
/// rebuilt from its checked parts and our own record of where the relay is, so
/// nothing a host or a relay wrote decides what gets dialed. Our own session
/// isn't shown back to us.
fn entries_from(resp: &SessionsResponse, code: &RelayCode, me: &PeerId) -> Vec<JsonValue> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for listed in resp.entries.iter().take(MAX_ENTRIES) {
        let entry = match open_entry(&listed.envelope, &code.addr.peer) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[reamesh] relay list: entry refused ({e:?})");
                continue;
            }
        };
        if entry.owner == me.to_string() || !seen.insert(entry.owner.clone()) {
            continue;
        }
        let Some(ticket) = crate::relay_only_ticket(&entry, &code.addr) else {
            eprintln!("[reamesh] relay list: entry with an unusable ticket");
            continue;
        };
        out.push(json!({
            "session_id": entry.session_id,
            "session_name": limits::clip(&entry.session_name, 128),
            "owner": entry.owner,
            "owner_name": limits::clip(&entry.owner_name, 64),
            "owner_color": limits::clip(&entry.owner_color, 16),
            "auth": entry.auth,
            "files": entry.files,
            "bytes": entry.bytes,
            "proto": entry.proto,
            "version": limits::clip(&entry.ver, 32),
            "ticket": ticket,
            "key_label": limits::clip(&listed.key_label, 64),
        }));
    }
    out
}

/// A refusal's reason as one of the stable codes.
fn refusal(resp: &SessionsResponse) -> &'static str {
    match resp.reason.as_deref() {
        Some(REASON_BAD_KEY) => REASON_BAD_KEY,
        Some(REASON_BANNED) => REASON_BANNED,
        Some("expired") => "expired",
        _ => "refused",
    }
}

// ---------- the watcher ----------

#[derive(NetworkBehaviour)]
struct WatchBehaviour {
    sessions: SessionsBehaviour,
    ping: ping::Behaviour,
}

struct WatchSlot {
    code: RelayCode,
    connected: bool,
    dialing: bool,
    /// The request out, and the `rev` the next one asks with.
    query: Option<OutboundRequestId>,
    rev: u64,
    query_at: Instant,
    dial_at: Instant,
    backoff: Duration,
    ping_misses: u32,
}

impl WatchSlot {
    fn new(code: RelayCode) -> Self {
        let now = Instant::now();
        Self {
            code,
            connected: false,
            dialing: false,
            query: None,
            rev: 0,
            query_at: now,
            dial_at: now,
            backoff: RETRY_MIN,
            ping_misses: 0,
        }
    }

    fn peer(&self) -> PeerId {
        self.code.addr.peer
    }

    /// The connection is gone or never came: try again later, and start over
    /// with `rev` 0 since a restarted relay counts from the beginning.
    fn lost(&mut self) {
        self.connected = false;
        self.dialing = false;
        self.query = None;
        self.rev = 0;
        self.dial_at = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(RETRY_MAX);
    }
}

fn watch_swarm(key: Keypair) -> anyhow::Result<libp2p::Swarm<WatchBehaviour>> {
    let (dns_cfg, dns_opts) = crate::dns_config();
    Ok(libp2p::SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_quic()
        .with_dns_config(dns_cfg, dns_opts)
        .with_behaviour(|_| WatchBehaviour { sessions: sessions_behaviour(), ping: ping::Behaviour::default() })
        .map_err(|e| anyhow::anyhow!("behaviour: {e}"))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
        .build())
}

/// Keeps one slot per configured relay in step with the app's list.
fn sync_slots(slots: &mut Vec<WatchSlot>, swarm: &mut libp2p::Swarm<WatchBehaviour>) {
    let codes = relay::configured_relays();
    slots.retain(|s| {
        let keep = codes.iter().any(|c| c.addr.peer == s.peer() && c.key == s.code.key);
        if !keep {
            let _ = swarm.disconnect_peer_id(s.peer());
            LISTS.lock().unwrap().remove(&s.peer());
            relay::bump_generation();
        }
        keep
    });
    for code in codes {
        if !slots.iter().any(|s| s.peer() == code.addr.peer) {
            update(code.addr.peer, |l| l.state = "connecting");
            slots.push(WatchSlot::new(code));
        }
    }
}

async fn run_watch() {
    let key = crate::identity_keypair();
    let me = key.public().to_peer_id();
    let mut swarm = match watch_swarm(key) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[reamesh] relay lists: {e}");
            return;
        }
    };
    let mut slots: Vec<WatchSlot> = Vec::new();
    let mut tick = tokio::time::interval(WATCH_TICK);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if !lease_held() {
                    break;
                }
                sync_slots(&mut slots, &mut swarm);
                let now = Instant::now();
                for slot in &mut slots {
                    if !slot.connected && !slot.dialing && now >= slot.dial_at {
                        let opts = DialOpts::peer_id(slot.peer())
                            .condition(PeerCondition::DisconnectedAndNotDialing)
                            .addresses(slot.code.addr.dial_addrs())
                            .build();
                        match swarm.dial(opts) {
                            Ok(()) => slot.dialing = true,
                            Err(e) => {
                                eprintln!("[reamesh] relay list dial: {e}");
                                slot.lost();
                                update(slot.peer(), |l| l.state = "down");
                            }
                        }
                    }
                    if slot.connected && slot.query.is_none() && now >= slot.query_at {
                        let proof = auth_proof(&slot.code.key, &slot.peer(), &me).to_vec();
                        let req = SessionsRequest { proof, rev: slot.rev };
                        slot.query = Some(swarm.behaviour_mut().sessions.send_request(&slot.peer(), req));
                        slot.query_at = now + QUERY_MIN_GAP;
                    }
                }
            }
            event = swarm.select_next_some() => on_watch_event(&mut swarm, &mut slots, &me, event),
        }
    }
}

fn on_watch_event(
    swarm: &mut libp2p::Swarm<WatchBehaviour>,
    slots: &mut [WatchSlot],
    me: &PeerId,
    event: SwarmEvent<WatchBehaviourEvent>,
) {
    let slot_of = |slots: &'_ mut [WatchSlot], peer: &PeerId| -> Option<usize> {
        slots.iter().position(|s| s.peer() == *peer)
    };
    match event {
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            if let Some(i) = slot_of(slots, &peer_id) {
                let s = &mut slots[i];
                s.connected = true;
                s.dialing = false;
                s.backoff = RETRY_MIN;
                s.ping_misses = 0;
            }
        }
        SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
            if let Some(i) = slot_of(slots, &peer_id) {
                // Quiet at first: a relay restarting is usually back at once,
                // and what it listed is most likely still true.
                slots[i].lost();
                update(peer_id, |l| l.state = "connecting");
            }
        }
        SwarmEvent::OutgoingConnectionError { peer_id: Some(peer), .. } => {
            if let Some(i) = slot_of(slots, &peer) {
                slots[i].lost();
                update(peer, |l| {
                    l.state = "down";
                    l.count = 0;
                    l.entries.clear();
                });
            }
        }
        SwarmEvent::Behaviour(WatchBehaviourEvent::Sessions(request_response::Event::Message {
            peer,
            message: request_response::Message::Response { request_id, response },
            ..
        })) => {
            let Some(i) = slot_of(slots, &peer) else { return };
            let s = &mut slots[i];
            if s.query != Some(request_id) {
                return;
            }
            s.query = None;
            if response.ok {
                s.rev = response.rev;
                let entries = if response.enabled { entries_from(&response, &s.code, me) } else { Vec::new() };
                update(peer, |l| {
                    l.state = "ok";
                    l.reason.clear();
                    l.enabled = Some(response.enabled);
                    l.budget_used = response.budget_used;
                    l.count = entries.len();
                    l.entries = entries;
                });
            } else {
                s.rev = 0;
                s.query_at = Instant::now() + REFUSED_RETRY;
                let reason = refusal(&response);
                update(peer, |l| {
                    l.state = "refused";
                    l.reason = reason.to_owned();
                    l.count = 0;
                    l.entries.clear();
                });
            }
        }
        SwarmEvent::Behaviour(WatchBehaviourEvent::Sessions(request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        })) => {
            let Some(i) = slot_of(slots, &peer) else { return };
            let s = &mut slots[i];
            if s.query != Some(request_id) {
                return;
            }
            s.query = None;
            if matches!(error, request_response::OutboundFailure::UnsupportedProtocols) {
                // A relay from before the list: no error, there just is none.
                // It won't learn in this connection's lifetime.
                s.query_at = Instant::now() + RETRY_MAX;
                update(peer, |l| {
                    l.state = "ok";
                    l.reason.clear();
                    l.enabled = Some(false);
                    l.count = 0;
                    l.entries.clear();
                });
            } else {
                s.query_at = Instant::now() + RETRY_MIN;
            }
        }
        SwarmEvent::Behaviour(WatchBehaviourEvent::Ping(ping::Event { peer, result, .. })) => {
            let Some(i) = slot_of(slots, &peer) else { return };
            let s = &mut slots[i];
            if result.is_ok() {
                s.ping_misses = 0;
            } else {
                s.ping_misses += 1;
                if s.ping_misses >= PING_MISSES {
                    s.ping_misses = 0;
                    let _ = swarm.disconnect_peer_id(peer);
                }
            }
        }
        _ => {}
    }
}

// ---------- the hosting run's listing ----------

/// One relay's list as a hosting run deals with it.
#[derive(Debug)]
pub(crate) struct HostListing {
    /// Whether the relay keeps a list; `None` until it said.
    enabled: Option<bool>,
    /// A relay from before the list never gets asked again.
    unsupported: bool,
    rev: u64,
    query: Option<OutboundRequestId>,
    query_at: Instant,
    /// The entry on the relay (its unsigned JSON); `None` for no entry.
    published: Option<String>,
    /// The publish out, and what it will have put there.
    sending: Option<(OutboundRequestId, Option<String>)>,
    /// When the entry last changed there.
    published_at: Option<Instant>,
    retry_at: Instant,
    /// The entry doesn't fit: said once.
    too_large: bool,
}

impl HostListing {
    pub(crate) fn new() -> Self {
        let now = Instant::now();
        Self {
            enabled: None,
            unsupported: false,
            rev: 0,
            query: None,
            query_at: now,
            published: None,
            sending: None,
            published_at: None,
            retry_at: now,
            too_large: false,
        }
    }

    /// The reservation is gone, and the relay dropped the entry with it. What
    /// the relay said about its list stays, until a new reservation asks.
    pub(crate) fn reservation_lost(&mut self) {
        *self = Self {
            enabled: self.enabled,
            unsupported: self.unsupported,
            published_at: self.published_at,
            ..Self::new()
        };
    }

    pub(crate) fn query_due(&self, now: Instant) -> bool {
        !self.unsupported && self.query.is_none() && now >= self.query_at
    }

    pub(crate) fn rev(&self) -> u64 {
        self.rev
    }

    pub(crate) fn query_sent(&mut self, id: OutboundRequestId, now: Instant) {
        self.query = Some(id);
        self.query_at = now + QUERY_MIN_GAP;
    }

    pub(crate) fn is_query(&self, id: OutboundRequestId) -> bool {
        self.query == Some(id)
    }

    /// What should be on this relay's list now, given the entry as it stands.
    /// `Some(what)` when a publish is due: `what` is the new entry, `None` to
    /// withdraw.
    pub(crate) fn publish_due(&self, relay: &PeerId, entry: Option<String>, now: Instant) -> Option<Option<String>> {
        self.publish_due_if(listing_allowed(relay), entry, now)
    }

    /// `publish_due`, with the user's choice for this relay given.
    fn publish_due_if(&self, allowed: bool, entry: Option<String>, now: Instant) -> Option<Option<String>> {
        if self.sending.is_some() || now < self.retry_at {
            return None;
        }
        let want = entry.filter(|_| self.enabled == Some(true) && allowed && !self.too_large);
        if want == self.published || (want.is_none() && self.enabled != Some(true)) {
            return None;
        }
        // Only a change of an entry that stays on the list waits its turn.
        if want.is_some()
            && self.published.is_some()
            && self.published_at.is_some_and(|at| now.duration_since(at) < REPUBLISH_EVERY)
        {
            return None;
        }
        Some(want)
    }

    pub(crate) fn publish_sent(&mut self, id: OutboundRequestId, what: Option<String>) {
        self.sending = Some((id, what));
    }

    /// The entry doesn't fit `ENTRY_MAX_BYTES`: leave it off this list.
    pub(crate) fn publish_too_large(&mut self) {
        if !self.too_large {
            eprintln!("[reamesh] relay list: the session's entry is too large to list");
        }
        self.too_large = true;
    }

    /// The list answered.
    pub(crate) fn on_query(&mut self, relay: PeerId, response: SessionsResponse, me: &str) {
        self.query = None;
        if !response.ok {
            self.rev = 0;
            self.query_at = Instant::now() + REFUSED_RETRY;
            return;
        }
        self.rev = response.rev;
        self.enabled = Some(response.enabled);
        if !response.enabled {
            self.published = None;
        }
        // Everyone but ourselves, as in the Lobby.
        let count = response
            .entries
            .iter()
            .filter(|l| {
                open_entry(&l.envelope, &relay).is_ok_and(|e: SessionEntry| e.owner != me)
            })
            .count();
        update(relay, |l| {
            l.state = "ok";
            l.reason.clear();
            l.enabled = Some(response.enabled);
            l.budget_used = response.budget_used;
            l.count = count;
        });
    }

    /// The list request failed.
    pub(crate) fn on_query_failed(&mut self, relay: PeerId, unsupported: bool) {
        self.query = None;
        if unsupported {
            self.unsupported = true;
            self.enabled = Some(false);
            self.published = None;
            update(relay, |l| {
                l.state = "ok";
                l.enabled = Some(false);
            });
        } else {
            self.query_at = Instant::now() + RETRY_MIN;
        }
    }

    pub(crate) fn is_publish(&self, id: OutboundRequestId) -> bool {
        self.sending.as_ref().is_some_and(|(s, _)| *s == id)
    }

    pub(crate) fn on_publish(&mut self, response: Result<PublishResponse, ()>, now: Instant) {
        let Some((_, what)) = self.sending.take() else { return };
        match response {
            Ok(r) if r.ok => {
                self.published = what;
                self.published_at = Some(now);
            }
            Ok(r) => match r.reason.as_deref() {
                Some(REASON_DISABLED) => {
                    self.enabled = Some(false);
                    self.published = None;
                }
                // The relay may not have taken in our reservation yet.
                Some(REASON_NOT_RESERVED) => self.retry_at = now + RETRY_MIN,
                other => {
                    eprintln!("[reamesh] relay list: publish refused ({})", other.unwrap_or("?"));
                    self.retry_at = now + REFUSED_RETRY;
                }
            },
            Err(()) => self.retry_at = now + RETRY_MIN,
        }
    }
}

/// Seals what `publish_due` asked for, into a request. `None` when the entry
/// doesn't fit.
pub(crate) fn publish_request(
    key: &Keypair,
    code: &RelayCode,
    me: &PeerId,
    what: Option<&str>,
) -> Option<PublishRequest> {
    let proof = auth_proof(&code.key, &code.addr.peer, me).to_vec();
    let envelope = match what {
        None => None,
        Some(json) => {
            let entry: SessionEntry = serde_json::from_str(json).ok()?;
            Some(seal_entry(key, &entry)?)
        }
    };
    Some(PublishRequest { proof, envelope })
}

/// A session's unsigned entry, as JSON, for one relay.
pub(crate) fn entry_json(entry: &SessionEntry) -> Option<String> {
    serde_json::to_string(entry).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_published_before_the_relay_and_the_app_agree() {
        let mut h = HostListing::new();
        let now = Instant::now();
        // The relay hasn't said yet.
        assert_eq!(h.publish_due_if(true, Some("e".into()), now), None);
        // It keeps a list, but the app chose otherwise.
        h.enabled = Some(true);
        assert_eq!(h.publish_due_if(false, Some("e".into()), now), None);
        // Both agree.
        assert_eq!(h.publish_due_if(true, Some("e".into()), now), Some(Some("e".into())));
    }

    #[test]
    fn the_app_has_to_say_before_anything_is_listed() {
        let relay = PeerId::random();
        // A process-global the watcher tests don't touch; this is its only test.
        assert!(!listing_allowed(&relay));
        set_listing(&[]);
        assert!(listing_allowed(&relay));
        set_listing(&[relay.to_string(), "not a peer id".to_owned()]);
        assert!(!listing_allowed(&relay));
        *HIDDEN.lock().unwrap() = None;
    }

    #[test]
    fn a_changed_entry_waits_but_taking_it_off_does_not() {
        let mut h = HostListing::new();
        let now = Instant::now();
        h.enabled = Some(true);
        h.published = Some("old".into());
        h.published_at = Some(now);
        assert_eq!(h.publish_due_if(true, Some("old".into()), now), None);
        assert_eq!(h.publish_due_if(true, Some("new".into()), now), None);
        let later = now + REPUBLISH_EVERY;
        assert_eq!(h.publish_due_if(true, Some("new".into()), later), Some(Some("new".into())));
        // Hidden right now: withdrawn at once.
        assert_eq!(h.publish_due_if(false, Some("old".into()), now), Some(None));
    }

    #[test]
    fn a_relay_without_a_list_gets_no_withdrawal_either() {
        let mut h = HostListing::new();
        let now = Instant::now();
        h.on_query_failed(PeerId::random(), true);
        assert_eq!(h.enabled, Some(false));
        assert!(!h.query_due(now));
        assert_eq!(h.publish_due_if(true, Some("e".into()), now), None);
        assert_eq!(h.publish_due_if(false, None, now), None);
    }

    #[test]
    fn a_lost_reservation_forgets_the_entry_but_not_the_list() {
        let mut h = HostListing::new();
        h.enabled = Some(true);
        h.published = Some("e".into());
        h.rev = 9;
        h.reservation_lost();
        assert_eq!(h.published, None);
        assert_eq!(h.rev, 0);
        assert_eq!(h.enabled, Some(true));
    }

    #[test]
    fn a_publish_the_relay_hadnt_caught_up_with_is_retried_soon() {
        let now = Instant::now();
        let mut h = HostListing::new();
        h.sending = Some((fake_id(), Some("e".into())));
        h.on_publish(Ok(PublishResponse::refused(REASON_NOT_RESERVED)), now);
        assert!(h.sending.is_none());
        assert_eq!(h.published, None);
        assert_eq!(h.retry_at, now + RETRY_MIN);
    }

    #[test]
    fn a_refusal_or_a_list_switched_off_leaves_nothing_published() {
        let now = Instant::now();
        let mut h = HostListing::new();
        h.enabled = Some(true);
        h.published = Some("old".into());
        h.sending = Some((fake_id(), Some("new".into())));
        h.on_publish(Ok(PublishResponse::refused(REASON_DISABLED)), now);
        assert_eq!(h.enabled, Some(false));
        assert_eq!(h.published, None);
    }

    /// `OutboundRequestId` has no constructor; any swarm's first id will do.
    fn fake_id() -> OutboundRequestId {
        let mut b = sessions_behaviour();
        b.send_request(&PeerId::random(), SessionsRequest { proof: Vec::new(), rev: 0 })
    }
}
