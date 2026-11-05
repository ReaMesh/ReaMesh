//! The optional session list, in-process against a real relay on loopback:
//! client swarms ask for the list, reserve and publish over TCP.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, OutboundRequestId, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{Multiaddr, PeerId, StreamProtocol, Swarm, noise, relay, tcp, yamux};
use reamesh_relay::config::{self, Config};
use reamesh_relay::server::{self, Options, RelayHandle};
use reamesh_relay::status::Status;
use reamesh_relay_proto::sessions::{
    self as list, PublishRequest, PublishResponse, SessionEntry, SessionsRequest, SessionsResponse,
    open_entry, seal_entry,
};
use reamesh_relay_proto::{self as proto, AuthRequest, AuthResponse};

const WAIT: Duration = Duration::from_secs(15);
const SESSION_NAME: &str = "Secret Song Title";

#[derive(NetworkBehaviour)]
struct Client {
    relay: relay::client::Behaviour,
    auth: request_response::cbor::Behaviour<AuthRequest, AuthResponse>,
    sessions: request_response::cbor::Behaviour<SessionsRequest, SessionsResponse>,
    publish: request_response::cbor::Behaviour<PublishRequest, PublishResponse>,
}

struct Peer {
    key: Keypair,
    swarm: Swarm<Client>,
}

impl Peer {
    fn id(&self) -> PeerId {
        *self.swarm.local_peer_id()
    }
}

fn outbound<A, B>(protocol: &'static str, timeout: u64) -> request_response::cbor::Behaviour<A, B>
where
    A: Send + Clone + serde::Serialize + serde::de::DeserializeOwned + 'static,
    B: Send + Clone + serde::Serialize + serde::de::DeserializeOwned + 'static,
{
    request_response::cbor::Behaviour::new(
        [(StreamProtocol::new(protocol), ProtocolSupport::Outbound)],
        request_response::Config::default().with_request_timeout(Duration::from_secs(timeout)),
    )
}

async fn peer(relay_addr: &Multiaddr, relay_id: PeerId) -> Peer {
    let key = Keypair::generate_ed25519();
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(key.clone())
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
            auth: outbound(proto::RELAY_AUTH_PROTOCOL, 10),
            sessions: outbound(list::RELAY_SESSIONS_PROTOCOL, list::SESSIONS_TIMEOUT_SECS),
            publish: outbound(list::RELAY_PUBLISH_PROTOCOL, 10),
        })
        .unwrap()
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();
    swarm.dial(relay_addr.clone()).unwrap();
    let mut p = Peer { key, swarm };
    until(&mut p.swarm, |e| match e {
        SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == relay_id => Some(()),
        SwarmEvent::OutgoingConnectionError { error, .. } => panic!("dial failed: {error}"),
        _ => None,
    })
    .await;
    p
}

fn data_dir(session_list: bool, extra: &str) -> tempfile::TempDir {
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
        )
        .replace(
            "session_list = false",
            &format!("session_list = {session_list}"),
        )
        .replace("max_hosts = 32", extra);
    std::fs::write(path, text).unwrap();
    dir
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

fn ask(p: &mut Peer, relay: PeerId, proof: Vec<u8>, rev: u64) -> OutboundRequestId {
    p.swarm
        .behaviour_mut()
        .sessions
        .send_request(&relay, SessionsRequest { proof, rev })
}

async fn answer(p: &mut Peer, id: OutboundRequestId) -> SessionsResponse {
    until(&mut p.swarm, |e| match e {
        SwarmEvent::Behaviour(ClientEvent::Sessions(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        })) if request_id == id => Some(response),
        SwarmEvent::Behaviour(ClientEvent::Sessions(request_response::Event::OutboundFailure {
            error,
            ..
        })) => panic!("list request failed: {error}"),
        _ => None,
    })
    .await
}

fn proof(p: &Peer, relay: PeerId, key: &[u8; 32]) -> Vec<u8> {
    proto::auth_proof(key, &relay, &p.id()).to_vec()
}

async fn read_list(p: &mut Peer, relay: PeerId, key: &[u8; 32]) -> SessionsResponse {
    let proof = proof(p, relay, key);
    let id = ask(p, relay, proof, 0);
    answer(p, id).await
}

async fn publish(
    p: &mut Peer,
    relay: PeerId,
    key: &[u8; 32],
    envelope: Option<Vec<u8>>,
) -> PublishResponse {
    let proof = proof(p, relay, key);
    let id = p
        .swarm
        .behaviour_mut()
        .publish
        .send_request(&relay, PublishRequest { proof, envelope });
    until(&mut p.swarm, |e| match e {
        SwarmEvent::Behaviour(ClientEvent::Publish(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        })) if request_id == id => Some(response),
        SwarmEvent::Behaviour(ClientEvent::Publish(request_response::Event::OutboundFailure {
            error,
            ..
        })) => panic!("publish failed: {error}"),
        _ => None,
    })
    .await
}

