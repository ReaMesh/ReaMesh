//! Internet sessions through a self-hosted relay (`reamesh-relay`).
//!
//! While hosting, the core authenticates on each configured relay with its
//! access key, reserves a circuit slot (libp2p circuit relay v2) and names the
//! relay (never the key) in the invite link, so a guest with no direct route
//! dials the host through it. DCUtR upgrades a relayed connection to a direct
//! one where the NATs allow. Noise runs end to end through the circuit, so the
//! relay only ever forwards ciphertext.
//!
//! What the relay server must agree on (code format, auth messages, proof)
//! lives in the `reamesh-relay-proto` crate, which the server shares. This module has the parts that don't need session
//! state: the configured relays, a relay's invite-link form, the Settings test,
//! and two swarm behaviours: [`LanOnly`] keeps the lobby off relayed and
//! non-LAN connections, [`IpLimits`] caps inbound connections per IP.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use either::Either;
use libp2p::core::Endpoint;
use libp2p::core::transport::PortUse;
use libp2p::futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::behaviour::{ConnectionClosed, ConnectionEstablished};
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::{
    ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, SwarmEvent, THandler,
    THandlerInEvent, THandlerOutEvent, ToSwarm, dummy,
};
use libp2p::{Multiaddr, PeerId, StreamProtocol, noise, tcp, yamux};
use serde_json::{Value as JsonValue, json};

pub use reamesh_relay_proto::{AuthRequest, AuthResponse, RelayAddr, RelayCode, auth_proof, refusal_reason};
use reamesh_relay_proto::{RELAY_AUTH_PROTOCOL, RELAY_LABEL_MAX, RELAY_MAX_IPS, valid_dns_name};

/// Most relays a user can configure. The host reserves on all of them and the
/// invite link names all of them.
pub(crate) const MAX_RELAYS: usize = 3;
/// How long the relay test may take as a whole.
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Parse a relay code (`reamesh_relay_proto::parse_relay_code`); loopback relays only
/// in the loopback-only test mode. Mirrored by `app/src/lib/relay.ts`.
pub fn parse_relay_code(code: &str) -> Option<RelayCode> {
    reamesh_relay_proto::parse_relay_code(code, crate::loopback_only())
}

/// The compact form an invite link carries in an `r=` parameter:
/// `<peer>~<ip>,<ip>~<tcp>~<quic>[~<dns>]` (empty fields allowed, no key).
pub fn ticket_param(a: &RelayAddr) -> String {
    let ips: Vec<String> = a.ips.iter().map(|ip| ip.to_string()).collect();
    let port = |p: u16| if p == 0 { String::new() } else { p.to_string() };
    let mut s = format!("{}~{}~{}~{}", a.peer, ips.join(","), port(a.tcp), port(a.quic));
    if !a.dns.is_empty() {
        s.push('~');
        s.push_str(&a.dns);
    }
    s
}

/// Read one `r=` entry back; `None` when it doesn't name a usable relay.
pub fn from_ticket_param(v: &str) -> Option<RelayAddr> {
    let parts: Vec<&str> = v.split('~').collect();
    if !(4..=5).contains(&parts.len()) {
        return None;
    }
    let peer: PeerId = parts[0].parse().ok()?;
    let ips = parts[1]
        .split(',')
        .filter_map(|ip| ip.parse().ok())
        .filter(crate::dialable_ip)
        .take(RELAY_MAX_IPS)
        .collect();
    let port = |s: &str| -> Option<u16> {
        if s.is_empty() { Some(0) } else { s.parse().ok().filter(|p| *p != 0) }
    };
    let (tcp, quic) = (port(parts[2])?, port(parts[3])?);
    let dns = match parts.get(4) {
        Some(d) if valid_dns_name(d) => d.to_ascii_lowercase(),
        Some(_) => return None,
        None => String::new(),
    };
    Some(RelayAddr { peer, ips, dns, tcp, quic }).filter(RelayAddr::is_usable)
}

/// cbindgen:ignore
pub type AuthBehaviour = request_response::cbor::Behaviour<AuthRequest, AuthResponse>;

/// The auth client: outbound only (a ReaMesh peer never answers relay auth).
pub fn auth_behaviour() -> AuthBehaviour {
    request_response::cbor::Behaviour::new(
        [(StreamProtocol::new(RELAY_AUTH_PROTOCOL), ProtocolSupport::Outbound)],
        request_response::Config::default().with_request_timeout(Duration::from_secs(10)),
    )
}

