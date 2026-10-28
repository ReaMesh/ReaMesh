//! The running relay: one libp2p swarm with the circuit relay server, the
//! auth protocol, identify, ping and AutoNAT servers.
//!
//! Reservation gating: libp2p-relay asks every `reservation_rate_limiters`
//! entry before it accepts a reservation (new or renewed). The first one is
//! ours and only says yes for a peer the auth handler authorized, which makes
//! the relay private: only key holders become hosts, and guests (no key) can
//! only open circuits to those hosts. A second hook on `circuit_src_rate_limiters`
//! enforces the circuit totals that can change at runtime.
//!
//! The per-circuit byte and time limits are copied into every connection
//! handler when the connection opens, so a mode switch (budget used up, a new
//! month, a config change to those limits or the ports) rebuilds the swarm:
//! everyone reconnects once, and hosts reserve again within the auth grace.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context as _;
use futures::StreamExt;
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::Transport as _;
use libp2p::core::upgrade;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{
    Multiaddr, PeerId, Swarm, autonat, connection_limits, identify, noise, ping, quic, relay, tcp,
    yamux,
};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::addrs;
use crate::auth::AuthState;
use crate::config::{self, Config, Mode};
use crate::proto::{self, AuthRequest, AuthResponse, RelayCode};
use crate::status::{HostStatus, Status};
use crate::traffic::{Counters, CountingMuxer, Ledger};

/// Per-circuit limits in coordination mode: plenty for identify and a DCUtR
/// hole punch, useless for bulk transfer.
pub const COORDINATION_CIRCUIT_BYTES: u64 = 128 * 1024;
pub const COORDINATION_CIRCUIT_DURATION: Duration = Duration::from_secs(2 * 60);

const STATUS_EVERY: Duration = Duration::from_secs(5);
const LEDGER_EVERY: Duration = Duration::from_secs(30);
/// Caps how many addresses hosts can add to the relay code by reporting
/// where they see the relay.
const MAX_OBSERVED: usize = 4;

pub struct Options {
    pub data_dir: PathBuf,
    /// How often config.toml is checked for changes.
    pub reload_poll: Duration,
    /// How often traffic is added up (and the budget checked).
    pub tick: Duration,
    /// Print the relay codes on start (the CLI does; tests don't).
    pub print_codes: bool,
}

impl Options {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            reload_poll: Duration::from_secs(2),
            tick: Duration::from_secs(1),
            print_codes: true,
        }
    }
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    limits: connection_limits::Behaviour,
    relay: relay::Behaviour,
    auth: request_response::cbor::Behaviour<AuthRequest, AuthResponse>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    autonat: autonat::v1::Behaviour,
    autonat_v2: autonat::v2::server::Behaviour,
}

/// State the relay's limiter hooks read, shared with the event loop.
#[derive(Debug, Default)]
struct Gate {
    auth: AuthState,
    /// Active circuits: (source, destination, source IP).
    circuits: Vec<(PeerId, PeerId, Option<IpAddr>)>,
    max_circuits: usize,
    max_circuits_per_ip: usize,
}

impl Gate {
    fn may_open_circuit(&self, src: &PeerId, ip: Option<IpAddr>) -> bool {
        if self.auth.banned.contains(src) || self.circuits.len() >= self.max_circuits {
            return false;
        }
        match ip {
            Some(ip) => {
                self.circuits.iter().filter(|c| c.2 == Some(ip)).count() < self.max_circuits_per_ip
            }
            None => true,
        }
    }

    fn apply(&mut self, cfg: &Config) {
        self.auth.max_hosts = cfg.limits.max_hosts;
        self.auth.banned = cfg.banned().unwrap_or_default();
        self.max_circuits = cfg.limits.max_circuits;
        self.max_circuits_per_ip = cfg.limits.max_circuits_per_ip;
    }
}

/// What the swarm is built from; any change rebuilds it.
#[derive(Debug, Clone, PartialEq)]
struct SwarmParams {
    mode: Mode,
    tcp_port: u16,
    quic_port: u16,
    listen_addrs: Vec<IpAddr>,
    circuit_duration: Duration,
    circuit_bytes: u64,
    max_circuits_per_host: usize,
}