async fn publish_own(p: &mut Peer, relay: PeerId, key: &[u8; 32]) -> PublishResponse {
    let env = sealed(p, relay);
    publish(p, relay, key, Some(env)).await
}

/// Authenticates and reserves, as a hosting run does.
async fn become_host(p: &mut Peer, relay: PeerId, addr: &Multiaddr, key: &[u8; 32]) {
    let proof = proof(p, relay, key);
    let id = p
        .swarm
        .behaviour_mut()
        .auth
        .send_request(&relay, AuthRequest { proof });
    let ok = until(&mut p.swarm, |e| match e {
        SwarmEvent::Behaviour(ClientEvent::Auth(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        })) if request_id == id => Some(response.ok),
        _ => None,
    })
    .await;
    assert!(ok, "auth refused");
    p.swarm
        .listen_on(addr.clone().with(Protocol::P2pCircuit))
        .unwrap();
    until(&mut p.swarm, |e| match e {
        SwarmEvent::Behaviour(ClientEvent::Relay(
            relay::client::Event::ReservationReqAccepted { .. },
        )) => Some(()),
        SwarmEvent::ListenerClosed { reason: Err(e), .. } => panic!("reservation refused: {e}"),
        _ => None,
    })
    .await;
}

fn entry_of(p: &Peer, relay: PeerId) -> SessionEntry {
    let owner = p.id();
    SessionEntry {
        v: SessionEntry::VERSION,
        relay: relay.to_string(),
        session_id: "abcd1234".into(),
        session_name: SESSION_NAME.into(),
        owner: owner.to_string(),
        owner_name: "Sam".into(),
        owner_color: "#f0a35e".into(),
        auth: false,
        salt: String::new(),
        files: 3,
        bytes: 1000,
        proto: 1,
        ver: "1.2.0".into(),
        ticket: format!("reamesh://join/abcd1234?p={owner}&a=&r={relay}~127.0.0.1~1~1"),
        extra: Default::default(),
    }
}

fn sealed(p: &Peer, relay: PeerId) -> Vec<u8> {
    seal_entry(&p.key, &entry_of(p, relay)).unwrap()
}

fn default_key(dir: &Path) -> [u8; 32] {
    Config::load(dir).unwrap().key("default").unwrap()
}

#[tokio::test]
async fn switching_the_list_applies_without_a_restart_and_off_forgets() {
    let dir = data_dir(false, "max_hosts = 32");
    let mut relay = start(dir.path());
    let id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());

    // Off, the standard: nothing to see, and nothing to publish to.
    let mut viewer = peer(&addr, id).await;
    let r = read_list(&mut viewer, id, &key).await;
    assert!(r.ok && !r.enabled && r.entries.is_empty());
    let mut host = peer(&addr, id).await;
    become_host(&mut host, id, &addr, &key).await;
    let env = sealed(&host, id);
    let refused = publish(&mut host, id, &key, Some(env.clone())).await;
    assert_eq!(refused.reason.as_deref(), Some(list::REASON_DISABLED));

    // A viewer waiting on the current state hears about `list on` by itself.
    let held = {
        let p = proof(&viewer, id, &key);
        ask(&mut viewer, id, p, r.rev)
    };
    config::set_session_list(dir.path(), true).unwrap();
    let r = answer(&mut viewer, held).await;
    assert!(r.ok && r.enabled && r.entries.is_empty());

    assert!(publish(&mut host, id, &key, Some(env)).await.ok);
    let r = read_list(&mut viewer, id, &key).await;
    assert_eq!(r.entries.len(), 1);
    assert_eq!(r.entries[0].key_label, "default");
    let opened = open_entry(&r.entries[0].envelope, &id).unwrap();
    assert_eq!(opened.session_name, SESSION_NAME);

    // Off again: entries are gone at once, and the viewer is told.
    let held = {
        let p = proof(&viewer, id, &key);
        ask(&mut viewer, id, p, r.rev)
    };
    config::set_session_list(dir.path(), false).unwrap();
    let r = answer(&mut viewer, held).await;
    assert!(r.ok && !r.enabled && r.entries.is_empty());
    relay.stop().await.unwrap();
}

