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
//! libp2p's own per-peer circuit limit counts the circuits of the peer that
//! asks (the guest), and no hook sees the destination before a circuit is
//! accepted, so the per-host limit is enforced afterwards: a guest over it is
//! disconnected right away.
//!
//! The per-circuit byte and time limits are copied into every connection
//! handler when the connection opens, so a mode switch (budget used up, a new
//! month, a config change to those limits or the ports) rebuilds the swarm:
//! everyone reconnects once, and hosts reserve again within the auth grace.
//!
//! Key holders can ask how the relay stands (`/reamesh/relay-info/1`). A request
//! carrying the `rev` of the last answer is held until the answer changes, so
//! hosts hear about a notice, a used-up budget or a shutdown right away without
//! the relay ever opening anything towards them.
//!
//! The optional session list (`list.rs`) answers two more request-response
//! protocols. It lives in memory and hangs on the reservations: when one ends,
//! the host's entry goes with it.

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
use libp2p::request_response::{self, ProtocolSupport, ResponseChannel};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, Swarm, autonat, connection_limits, identify, noise, ping,
    quic, relay, tcp, yamux,
};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::addrs;
use crate::alert::{self, HostsWatch};
use crate::auth::AuthState;
use crate::config::{self, Config, Mode};
use crate::list::{Access, SessionList};
use crate::seen::Seen;
use crate::status::{self, HostStatus, Inbound, KeyUsage, Status};
use crate::term::{self, Style, problem};
use crate::traffic::{Counters, CountingMuxer, Ledger, percent_of};
use crate::update::{self, Checked};
use reamesh_relay_proto::{
    self as proto, AuthRequest, AuthResponse, InfoRequest, InfoResponse, Notice, RelayAddr,
    RelayCode,
};
use reamesh_relay_proto::sessions::{
    PublishRequest, PublishResponse, RELAY_PUBLISH_PROTOCOL, RELAY_SESSIONS_PROTOCOL,
    SESSIONS_TIMEOUT_SECS, SessionsRequest, SessionsResponse,
};

/// Per-circuit limits in coordination mode: plenty for identify and a DCUtR
/// hole punch, useless for bulk transfer.
pub const COORDINATION_CIRCUIT_BYTES: u64 = 128 * 1024;
pub const COORDINATION_CIRCUIT_DURATION: Duration = Duration::from_secs(2 * 60);

const STATUS_EVERY: Duration = Duration::from_secs(5);
const LEDGER_EVERY: Duration = Duration::from_secs(30);
/// Caps how many addresses hosts can add to the relay code by reporting
/// where they see the relay.
const MAX_OBSERVED: usize = 4;
/// The same `limit` line at most this often, so a guest retrying against a
/// full host doesn't flood the log.
const LIMIT_LOG_EVERY: Duration = Duration::from_secs(60);
/// Room for a banner value in 80 columns, after the R and the label.
const BANNER_VALUE_WIDTH: usize = 52;
/// How long a stopping relay keeps going so hosts get to read why.
const FAREWELL: Duration = Duration::from_millis(500);
/// An `expires` date that passed longer ago than this was dealt with before;
/// no alert for it after a restart or at the start of a month.
const EXPIRED_ALERT_DAYS: i32 = 3;
/// How often the release channel is read, the first time soon after the start.
const UPDATE_CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const UPDATE_FIRST_CHECK: Duration = Duration::from_secs(60);
/// How often a running relay looks for a downloaded update to restart into.
const UPDATE_RESTART_POLL: Duration = Duration::from_secs(30);
/// Pings in a row a listed host may miss before its connection is dropped: a
/// host that vanished without a word can look connected for many minutes over
/// TCP, and its session would stay on the list that long. It comes back on its
/// own when it is still there.
const LISTED_PING_MISSES: u32 = 3;

pub struct Options {
    pub data_dir: PathBuf,
    /// How often config.toml is checked for changes.
    pub reload_poll: Duration,
    /// How often traffic is added up (and the budget checked).
    pub tick: Duration,
    /// Print the start banner (the CLI does; tests don't).
    pub banner: bool,
    /// The data dir was just created: the banner includes the relay code.
    pub first_start: bool,
    /// Read the release channel (the CLI does; tests don't). Which also decides
    /// whether a downloaded update ends the relay.
    pub updates: bool,
}

impl Options {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            reload_poll: Duration::from_secs(2),
            tick: Duration::from_secs(1),
            banner: true,
            first_start: false,
            updates: false,
        }
    }
}

/// Why a relay stopped by itself or on request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    Asked,
    /// A newer version is ready: start it (`update::start_staged`).
    ForUpdate,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    limits: connection_limits::Behaviour,
    relay: relay::Behaviour,
    auth: request_response::cbor::Behaviour<AuthRequest, AuthResponse>,
    info: request_response::cbor::Behaviour<InfoRequest, InfoResponse>,
    sessions: request_response::cbor::Behaviour<SessionsRequest, SessionsResponse>,
    publish: request_response::cbor::Behaviour<PublishRequest, PublishResponse>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    autonat: autonat::v1::Behaviour,
    autonat_v2: autonat::v2::server::Behaviour,
}

/// An active relayed connection.
#[derive(Debug)]
struct Circuit {
    src: PeerId,
    dst: PeerId,
    src_ip: Option<IpAddr>,
    opened: Instant,
    /// What its guest sent and received since, in bytes.
    bytes: u64,
}

/// An `InfoRequest` waiting for its answer to change.
struct HeldInfo {
    channel: ResponseChannel<InfoResponse>,
    label: String,
    rev: u64,
    since: Instant,
}

/// State the relay's limiter hooks read, shared with the event loop.
#[derive(Debug, Default)]
struct Gate {
    auth: AuthState,
    circuits: Vec<Circuit>,
    max_circuits: usize,
    max_circuits_per_ip: usize,
    max_circuits_per_host: usize,
}

impl Gate {
    fn may_open_circuit(&self, src: &PeerId, ip: Option<IpAddr>) -> bool {
        if self.auth.banned.contains(src) || self.circuits.len() >= self.max_circuits {
            return false;
        }
        match ip {
            Some(ip) => {
                self.circuits
                    .iter()
                    .filter(|c| c.src_ip == Some(ip))
                    .count()
                    < self.max_circuits_per_ip
            }
            None => true,
        }
    }

    fn apply(&mut self, cfg: &Config) {
        self.auth.max_hosts = cfg.limits.max_hosts;
        self.auth.banned = cfg.banned().unwrap_or_default();
        self.auth.key_max_hosts = cfg
            .keys
            .iter()
            .filter_map(|k| Some((k.label.clone(), k.max_hosts?)))
            .collect();
        self.max_circuits = cfg.limits.max_circuits;
        self.max_circuits_per_ip = cfg.limits.max_circuits_per_ip;
        self.max_circuits_per_host = cfg.limits.max_circuits_per_host;
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
    task: tokio::task::JoinHandle<anyhow::Result<Stopped>>,
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
        self.task.await.context("relay task panicked")?.map(|_| ())
    }