impl SwarmParams {
    fn new(cfg: &Config, mode: Mode) -> Self {
        let (circuit_duration, circuit_bytes) = match mode {
            Mode::Full => (
                Duration::from_secs(cfg.limits.circuit_max_minutes * 60),
                cfg.limits.circuit_max_mb.saturating_mul(1024 * 1024),
            ),
            Mode::Coordination => (COORDINATION_CIRCUIT_DURATION, COORDINATION_CIRCUIT_BYTES),
        };
        Self {
            mode,
            tcp_port: cfg.tcp_port,
            quic_port: cfg.quic_port,
            listen_addrs: cfg.listen_addrs.clone(),
            circuit_duration,
            circuit_bytes,
            max_circuits_per_host: cfg.limits.max_circuits_per_host,
        }
    }
}

pub struct RelayHandle {
    pub peer_id: PeerId,
    listen: watch::Receiver<Vec<Multiaddr>>,
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl RelayHandle {
    /// The current listen addresses, once there is at least one TCP and one
    /// QUIC address.
    pub async fn listen_addrs(&mut self) -> Vec<Multiaddr> {
        let ready = |v: &Vec<Multiaddr>| {
            v.iter()
                .any(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_))))
                && v.iter()
                    .any(|a| a.iter().any(|p| matches!(p, Protocol::QuicV1)))
        };
        self.listen
            .wait_for(ready)
            .await
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Waits for listen addresses that differ from `old` (after a rebuild).
    pub async fn listen_addrs_changed(&mut self, old: &[Multiaddr]) -> Vec<Multiaddr> {
        let _ = self
            .listen
            .wait_for(|v| !v.is_empty() && v.as_slice() != old)
            .await;
        self.listen_addrs().await
    }

    pub async fn stop(self) -> anyhow::Result<()> {
        let _ = self.shutdown.send(true);
        self.task.await.context("relay task panicked")?
    }

    /// Resolves when the relay stops by itself (an error).
    pub async fn join(self) -> anyhow::Result<()> {
        self.task.await.context("relay task panicked")?
    }

    pub fn shutdown_sender(&self) -> watch::Sender<bool> {
        self.shutdown.clone()
    }
}

/// Starts the relay (creating the data dir on first start) on the current
/// Tokio runtime.
pub fn spawn(opts: Options) -> anyhow::Result<RelayHandle> {
    config::init(&opts.data_dir)?;
    let key = config::load_identity(&opts.data_dir)?;
    let cfg = Config::load(&opts.data_dir)?;
    let peer_id = key.public().to_peer_id();
    let (listen_tx, listen) = watch::channel(Vec::new());
    let (shutdown, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run(opts, key, cfg, listen_tx, shutdown_rx));
    Ok(RelayHandle {
        peer_id,
        listen,
        shutdown,
        task,
    })
}

enum Outcome {
    Rebuild,
    Shutdown,
}

struct Relay {
    opts: Options,
    key: Keypair,
    peer_id: PeerId,
    cfg: Config,
    cfg_stamp: Option<(SystemTime, u64)>,
    gate: Arc<Mutex<Gate>>,
    counters: Arc<Counters>,
    ledger: Ledger,
    observed: Vec<IpAddr>,
    /// Open direct connections per peer, and the IP of the newest one.
    peers: HashMap<PeerId, (usize, Option<IpAddr>)>,
    bound_tcp: Option<u16>,
    bound_quic: Option<u16>,
    listen_tx: watch::Sender<Vec<Multiaddr>>,
    started: SystemTime,
}