#[tokio::test]
async fn nobody_without_a_valid_proof_sees_anything() {
    let dir = data_dir(true, "max_hosts = 32");
    let mut relay = start(dir.path());
    let id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());
    let mut host = peer(&addr, id).await;
    become_host(&mut host, id, &addr, &key).await;
    assert!(publish_own(&mut host, id, &key).await.ok);

    let mut stranger = peer(&addr, id).await;
    let proofs = [
        Vec::new(),
        vec![0u8; 32],
        proto::auth_proof(&[0x42; 32], &id, &stranger.id()).to_vec(),
        // The right key, but a proof someone else made for themselves: a
        // proof overheard or passed on is worth nothing.
        proof(&host, id, &key),
    ];
    for p in proofs {
        let req = ask(&mut stranger, id, p, 0);
        let r = answer(&mut stranger, req).await;
        assert!(!r.ok);
        assert_eq!(r.reason.as_deref(), Some(proto::REASON_BAD_KEY));
        assert!(r.entries.is_empty() && !r.enabled);
    }
    // A stranger can't publish or withdraw either.
    let r = publish(&mut stranger, id, &[0x42; 32], None).await;
    assert_eq!(r.reason.as_deref(), Some(proto::REASON_BAD_KEY));
    let mut viewer = peer(&addr, id).await;
    assert_eq!(read_list(&mut viewer, id, &key).await.entries.len(), 1);
    relay.stop().await.unwrap();
}

#[tokio::test]
async fn only_a_host_with_a_reservation_publishes_and_the_entry_ends_with_it() {
    let dir = data_dir(true, "max_hosts = 32");
    let mut relay = start(dir.path());
    let id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());

    let mut host = peer(&addr, id).await;
    let env = sealed(&host, id);
    let r = publish(&mut host, id, &key, Some(env.clone())).await;
    assert_eq!(r.reason.as_deref(), Some(list::REASON_NOT_RESERVED));

    become_host(&mut host, id, &addr, &key).await;
    // Someone else's signature, on a reserved host's connection.
    let other = peer(&addr, id).await;
    let r = publish(&mut host, id, &key, Some(sealed(&other, id))).await;
    assert_eq!(r.reason.as_deref(), Some(list::REASON_BAD_ENTRY));
    let r = publish(&mut host, id, &key, Some(vec![0; list::ENTRY_MAX_BYTES + 1])).await;
    assert_eq!(r.reason.as_deref(), Some(list::REASON_TOO_LARGE));
    assert!(publish(&mut host, id, &key, Some(env.clone())).await.ok);

    // Withdrawing during the session wakes a waiting viewer.
    let mut viewer = peer(&addr, id).await;
    let r = read_list(&mut viewer, id, &key).await;
    assert_eq!(r.entries.len(), 1);
    let held = {
        let p = proof(&viewer, id, &key);
        ask(&mut viewer, id, p, r.rev)
    };
    assert!(publish(&mut host, id, &key, None).await.ok);
    let r = answer(&mut viewer, held).await;
    assert!(r.entries.is_empty());

    // Listed again, the entry ends when the host goes.
    assert!(publish(&mut host, id, &key, Some(env)).await.ok);
    let r = read_list(&mut viewer, id, &key).await;
    assert_eq!(r.entries.len(), 1);
    let held = {
        let p = proof(&viewer, id, &key);
        ask(&mut viewer, id, p, r.rev)
    };
    drop(host);
    let r = answer(&mut viewer, held).await;
    assert!(r.entries.is_empty());
    relay.stop().await.unwrap();
}