    /// Resolves when the relay stops.
    pub async fn join(self) -> anyhow::Result<Stopped> {
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
    // Before anything binds: a second relay on the same dir would fight the
    // first one over status.json and the ports.
    let lock = config::lock_data_dir(&opts.data_dir)?;
    let key = config::load_identity(&opts.data_dir)?;
    let cfg = Config::load(&opts.data_dir)?;
    let peer_id = key.public().to_peer_id();
    let (listen_tx, listen) = watch::channel(Vec::new());
    let (shutdown, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run(opts, lock, key, cfg, listen_tx, shutdown_rx));
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
    Update,
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
    /// Address families with a listener, for the banner: (IPv4, IPv6).
    families: (bool, bool),
    listen_tx: watch::Sender<Vec<Multiaddr>>,
    started: SystemTime,
    /// When each host's reservation began, for `status`.
    reserved_since: HashMap<PeerId, SystemTime>,
    limit_logged: HashMap<String, Instant>,
    /// Circuits accepted over the per-host limit and then dropped.
    refused: HashSet<(PeerId, PeerId)>,
    /// Swarms built so far; only the first one's failure to listen is fatal.
    builds: u32,
    /// Direct connections that came in, per transport, since the start.
    inbound_tcp: Inbound,
    inbound_quic: Inbound,
    /// The QUIC-looks-blocked warning went out (once per run).
    quic_warned: bool,
    /// Bytes sent since the last status write, for the rate.
    rate_bytes: u64,
    rate_since: Instant,
    seen: Seen,
    /// For the line when it stops: hosts that reserved, circuits opened,
    /// bytes sent.
    hosts_seen: HashSet<PeerId>,
    circuits_opened: u64,
    sent_total: u64,
    /// Requests waiting for their answer to change, one per peer.
    held_info: HashMap<PeerId, HeldInfo>,
    /// The day keys were last checked against their dates.
    expiry_day: Option<jiff::civil::Date>,
    hosts_watch: HostsWatch,
    /// A newer version the channel named, not downloaded (yet).
    update_available: Arc<Mutex<Option<String>>>,
    update_polled: Instant,
    list: SessionList<ResponseChannel<SessionsResponse>>,
    /// Pings in a row each listed host missed.
    ping_misses: HashMap<PeerId, u32>,
    _lock: std::fs::File,
}

async fn run(
    opts: Options,
    lock: std::fs::File,
    key: Keypair,
    cfg: Config,
    listen_tx: watch::Sender<Vec<Multiaddr>>,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<Stopped> {
    let gate = Arc::new(Mutex::new(Gate::default()));
    gate.lock().unwrap().apply(&cfg);
    let ledger = Ledger::load(&opts.data_dir.join(config::TRAFFIC_FILE));
    let mut relay = Relay {
        peer_id: key.public().to_peer_id(),
        cfg_stamp: config_stamp(&opts.data_dir),
        key,
        list: SessionList::new(cfg.session_list),
        ping_misses: HashMap::new(),
        cfg,
        gate,
        counters: Arc::default(),
        ledger,
        observed: Vec::new(),
        peers: HashMap::new(),
        bound_tcp: None,
        bound_quic: None,
        families: (false, false),
        listen_tx,
        started: SystemTime::now(),
        reserved_since: HashMap::new(),
        limit_logged: HashMap::new(),
        refused: HashSet::new(),
        builds: 0,
        inbound_tcp: Inbound::default(),
        inbound_quic: Inbound::default(),
        quic_warned: false,
        rate_bytes: 0,
        rate_since: Instant::now(),
        seen: Seen::load(&opts.data_dir),
        hosts_seen: HashSet::new(),
        circuits_opened: 0,
        sent_total: 0,
        held_info: HashMap::new(),
        expiry_day: None,
        hosts_watch: HostsWatch::default(),
        update_available: Arc::default(),
        update_polled: Instant::now(),
        _lock: lock,
        opts,
    };
    if relay.opts.updates {
        tokio::spawn(update_loop(
            relay.opts.data_dir.clone(),
            relay.update_available.clone(),
        ));
    }
    #[cfg(unix)]
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .context("installing the SIGHUP handler")?;
    #[cfg(not(unix))]
    let mut hangup = ();
    let stopped;
    loop {
        let params = relay.params();
        debug!(mode = ?params.mode, "starting the relay");
        let mut swarm = relay.build_swarm(&params)?;
        relay.builds += 1;
        if let Err(e) = relay.listen(&mut swarm, &params) {
            if relay.builds == 1 {
                return Err(e);
            }
            // A port changed in the config is taken: say so and keep
            // running, the next config change gets another try.
            let hint = e
                .downcast_ref::<term::Problem>()
                .map(|p| p.hint.clone())
                .unwrap_or_default();
            error!("{e} {hint}");
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
        relay.held_info.clear();
        relay.peers.clear();
        relay.reserved_since.clear();
        relay.list.all_disconnected();
        relay.ping_misses.clear();
        relay.bound_tcp = None;
        relay.bound_quic = None;
        relay.families = (false, false);
        let _ = relay.listen_tx.send(Vec::new());
        drop(swarm);
        match outcome {
            Outcome::Shutdown => {
                stopped = Stopped::Asked;
                break;
            }
            Outcome::Update => {
                stopped = Stopped::ForUpdate;
                break;
            }
            // Give the old sockets a moment to close before binding again.
            Outcome::Rebuild => tokio::time::sleep(Duration::from_millis(300)).await,
        }
    }
    relay.save_ledger();
    relay.write_status(false, &[]);
    info!("{}", relay.stop_summary());
    Ok(stopped)
}

/// Reads the release channel every few hours, on a thread of its own: a slow
/// download never holds up the relay. A running relay notices the finished copy
/// by itself.
async fn update_loop(data: PathBuf, available: Arc<Mutex<Option<String>>>) {
    tokio::time::sleep(UPDATE_FIRST_CHECK).await;
    let mut last_error = String::new();
    loop {
        if let Ok(cfg) = Config::load(&data) {
            let dir = data.clone();
            let found = tokio::task::spawn_blocking(move || {
                update::check(&dir, &cfg.update.channel, cfg.update.auto)
            })
            .await;
            match found {
                Ok(Ok(Checked::UpToDate)) => *available.lock().unwrap() = None,
                Ok(Ok(Checked::Available(v))) => {
                    let mut now = available.lock().unwrap();
                    if now.as_deref() != Some(v.as_str()) {
                        info!("version {v} is out: reamesh-relay update");
                    }
                    *now = Some(v);
                }
                Ok(Ok(Checked::Staged(v))) => {
                    let mut now = available.lock().unwrap();
                    if now.as_deref() != Some(v.as_str()) {
                        info!("version {v} is ready, restarting once nothing is being relayed");
                    }
                    *now = Some(v);
                }
                Ok(Err(e)) => {
                    // Once per different problem: every 6 hours is still noise.
                    let text = format!("{e:#}");
                    if text != last_error {
                        warn!(kind = "update", "cannot update: {text}");
                        last_error = text;
                    }
                }
                Err(e) => error!("the update check failed: {e}"),
            }
        }
        tokio::time::sleep(UPDATE_CHECK_EVERY).await;
    }
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
                        let counted = CountingMuxer::new(m, counters.clone(), p);
                        (p, StreamMuxerBox::new(counted))
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
                        [(
                            StreamProtocol::new(proto::RELAY_AUTH_PROTOCOL),
                            ProtocolSupport::Inbound,
                        )],
                        request_response::Config::default()
                            .with_request_timeout(Duration::from_secs(20)),
                    ),
                    info: request_response::cbor::Behaviour::new(
                        [(
                            StreamProtocol::new(proto::RELAY_INFO_PROTOCOL),
                            ProtocolSupport::Inbound,
                        )],
                        request_response::Config::default()
                            .with_request_timeout(Duration::from_secs(proto::INFO_TIMEOUT_SECS)),
                    ),
                    sessions: request_response::cbor::Behaviour::new(
                        [(
                            StreamProtocol::new(RELAY_SESSIONS_PROTOCOL),
                            ProtocolSupport::Inbound,
                        )],
                        // A held request is a stream waiting for its answer.
                        request_response::Config::default()
                            .with_request_timeout(Duration::from_secs(SESSIONS_TIMEOUT_SECS)),
                    ),
                    publish: request_response::cbor::Behaviour::new(
                        [(
                            StreamProtocol::new(RELAY_PUBLISH_PROTOCOL),
                            ProtocolSupport::Inbound,
                        )],
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

    /// Listens on every configured address over TCP and QUIC. An error only
    /// when a transport gets no listener at all: a host without IPv6 just
    /// listens on IPv4.
    fn listen(&mut self, swarm: &mut Swarm<Behaviour>, params: &SwarmParams) -> anyhow::Result<()> {
        let mut ok = [false, false];
        let mut first_error: [Option<String>; 2] = [None, None];
        let mut kinds: [Option<std::io::ErrorKind>; 2] = [None, None];
        for ip in &params.listen_addrs {
            let base = Multiaddr::from(*ip);
            let addrs = [
                base.clone().with(Protocol::Tcp(params.tcp_port)),
                base.with(Protocol::Udp(params.quic_port))
                    .with(Protocol::QuicV1),
            ];
            for (i, addr) in addrs.into_iter().enumerate() {
                match swarm.listen_on(addr) {
                    Ok(_) => {
                        ok[i] = true;
                        if ip.is_ipv4() {
                            self.families.0 = true;
                        } else {
                            self.families.1 = true;
                        }
                    }
                    Err(e) => {
                        debug!("cannot listen on {ip}: {e}");
                        kinds[i] = kinds[i].or(failure_kind(&e));
                        first_error[i].get_or_insert_with(|| describe(&e));
                    }
                }
            }
        }
        let config = self.opts.data_dir.join(config::CONFIG_FILE);
        for (i, (what, port)) in [
            ("TCP port", params.tcp_port),
            ("UDP port", params.quic_port),
        ]
        .into_iter()
        .enumerate()
        {
            if ok[i] {
                continue;
            }
            let quic = if i == 1 { " (QUIC)" } else { "" };
            return Err(match kinds[i] {
                Some(std::io::ErrorKind::AddrInUse) => problem(
                    format!("{what} {port}{quic} is already in use."),
                    format!(
                        "Another relay may be running (see reamesh-relay status),\nor set another port in {}.",
                        term::path(&config)
                    ),
                ),
                Some(std::io::ErrorKind::PermissionDenied) => problem(
                    format!("No permission to use {what} {port}{quic}."),
                    format!(
                        "Ports below 1024 need root or CAP_NET_BIND_SERVICE.\nSet one above 1024 in {}.",
                        term::path(&config)
                    ),
                ),
                _ => problem(
                    match first_error[i].as_deref() {
                        Some("") => format!("Cannot listen on {what} {port}{quic}."),
                        Some(e) => format!("Cannot listen on {what} {port}{quic}: {e}."),
                        None => format!("Cannot listen on {what} {port}{quic}: no listen_addrs."),
                    },
                    format!(
                        "Another program may use the port, or listen_addrs in {}\nnames an address this machine doesn't have.",
                        term::path(&config)
                    ),
                ),
            });
        }
        Ok(())
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
        let mut ready = false;
        loop {
            tokio::select! {
                // In a block: the guard it returns must not outlive the wait.
                _ = async { let _ = shutdown.wait_for(|stop| *stop).await; } => {
                    self.account();
                    self.farewell(swarm).await;
                    return Outcome::Shutdown;
                }
                event = swarm.select_next_some() => {
                    self.on_event(swarm, event, &mut listen);
                    if !ready && has_tcp_and_quic(&listen) {
                        ready = true;
                        self.on_ready();
                    }
                }
                _ = tick.tick() => {
                    self.account();
                    let now = Instant::now();
                    self.gate.lock().unwrap().auth.purge(now);
                    self.check_key_dates(swarm);
                    self.enforce_key_budgets(swarm);
                    self.check_alerts(now);
                    self.answer_held_info(swarm, false);
                    if self.update_due(now) {
                        self.farewell(swarm).await;
                        return Outcome::Update;
                    }
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
                        self.farewell(swarm).await;
                        return Outcome::Rebuild;
                    }
                    self.answer_list(swarm);
                }
                _ = reload.tick() => {
                    if config_stamp(&self.opts.data_dir) != self.cfg_stamp && self.reload(swarm) {
                        self.farewell(swarm).await;
                        return Outcome::Rebuild;
                    }
                }
                _ = hangup_recv(hangup) => {
                    info!("reloading the config");
                    if self.reload(swarm) {
                        self.farewell(swarm).await;
                        return Outcome::Rebuild;
                    }
                }
            }
        }
    }

    /// Both transports listen: the banner on the first start of the swarm,
    /// a log line every time.
    fn on_ready(&self) {
        update::confirm_started(&self.opts.data_dir);
        if self.builds == 1 {
            let first_code = || {
                let label = &self.cfg.keys.first()?.label;
                Some((
                    label.clone(),
                    self.code_for(label).filter(|c| c.addr.is_usable())?,
                ))
            };
            if self.opts.banner {
                self.print_banner();
            } else if let Some((label, code)) = first_code() {
                if self.opts.first_start {
                    info!(
                        key = label.as_str(),
                        code = code.encode(),
                        "new relay set up"
                    );
                }
            } else {
                warn!("no public address found: set public_addrs or dns_name in config.toml");
            }
        }
        info!("ready");
        // Started with the month's budget already used up: the switch
        // happened in an earlier run, so say it once more.
        if self.builds == 1
            && self.params().mode == Mode::Coordination
            && self.cfg.mode == Mode::Full
        {
            self.announce_mode_change(Mode::Full);
        }
    }

    fn account(&mut self) {
        let (sent, received) = self.counters.take();
        if self.ledger.roll(&crate::traffic::current_month()) {
            info!("new month, traffic counter reset");
        }
        self.ledger.add(sent, received);
        self.rate_bytes = self.rate_bytes.saturating_add(sent);
        self.sent_total = self.sent_total.saturating_add(sent);
        // Traffic counts for the key of the host it serves: a host's own, or
        // the host a guest's relayed connection leads to.
        let peers = self.counters.take_peers();
        if peers.is_empty() {
            return;
        }
        let gate = self.gate.clone();
        let mut gate = gate.lock().unwrap();
        for (peer, sent, received) in peers {
            let mut host = None;
            for c in gate.circuits.iter_mut().filter(|c| c.src == peer) {
                c.bytes = c.bytes.saturating_add(sent).saturating_add(received);
                host.get_or_insert(c.dst);
            }
            let label = gate
                .auth
                .label(&peer)
                .or_else(|| host.and_then(|h| gate.auth.label(&h)));
            if let Some(label) = label {
                self.ledger.add_key(label, sent);
            }
        }
    }

    /// A downloaded version is waiting and this is a moment nobody would notice:
    /// nothing is being relayed. Hosts lose their reservation for a few seconds,
    /// guests already connected directly don't notice at all.
    fn update_due(&mut self, now: Instant) -> bool {
        if !self.opts.updates
            || now.duration_since(self.update_polled) < update::restart_poll(UPDATE_RESTART_POLL)
        {
            return false;
        }
        self.update_polled = now;
        let Some(version) = update::pending_version(&self.opts.data_dir) else {
            return false;
        };
        if !self.gate.lock().unwrap().circuits.is_empty() {
            return false;
        }
        info!("restarting into version {version}");
        true
    }

    /// The newer version, as one line for `status`.
    fn update_line(&self) -> Option<String> {
        if let Some(v) = update::pending_version(&self.opts.data_dir) {
            return Some(format!("{v} ready, restarts once nothing is being relayed"));
        }
        let available = self.update_available.lock().unwrap();
        available
            .as_ref()
            .map(|v| format!("{v} is out, get it with: reamesh-relay update"))
    }

    /// The operator's notice, unless its time is over.
    fn active_notice(&self) -> Option<Notice> {
        let n = self.cfg.notice.as_ref()?;
        let (from, until) = n.window();
        (until == 0 || until > unix_now()).then(|| Notice {
            text: n.text.clone(),
            from,
            until,
        })
    }

    /// What a holder of the key `label` gets to know.
    fn info_for(&self, label: &str, stopping: bool) -> InfoResponse {
        let entry = self.cfg.entry(label);
        let key_budget = entry.map_or(0, |k| k.budget_bytes());
        let key_sent = self.ledger.key_sent(label);
        let budget = self.cfg.budget_bytes();
        let (hosts, max_hosts) = {
            let gate = self.gate.lock().unwrap();
            (gate.auth.reserved.len(), gate.auth.max_hosts)
        };
        let (tcp, quic) = self.ports();
        let mut r = InfoResponse {
            ok: true,
            reason: None,
            rev: 0,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            level: proto::RELAY_LEVEL,
            mode: mode_name(self.params().mode).to_owned(),
            budget_percent: percent_of(self.ledger.sent, budget),
            budget_exhausted: self.ledger.exhausted(budget),
            key_budget_percent: percent_of(key_sent, key_budget),
            key_budget_exhausted: key_budget > 0 && key_sent >= key_budget,
            budget_resets: term::month_bounds(&self.ledger.month).map_or(0, |(_, end)| end),
            hosts: hosts as u32,
            max_hosts: max_hosts as u32,
            notice: self.active_notice(),
            name: self.cfg.name.clone().unwrap_or_default(),
            contact: self.cfg.contact.clone().unwrap_or_default(),
            key_expires: entry.map_or(0, |k| k.expires_at()),
            stopping,
            ips: self
                .advertised_ips()
                .into_iter()
                .filter(|ip| !addrs::unspecified(ip))
                .collect(),
            dns: self.cfg.dns_name.clone().unwrap_or_default(),
            tcp,
            quic,
        };
        r.rev = r.content_rev();
        r
    }

    /// Answers an `InfoRequest` now, or holds it while nothing has changed.
    fn on_info_request(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        peer: PeerId,
        req: InfoRequest,
        channel: ResponseChannel<InfoResponse>,
    ) {
        let keys = self.cfg.keys();
        let label = proto::verify_proof(
            keys.iter().map(|(l, k)| (l.as_str(), k)),
            &self.peer_id,
            &peer,
            &req.proof,
        )
        .map(str::to_owned);
        let banned = self.gate.lock().unwrap().auth.banned.contains(&peer);
        let response = match label {
            _ if banned => InfoResponse::refused(proto::REASON_BANNED),
            None => InfoResponse::refused(proto::REASON_BAD_KEY),
            Some(label) => {
                let expired = self
                    .cfg
                    .entry(&label)
                    .is_some_and(|k| k.is_expired(config::local_today()));
                if expired {
                    // Whoever holds it should know whom to ask.
                    InfoResponse {
                        name: self.cfg.name.clone().unwrap_or_default(),
                        contact: self.cfg.contact.clone().unwrap_or_default(),
                        ..InfoResponse::refused(proto::REASON_EXPIRED)
                    }
                } else {
                    let info = self.info_for(&label, false);
                    if req.rev != 0 && req.rev == info.rev {
                        self.held_info.insert(
                            peer,
                            HeldInfo {
                                channel,
                                label,
                                rev: req.rev,
                                since: Instant::now(),
                            },
                        );
                        return;
                    }
                    info
                }
            }
        };
        let _ = swarm.behaviour_mut().info.send_response(channel, response);
    }

    /// Answers the held requests whose answer changed or that waited long
    /// enough; with `stopping`, all of them. Returns how many went out.
    fn answer_held_info(&mut self, swarm: &mut Swarm<Behaviour>, stopping: bool) -> usize {
        let hold = Duration::from_secs(proto::INFO_HOLD_SECS);
        let mut sent = 0;
        for (peer, held) in std::mem::take(&mut self.held_info) {
            if !held.channel.is_open() {
                continue;
            }
            let info = self.info_for(&held.label, stopping);
            if stopping || info.rev != held.rev || held.since.elapsed() >= hold {
                sent += 1;
                let _ = swarm.behaviour_mut().info.send_response(held.channel, info);
            } else {
                self.held_info.insert(peer, held);
            }
        }
        sent
    }

    /// Tells the hosts the relay is going away, and keeps the connections open
    /// for a moment: "sent" only means handed to the connection, and dropping
    /// it right away can lose what is still on its way.
    async fn farewell(&mut self, swarm: &mut Swarm<Behaviour>) {
        if self.answer_held_info(swarm, true) == 0 {
            return;
        }
        let deadline = tokio::time::sleep(FAREWELL);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return,
                _ = swarm.select_next_some() => {}
            }
        }
    }

