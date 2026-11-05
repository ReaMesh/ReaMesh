//! In-process tests against a real relay on loopback: client swarms dial it
//! over TCP, authenticate, reserve and open circuits.

use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, OutboundRequestId, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{Multiaddr, PeerId, StreamProtocol, Swarm, noise, relay, tcp, yamux};
use reamesh_relay::config::{self, Config};
use reamesh_relay::server::{self, COORDINATION_CIRCUIT_BYTES, Options, RelayHandle};
use reamesh_relay::status::Status;
use reamesh_relay::traffic::{Ledger, current_month};
use reamesh_relay_proto::{self as proto, AuthRequest, AuthResponse, InfoRequest, InfoResponse};

const WAIT: Duration = Duration::from_secs(15);

#[derive(NetworkBehaviour)]
struct Client {
    relay: relay::client::Behaviour,
    auth: request_response::cbor::Behaviour<AuthRequest, AuthResponse>,
    info: request_response::cbor::Behaviour<InfoRequest, InfoResponse>,
}

fn client() -> Swarm<Client> {
    libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .unwrap()
        .with_relay_client(noise::Config::new, yamux::Config::default)
        .unwrap()
        .with_behaviour(|_, relay| Client {
            relay,
            auth: request_response::cbor::Behaviour::new(
                [(
                    StreamProtocol::new(proto::RELAY_AUTH_PROTOCOL),
                    ProtocolSupport::Outbound,
                )],
                request_response::Config::default(),
            ),
            info: request_response::cbor::Behaviour::new(
                [(
                    StreamProtocol::new(proto::RELAY_INFO_PROTOCOL),
                    ProtocolSupport::Outbound,
                )],
                request_response::Config::default()
                    .with_request_timeout(Duration::from_secs(proto::INFO_TIMEOUT_SECS)),
            ),
        })
        .unwrap()
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build()
}

/// A data dir listening on loopback with OS-picked ports.
fn data_dir(extra: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    config::init(dir.path()).unwrap();
    let path = dir.path().join(config::CONFIG_FILE);
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("tcp_port = 4001", "tcp_port = 0")
        .replace("quic_port = 4001", "quic_port = 0")
        .replace(
            r#"listen_addrs = ["0.0.0.0", "::"]"#,
            r#"listen_addrs = ["127.0.0.1"]"#,
        )
        .replace(
            r#"# public_addrs = ["203.0.113.7"]"#,
            r#"public_addrs = ["127.0.0.1"]"#,
        );
    let text = apply_extra(&text, extra);
    std::fs::write(path, text).unwrap();
    dir
}

/// Replaces `key = …` lines with the ones in `extra`.
fn apply_extra(text: &str, extra: &str) -> String {
    let mut out = text.to_owned();
    for line in extra.lines().filter(|l| !l.trim().is_empty()) {
        let key = line.split('=').next().unwrap().trim();
        let old = out
            .lines()
            .find(|l| l.split('=').next().unwrap().trim() == key)
            .unwrap()
            .to_owned();
        out = out.replace(&old, line);
    }
    out
}

fn edit_config(dir: &Path, extra: &str) {
    let path = dir.join(config::CONFIG_FILE);
    let text = apply_extra(&std::fs::read_to_string(&path).unwrap(), extra);
    std::fs::write(path, text).unwrap();
}

fn start(dir: &Path) -> RelayHandle {
    server::spawn(Options {
        reload_poll: Duration::from_millis(100),
        tick: Duration::from_millis(100),
        banner: false,
        ..Options::new(dir)
    })
    .unwrap()
}

async fn relay_tcp_addr(relay: &mut RelayHandle) -> Multiaddr {
    relay
        .listen_addrs()
        .await
        .into_iter()
        .find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_))))
        .unwrap()
        .with(Protocol::P2p(relay.peer_id))
}