#[tokio::test]
async fn revoking_a_key_drops_its_entries_and_ends_its_waiting_requests() {
    let dir = data_dir(true, "max_hosts = 32");
    let mut relay = start(dir.path());
    let id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());
    let lena = config::add_key(dir.path(), "lena").unwrap();
    // The relay sees the new key once it re-read the file.
    let mut viewer = peer(&addr, id).await;
    tokio::time::timeout(WAIT, async {
        while !read_list(&mut viewer, id, &lena).await.ok {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();

    let mut lena_host = peer(&addr, id).await;
    become_host(&mut lena_host, id, &addr, &lena).await;
    assert!(publish_own(&mut lena_host, id, &lena).await.ok);
    let mut default_host = peer(&addr, id).await;
    become_host(&mut default_host, id, &addr, &key).await;
    assert!(publish_own(&mut default_host, id, &key).await.ok);

    // Two keys, one list: each sees the other's entry.
    let r = read_list(&mut viewer, id, &lena).await;
    assert_eq!(r.entries.len(), 2);
    let labels: Vec<&str> = r.entries.iter().map(|e| e.key_label.as_str()).collect();
    assert!(labels.contains(&"lena") && labels.contains(&"default"));

    let mut other_viewer = peer(&addr, id).await;
    let r2 = read_list(&mut other_viewer, id, &key).await;
    let (held, other_held) = {
        let p = proof(&viewer, id, &lena);
        let held = ask(&mut viewer, id, p, r.rev);
        let p = proof(&other_viewer, id, &key);
        (held, ask(&mut other_viewer, id, p, r2.rev))
    };

    config::remove_key(dir.path(), "lena").unwrap();

    let refused = answer(&mut viewer, held).await;
    assert!(!refused.ok);
    assert_eq!(refused.reason.as_deref(), Some(proto::REASON_BAD_KEY));
    assert!(refused.entries.is_empty());
    let left = answer(&mut other_viewer, other_held).await;
    assert_eq!(left.entries.len(), 1);
    assert_eq!(left.entries[0].key_label, "default");
    // Asking again doesn't help.
    assert!(!read_list(&mut viewer, id, &lena).await.ok);
    relay.stop().await.unwrap();
}

#[tokio::test]
async fn a_key_past_its_last_day_sees_nothing_and_its_entry_goes() {
    let dir = data_dir(true, "max_hosts = 32");
    let mut relay = start(dir.path());
    let id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());
    let lena = config::add_key(dir.path(), "lena").unwrap();
    let mut viewer = peer(&addr, id).await;
    tokio::time::timeout(WAIT, async {
        while !read_list(&mut viewer, id, &lena).await.ok {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let mut host = peer(&addr, id).await;
    become_host(&mut host, id, &addr, &lena).await;
    assert!(publish_own(&mut host, id, &lena).await.ok);
    let mut other = peer(&addr, id).await;
    let r = read_list(&mut other, id, &key).await;
    assert_eq!(r.entries.len(), 1);
    let (held, other_held) = {
        let p = proof(&viewer, id, &lena);
        let held = ask(&mut viewer, id, p, r.rev);
        let p = proof(&other, id, &key);
        (held, ask(&mut other, id, p, r.rev))
    };

    let yesterday = config::local_today().yesterday().unwrap();
    config::set_key_expiry(dir.path(), "lena", Some(yesterday)).unwrap();

    let refused = answer(&mut viewer, held).await;
    assert!(!refused.ok);
    assert_eq!(refused.reason.as_deref(), Some(proto::REASON_EXPIRED));
    assert!(refused.entries.is_empty());
    let left = answer(&mut other, other_held).await;
    assert!(left.entries.is_empty(), "the expired key's entry stayed");
    relay.stop().await.unwrap();
}

#[tokio::test]
async fn viewers_take_no_host_places() {
    let dir = data_dir(true, "max_hosts = 1");
    let mut relay = start(dir.path());
    let id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());

    // Three viewers, each with a request held open, and the one host place
    // is still free.
    let mut viewers = Vec::new();
    for _ in 0..3 {
        let mut v = peer(&addr, id).await;
        let r = read_list(&mut v, id, &key).await;
        assert!(r.ok);
        let p = proof(&v, id, &key);
        let held = ask(&mut v, id, p, r.rev);
        viewers.push((v, held));
    }
    let mut host = peer(&addr, id).await;
    become_host(&mut host, id, &addr, &key).await;
    assert!(publish_own(&mut host, id, &key).await.ok);
    for (v, held) in &mut viewers {
        let r = answer(v, *held).await;
        assert_eq!(r.entries.len(), 1);
    }
    relay.stop().await.unwrap();
}

/// A `tracing` writer that keeps everything for the assert.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn session_names_stay_out_of_the_log_and_status() {
    // For the whole process: tracing caches per call site whether anyone
    // listens, so a subscriber on this thread alone would miss logs depending
    // on what ran first. The other tests list the same name, and it mustn't
    // show there either.
    let log = Captured::default();
    let sink = log.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("reamesh_relay=trace"))
            .with_writer(move || sink.clone())
            .finish(),
    )
    .unwrap();

    let dir = data_dir(true, "max_hosts = 32");
    let mut relay = start(dir.path());
    let id = relay.peer_id;
    let addr = relay_tcp_addr(&mut relay).await;
    let key = default_key(dir.path());
    let mut host = peer(&addr, id).await;
    become_host(&mut host, id, &addr, &key).await;
    assert!(publish_own(&mut host, id, &key).await.ok);
    let mut viewer = peer(&addr, id).await;
    assert_eq!(read_list(&mut viewer, id, &key).await.entries.len(), 1);

    // status.json comes every few seconds; `status` reads counts from it.
    let status = tokio::time::timeout(WAIT, async {
        loop {
            tokio::select! {
                _ = host.swarm.select_next_some() => {}
                _ = viewer.swarm.select_next_some() => {}
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if let Some(st) = Status::read(dir.path()).filter(|s| s.listed == 1) {
                        return st;
                    }
                }
            }
        }
    })
    .await
    .expect("status.json never showed the entry");
    assert!(status.session_list);
    assert!(status.hosts[0].listed);

    let raw = std::fs::read_to_string(dir.path().join(config::STATUS_FILE)).unwrap();
    assert!(!raw.contains(SESSION_NAME));
    assert!(!raw.contains("abcd1234"));
    relay.stop().await.unwrap();

    let logged = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(!logged.is_empty(), "the capture caught nothing");
    assert!(!logged.contains(SESSION_NAME));
    assert!(!logged.contains("abcd1234"));
}