    /// Once a day (and once right after the start): keys whose last day has
    /// passed lose access.
    fn check_key_dates(&mut self, swarm: &mut Swarm<Behaviour>) {
        let today = config::local_today();
        if self.expiry_day == Some(today) {
            return;
        }
        self.expiry_day = Some(today);
        self.revoke_expired(swarm, today);
    }

    fn revoke_expired(&mut self, swarm: &mut Swarm<Behaviour>, today: jiff::civil::Date) {
        let active = self.cfg.active_labels(today);
        let revoked = self.gate.lock().unwrap().auth.revoke(&active);
        for peer in &revoked {
            self.reserved_since.remove(peer);
            self.list.withdraw(peer);
            let _ = swarm.disconnect_peer_id(*peer);
        }
        self.recheck_list(swarm);
        let command = self.cfg.alerts.command.clone();
        let expired: Vec<(String, jiff::civil::Date)> = self
            .cfg
            .keys
            .iter()
            .filter(|k| k.is_expired(today))
            .filter_map(|k| Some((k.label.clone(), k.expiry_date()?)))
            .collect();
        for (label, last) in expired {
            let recent = (today - last).get_days() <= EXPIRED_ALERT_DAYS;
            if recent && self.ledger.first_alert(&format!("key_expired:{label}")) {
                let msg = format!("key {label} expired on {last}");
                warn!(kind = "key", key = label.as_str(), "{msg}");
                alert::fire(command.as_deref(), "key_expired", &msg);
            }
        }
    }