async fn until<T>(
    swarm: &mut Swarm<Client>,
    mut f: impl FnMut(SwarmEvent<ClientEvent>) -> Option<T>,
) -> T {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(t) = f(swarm.select_next_some().await) {
                return t;
            }
        }
    })
    .await
    .expect("timed out waiting for a swarm event")
}

/// Like `until`, while also driving `other` (whose events are dropped).
async fn until_with<T>(
    swarm: &mut Swarm<Client>,
    other: &mut Swarm<Client>,
    mut f: impl FnMut(SwarmEvent<ClientEvent>) -> Option<T>,
) -> T {
    tokio::time::timeout(WAIT, async {
        loop {
            tokio::select! {
                e = swarm.select_next_some() => if let Some(t) = f(e) { return t },
                _ = other.select_next_some() => {}
            }
        }
    })
    .await
    .expect("timed out waiting for a swarm event")
}

async fn connect(swarm: &mut Swarm<Client>, relay: &Multiaddr, relay_id: PeerId) {
    swarm.dial(relay.clone()).unwrap();
    until(swarm, |e| match e {
        SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == relay_id => Some(()),
        SwarmEvent::OutgoingConnectionError { error, .. } => panic!("dial failed: {error}"),
        _ => None,
    })
    .await;
}

async fn authenticate(swarm: &mut Swarm<Client>, relay_id: PeerId, key: &[u8; 32]) -> AuthResponse {
    let me = *swarm.local_peer_id();
    let proof = proto::auth_proof(key, &relay_id, &me).to_vec();
    let id: OutboundRequestId = swarm
        .behaviour_mut()
        .auth
        .send_request(&relay_id, AuthRequest { proof });
    until(swarm, |e| match e {
        SwarmEvent::Behaviour(ClientEvent::Auth(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        })) if request_id == id => Some(response),
        SwarmEvent::Behaviour(ClientEvent::Auth(request_response::Event::OutboundFailure {
            error,
            ..
        })) => panic!("auth request failed: {error}"),
        _ => None,
    })
    .await
}

enum Reservation {
    Accepted { data_limit: Option<u64> },
    Refused,
}

fn ask_info(
    swarm: &mut Swarm<Client>,
    relay_id: PeerId,
    key: &[u8; 32],
    rev: u64,
) -> OutboundRequestId {
    let me = *swarm.local_peer_id();
    let proof = proto::auth_proof(key, &relay_id, &me).to_vec();
    swarm
        .behaviour_mut()
        .info
        .send_request(&relay_id, InfoRequest { proof, rev })
}

fn info_answer(e: SwarmEvent<ClientEvent>, id: OutboundRequestId) -> Option<InfoResponse> {
    match e {
        SwarmEvent::Behaviour(ClientEvent::Info(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        })) if request_id == id => Some(response),
        SwarmEvent::Behaviour(ClientEvent::Info(request_response::Event::OutboundFailure {
            error,
            ..
        })) => panic!("info request failed: {error}"),
        _ => None,
    }
}

async fn info(
    swarm: &mut Swarm<Client>,
    relay_id: PeerId,
    key: &[u8; 32],
    rev: u64,
) -> InfoResponse {
    let id = ask_info(swarm, relay_id, key, rev);
    until(swarm, |e| info_answer(e, id)).await
}

/// Text at the end of config.toml: belongs to the last `[[keys]]` entry, or
/// starts a table of its own.
fn append_config(dir: &Path, extra: &str) {
    let path = dir.join(config::CONFIG_FILE);
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(path, format!("{text}{extra}\n")).unwrap();
}

async fn reserve(swarm: &mut Swarm<Client>, relay: &Multiaddr) -> Reservation {
    let listener = swarm
        .listen_on(relay.clone().with(Protocol::P2pCircuit))
        .unwrap();
    until(swarm, |e| match e {
        SwarmEvent::Behaviour(ClientEvent::Relay(
            relay::client::Event::ReservationReqAccepted { limit, .. },
        )) => Some(Reservation::Accepted {
            data_limit: limit.and_then(|l| l.data_in_bytes()),
        }),
        SwarmEvent::ListenerClosed {
            listener_id,
            reason: Err(_),
            ..
        } if listener_id == listener => Some(Reservation::Refused),
        _ => None,
    })
    .await
}