// ---------- configured relays (process-global, set by the app) ----------

/// The relays the app configured. Global like the
/// profile: it describes this install, not one session. A hosting run takes
/// a snapshot when it starts.
static RELAYS: Mutex<Vec<RelayCode>> = Mutex::new(Vec::new());

/// Replace the configured relays with the codes that parse (at most
/// `MAX_RELAYS`, the first of each relay id kept). Returns how many were kept.
pub fn set_relays(codes: &[String]) -> usize {
    let mut out: Vec<RelayCode> = Vec::new();
    for c in codes {
        if out.len() >= MAX_RELAYS {
            break;
        }
        if let Some(code) = parse_relay_code(c)
            && !out.iter().any(|o| o.addr.peer == code.addr.peer)
        {
            out.push(code);
        }
    }
    let n = out.len();
    *RELAYS.lock().unwrap() = out;
    n
}

pub fn configured_relays() -> Vec<RelayCode> {
    RELAYS.lock().unwrap().clone()
}

/// What the hosting run reports about each relay (`[{peer,label,host,state,
/// reason}]`), for the app's `relay` message. Empty when not hosting.
static STATUS: LazyLock<Mutex<JsonValue>> = LazyLock::new(|| Mutex::new(json!([])));
/// Latest relay test result (`{id,peer,outcome,label}`), Null before any.
static TEST_RESULT: LazyLock<Mutex<JsonValue>> = LazyLock::new(|| Mutex::new(JsonValue::Null));
/// Bumped on every change to either, so the control server knows to send.
static GEN: AtomicU64 = AtomicU64::new(0);

pub fn set_status(v: JsonValue) {
    let mut s = STATUS.lock().unwrap();
    if *s != v {
        *s = v;
        GEN.fetch_add(1, Ordering::Relaxed);
    }
}

/// The hosting run's relay status (see `STATUS`).
pub fn status() -> JsonValue {
    STATUS.lock().unwrap().clone()
}

/// The `relay` control message (`{type:"relay", status, test}`) and its
/// generation number.
pub fn ctl_message() -> (u64, String) {
    let generation = GEN.load(Ordering::Relaxed);
    let msg = json!({
        "type": "relay",
        "status": STATUS.lock().unwrap().clone(),
        "test": TEST_RESULT.lock().unwrap().clone(),
    });
    (generation, msg.to_string())
}

// ---------- relay test (Settings → Relay → Test) ----------

#[derive(NetworkBehaviour)]
struct TestBehaviour {
    auth: AuthBehaviour,
}

fn test_swarm() -> anyhow::Result<libp2p::Swarm<TestBehaviour>> {
    let (dns_cfg, dns_opts) = crate::dns_config();
    Ok(libp2p::SwarmBuilder::with_existing_identity(crate::identity_keypair())
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_quic()
        .with_dns_config(dns_cfg, dns_opts)
        .with_behaviour(|_| TestBehaviour { auth: auth_behaviour() })
        .map_err(|e| anyhow::anyhow!("behaviour: {e}"))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(5)))
        .build())
}

/// Run the app's relay test for `code` and publish the outcome under `id`.
/// Dials the relay with this install's identity (a ban is per device) and
/// runs the auth exchange; no reservation, which would replace a running
/// host's own. Outcomes: `ok` (reachable, key accepted), `bad_key`,
/// `banned`, `full`, `not_relay` (answers, but not the auth protocol),
/// `unreachable`, `timeout`, `invalid` (the code doesn't parse).
pub async fn run_test(code: String, id: String) {
    let parsed = parse_relay_code(&code);
    let peer = parsed.as_ref().map(|c| c.addr.peer.to_string()).unwrap_or_default();
    *TEST_RESULT.lock().unwrap() = json!({ "id": id, "peer": peer, "outcome": "testing" });
    GEN.fetch_add(1, Ordering::Relaxed);
    let (outcome, label) = match parsed {
        None => ("invalid", String::new()),
        Some(c) => match tokio::time::timeout(TEST_TIMEOUT, test_relay(&c)).await {
            Ok(r) => r,
            Err(_) => ("timeout", String::new()),
        },
    };
    *TEST_RESULT.lock().unwrap() =
        json!({ "id": id, "peer": peer, "outcome": outcome, "label": label });
    GEN.fetch_add(1, Ordering::Relaxed);
}