    /// A key whose own budget is used up keeps what coordination mode gives
    /// everyone: enough to go direct, not enough to carry a session. libp2p
    /// fixes a circuit's limits when the connection opens, so the guest is
    /// dropped once its relayed connection went past them.
    fn enforce_key_budgets(&mut self, swarm: &mut Swarm<Behaviour>) {
        let used_up: HashSet<&str> = self
            .cfg
            .keys
            .iter()
            .filter(|k| k.budget_bytes() > 0 && self.ledger.key_sent(&k.label) >= k.budget_bytes())
            .map(|k| k.label.as_str())
            .collect();
        if used_up.is_empty() {
            return;
        }
        let over = {
            let gate = self.gate.lock().unwrap();
            guests_over_budget(&gate, &used_up, Instant::now())
        };
        for (guest, label) in over {
            let _ = swarm.disconnect_peer_id(guest);
            self.limit(format!(
                "key {label} used up its budget, ending its relayed connections"
            ));
        }
    }

    /// What goes to the operator's command, each at most once a month.
    fn check_alerts(&mut self, now: Instant) {
        let command = self.cfg.alerts.command.clone();
        let budget = self.cfg.budget_bytes();
        if self.ledger.exhausted(budget) {
            // The switch to coordination mode logs this one itself.
            self.ledger.first_alert("budget_warning");
            if self.ledger.first_alert("budget_used_up") {
                let msg = format!(
                    "the monthly budget of {} GB is used up, coordination only until {}",
                    self.cfg.limits.monthly_budget_gb,
                    term::next_month_start(&self.ledger.month)
                );
                alert::fire(command.as_deref(), "budget_used_up", &msg);
            }
        } else {
            let used = percent_of(self.ledger.sent, budget);
            if budget > 0
                && used >= self.cfg.alerts.budget_percent
                && self.ledger.first_alert("budget_warning")
            {
                let msg = format!(
                    "{used} % of the monthly budget of {} GB used",
                    self.cfg.limits.monthly_budget_gb
                );
                warn!(kind = "budget", "{msg}");
                alert::fire(command.as_deref(), "budget_warning", &msg);
            }
        }
        let used_up: Vec<String> = self
            .cfg
            .keys
            .iter()
            .filter(|k| k.budget_bytes() > 0 && self.ledger.key_sent(&k.label) >= k.budget_bytes())
            .map(|k| k.label.clone())
            .collect();
        for label in used_up {
            if self.ledger.first_alert(&format!("key_budget:{label}")) {
                let msg = format!("key {label} used up its monthly budget");
                warn!(kind = "budget", key = label.as_str(), "{msg}");
                alert::fire(command.as_deref(), "key_budget_used_up", &msg);
            }
        }
        let hosts = self.gate.lock().unwrap().auth.reserved.len();
        if self.hosts_watch.tick(hosts, now) {
            let msg = format!(
                "every host left at once and none came back in {} min: is the relay still reachable from outside?",
                alert::HOSTS_GONE_AFTER.as_secs() / 60
            );
            warn!(kind = "reachability", "{msg}");
            alert::fire(command.as_deref(), "hosts_gone", &msg);
        }
    }