fn default_key(dir: &Path) -> [u8; 32] {
    Config::load(dir).unwrap().key("default").unwrap()
}

#[tokio::test]
async fn reservation_needs_auth_and_guests_reach_only_hosts() {
    let dir = data_dir("");
    let mut relay = start(dir.path());
    let relay_id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;

    let mut host = client();
    connect(&mut host, &addr, relay_id).await;

    // No auth yet: the relay refuses the reservation.
    assert!(matches!(
        reserve(&mut host, &addr).await,
        Reservation::Refused
    ));

    // A wrong key is refused.
    let r = authenticate(&mut host, relay_id, &[0x42; 32]).await;
    assert_eq!(r, AuthResponse::refused(proto::REASON_BAD_KEY));

    // A proof made for another peer is refused, even with the right key.
    let other = PeerId::random();
    let proof = proto::auth_proof(&default_key(dir.path()), &relay_id, &other).to_vec();
    host.behaviour_mut()
        .auth
        .send_request(&relay_id, AuthRequest { proof });
    let r = until(&mut host, |e| match e {
        SwarmEvent::Behaviour(ClientEvent::Auth(request_response::Event::Message {
            message: request_response::Message::Response { response, .. },
            ..
        })) => Some(response),
        _ => None,
    })
    .await;
    assert_eq!(r.reason.as_deref(), Some(proto::REASON_BAD_KEY));

    // The right key: authorized, and the reservation goes through.
    let r = authenticate(&mut host, relay_id, &default_key(dir.path())).await;
    assert_eq!(r, AuthResponse::accepted("default"));
    match reserve(&mut host, &addr).await {
        // circuit_max_mb = 0: libp2p reports 0, meaning unlimited.
        Reservation::Accepted { data_limit } => assert_eq!(data_limit, Some(0)),
        Reservation::Refused => panic!("authorized host was refused"),
    }

    // A guest (no key) reaches the host through the relay.
    let host_id = *host.local_peer_id();
    let mut guest = client();
    connect(&mut guest, &addr, relay_id).await;
    guest
        .dial(
            addr.clone()
                .with(Protocol::P2pCircuit)
                .with(Protocol::P2p(host_id)),
        )
        .unwrap();
    until_with(&mut guest, &mut host, |e| match e {
        SwarmEvent::ConnectionEstablished {
            peer_id, endpoint, ..
        } if peer_id == host_id => {
            assert!(endpoint.is_relayed());
            Some(())
        }
        SwarmEvent::OutgoingConnectionError { error, .. } => panic!("circuit failed: {error}"),
        _ => None,
    })
    .await;

    // A peer without a reservation can't be reached through the relay.
    let mut loner = client();
    connect(&mut loner, &addr, relay_id).await;
    let loner_id = *loner.local_peer_id();
    guest
        .dial(
            addr.clone()
                .with(Protocol::P2pCircuit)
                .with(Protocol::P2p(loner_id)),
        )
        .unwrap();
    until_with(&mut guest, &mut loner, |e| match e {
        SwarmEvent::OutgoingConnectionError {
            peer_id: Some(p), ..
        } if p == loner_id => Some(()),
        SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == loner_id => {
            panic!("reached a peer without a reservation")
        }
        _ => None,
    })
    .await;

    // The status file lists the host under its key label.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let status = Status::read(dir.path()).unwrap();
    assert!(status.running);
    assert_eq!(status.hosts.len(), 1);
    assert_eq!(status.hosts[0].label, "default");
    assert_eq!(status.hosts[0].peer_id, host_id.to_string());
    assert!(status.sent > 0 && status.received > 0);
    // Every peer here came in over TCP.
    assert!(status.inbound_tcp.count >= 2 && status.inbound_tcp.last > 0);
    assert_eq!(status.inbound_quic.count, 0);

    // While it runs, the data dir is taken.
    let second = server::spawn(Options::new(dir.path()));
    assert!(second.is_err_and(|e| e.to_string().contains("already runs")));

    relay.stop().await.unwrap();
    assert!(!Status::read(dir.path()).unwrap().running);
    // The host's key was seen, and the dir is free again.
    let seen = reamesh_relay::seen::Seen::load(dir.path());
    assert!(seen.last.contains_key("default"));
    start(dir.path()).stop().await.unwrap();
}