async fn run(
    opts: Options,
    key: Keypair,
    cfg: Config,
    listen_tx: watch::Sender<Vec<Multiaddr>>,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let gate = Arc::new(Mutex::new(Gate::default()));
    gate.lock().unwrap().apply(&cfg);
    let ledger = Ledger::load(&opts.data_dir.join(config::TRAFFIC_FILE));
    let mut relay = Relay {
        peer_id: key.public().to_peer_id(),
        cfg_stamp: config_stamp(&opts.data_dir),
        key,
        cfg,
        gate,
        counters: Arc::default(),
        ledger,
        observed: Vec::new(),
        peers: HashMap::new(),
        bound_tcp: None,
        bound_quic: None,
        listen_tx,
        started: SystemTime::now(),
        opts,
    };
    if relay.opts.print_codes {
        relay.print_codes(None);
    }
    #[cfg(unix)]
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .context("installing the SIGHUP handler")?;
    #[cfg(not(unix))]
    let mut hangup = ();
    loop {
        let params = relay.params();
        info!(mode = ?params.mode, "starting the relay");
        let mut swarm = relay.build_swarm(&params)?;
        for ip in &params.listen_addrs {
            let ip_proto = match ip {
                IpAddr::V4(v4) => Protocol::Ip4(*v4),
                IpAddr::V6(v6) => Protocol::Ip6(*v6),
            };
            let base = Multiaddr::empty().with(ip_proto);
            for addr in [
                base.clone().with(Protocol::Tcp(params.tcp_port)),
                base.with(Protocol::Udp(params.quic_port))
                    .with(Protocol::QuicV1),
            ] {
                if let Err(e) = swarm.listen_on(addr.clone()) {
                    warn!("cannot listen on {addr}: {e}");
                }
            }
        }
        let outcome = relay
            .drive(&mut swarm, &params, &mut shutdown, &mut hangup)
            .await;
        relay
            .gate
            .lock()
            .unwrap()
            .auth
            .all_disconnected(Instant::now());
        relay.gate.lock().unwrap().circuits.clear();
        relay.peers.clear();
        relay.bound_tcp = None;
        relay.bound_quic = None;
        let _ = relay.listen_tx.send(Vec::new());
        drop(swarm);
        match outcome {
            Outcome::Shutdown => break,
            // Give the old sockets a moment to close before binding again.
            Outcome::Rebuild => tokio::time::sleep(Duration::from_millis(300)).await,
        }
    }
    relay.save_ledger();
    relay.write_status(false, &[]);
    info!("relay stopped");
    Ok(())
}

impl Relay {
    fn params(&self) -> SwarmParams {
        SwarmParams::new(
            &self.cfg,
            self.ledger
                .effective_mode(self.cfg.mode, self.cfg.budget_bytes()),
        )
    }

    fn build_swarm(&self, params: &SwarmParams) -> anyhow::Result<Swarm<Behaviour>> {
        let counters = self.counters.clone();
        let reserve_gate = self.gate.clone();
        let circuit_gate = self.gate.clone();
        let relay_config = relay::Config {
            // Hosts are counted (and limited) by the gate; these only bound
            // what one peer can hold.
            max_reservations: 4096,
            max_reservations_per_peer: 4,
            reservation_duration: Duration::from_secs(60 * 60),
            reservation_rate_limiters: vec![Box::new(
                move |peer: PeerId, _: &Multiaddr, _: Instant| {
                    reserve_gate
                        .lock()
                        .unwrap()
                        .auth
                        .may_reserve(&peer, Instant::now())
                },
            )],
            max_circuits: 4096,
            max_circuits_per_peer: params.max_circuits_per_host,
            max_circuit_duration: params.circuit_duration,
            max_circuit_bytes: params.circuit_bytes,
            circuit_src_rate_limiters: vec![Box::new(
                move |peer: PeerId, addr: &Multiaddr, _: Instant| {
                    circuit_gate
                        .lock()
                        .unwrap()
                        .may_open_circuit(&peer, ip_of(addr))
                },
            )],
        }
        // libp2p's default request rates, against floods.
        .reservation_rate_per_peer(NonZeroU32::new(30).unwrap(), Duration::from_secs(120))
        .reservation_rate_per_ip(NonZeroU32::new(60).unwrap(), Duration::from_secs(60))
        .circuit_src_per_peer(NonZeroU32::new(30).unwrap(), Duration::from_secs(120))
        .circuit_src_per_ip(NonZeroU32::new(60).unwrap(), Duration::from_secs(60));

        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(self.key.clone())
            .with_tokio()
            .with_other_transport(
                |key| -> Result<_, Box<dyn std::error::Error + Send + Sync>> {
                    let tcp = tcp::tokio::Transport::new(tcp::Config::default().nodelay(true))
                        .upgrade(upgrade::Version::V1Lazy)
                        .authenticate(noise::Config::new(key)?)
                        .multiplex(yamux::Config::default())
                        .map(|(p, m), _| (p, StreamMuxerBox::new(m)));
                    let quic = quic::tokio::Transport::new(quic::Config::new(key))
                        .map(|(p, m), _| (p, StreamMuxerBox::new(m)));
                    Ok(quic.or_transport(tcp).map(move |either, _| {
                        let (p, m) = either.into_inner();
                        (
                            p,
                            StreamMuxerBox::new(CountingMuxer::new(m, counters.clone())),
                        )
                    }))
                },
            )
            .map_err(|e| anyhow::anyhow!("transport: {e}"))?
            .with_behaviour(|key| {
                let peer_id = key.public().to_peer_id();
                Behaviour {
                    // Anyone may connect (guests need no key); bound how many.
                    limits: connection_limits::Behaviour::new(
                        connection_limits::ConnectionLimits::default()
                            .with_max_pending_incoming(Some(256))
                            .with_max_established_incoming(Some(2048))
                            .with_max_established_per_peer(Some(8)),
                    ),
                    relay: relay::Behaviour::new(peer_id, relay_config),
                    auth: request_response::cbor::Behaviour::new(
                        [(proto::PROTOCOL, ProtocolSupport::Inbound)],
                        request_response::Config::default()
                            .with_request_timeout(Duration::from_secs(20)),
                    ),
                    identify: identify::Behaviour::new(
                        identify::Config::new("/reamesh-relay/1".into(), key.public())
                            .with_agent_version(format!(
                                "reamesh-relay/{}",
                                env!("CARGO_PKG_VERSION")
                            )),
                    ),
                    ping: ping::Behaviour::default(),
                    autonat: autonat::v1::Behaviour::new(
                        peer_id,
                        autonat::v1::Config {
                            // Server only: never probe our own reachability.
                            use_connected: false,
                            only_global_ips: true,
                            ..Default::default()
                        },
                    ),
                    autonat_v2: autonat::v2::server::Behaviour::default(),
                }
            })
            .map_err(|e| anyhow::anyhow!("behaviour: {e}"))?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
            .build();
        // libp2p only advertises the hop protocol once an external address is
        // confirmed, and nothing confirms one here (the relay never probes its
        // own reachability).
        swarm
            .behaviour_mut()
            .relay
            .set_status(Some(relay::Status::Enable));
        Ok(swarm)
    }