/// The test itself (see `run_test`), unbounded in time.
pub async fn test_relay(code: &RelayCode) -> (&'static str, String) {
    let Ok(mut swarm) = test_swarm() else { return ("unreachable", String::new()) };
    let relay = code.addr.peer;
    if swarm
        .dial(DialOpts::peer_id(relay).addresses(code.addr.dial_addrs()).build())
        .is_err()
    {
        return ("unreachable", String::new());
    }
    let me = *swarm.local_peer_id();
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == relay => {
                let proof = auth_proof(&code.key, &relay, &me).to_vec();
                swarm.behaviour_mut().auth.send_request(&relay, AuthRequest { proof });
            }
            SwarmEvent::OutgoingConnectionError { peer_id: Some(p), .. } if p == relay => {
                return ("unreachable", String::new());
            }
            SwarmEvent::Behaviour(TestBehaviourEvent::Auth(request_response::Event::Message {
                message: request_response::Message::Response { response, .. },
                ..
            })) => {
                let label = response.key_label.clone().unwrap_or_default();
                let label = crate::limits::clip(&label, RELAY_LABEL_MAX);
                return if response.ok { ("ok", label) } else { (refusal_reason(&response), label) };
            }
            SwarmEvent::Behaviour(TestBehaviourEvent::Auth(
                request_response::Event::OutboundFailure { error, .. },
            )) => {
                return match error {
                    request_response::OutboundFailure::UnsupportedProtocols => {
                        ("not_relay", String::new())
                    }
                    request_response::OutboundFailure::Timeout => ("timeout", String::new()),
                    _ => ("unreachable", String::new()),
                };
            }
            _ => {}
        }
    }
}

// ---------- which connections count as LAN ----------

/// Whether a connection runs through a relay circuit. An inbound circuit has
/// the `/p2p-circuit` in its local address (the remote is just `/p2p/<src>`),
/// an outbound one in the dialed address.
pub fn is_relayed(local: Option<&Multiaddr>, remote: &Multiaddr) -> bool {
    let circ = |a: &Multiaddr| a.iter().any(|p| matches!(p, Protocol::P2pCircuit));
    circ(remote) || local.is_some_and(circ)
}

/// Whether an IP is on "this" network for lobby purposes: loopback, the
/// private and link-local ranges, IPv6 ULA, or inside one of our own
/// interface subnets (a campus LAN with public addresses). CGNAT 100.64/10
/// (Tailscale and friends) is not: a VPN peer joins by invite link and never
/// needs the lobby.
fn lan_ip(ip: IpAddr, subnets: &[(IpAddr, IpAddr)]) -> bool {
    let local = match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => return lan_ip(IpAddr::V4(v4), subnets),
            None => {
                v6.is_loopback()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
        },
    };
    local || crate::in_local_subnet(ip, subnets)
}

/// A behaviour that only sees LAN connections: every relayed connection, and
/// every connection whose remote IP is not on this network (a hole-punched
/// internet peer, a VPN peer), gets a do-nothing handler and is hidden from
/// the inner behaviour entirely. Wraps the lobby gossipsub, so a session's
/// announce (which carries an open session's link secret) and other hosts'
/// announces never leave the LAN through a relay or an internet guest, and
/// nothing an internet peer publishes enters this LAN's lobby.
///
/// The connection counts the inner behaviour sees are rewritten to count LAN
/// connections only, so gossipsub's per-peer bookkeeping stays consistent
/// when a peer has a LAN connection and a hidden one at the same time.
pub struct LanOnly<B> {
    inner: B,
    lan: HashSet<ConnectionId>,
    per_peer: HashMap<PeerId, usize>,
    subnets: Option<(Instant, Vec<(IpAddr, IpAddr)>)>,
}

impl<B> LanOnly<B> {
    pub fn new(inner: B) -> Self {
        Self { inner, lan: HashSet::new(), per_peer: HashMap::new(), subnets: None }
    }

    pub fn inner_mut(&mut self) -> &mut B {
        &mut self.inner
    }

    fn is_lan(&mut self, local: Option<&Multiaddr>, remote: &Multiaddr) -> bool {
        if is_relayed(local, remote) {
            return false;
        }
        let Some(ip) = crate::addr_ip(remote) else { return false };
        // Interface subnets change rarely; re-read them at most every 30s.
        let fresh = self.subnets.as_ref().is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(30));
        if !fresh {
            self.subnets = Some((Instant::now(), crate::local_subnets()));
        }
        lan_ip(ip, &self.subnets.as_ref().unwrap().1)
    }
}