    /// "stopped after 3 h 12 min: 4 hosts, 11 relayed connections, 1.2 GB
    /// sent", leaving out what didn't happen.
    fn stop_summary(&self) -> String {
        let up = SystemTime::now()
            .duration_since(self.started)
            .unwrap_or_default();
        let mut parts = Vec::new();
        match self.hosts_seen.len() {
            0 => {}
            1 => parts.push("1 host".to_owned()),
            n => parts.push(format!("{n} hosts")),
        }
        if self.circuits_opened > 0 {
            parts.push(connections(self.circuits_opened as usize));
        }
        if self.sent_total > 0 {
            parts.push(format!("{} sent", term::bytes(self.sent_total)));
        }
        let mut line = format!("stopped after {}", term::duration(up));
        if !parts.is_empty() {
            line += &format!(": {}", parts.join(", "));
        }
        line
    }

    /// A direct connection came in: count it per transport, and warn once
    /// when every one so far took TCP.
    fn note_inbound(&mut self, addr: &Multiaddr) {
        let quic = addr.iter().any(|p| matches!(p, Protocol::QuicV1));
        let entry = if quic {
            &mut self.inbound_quic
        } else {
            &mut self.inbound_tcp
        };
        entry.count += 1;
        entry.last = unix_now();
        if !self.quic_warned
            && self.inbound_tcp.count >= status::QUIC_SUSPECT_AFTER
            && self.inbound_quic.count == 0
        {
            self.quic_warned = true;
            let (_, q) = self.ports();
            let msg = format!(
                "{} connections came in over TCP and none over QUIC: check that UDP {q} is open",
                self.inbound_tcp.count
            );
            warn!(kind = "firewall", "{msg}");
            alert::fire(self.cfg.alerts.command.as_deref(), "udp_blocked", &msg);
        }
    }

    fn announce_mode_change(&self, old: Mode) {
        let new = self.params().mode;
        if new == old {
            info!("everyone reconnects once to apply it");
        } else if new == Mode::Coordination && self.cfg.mode == Mode::Full {
            warn!(
                kind = "budget",
                "{} GB used this month, coordination only until {}",
                self.cfg.limits.monthly_budget_gb,
                term::next_month_start(&self.ledger.month)
            );
        } else if new == Mode::Full && old == Mode::Coordination && self.cfg.mode == Mode::Full {
            info!("full relaying again");
        } else {
            info!("mode is now {}", mode_name(new));
        }
    }

    /// Re-reads config.toml; keys, bans and the gate limits apply at once.
    /// Returns whether the swarm must be rebuilt for the rest.
    fn reload(&mut self, swarm: &mut Swarm<Behaviour>) -> bool {
        self.cfg_stamp = config_stamp(&self.opts.data_dir);
        let cfg = match Config::load(&self.opts.data_dir) {
            Ok(cfg) => cfg,
            Err(e) => {
                error!("{e}. The relay keeps the previous config.");
                return false;
            }
        };
        if cfg == self.cfg {
            return false;
        }
        let before = self.params();
        let old_labels: HashSet<String> = self.cfg.keys.iter().map(|k| k.label.clone()).collect();
        let settings = config_changes(&self.cfg, &cfg);
        self.cfg = cfg;
        let labels: HashSet<String> = self.cfg.keys.iter().map(|k| k.label.clone()).collect();
        let (revoked, lost) = {
            let mut gate = self.gate.lock().unwrap();
            gate.apply(&self.cfg);
            let names: HashMap<PeerId, String> = gate
                .auth
                .reserved
                .iter()
                .map(|p| (*p, host_name(&gate.auth, p)))
                .collect();
            let revoked = gate
                .auth
                .revoke(&self.cfg.active_labels(config::local_today()));
            let lost: Vec<String> = revoked
                .iter()
                .filter_map(|p| names.get(p).cloned())
                .collect();
            (revoked, lost)
        };
        for peer in &revoked {
            self.reserved_since.remove(peer);
            self.list.withdraw(peer);
            let _ = swarm.disconnect_peer_id(*peer);
        }
        self.list.set_enabled(self.cfg.session_list);
        self.recheck_list(swarm);
        let mut added: Vec<&String> = labels.difference(&old_labels).collect();
        let mut removed: Vec<&String> = old_labels.difference(&labels).collect();
        added.sort();
        removed.sort();
        // Each change gets its own line; "config reloaded" only says that
        // something changed where nothing more specific can.
        if added.is_empty() && removed.is_empty() && settings.is_empty() && lost.is_empty() {
            info!(
                keys = labels.len(),
                hosts_revoked = revoked.len(),
                "config reloaded"
            );
        }
        for line in &settings {
            info!("{line}");
        }
        for label in added {
            info!(key = label.as_str(), "key {label} added");
        }
        for label in removed {
            info!(key = label.as_str(), "key {label} removed");
        }
        for name in lost {
            info!("host {name} lost access");
        }
        self.revoke_expired(swarm, config::local_today());
        if self.params() != before {
            self.announce_mode_change(before.mode);
            return true;
        }
        false
    }