#[tokio::test]
async fn used_up_budget_switches_to_coordination_mode() {
    // A budget of 1 GB with all but 100 bytes already sent this month.
    let dir = data_dir("monthly_budget_gb = 1");
    let ledger_path = dir.path().join(config::TRAFFIC_FILE);
    let mut ledger = Ledger::load(&ledger_path);
    assert_eq!(ledger.month, current_month());
    ledger.add(1_000_000_000 - 100, 0);
    ledger.save().unwrap();

    let mut relay = start(dir.path());
    let relay_id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let old = relay.listen_addrs().await;

    // The host's handshake alone (identify, auth) uses up the rest; the relay
    // rebuilds in coordination mode and the host reconnects.
    let mut host = client();
    connect(&mut host, &addr, relay_id).await;
    let r = authenticate(&mut host, relay_id, &default_key(dir.path())).await;
    assert!(r.ok);
    let new_addr = tokio::time::timeout(WAIT, relay.listen_addrs_changed(&old))
        .await
        .expect("relay did not switch modes")
        .into_iter()
        .find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_))))
        .unwrap()
        .with(Protocol::P2p(relay_id));
    until(&mut host, |e| match e {
        SwarmEvent::ConnectionClosed {
            peer_id,
            num_established: 0,
            ..
        } if peer_id == relay_id => Some(()),
        _ => None,
    })
    .await;
    connect(&mut host, &new_addr, relay_id).await;
    match reserve(&mut host, &new_addr).await {
        Reservation::Accepted { data_limit } => {
            assert_eq!(data_limit, Some(COORDINATION_CIRCUIT_BYTES))
        }
        Reservation::Refused => panic!("refused"),
    }
    tokio::time::sleep(Duration::from_secs(6)).await;
    let status = Status::read(dir.path()).unwrap();
    assert!(status.budget_exhausted);
    assert_eq!(status.mode, config::Mode::Coordination);
    assert_eq!(status.configured_mode, config::Mode::Full);
    assert!(status.sent >= 1_000_000_000);
    relay.stop().await.unwrap();

    // The counter persisted: a restart stays in coordination mode.
    assert!(Ledger::load(&ledger_path).exhausted(1_000_000_000));
}