impl<B: NetworkBehaviour> NetworkBehaviour for LanOnly<B> {
    type ConnectionHandler = Either<THandler<B>, dummy::ConnectionHandler>;
    type ToSwarm = B::ToSwarm;

    fn handle_pending_inbound_connection(
        &mut self,
        id: ConnectionId,
        local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.inner.handle_pending_inbound_connection(id, local, remote)
    }

    fn handle_established_inbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        if !self.is_lan(Some(local), remote) {
            return Ok(Either::Right(dummy::ConnectionHandler));
        }
        let h = self.inner.handle_established_inbound_connection(id, peer, local, remote)?;
        self.lan.insert(id);
        Ok(Either::Left(h))
    }

    fn handle_pending_outbound_connection(
        &mut self,
        id: ConnectionId,
        peer: Option<PeerId>,
        addrs: &[Multiaddr],
        role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        self.inner.handle_pending_outbound_connection(id, peer, addrs, role)
    }

    fn handle_established_outbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        if !self.is_lan(None, addr) {
            return Ok(Either::Right(dummy::ConnectionHandler));
        }
        let h = self.inner.handle_established_outbound_connection(id, peer, addr, role, port_use)?;
        self.lan.insert(id);
        Ok(Either::Left(h))
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(e) => {
                if !self.lan.contains(&e.connection_id) {
                    return;
                }
                let n = self.per_peer.entry(e.peer_id).or_insert(0);
                let other = *n;
                *n += 1;
                self.inner.on_swarm_event(FromSwarm::ConnectionEstablished(
                    ConnectionEstablished { other_established: other, ..e },
                ));
            }
            FromSwarm::ConnectionClosed(e) => {
                if !self.lan.remove(&e.connection_id) {
                    return;
                }
                let remaining = match self.per_peer.get_mut(&e.peer_id) {
                    Some(n) => {
                        *n = n.saturating_sub(1);
                        *n
                    }
                    None => 0,
                };
                if remaining == 0 {
                    self.per_peer.remove(&e.peer_id);
                }
                self.inner.on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
                    remaining_established: remaining,
                    ..e
                }));
            }
            FromSwarm::AddressChange(e) => {
                if self.lan.contains(&e.connection_id) {
                    self.inner.on_swarm_event(event);
                }
            }
            // A connection a later behaviour refused never establishes.
            FromSwarm::DialFailure(e) => {
                self.lan.remove(&e.connection_id);
                self.inner.on_swarm_event(event);
            }
            FromSwarm::ListenFailure(e) => {
                self.lan.remove(&e.connection_id);
                self.inner.on_swarm_event(event);
            }
            other => self.inner.on_swarm_event(other),
        }
    }

    fn on_connection_handler_event(
        &mut self,
        peer: PeerId,
        id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {
            Either::Left(ev) => self.inner.on_connection_handler_event(peer, id, ev),
            Either::Right(never) => match never {},
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        // Gossipsub always addresses one connection it was told about
        // (`NotifyHandler::One`), i.e. a LAN connection with a real handler.
        self.inner.poll(cx).map(|ev| ev.map_in(Either::Left))
    }
}

/// Inbound connections allowed per remote IP. Loopback is exempt (several
/// REAPER instances on one machine, the in-process tests); relayed
/// connections carry no IP and are bounded by the connection limits instead.
pub(crate) const MAX_INBOUND_PER_IP: usize = 16;

/// Per-IP cap on inbound connections (libp2p's connection limits only count
/// per peer id, which a dialer mints for free).
#[derive(Default)]
pub struct IpLimits {
    per_ip: HashMap<IpAddr, usize>,
    conns: HashMap<ConnectionId, IpAddr>,
}

impl IpLimits {
    fn limited_ip(remote: &Multiaddr) -> Option<IpAddr> {
        crate::addr_ip(remote).filter(|ip| !ip.is_loopback())
    }

    fn full(&self, ip: &IpAddr) -> bool {
        self.per_ip.get(ip).copied().unwrap_or(0) >= MAX_INBOUND_PER_IP
    }
}

#[derive(Debug)]
struct TooManyFromIp;

impl std::fmt::Display for TooManyFromIp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "too many connections from one address")
    }
}

impl std::error::Error for TooManyFromIp {}

impl NetworkBehaviour for IpLimits {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = Infallible;