    /// A `limit` line, at most once a minute each.
    fn limit(&mut self, msg: String) {
        let now = Instant::now();
        self.limit_logged
            .retain(|_, t| now.duration_since(*t) < LIMIT_LOG_EVERY);
        if self.limit_logged.insert(msg.clone(), now).is_none() {
            warn!(kind = "limit", "{msg}");
        }
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
                debug!("listening on {address}");
                listen.push(address);
                let _ = self.listen_tx.send(listen.clone());
                self.refresh_external(swarm);
            }
            SwarmEvent::ExpiredListenAddr { address, .. } => {
                listen.retain(|a| *a != address);
                let _ = self.listen_tx.send(listen.clone());
            }
            SwarmEvent::ListenerError { error, .. } => error!("a listener failed: {error}"),
            SwarmEvent::ListenerClosed { reason: Err(e), .. } => error!("a listener stopped: {e}"),
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
                if endpoint.is_listener() {
                    self.note_inbound(addr);
                }
                if log_addrs {
                    info!("connection from {addr}");
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
                if n == 0 {
                    // Its reservation, and with it any list entry, ends here.
                    self.list.withdraw(&peer_id);
                    self.ping_misses.remove(&peer_id);
                }
                let mut gate = self.gate.lock().unwrap();
                // The reservation dies with the last connection.
                let was_host = n == 0 && gate.auth.reserved.contains(&peer_id);
                let name = host_name(&gate.auth, &peer_id);
                gate.auth.connection_count(&peer_id, n, Instant::now());
                if was_host {
                    self.reserved_since.remove(&peer_id);
                    info!(
                        key = gate.auth.label(&peer_id).unwrap_or_default(),
                        "host {name} disconnected"
                    );
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
            SwarmEvent::Behaviour(BehaviourEvent::Sessions(request_response::Event::Message {
                peer,
                connection_id,
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
            })) => {
                let keys = self.cfg.keys();
                let banned = self.gate.lock().unwrap().auth.banned.clone();
                let who = access(&self.cfg, &keys, &banned, &self.peer_id, &peer, &request.proof);
                let answers = self.list.query(
                    peer,
                    connection_id,
                    who,
                    &request.proof,
                    request.rev,
                    channel,
                    Instant::now(),
                );
                for (channel, response) in answers {
                    let _ = swarm.behaviour_mut().sessions.send_response(channel, response);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Publish(request_response::Event::Message {
                peer,
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
                ..
            })) => {
                let keys = self.cfg.keys();
                let (banned, reserved) = {
                    let gate = self.gate.lock().unwrap();
                    (gate.auth.banned.clone(), gate.auth.reserved.contains(&peer))
                };
                let who = access(&self.cfg, &keys, &banned, &self.peer_id, &peer, &request.proof);
                let response =
                    self.list
                        .publish(peer, who, &request.proof, request.envelope, reserved);
                if let Some(reason) = response.reason.as_deref() {
                    debug!(reason, "publish refused");
                }
                let _ = swarm.behaviour_mut().publish.send_response(channel, response);
            }
            SwarmEvent::Behaviour(BehaviourEvent::Ping(ping::Event { peer, result, .. })) => {
                if result.is_ok() || !self.list.is_listed(&peer) {
                    self.ping_misses.remove(&peer);
                } else {
                    let misses = self.ping_misses.entry(peer).or_default();
                    *misses += 1;
                    if *misses >= LISTED_PING_MISSES {
                        self.ping_misses.remove(&peer);
                        self.list.withdraw(&peer);
                        let _ = swarm.disconnect_peer_id(peer);
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Info(request_response::Event::Message {
                peer,
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
                ..
            })) => self.on_info_request(swarm, peer, request, channel),
            SwarmEvent::Behaviour(BehaviourEvent::Relay(e)) => self.on_relay_event(swarm, e),
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
                    if log_addrs {
                        info!("a host sees this relay at {ip}");
                    }
                    self.refresh_external(swarm);
                }
            }
            _ => {}
        }
    }

    fn authenticate(&mut self, peer: &PeerId, req: &AuthRequest) -> AuthResponse {
        let keys = self.cfg.keys();
        let label = proto::verify_proof(
            keys.iter().map(|(l, k)| (l.as_str(), k)),
            &self.peer_id,
            peer,
            &req.proof,
        );
        let expired = label
            .and_then(|l| self.cfg.entry(l))
            .is_some_and(|k| k.is_expired(config::local_today()));
        let mut gate = self.gate.lock().unwrap();
        let response = if gate.auth.banned.contains(peer) {
            AuthResponse::refused(proto::REASON_BANNED)
        } else if expired {
            AuthResponse::refused(proto::REASON_EXPIRED)
        } else if let Some(label) = label {
            if gate.auth.is_full(peer) || gate.auth.key_is_full(peer, label) {
                AuthResponse::refused(proto::REASON_FULL)
            } else {
                let conns = self.peers.get(peer).map_or(1, |e| e.0);
                gate.auth.authorize(*peer, label, conns);
                AuthResponse::accepted(label)
            }
        } else {
            AuthResponse::refused(proto::REASON_BAD_KEY)
        };
        let max_hosts = gate.auth.max_hosts;
        let key_full =
            label.is_some_and(|l| !gate.auth.is_full(peer) && gate.auth.key_is_full(peer, l));
        drop(gate);
        match response.reason.as_deref() {
            None => debug!(key = label.unwrap_or_default(), "host authorized"),
            Some(proto::REASON_FULL) if key_full => {
                let label = label.unwrap_or_default();
                self.limit(format!(
                    "key {label} has as many hosts online as it may, refusing one more"
                ));
            }
            Some(proto::REASON_FULL) => {
                let label = label.unwrap_or_default();
                self.limit(format!("{max_hosts} hosts connected, refusing {label}"));
            }
            Some(proto::REASON_BANNED) => info!("a banned host was refused"),
            Some(proto::REASON_EXPIRED) => {
                info!(
                    key = label.unwrap_or_default(),
                    "a host with an expired key was refused"
                );
            }
            Some(_) => info!("a host with an unknown key was refused"),
        }
        response
    }

    /// Keys, bans or the date changed: whoever relies on them finds out now.
    fn recheck_list(&mut self, swarm: &mut Swarm<Behaviour>) {
        let keys = self.cfg.keys();
        let banned = self.gate.lock().unwrap().auth.banned.clone();
        let relay_id = self.peer_id;
        let cfg = &self.cfg;
        let refused = self
            .list
            .recheck(|peer, proof| access(cfg, &keys, &banned, &relay_id, peer, proof));
        for (channel, response) in refused {
            let _ = swarm.behaviour_mut().sessions.send_response(channel, response);
        }
    }

    /// This tick's answers to viewers: the list changed, or their wait is over.
    fn answer_list(&mut self, swarm: &mut Swarm<Behaviour>) {
        self.list
            .set_budget_used(self.ledger.exhausted(self.cfg.budget_bytes()));
        for (channel, response) in self.list.due(Instant::now(), ResponseChannel::is_open) {
            let _ = swarm.behaviour_mut().sessions.send_response(channel, response);
        }
    }

    fn on_relay_event(&mut self, swarm: &mut Swarm<Behaviour>, e: relay::Event) {
        let gate = self.gate.clone();
        let mut gate = gate.lock().unwrap();
        match e {
            relay::Event::ReservationReqAccepted {
                src_peer_id,
                renewed,
            } => {
                gate.auth.reserved.insert(src_peer_id);
                if let Some(label) = gate.auth.label(&src_peer_id) {
                    self.seen.touch(label, unix_now());
                }
                self.hosts_seen.insert(src_peer_id);
                if !renewed {
                    self.reserved_since.insert(src_peer_id, SystemTime::now());
                    let name = host_name(&gate.auth, &src_peer_id);
                    info!(
                        key = gate.auth.label(&src_peer_id).unwrap_or_default(),
                        "host {name} connected"
                    );
                }
            }
            relay::Event::ReservationReqDenied { .. } => debug!("reservation refused"),
            relay::Event::ReservationClosed { src_peer_id }
            | relay::Event::ReservationTimedOut { src_peer_id } => {
                let name = host_name(&gate.auth, &src_peer_id);
                if gate.auth.reserved.remove(&src_peer_id) {
                    self.reserved_since.remove(&src_peer_id);
                    self.list.withdraw(&src_peer_id);
                    info!(
                        key = gate.auth.label(&src_peer_id).unwrap_or_default(),
                        "host {name} disconnected"
                    );
                }
            }
            relay::Event::CircuitReqAccepted {
                src_peer_id,
                dst_peer_id,
            } => {
                let name = host_name(&gate.auth, &dst_peer_id);
                let max = gate.max_circuits_per_host;
                if gate
                    .circuits
                    .iter()
                    .filter(|c| c.dst == dst_peer_id)
                    .count()
                    >= max
                {
                    // No hook sees the destination before this, so the guest
                    // over the limit is dropped instead of refused.
                    self.refused.insert((src_peer_id, dst_peer_id));
                    let _ = swarm.disconnect_peer_id(src_peer_id);
                    drop(gate);
                    self.limit(format!(
                        "host {name} has {}, refusing more",
                        connections(max)
                    ));
                    return;
                }
                let ip = self.peers.get(&src_peer_id).and_then(|e| e.1);
                gate.circuits.push(Circuit {
                    src: src_peer_id,
                    dst: dst_peer_id,
                    src_ip: ip,
                    opened: Instant::now(),
                    bytes: 0,
                });
                self.circuits_opened += 1;
                info!(
                    key = gate.auth.label(&dst_peer_id).unwrap_or_default(),
                    "guest joined {name} via relay"
                );
            }
            relay::Event::CircuitClosed {
                src_peer_id,
                dst_peer_id,
                ..
            } => {
                if self.refused.remove(&(src_peer_id, dst_peer_id)) {
                    return;
                }
                if let Some(i) = gate
                    .circuits
                    .iter()
                    .position(|c| c.src == src_peer_id && c.dst == dst_peer_id)
                {
                    // A few seconds usually means the two went direct; hours,
                    // that the relay carried the session.
                    let opened = gate.circuits.swap_remove(i).opened;
                    let name = host_name(&gate.auth, &dst_peer_id);
                    info!(
                        key = gate.auth.label(&dst_peer_id).unwrap_or_default(),
                        "relayed connection to {name} ended after {}",
                        term::duration(opened.elapsed())
                    );
                }
            }
            relay::Event::CircuitReqDenied {
                src_peer_id,
                status,
                ..
            } => {
                let ip = self.peers.get(&src_peer_id).and_then(|e| e.1);
                let msg = if gate.auth.banned.contains(&src_peer_id) {
                    None
                } else if gate.circuits.len() >= gate.max_circuits {
                    Some(format!(
                        "{} in all, refusing more",
                        connections(gate.max_circuits)
                    ))
                } else if ip.is_some()
                    && gate.circuits.iter().filter(|c| c.src_ip == ip).count()
                        >= gate.max_circuits_per_ip
                {
                    Some(format!(
                        "a guest address has {}, refusing more",
                        connections(gate.max_circuits_per_ip)
                    ))
                } else {
                    None
                };
                drop(gate);
                match msg {
                    Some(m) => self.limit(m),
                    None => debug!(?status, "circuit refused"),
                }
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
            addr: RelayAddr {
                peer: self.peer_id,
                ips: self.advertised_ips(),
                dns: self.cfg.dns_name.clone().unwrap_or_default(),
                tcp: tcp_port,
                quic: quic_port,
            },
            key: self.cfg.key(label)?,
            label: label.to_owned(),
        })
    }

    /// The R, version, state, ports, address and data dir; on the first
    /// start also the code of the key that was just made.
    fn print_banner(&self) {
        let s = Style::stdout();
        let (t, q) = self.ports();
        let port = if t == q {
            format!("{t}, TCP and QUIC")
        } else {
            format!("TCP {t}, QUIC {q}")
        };
        let families = match self.families {
            (true, true) => ", IPv4 and IPv6",
            (true, false) => ", IPv4",
            (false, true) => ", IPv6",
            (false, false) => "",
        };
        let mut places: Vec<String> = self
            .advertised_ips()
            .iter()
            .filter(|ip| !addrs::unspecified(ip))
            .map(IpAddr::to_string)
            .collect();
        places.extend(self.cfg.dns_name.clone());
        let address = if places.is_empty() {
            s.dim("none found")
        } else {
            places.join(", ")
        };
        let dot = if s.rich {
            format!("{} ", s.strong("●"))
        } else {
            String::new()
        };
        // Beside the R a long path would wrap and pull the block apart; logs
        // keep it whole.
        let data = term::path(&self.opts.data_dir);
        let data = if s.rich {
            term::fit(s, &data, BANNER_VALUE_WIDTH)
        } else {
            data
        };
        let lines = [
            s.strong(&format!("ReaMesh Relay {}", env!("CARGO_PKG_VERSION"))),
            String::new(),
            format!(
                "{dot}{}   {} {}",
                s.strong("running"),
                s.dim("mode"),
                s.strong(mode_name(self.params().mode))
            ),
            s.field("port", &format!("{port}{families}")),
            s.field("address", &address),
            s.field("data", &data),
        ];
        let mut out = term::with_mark(s, &lines);
        out.push('\n');
        if places.is_empty() {
            out += &term::no_address_hint(s, &self.opts.data_dir.join(config::CONFIG_FILE));
            out.push('\n');
        } else if self.opts.first_start
            && let Some(key) = self.cfg.keys.first()
            && let Some(code) = self.code_for(&key.label)
        {
            let intro = format!(
                "{}{}{}",
                s.dim("New relay set up. Relay code for "),
                s.strong(&key.label),
                s.dim(":")
            );
            out += &term::code_block(s, &intro, &code.encode());
            out += &term::share_hint(s);
            out.push('\n');
        }
        anstream::print!("{out}");
    }

    fn save_ledger(&mut self) {
        if let Err(e) = self.ledger.save() {
            error!("cannot save the traffic counter: {e}");
        }
        let labels: Vec<String> = self.cfg.keys.iter().map(|k| k.label.clone()).collect();
        if let Err(e) = self.seen.save(labels.iter().map(String::as_str)) {
            error!("cannot save seen.json: {e}");
        }
    }

    fn write_status(&mut self, running: bool, listen: &[Multiaddr]) {
        let elapsed = self.rate_since.elapsed().as_secs_f64();
        let rate = if running && elapsed > 0.5 {
            (self.rate_bytes as f64 / elapsed) as u64
        } else {
            0
        };
        self.rate_bytes = 0;
        self.rate_since = Instant::now();
        let gate = self.gate.clone();
        let gate = gate.lock().unwrap();
        // Hosts online now count as seen now, so a host that stays for days
        // doesn't read as gone in `key list`.
        for (_, label) in gate.auth.hosts() {
            self.seen.touch(&label, unix_now());
        }
        let unix = |t: SystemTime| {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        };
        let status = Status {
            running,
            pid: std::process::id(),
            updated: unix_now(),
            started: unix(self.started),
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
                    fingerprint: self
                        .cfg
                        .key(&label)
                        .map(|k| config::fingerprint(&k))
                        .unwrap_or_default(),
                    since: self.reserved_since.get(&p).map_or(unix_now(), |t| unix(*t)),
                    guests: gate.circuits.iter().filter(|c| c.dst == p).count(),
                    listed: self.list.is_listed(&p),
                    label,
                    peer_id: p.to_string(),
                })
                .collect(),
            circuits: gate.circuits.len(),
            peers: self.peers.len(),
            observed_ips: self.observed.clone(),
            listening: listen.iter().map(|a| a.to_string()).collect(),
            rate,
            inbound_tcp: self.inbound_tcp,
            inbound_quic: self.inbound_quic,
            keys: self
                .cfg
                .keys
                .iter()
                .map(|k| KeyUsage {
                    label: k.label.clone(),
                    sent: self.ledger.key_sent(&k.label),
                    budget: k.budget_bytes(),
                })
                .collect(),
            notice: self.active_notice().map(|n| term::notice_line(&n)),
            update: self.update_line(),
            session_list: self.list.enabled(),
            listed: self.list.len(),
            watching: self.list.watching(),
        };
        drop(gate);
        if let Err(e) = status.write(&self.opts.data_dir) {
            error!("cannot write status.json: {e}");
        }
    }
}