#[tokio::test]
async fn config_reload_applies_keys_and_limits_live() {
    let dir = data_dir("");
    let mut relay = start(dir.path());
    let relay_id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;

    // A key added while the relay runs works without a restart.
    let mut host = client();
    connect(&mut host, &addr, relay_id).await;
    let lena = config::add_key(dir.path(), "Lena").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let r = authenticate(&mut host, relay_id, &lena).await;
    assert_eq!(r, AuthResponse::accepted("Lena"));
    assert!(matches!(
        reserve(&mut host, &addr).await,
        Reservation::Accepted { .. }
    ));

    // Removing the key disconnects its host and refuses it from then on.
    let host_id = *host.local_peer_id();
    config::remove_key(dir.path(), "Lena").unwrap();
    until(&mut host, |e| match e {
        SwarmEvent::ConnectionClosed {
            peer_id,
            num_established: 0,
            ..
        } if peer_id == relay_id => Some(()),
        _ => None,
    })
    .await;
    connect(&mut host, &addr, relay_id).await;
    let r = authenticate(&mut host, relay_id, &lena).await;
    assert_eq!(r.reason.as_deref(), Some(proto::REASON_BAD_KEY));
    assert!(matches!(
        reserve(&mut host, &addr).await,
        Reservation::Refused
    ));

    // Switching to coordination mode rebuilds the swarm (new ports here, as
    // they are OS-picked); an authorized host re-reserves within the grace
    // period without authenticating again.
    let r = authenticate(&mut host, relay_id, &default_key(dir.path())).await;
    assert!(r.ok, "{r:?}");
    assert!(matches!(
        reserve(&mut host, &addr).await,
        Reservation::Accepted { .. }
    ));
    let old = relay.listen_addrs().await;
    edit_config(dir.path(), r#"mode = "coordination""#);
    let new_addr = relay
        .listen_addrs_changed(&old)
        .await
        .into_iter()
        .find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_))))
        .unwrap()
        .with(Protocol::P2p(relay_id));
    assert_ne!(new_addr, addr);
    until(&mut host, |e| match e {
        SwarmEvent::ConnectionClosed {
            peer_id,
            num_established: 0,
            ..
        } if peer_id == relay_id => Some(()),
        _ => None,
    })
    .await;
    connect(&mut host, &new_addr, relay_id).await;
    match reserve(&mut host, &new_addr).await {
        Reservation::Accepted { data_limit } => {
            assert_eq!(data_limit, Some(COORDINATION_CIRCUIT_BYTES))
        }
        Reservation::Refused => panic!("grace period did not carry the authorization"),
    }

    // A broken config is ignored; the relay keeps running on the old one.
    edit_config(dir.path(), "max_hosts = \"lots\"");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut guest = client();
    connect(&mut guest, &new_addr, relay_id).await;
    guest
        .dial(
            new_addr
                .clone()
                .with(Protocol::P2pCircuit)
                .with(Protocol::P2p(host_id)),
        )
        .unwrap();
    until_with(&mut guest, &mut host, |e| match e {
        SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == host_id => Some(()),
        SwarmEvent::OutgoingConnectionError { error, .. } => panic!("circuit failed: {error}"),
        _ => None,
    })
    .await;

    relay.stop().await.unwrap();
}