    async fn drive(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        params: &SwarmParams,
        shutdown: &mut watch::Receiver<bool>,
        hangup: &mut Hangup,
    ) -> Outcome {
        let mut tick = tokio::time::interval(self.opts.tick);
        let mut reload = tokio::time::interval(self.opts.reload_poll);
        let (mut last_status, mut last_ledger) = (Instant::now() - STATUS_EVERY, Instant::now());
        let mut listen: Vec<Multiaddr> = Vec::new();
        loop {
            tokio::select! {
                _ = shutdown.wait_for(|stop| *stop) => {
                    self.account();
                    return Outcome::Shutdown;
                }
                event = swarm.select_next_some() => {
                    self.on_event(swarm, event, &mut listen);
                }
                _ = tick.tick() => {
                    self.account();
                    let now = Instant::now();
                    self.gate.lock().unwrap().auth.purge(now);
                    if now.duration_since(last_ledger) >= LEDGER_EVERY {
                        self.save_ledger();
                        last_ledger = now;
                    }
                    if now.duration_since(last_status) >= STATUS_EVERY {
                        self.write_status(true, &listen);
                        last_status = now;
                    }
                    if self.params() != *params {
                        self.announce_mode_change(params.mode);
                        return Outcome::Rebuild;
                    }
                }
                _ = reload.tick() => {
                    if config_stamp(&self.opts.data_dir) != self.cfg_stamp && self.reload(swarm) {
                        return Outcome::Rebuild;
                    }
                }
                _ = hangup_recv(hangup) => {
                    info!("SIGHUP: reloading the config");
                    if self.reload(swarm) {
                        return Outcome::Rebuild;
                    }
                }
            }
        }
    }

    fn account(&mut self) {
        let (sent, received) = self.counters.take();
        if self.ledger.roll(&crate::traffic::current_month()) {
            info!("new month: traffic counter reset");
        }
        self.ledger.add(sent, received);
    }

    fn announce_mode_change(&self, old: Mode) {
        let new = self.params().mode;
        if new == old {
            info!("relay limits changed, reconnecting everyone");
        } else if new == Mode::Coordination && self.cfg.mode == Mode::Full {
            warn!(
                "monthly traffic budget used up ({} GB): coordination mode until the month ends",
                self.cfg.limits.monthly_budget_gb
            );
        } else {
            info!(mode = ?new, "switching relay mode");
        }
    }