/// The guests whose relayed connection to a host of a used-up key went past
/// what coordination mode allows, with that key's label.
fn guests_over_budget(gate: &Gate, used_up: &HashSet<&str>, now: Instant) -> Vec<(PeerId, String)> {
    gate.circuits
        .iter()
        .filter(|c| {
            c.bytes >= COORDINATION_CIRCUIT_BYTES
                || now.duration_since(c.opened) >= COORDINATION_CIRCUIT_DURATION
        })
        .filter_map(|c| {
            let label = gate.auth.label(&c.dst)?;
            used_up.contains(label).then(|| (c.src, label.to_owned()))
        })
        .collect()
}

/// What a proof sent with a list request is worth right now. Checked on every
/// request, again when the keys change and again when a day begins, so a key
/// that was removed or ran out stops working at once, without any login to end.
fn access<'a>(
    cfg: &Config,
    keys: &'a [(String, proto::AccessKey)],
    banned: &HashSet<PeerId>,
    relay: &PeerId,
    peer: &PeerId,
    proof: &[u8],
) -> Access<'a> {
    if banned.contains(peer) {
        return Access::Banned;
    }
    let keys = keys.iter().map(|(l, k)| (l.as_str(), k));
    match proto::verify_proof(keys, relay, peer, proof) {
        Some(label)
            if cfg
                .entry(label)
                .is_some_and(|k| k.is_expired(config::local_today())) =>
        {
            Access::Expired
        }
        Some(label) => Access::Key(label),
        None => Access::BadKey,
    }
}