#[tokio::test]
async fn info_needs_the_key_and_a_held_request_hears_about_changes() {
    let dir = data_dir("");
    let path = dir.path().join(config::CONFIG_FILE);
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("# name = \"Studio relay\"", "name = \"Studio\"")
        .replace(
            "# contact = \"lena@example.org\"",
            "contact = \"lena@example.org\"",
        );
    std::fs::write(&path, text).unwrap();
    let mut relay = start(dir.path());
    let relay_id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let mut host = client();
    connect(&mut host, &addr, relay_id).await;

    // A wrong key learns nothing.
    let r = info(&mut host, relay_id, &[9u8; 32], 0).await;
    assert!(!r.ok);
    assert_eq!(r.reason.as_deref(), Some(proto::REASON_BAD_KEY));
    assert!(r.name.is_empty() && r.version.is_empty() && r.ips.is_empty());

    let key = default_key(dir.path());
    let r = info(&mut host, relay_id, &key, 0).await;
    assert!(r.ok);
    assert_eq!(r.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(r.mode, "full");
    assert_eq!(
        (r.name.as_str(), r.contact.as_str()),
        ("Studio", "lena@example.org")
    );
    assert_eq!(r.max_hosts, 32);
    assert_ne!(r.rev, 0);
    assert_eq!(r.notice, None);
    assert!(r.budget_resets > 0);
    assert!(r.tcp != 0, "the relay names the port it really got");

    // Nothing changed: the request waits.
    let id = ask_info(&mut host, relay_id, &key, r.rev);
    let early = tokio::time::timeout(
        Duration::from_secs(2),
        until(&mut host, |e| info_answer(e, id)),
    )
    .await;
    assert!(early.is_err(), "answered although nothing changed");

    // A notice wakes it up.
    config::set_notice(
        dir.path(),
        Some((
            "Moving servers",
            None,
            Some(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(5)),
        )),
    )
    .unwrap();
    let woke = until(&mut host, |e| info_answer(e, id)).await;
    assert_eq!(woke.notice.as_ref().unwrap().text, "Moving servers");
    assert_ne!(woke.rev, r.rev);

    // A notice whose time is over isn't sent any more.
    config::set_notice(
        dir.path(),
        Some((
            "Old",
            None,
            Some(jiff::Timestamp::now() - jiff::SignedDuration::from_hours(1)),
        )),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = info(&mut host, relay_id, &key, 0).await;
    assert_eq!(r.notice, None);

    relay.stop().await.unwrap();
}

#[tokio::test]
async fn a_stopping_relay_tells_the_hosts_that_are_listening() {
    let dir = data_dir("");
    let mut relay = start(dir.path());
    let relay_id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let mut host = client();
    connect(&mut host, &addr, relay_id).await;
    let key = default_key(dir.path());
    let r = info(&mut host, relay_id, &key, 0).await;
    assert!(!r.stopping);
    let id = ask_info(&mut host, relay_id, &key, r.rev);
    // Let the request reach the relay before it stops.
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        until(&mut host, |e| info_answer(e, id)),
    )
    .await;
    let stopping = tokio::spawn(relay.stop());
    let r = until(&mut host, |e| info_answer(e, id)).await;
    assert!(r.stopping);
    stopping.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_expired_key_is_refused_but_still_learns_whom_to_ask() {
    let dir = data_dir("");
    let path = dir.path().join(config::CONFIG_FILE);
    let text = std::fs::read_to_string(&path).unwrap().replace(
        "# contact = \"lena@example.org\"",
        "contact = \"lena@example.org\"",
    );
    std::fs::write(&path, text).unwrap();
    let mut relay = start(dir.path());
    let relay_id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());

    // Still valid, a running host holds a reservation.
    let mut host = client();
    connect(&mut host, &addr, relay_id).await;
    assert!(authenticate(&mut host, relay_id, &key).await.ok);
    assert!(matches!(
        reserve(&mut host, &addr).await,
        Reservation::Accepted { .. }
    ));

    // The date passes (here: is set in the past): it loses access.
    config::set_key_expiry(dir.path(), "default", Some("2000-01-01".parse().unwrap())).unwrap();
    until(&mut host, |e| match e {
        SwarmEvent::ConnectionClosed {
            peer_id,
            num_established: 0,
            ..
        } if peer_id == relay_id => Some(()),
        _ => None,
    })
    .await;

    connect(&mut host, &addr, relay_id).await;
    let r = authenticate(&mut host, relay_id, &key).await;
    assert_eq!(r.reason.as_deref(), Some(proto::REASON_EXPIRED));
    assert_eq!(proto::refusal_reason(&r), proto::REASON_EXPIRED);
    let r = info(&mut host, relay_id, &key, 0).await;
    assert!(!r.ok);
    assert_eq!(r.reason.as_deref(), Some(proto::REASON_EXPIRED));
    assert_eq!(r.contact, "lena@example.org");

    // Renewed: it works again.
    config::set_key_expiry(dir.path(), "default", None).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(authenticate(&mut host, relay_id, &key).await.ok);

    relay.stop().await.unwrap();
}

#[tokio::test]
async fn a_key_can_have_its_own_host_limit_and_traffic_share() {
    let dir = data_dir("");
    append_config(dir.path(), "max_hosts = 1\nmonthly_budget_gb = 1");
    let mut ledger = Ledger::load(&dir.path().join(config::TRAFFIC_FILE));
    ledger.add_key("default", 950_000_000);
    ledger.save().unwrap();
    let mut relay = start(dir.path());
    let relay_id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());

    let mut first = client();
    connect(&mut first, &addr, relay_id).await;
    assert!(authenticate(&mut first, relay_id, &key).await.ok);
    assert!(matches!(
        reserve(&mut first, &addr).await,
        Reservation::Accepted { .. }
    ));

    // The key's own limit is reached; the relay as a whole has room.
    let mut second = client();
    connect(&mut second, &addr, relay_id).await;
    let id = {
        let me = *second.local_peer_id();
        let proof = proto::auth_proof(&key, &relay_id, &me).to_vec();
        second
            .behaviour_mut()
            .auth
            .send_request(&relay_id, AuthRequest { proof })
    };
    let r = until_with(&mut second, &mut first, |e| match e {
        SwarmEvent::Behaviour(ClientEvent::Auth(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        })) if request_id == id => Some(response),
        _ => None,
    })
    .await;
    assert_eq!(r.reason.as_deref(), Some(proto::REASON_FULL));

    // The share shows in the answer, and what this host caused is counted.
    let id = ask_info(&mut first, relay_id, &key, 0);
    let r = until(&mut first, |e| info_answer(e, id)).await;
    assert_eq!(r.key_budget_percent, 95);
    assert!(!r.key_budget_exhausted);
    assert_eq!(r.budget_percent, 0);
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            tokio::select! {
                _ = first.select_next_some() => {}
                _ = second.select_next_some() => {}
            }
        }
    })
    .await
    .unwrap_err();
    let status = Status::read(dir.path()).unwrap();
    let usage = status.keys.iter().find(|k| k.label == "default").unwrap();
    assert!(usage.sent > 950_000_000, "{usage:?}");
    assert_eq!(usage.budget, 1_000_000_000);

    relay.stop().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn the_alert_command_hears_about_the_budget_once_a_month() {
    let dir = data_dir("monthly_budget_gb = 1");
    let out = dir.path().join("alerts");
    append_config(
        dir.path(),
        &format!(
            "[alerts]\ncommand = 'echo \"$REAMESH_RELAY_ALERT: $REAMESH_RELAY_MESSAGE\" >> {}'\nbudget_percent = 80",
            out.display()
        ),
    );
    let path = dir.path().join(config::CONFIG_FILE);
    // The template has its own [alerts]; a second would not parse.
    let text = std::fs::read_to_string(&path).unwrap().replacen(
        "[alerts]\n# Runs through the shell",
        "# Runs through the shell",
        1,
    );
    let text = text.replacen(
        &format!("budget_percent = {}\n", config::DEFAULT_ALERT_PERCENT),
        "",
        1,
    );
    std::fs::write(&path, text).unwrap();
    let ledger_path = dir.path().join(config::TRAFFIC_FILE);
    let mut ledger = Ledger::load(&ledger_path);
    ledger.add(850_000_000, 0);
    ledger.save().unwrap();

    let mut relay = start(dir.path());
    let _ = relay_tcp_addr(&mut relay).await;
    let read = || std::fs::read_to_string(&out).unwrap_or_default();
    tokio::time::timeout(WAIT, async {
        while !read().contains("budget_warning") {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the alert command did not run");
    assert!(
        read().contains("85 % of the monthly budget of 1 GB used"),
        "{}",
        read()
    );
    relay.stop().await.unwrap();

    // A restart in the same month stays quiet.
    let mut relay = start(dir.path());
    let _ = relay_tcp_addr(&mut relay).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(read().matches("budget_warning").count(), 1, "{}", read());
    relay.stop().await.unwrap();
}

#[test]
fn info_messages_survive_fields_the_other_side_does_not_know() {
    // Serde ignores unknown fields and defaults missing ones, which is what
    // lets relay and app be updated apart.
    let older: InfoResponse = serde_json::from_str(r#"{"ok":true,"future":1}"#).unwrap();
    assert!(older.ok);
    assert_eq!(older.notice, None);
}