    /// Re-reads config.toml; keys, bans and the gate limits apply at once.
    /// Returns whether the swarm must be rebuilt for the rest.
    fn reload(&mut self, swarm: &mut Swarm<Behaviour>) -> bool {
        self.cfg_stamp = config_stamp(&self.opts.data_dir);
        let cfg = match Config::load(&self.opts.data_dir) {
            Ok(cfg) => cfg,
            Err(e) => {
                warn!("config not reloaded, keeping the previous one: {e:#}");
                return false;
            }
        };
        if cfg == self.cfg {
            return false;
        }
        let before = self.params();
        let old_labels: HashSet<String> = self.cfg.keys.iter().map(|k| k.label.clone()).collect();
        self.cfg = cfg;
        let labels: HashSet<String> = self.cfg.keys.iter().map(|k| k.label.clone()).collect();
        let revoked = {
            let mut gate = self.gate.lock().unwrap();
            gate.apply(&self.cfg);
            gate.auth.revoke(&labels)
        };
        for peer in &revoked {
            let _ = swarm.disconnect_peer_id(*peer);
        }
        info!(
            keys = labels.len(),
            hosts_revoked = revoked.len(),
            "config reloaded"
        );
        let added: Vec<&String> = labels.difference(&old_labels).collect();
        if self.opts.print_codes && !added.is_empty() {
            for label in added {
                self.print_codes(Some(label));
            }
        }
        if self.params() != before {
            self.announce_mode_change(before.mode);
            return true;
        }
        false
    }