fn connections(n: usize) -> String {
    if n == 1 {
        "1 relayed connection".to_owned()
    } else {
        format!("{n} relayed connections")
    }
}

/// The settings that differ, one line each, in config.toml's own names:
/// "max_hosts 32 → 48". Keys are logged on their own.
fn config_changes(old: &Config, new: &Config) -> Vec<String> {
    fn ips(v: &[IpAddr]) -> String {
        if v.is_empty() {
            "none".to_owned()
        } else {
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
    let mut out = Vec::new();
    let arrow = Style::stderr().sym("→", "->");
    let mut diff = |name: &str, a: String, b: String| {
        if a != b {
            out.push(format!("{name} {a} {arrow} {b}"));
        }
    };
    diff(
        "tcp_port",
        old.tcp_port.to_string(),
        new.tcp_port.to_string(),
    );
    diff(
        "quic_port",
        old.quic_port.to_string(),
        new.quic_port.to_string(),
    );
    diff(
        "listen_addrs",
        ips(&old.listen_addrs),
        ips(&new.listen_addrs),
    );
    diff(
        "public_addrs",
        ips(&old.public_addrs),
        ips(&new.public_addrs),
    );
    let dns = |d: &Option<String>| d.clone().unwrap_or_else(|| "none".to_owned());
    diff("dns_name", dns(&old.dns_name), dns(&new.dns_name));
    diff(
        "mode",
        mode_name(old.mode).into(),
        mode_name(new.mode).into(),
    );
    diff(
        "log_addresses",
        old.log_addresses.to_string(),
        new.log_addresses.to_string(),
    );
    let text = |t: &Option<String>| t.clone().unwrap_or_else(|| "none".to_owned());
    diff("name", text(&old.name), text(&new.name));
    diff("contact", text(&old.contact), text(&new.contact));
    let notice = |n: &Option<config::NoticeEntry>| {
        n.as_ref().map_or("none".to_owned(), |n| {
            let (from, until) = n.window();
            term::notice_line(&Notice {
                text: n.text.clone(),
                from,
                until,
            })
        })
    };
    diff("notice", notice(&old.notice), notice(&new.notice));
    diff(
        "alert_command",
        text(&old.alerts.command),
        text(&new.alerts.command),
    );
    diff(
        "alert_budget_percent",
        old.alerts.budget_percent.to_string(),
        new.alerts.budget_percent.to_string(),
    );
    diff(
        "session_list",
        old.session_list.to_string(),
        new.session_list.to_string(),
    );
    let (a, b) = (&old.limits, &new.limits);
    diff(
        "max_hosts",
        a.max_hosts.to_string(),
        b.max_hosts.to_string(),
    );
    diff(
        "max_circuits",
        a.max_circuits.to_string(),
        b.max_circuits.to_string(),
    );
    diff(
        "max_circuits_per_host",
        a.max_circuits_per_host.to_string(),
        b.max_circuits_per_host.to_string(),
    );
    diff(
        "max_circuits_per_ip",
        a.max_circuits_per_ip.to_string(),
        b.max_circuits_per_ip.to_string(),
    );
    diff(
        "circuit_max_minutes",
        a.circuit_max_minutes.to_string(),
        b.circuit_max_minutes.to_string(),
    );
    diff(
        "circuit_max_mb",
        a.circuit_max_mb.to_string(),
        b.circuit_max_mb.to_string(),
    );
    diff(
        "monthly_budget_gb",
        a.monthly_budget_gb.to_string(),
        b.monthly_budget_gb.to_string(),
    );
    let banned: HashSet<&String> = new.banned_peers.iter().collect();
    let was: HashSet<&String> = old.banned_peers.iter().collect();
    for (n, what) in [
        (banned.difference(&was).count(), "banned"),
        (was.difference(&banned).count(), "unbanned"),
    ] {
        match n {
            0 => {}
            1 => out.push(format!("1 peer {what}")),
            n => out.push(format!("{n} peers {what}")),
        }
    }
    for k in &new.keys {
        if old
            .keys
            .iter()
            .any(|o| o.label == k.label && o.key != k.key)
        {
            out.push(format!("key {} replaced", k.label));
        }
        if let Some(o) = old.keys.iter().find(|o| o.label == k.label)
            && (o.expires != k.expires
                || o.max_hosts != k.max_hosts
                || o.monthly_budget_gb != k.monthly_budget_gb)
        {
            out.push(format!("key {} changed", k.label));
        }
    }
    out
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Full => "full",
        Mode::Coordination => "coordination",
    }
}

/// How logs and `status` name a host: its key's label, plus the end of its
/// peer id when another host uses the same key.
fn host_name(auth: &AuthState, peer: &PeerId) -> String {
    let label = auth.label(peer).unwrap_or("?");
    let shared = auth
        .reserved
        .iter()
        .any(|p| p != peer && auth.label(p) == Some(label));
    if shared {
        let id = peer.to_string();
        format!("{label} …{}", &id[id.len() - 6..])
    } else {
        label.to_owned()
    }
}

fn has_tcp_and_quic(listen: &[Multiaddr]) -> bool {
    listen
        .iter()
        .any(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_))))
        && listen
            .iter()
            .any(|a| a.iter().any(|p| matches!(p, Protocol::QuicV1)))
}

/// Every non-empty message along the error chain: some transports wrap the
/// OS error in a layer that prints nothing.
fn describe(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = Some(e);
    while let Some(err) = cur {
        let msg = err.to_string();
        let msg = msg.trim();
        if !msg.is_empty() && !parts.iter().any(|p| p.contains(msg)) {
            parts.push(msg.to_owned());
        }
        cur = err.source();
    }
    parts.join(": ")
}

/// Why binding failed, as far as the error chain tells; the message as a
/// fallback, since transports don't all keep the `io::ErrorKind`.
fn failure_kind(e: &(dyn std::error::Error + 'static)) -> Option<std::io::ErrorKind> {
    use std::io::ErrorKind::{AddrInUse, PermissionDenied};
    let mut cur = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            if matches!(io.kind(), AddrInUse | PermissionDenied) {
                return Some(io.kind());
            }
            if let Some(k) = io.get_ref().and_then(|inner| failure_kind(inner)) {
                return Some(k);
            }
        }
        cur = err.source();
    }
    let text = describe(e).to_lowercase();
    // Windows says "Only one usage of each socket address … (os error 10048)".
    if text.contains("in use") || text.contains("only one usage") {
        Some(AddrInUse)
    } else if text.contains("permission denied") || text.contains("access is denied") {
        Some(PermissionDenied)
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_used_up_key_keeps_only_a_short_relayed_connection() {
        let (lena, band) = (PeerId::random(), PeerId::random());
        let now = Instant::now();
        let mut gate = Gate::default();
        gate.auth.authorize(lena, "lena", 1);
        gate.auth.authorize(band, "band", 1);
        let circuit = |dst, bytes, age: u64| Circuit {
            src: PeerId::random(),
            dst,
            src_ip: None,
            opened: now - Duration::from_secs(age),
            bytes,
        };
        gate.circuits = vec![
            // Fresh and small: may still go direct.
            circuit(lena, 1_000, 1),
            // Past the byte limit.
            circuit(lena, COORDINATION_CIRCUIT_BYTES, 1),
            // Past the time limit.
            circuit(lena, 0, COORDINATION_CIRCUIT_DURATION.as_secs() + 1),
            // The other key is fine, however much it carries.
            circuit(band, u64::MAX / 2, 100_000),
        ];
        let want: Vec<PeerId> = vec![gate.circuits[1].src, gate.circuits[2].src];
        let used_up = HashSet::from(["lena"]);
        let over = guests_over_budget(&gate, &used_up, now);
        assert_eq!(over.iter().map(|o| o.0).collect::<Vec<_>>(), want);
        assert!(over.iter().all(|o| o.1 == "lena"));
    }
}