    fn handle_pending_inbound_connection(
        &mut self,
        _id: ConnectionId,
        _local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        match Self::limited_ip(remote) {
            Some(ip) if self.full(&ip) => Err(ConnectionDenied::new(TooManyFromIp)),
            _ => Ok(()),
        }
    }

    fn handle_established_inbound_connection(
        &mut self,
        id: ConnectionId,
        _peer: PeerId,
        _local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        if let Some(ip) = Self::limited_ip(remote) {
            if self.full(&ip) {
                return Err(ConnectionDenied::new(TooManyFromIp));
            }
            *self.per_ip.entry(ip).or_insert(0) += 1;
            self.conns.insert(id, ip);
        }
        Ok(dummy::ConnectionHandler)
    }

    fn handle_established_outbound_connection(
        &mut self,
        _id: ConnectionId,
        _peer: PeerId,
        _addr: &Multiaddr,
        _role: Endpoint,
        _port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        let id = match event {
            FromSwarm::ConnectionClosed(e) => e.connection_id,
            FromSwarm::ListenFailure(e) => e.connection_id,
            _ => return,
        };
        if let Some(ip) = self.conns.remove(&id)
            && let Some(n) = self.per_ip.get_mut(&ip)
        {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.per_ip.remove(&ip);
            }
        }
    }

    fn on_connection_handler_event(&mut self, _: PeerId, _: ConnectionId, never: Infallible) {
        match never {}
    }

    fn poll(&mut self, _: &mut Context<'_>) -> Poll<ToSwarm<Infallible, Infallible>> {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str = "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN";
    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn ticket_param_round_trips_without_the_key() {
        let c = parse_relay_code(&format!(
            "reamesh://relay/{PEER}?a=203.0.113.5&d=relay.example.org&q=4002&k={KEY}"
        ))
        .unwrap();
        let p = ticket_param(&c.addr);
        assert!(!p.contains(KEY));
        assert_eq!(p, format!("{PEER}~203.0.113.5~~4002~relay.example.org"));
        assert_eq!(from_ticket_param(&p), Some(c.addr.clone()));
        let no_dns = RelayAddr { dns: String::new(), ..c.addr.clone() };
        assert_eq!(from_ticket_param(&ticket_param(&no_dns)), Some(no_dns));
        assert!(from_ticket_param(&format!("{PEER}~~~4002")).is_none());
        assert!(from_ticket_param(&format!("{PEER}~203.0.113.5~~")).is_none());
        assert!(from_ticket_param("garbage").is_none());
    }

    #[test]
    fn set_relays_keeps_valid_unique_codes_in_order() {
        let a = format!("reamesh://relay/{PEER}?a=203.0.113.5&t=4001&k={KEY}");
        let other = PeerId::random();
        let b = format!("reamesh://relay/{other}?a=203.0.113.6&t=4001&k={KEY}");
        let n = set_relays(&[a.clone(), "nonsense".into(), a.clone(), b]);
        assert_eq!(n, 2);
        let got = configured_relays();
        assert_eq!(got[0].addr.peer.to_string(), PEER);
        assert_eq!(got[1].addr.peer, other);
        set_relays(&[]);
        assert!(configured_relays().is_empty());
    }

    #[test]
    fn lan_ips_are_private_loopback_or_local_subnet() {
        let none: Vec<(IpAddr, IpAddr)> = Vec::new();
        for ip in ["192.168.1.4", "10.0.0.2", "172.20.1.1", "127.0.0.1", "169.254.3.3", "fd00::1", "fe80::1", "::1"] {
            assert!(lan_ip(ip.parse().unwrap(), &none), "{ip}");
        }
        for ip in ["203.0.113.5", "100.100.1.1", "8.8.8.8", "2001:db8::1"] {
            assert!(!lan_ip(ip.parse().unwrap(), &none), "{ip}");
        }
        let campus = vec![("130.1.2.3".parse().unwrap(), "255.255.0.0".parse().unwrap())];
        assert!(lan_ip("130.1.9.9".parse().unwrap(), &campus));
        let relayed: Multiaddr = format!("/ip4/203.0.113.5/tcp/4001/p2p/{PEER}/p2p-circuit").parse().unwrap();
        let src: Multiaddr = format!("/p2p/{PEER}").parse().unwrap();
        assert!(is_relayed(None, &relayed));
        assert!(is_relayed(Some(&relayed), &src));
        assert!(!is_relayed(None, &"/ip4/192.168.1.4/tcp/1".parse().unwrap()));
    }
}