    fn on_event(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        event: SwarmEvent<BehaviourEvent>,
        listen: &mut Vec<Multiaddr>,
    ) {
        let log_addrs = self.cfg.log_addresses;
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                for p in address.iter() {
                    match p {
                        Protocol::Tcp(port) => self.bound_tcp = Some(port),
                        Protocol::Udp(port) => self.bound_quic = Some(port),
                        _ => {}
                    }
                }
                info!("listening on {address}");
                listen.push(address);
                let _ = self.listen_tx.send(listen.clone());
                self.refresh_external(swarm);
            }
            SwarmEvent::ExpiredListenAddr { address, .. } => {
                listen.retain(|a| *a != address);
                let _ = self.listen_tx.send(listen.clone());
            }
            SwarmEvent::ListenerError { error, .. } => warn!("listener error: {error}"),
            SwarmEvent::ListenerClosed {
                addresses,
                reason: Err(e),
                ..
            } => {
                warn!("stopped listening on {addresses:?}: {e}")
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                endpoint,
                num_established,
                ..
            } => {
                let addr = endpoint.get_remote_address();
                let ip = (!endpoint.is_relayed()).then(|| ip_of(addr)).flatten();
                let entry = self.peers.entry(peer_id).or_insert((0, None));
                entry.0 = num_established.get() as usize;
                if ip.is_some() {
                    entry.1 = ip;
                }
                self.gate.lock().unwrap().auth.connection_count(
                    &peer_id,
                    num_established.get() as usize,
                    Instant::now(),
                );
                if log_addrs {
                    info!("connection from {addr}");
                } else {
                    debug!(peers = self.peers.len(), "connection opened");
                }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established,
                ..
            } => {
                let n = num_established as usize;
                if n == 0 {
                    self.peers.remove(&peer_id);
                } else if let Some(e) = self.peers.get_mut(&peer_id) {
                    e.0 = n;
                }
                let mut gate = self.gate.lock().unwrap();
                let was_host = gate.auth.reserved.contains(&peer_id);
                gate.auth.connection_count(&peer_id, n, Instant::now());
                if was_host && n == 0 {
                    info!(hosts = gate.auth.reserved.len(), "host disconnected");
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Auth(request_response::Event::Message {
                peer,
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
                ..
            })) => {
                let response = self.authenticate(&peer, &request);
                let _ = swarm.behaviour_mut().auth.send_response(channel, response);
            }
            SwarmEvent::Behaviour(BehaviourEvent::Relay(e)) => self.on_relay_event(e),
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                // Only hosts are trusted to tell us our own public address.
                let trusted = self
                    .gate
                    .lock()
                    .unwrap()
                    .auth
                    .is_authorized(&peer_id, Instant::now());
                if let Some(ip) = ip_of(&info.observed_addr)
                    && trusted
                    && addrs::is_public(&ip)
                    && !self.observed.contains(&ip)
                    && self.observed.len() < MAX_OBSERVED
                {
                    self.observed.push(ip);
                    info!("public address seen by a host: {ip}");
                    self.refresh_external(swarm);
                }
            }
            _ => {}
        }
    }

    fn authenticate(&mut self, peer: &PeerId, req: &AuthRequest) -> AuthResponse {
        let keys = self.cfg.keys();
        let label = proto::verify(
            keys.iter().map(|(l, k)| (l.as_str(), k)),
            &self.peer_id,
            peer,
            &req.proof,
        );
        let mut gate = self.gate.lock().unwrap();
        let response = if gate.auth.banned.contains(peer) {
            AuthResponse::refused(proto::REASON_BANNED)
        } else if let Some(label) = label {
            if gate.auth.is_full(peer) {
                AuthResponse::refused(proto::REASON_FULL)
            } else {
                let conns = self.peers.get(peer).map_or(1, |e| e.0);
                gate.auth.authorize(*peer, label, conns);
                AuthResponse::accepted(label)
            }
        } else {
            AuthResponse::refused(proto::REASON_BAD_KEY)
        };
        match &response.reason {
            None => info!(key = label.unwrap_or_default(), "host authorized"),
            Some(r) => info!(reason = r.as_str(), "host refused"),
        }
        response
    }

    fn on_relay_event(&mut self, e: relay::Event) {
        let mut gate = self.gate.lock().unwrap();
        match e {
            relay::Event::ReservationReqAccepted {
                src_peer_id,
                renewed,
            } => {
                gate.auth.reserved.insert(src_peer_id);
                if !renewed {
                    let label = gate.auth.label(&src_peer_id).unwrap_or("?").to_owned();
                    info!(
                        key = label.as_str(),
                        hosts = gate.auth.reserved.len(),
                        "host reserved"
                    );
                }
            }
            relay::Event::ReservationReqDenied { src_peer_id, .. } => {
                let why = if !gate.auth.is_authorized(&src_peer_id, Instant::now()) {
                    "not authorized"
                } else if gate.auth.is_full(&src_peer_id) {
                    "relay full"
                } else {
                    "rate limit"
                };
                info!(reason = why, "reservation refused");
            }
            relay::Event::ReservationClosed { src_peer_id }
            | relay::Event::ReservationTimedOut { src_peer_id } => {
                if gate.auth.reserved.remove(&src_peer_id) {
                    debug!(hosts = gate.auth.reserved.len(), "reservation ended");
                }
            }
            relay::Event::CircuitReqAccepted {
                src_peer_id,
                dst_peer_id,
            } => {
                let ip = self.peers.get(&src_peer_id).and_then(|e| e.1);
                gate.circuits.push((src_peer_id, dst_peer_id, ip));
                debug!(circuits = gate.circuits.len(), "circuit opened");
            }
            relay::Event::CircuitClosed {
                src_peer_id,
                dst_peer_id,
                ..
            } => {
                if let Some(i) = gate
                    .circuits
                    .iter()
                    .position(|c| c.0 == src_peer_id && c.1 == dst_peer_id)
                {
                    gate.circuits.swap_remove(i);
                }
                debug!(circuits = gate.circuits.len(), "circuit closed");
            }
            relay::Event::CircuitReqDenied { status, .. } => {
                debug!(?status, "circuit refused");
            }
            _ => {}
        }
    }

    fn advertised_ips(&self) -> Vec<IpAddr> {
        addrs::advertised(&self.cfg.public_addrs, &self.observed)
    }

    fn ports(&self) -> (u16, u16) {
        let pick = |cfg: u16, bound: Option<u16>| if cfg == 0 { bound.unwrap_or(0) } else { cfg };
        (
            pick(self.cfg.tcp_port, self.bound_tcp),
            pick(self.cfg.quic_port, self.bound_quic),
        )
    }

    /// Hands the relay's public addresses to the swarm; reservations carry
    /// them to hosts, and identify to everyone.
    fn refresh_external(&self, swarm: &mut Swarm<Behaviour>) {
        let (t, q) = self.ports();
        let mut hosts: Vec<Multiaddr> = self
            .advertised_ips()
            .iter()
            .filter(|ip| !addrs::unspecified(ip))
            .map(|ip| Multiaddr::from(*ip))
            .collect();
        if let Some(d) = &self.cfg.dns_name {
            hosts.push(Multiaddr::empty().with(Protocol::Dns(d.clone().into())));
        }
        for h in hosts {
            if t != 0 {
                swarm.add_external_address(h.clone().with(Protocol::Tcp(t)));
            }
            if q != 0 {
                swarm.add_external_address(h.with(Protocol::Udp(q)).with(Protocol::QuicV1));
            }
        }
    }

    fn code_for(&self, label: &str) -> Option<RelayCode> {
        let (tcp_port, quic_port) = self.ports();
        Some(RelayCode {
            peer_id: self.peer_id,
            ips: self.advertised_ips(),
            dns: self.cfg.dns_name.clone(),
            tcp_port,
            quic_port,
            key: self.cfg.key(label)?,
            label: label.to_owned(),
        })
    }

    fn print_codes(&self, only: Option<&str>) {
        let labels: Vec<String> = match only {
            Some(l) => vec![l.to_owned()],
            None => self.cfg.keys.iter().map(|k| k.label.clone()).collect(),
        };
        let codes: Vec<RelayCode> = labels.iter().filter_map(|l| self.code_for(l)).collect();
        print_code_banner(&codes, &self.opts.data_dir);
    }

    fn save_ledger(&self) {
        if let Err(e) = self.ledger.save() {
            warn!("cannot save the traffic counter: {e}");
        }
    }

    fn write_status(&self, running: bool, listen: &[Multiaddr]) {
        let gate = self.gate.lock().unwrap();
        let status = Status {
            running,
            pid: std::process::id(),
            updated: unix_now(),
            started: self
                .started
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            peer_id: self.peer_id.to_string(),
            mode: self.params().mode,
            configured_mode: self.cfg.mode,
            budget_exhausted: self.ledger.exhausted(self.cfg.budget_bytes()),
            month: self.ledger.month.clone(),
            sent: self.ledger.sent,
            received: self.ledger.received,
            budget: self.cfg.budget_bytes(),
            hosts: gate
                .auth
                .hosts()
                .into_iter()
                .map(|(p, label)| HostStatus {
                    label,
                    peer_id: p.to_string(),
                })
                .collect(),
            circuits: gate.circuits.len(),
            peers: self.peers.len(),
            observed_ips: self.observed.clone(),
            listening: listen.iter().map(|a| a.to_string()).collect(),
        };
        drop(gate);
        if let Err(e) = status.write(&self.opts.data_dir) {
            warn!("cannot write status.json: {e}");
        }
    }
}

/// Prints relay codes in a box that stands out in a terminal or `docker logs`.
pub fn print_code_banner(codes: &[RelayCode], data_dir: &Path) {
    let rule = "=".repeat(72);
    println!("\n{rule}");
    if codes.is_empty() {
        println!("  This relay has no access keys. Add one: reamesh-relay key add <label>");
    }
    let mut no_addr = false;
    for code in codes {
        println!("  Relay code for key \"{}\":\n", code.label);
        println!("  {}\n", code.encode());
        no_addr |= code.ips.is_empty() && code.dns.is_none();
    }
    if !codes.is_empty() {
        println!("  Paste it in ReaMesh under Settings > Relay. Whoever has it can host");
        println!("  sessions through this relay, so share it only within your circle.");
    }
    if no_addr {
        println!();
        println!("  WARNING: no public address found. Set public_addrs or dns_name in");
        println!(
            "  {} (the relay picks it up by itself),",
            data_dir.join(config::CONFIG_FILE).display()
        );
        println!("  then print the code again with: reamesh-relay code");
    }
    println!("{rule}\n");
}

#[cfg(unix)]
type Hangup = tokio::signal::unix::Signal;
#[cfg(not(unix))]
type Hangup = ();

/// Resolves on SIGHUP (never on Windows, which has no SIGHUP; the config
/// file watch covers it).
async fn hangup_recv(_h: &mut Hangup) {
    #[cfg(unix)]
    if _h.recv().await.is_some() {
        return;
    }
    futures::future::pending::<()>().await
}

fn ip_of(addr: &Multiaddr) -> Option<IpAddr> {
    addr.iter().find_map(|p| match p {
        Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
        Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
        _ => None,
    })
}

fn config_stamp(dir: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(dir.join(config::CONFIG_FILE)).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
