//! Internet sessions through a relay, end to end over loopback.
//!
//! The relay is built here rather than taken from the `reamesh-relay` crate:
//! libp2p's circuit relay v2 server plus the `/reamesh/relay-auth/1` exchange,
//! with reservations admitted only after a valid proof. That pins what the
//! client expects of any relay. Run serially like the session tests
//! (`--test-threads=1`).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use libp2p::futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{Multiaddr, PeerId, StreamProtocol, identify, noise, ping, relay, tcp, yamux};
use reamesh::Reamesh;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const KEY: [u8; 32] = [7u8; 32];

#[derive(Debug, Serialize, Deserialize)]
struct AuthRequest {
    #[serde(with = "serde_bytes")]
    proof: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthResponse {
    ok: bool,
    reason: Option<String>,
    key_label: Option<String>,
}

/// What a test relay says about itself, shared with the test.
type Said = Arc<Mutex<Option<reamesh_relay_proto::InfoResponse>>>;

#[derive(NetworkBehaviour)]
struct RelayBehaviour {
    relay: relay::Behaviour,
    auth: request_response::cbor::Behaviour<AuthRequest, AuthResponse>,
    info: request_response::cbor::Behaviour<reamesh_relay_proto::InfoRequest, reamesh_relay_proto::InfoResponse>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
}

/// A relay running on its own thread until dropped.
struct TestRelay {
    peer: PeerId,
    tcp: u16,
    quic: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// `Some`: the relay answers `/reamesh/relay-info/1` with this. Set by the
    /// test at any time; a held request is answered when it changes.
    said: Said,
}

impl TestRelay {
    /// The relay code a user would paste, carrying `key`.
    fn code(&self, key: &[u8; 32]) -> String {
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "reamesh://relay/{}?a=127.0.0.1&t={}&q={}&k={hex}&n=Test%20Relay",
            self.peer, self.tcp, self.quic
        )
    }
}

impl Drop for TestRelay {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A relay from before the info protocol.
fn start_relay(key: [u8; 32]) -> TestRelay {
    start_relay_with(key, false)
}

fn start_relay_with(key: [u8; 32], speaks_info: bool) -> TestRelay {
    let said: Said = Arc::default();
    let said_by_relay = said.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<(PeerId, u16, u16)>();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let authed: Arc<Mutex<HashSet<PeerId>>> = Arc::default();
            let gate = authed.clone();
            let mut swarm = libp2p::SwarmBuilder::with_new_identity()
                .with_tokio()
                .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)
                .unwrap()
                .with_quic()
                .with_behaviour(|key| {
                    let config = relay::Config {
                        // Only a peer that proved the key may reserve.
                        reservation_rate_limiters: vec![Box::new(
                            move |peer: PeerId, _: &Multiaddr, _: Instant| {
                                gate.lock().unwrap().contains(&peer)
                            },
                        )],
                        circuit_src_rate_limiters: Vec::new(),
                        max_circuit_duration: Duration::from_secs(3600),
                        max_circuit_bytes: 0,
                        ..Default::default()
                    };
                    RelayBehaviour {
                        relay: relay::Behaviour::new(key.public().to_peer_id(), config),
                        auth: request_response::cbor::Behaviour::new(
                            [(StreamProtocol::new("/reamesh/relay-auth/1"), ProtocolSupport::Inbound)],
                            request_response::Config::default(),
                        ),
                        info: request_response::cbor::Behaviour::new(
                            speaks_info.then(|| {
                                (StreamProtocol::new(reamesh_relay_proto::RELAY_INFO_PROTOCOL), ProtocolSupport::Inbound)
                            }),
                            request_response::Config::default().with_request_timeout(Duration::from_secs(300)),
                        ),
                        identify: identify::Behaviour::new(identify::Config::new(
                            "/reamesh-relay/1".into(),
                            key.public(),
                        )),
                        ping: ping::Behaviour::new(ping::Config::new()),
                    }
                })
                .unwrap()
                .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(3600)))
                .build();
            swarm.listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap()).unwrap();
            swarm.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()).unwrap();
            let me = *swarm.local_peer_id();
            let (mut tcp_port, mut quic_port, mut sent) = (0u16, 0u16, false);
            let mut held = Vec::new();
            let mut poll = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    _ = poll.tick() => {
                        // A held info request is answered once the answer differs.
                        let now = said_by_relay.lock().unwrap().clone();
                        if let Some(mut r) = now {
                            r.rev = r.content_rev();
                            let mut still = Vec::new();
                            for (rev, channel) in held.drain(..) {
                                if rev == r.rev {
                                    still.push((rev, channel));
                                } else {
                                    let _ = swarm.behaviour_mut().info.send_response(channel, r.clone());
                                }
                            }
                            held = still;
                        }
                    }
                    ev = swarm.select_next_some() => match ev {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            swarm.add_external_address(address.clone());
                            for p in address.iter() {
                                match p {
                                    Protocol::Tcp(port) => tcp_port = port,
                                    Protocol::Udp(port) => quic_port = port,
                                    _ => {}
                                }
                            }
                            if !sent && tcp_port != 0 && quic_port != 0 {
                                sent = true;
                                let _ = ready_tx.send((me, tcp_port, quic_port));
                            }
                        }
                        SwarmEvent::Behaviour(RelayBehaviourEvent::Auth(request_response::Event::Message {
                            peer,
                            message: request_response::Message::Request { request, channel, .. },
                            ..
                        })) => {
                            let mut msg = b"reamesh-relay-auth/1".to_vec();
                            msg.extend_from_slice(&me.to_bytes());
                            msg.extend_from_slice(&peer.to_bytes());
                            let ok = blake3::keyed_hash(&key, &msg).as_bytes()[..] == request.proof[..];
                            if ok {
                                authed.lock().unwrap().insert(peer);
                            }
                            let resp = AuthResponse {
                                ok,
                                reason: (!ok).then(|| "bad_key".to_string()),
                                key_label: ok.then(|| "Test key".to_string()),
                            };
                            let _ = swarm.behaviour_mut().auth.send_response(channel, resp);
                        }
                        SwarmEvent::Behaviour(RelayBehaviourEvent::Info(request_response::Event::Message {
                            message: request_response::Message::Request { request, channel, .. },
                            ..
                        })) => {
                            let now = said_by_relay.lock().unwrap().clone();
                            match now {
                                Some(mut r) => {
                                    r.rev = r.content_rev();
                                    if request.rev == r.rev {
                                        held.push((request.rev, channel));
                                    } else {
                                        let _ = swarm.behaviour_mut().info.send_response(channel, r);
                                    }
                                }
                                None => drop(channel),
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
    });
    let (peer, tcp, quic) = ready_rx.recv_timeout(Duration::from_secs(10)).expect("relay never listened");
    TestRelay { peer, tcp, quic, stop: Some(stop_tx), thread: Some(thread), said }
}

fn host(name: &str, dir: &Path, pass: &str) -> Box<Reamesh> {
    reamesh::set_loopback_only(true);
    Reamesh::host(name, dir.to_str().unwrap(), "Song.rpp", pass).expect("host failed")
}

fn join(staging: &Path) -> Box<Reamesh> {
    reamesh::set_loopback_only(true);
    Reamesh::join(staging.to_str().unwrap()).expect("join failed")
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("reamesh-relay-{tag}-{}-{nanos:x}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_project(dir: &Path) {
    fs::write(
        dir.join("Song.rpp"),
        "<REAPER_PROJECT 0.1\n<TRACK\n  <ITEM\n    POSITION 0\n    <SOURCE WAVE\n      FILE \"Audio/kick.wav\"\n    >\n  >\n>\n>\n",
    )
    .unwrap();
    fs::create_dir_all(dir.join("Audio")).unwrap();
    // Several transfer chunks, so the download really streams through the circuit.
    let kick: Vec<u8> = (0..(9 * 1024 * 1024 + 17)).map(|i| (i % 251) as u8).collect();
    fs::write(dir.join("Audio/kick.wav"), kick).unwrap();
}

fn status(r: &Reamesh) -> Value {
    serde_json::from_str(&r.status_json()).unwrap()
}

/// The relay entry for `relay` in the host's relay status, once present.
fn relay_state(relay: &PeerId) -> Option<Value> {
    let v: Value = serde_json::from_str(&reamesh::relay_status_json()).unwrap();
    v.as_array()?.iter().find(|e| e["peer"] == relay.to_string()).cloned()
}

/// A ticket with the host's direct addresses taken out: only the relay can
/// reach it.
fn relay_only(ticket: &str) -> String {
    let (head, query) = ticket.split_once('?').unwrap();
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !(p.starts_with("a=") || p.starts_with("t=") || p.starts_with("q=")))
        .collect();
    format!("{head}?{}", kept.join("&"))
}

fn without_secret(ticket: &str) -> String {
    let (head, query) = ticket.split_once('?').unwrap();
    let kept: Vec<&str> = query.split('&').filter(|p| !p.starts_with("x=")).collect();
    format!("{head}?{}", kept.join("&"))
}

/// Host over `project` with `relay` configured and wait for the reservation
/// and a ticket that names it.
fn host_on_relay(relay: &TestRelay, name: &str, project: &Path, pass: &str) -> (Box<Reamesh>, String) {
    reamesh::set_loopback_only(true);
    assert_eq!(reamesh::set_relays(&[relay.code(&KEY)]), 1, "relay code refused");
    let owner = host(name, project, pass);
    let reserved = wait_until(Duration::from_secs(20), || {
        relay_state(&relay.peer).is_some_and(|s| s["state"] == "reserved")
    });
    assert!(reserved, "host never reserved: {}", reamesh::relay_status_json());
    let mut ticket = String::new();
    assert!(wait_until(Duration::from_secs(10), || {
        ticket = owner.session_ticket();
        !ticket.is_empty()
    }));
    (owner, ticket)
}

#[test]
fn host_reserves_on_authed_relay_and_guest_joins_through_the_circuit() {
    let relay = start_relay(KEY);
    // The Settings test reaches the relay and has the key accepted.
    let (outcome, label) = {
        reamesh::set_loopback_only(true);
        reamesh::relay_test_blocking(&relay.code(&KEY))
    };
    assert_eq!((outcome.as_str(), label.as_str()), ("ok", "Test key"));

    let project = temp_dir("proj");
    let staging = temp_dir("staging");
    make_project(&project);
    // An open session: with a relay configured it gets a link secret.
    let (owner, ticket) = host_on_relay(&relay, "relay-session", &project, "");
    let state = relay_state(&relay.peer).unwrap();
    assert_eq!(state["label"], "Test key");
    let key_hex = hex(&KEY);
    assert!(!ticket.contains(&key_hex), "the relay key leaked into the invite link: {ticket}");
    assert!(ticket.contains(&format!("&r={}~", relay.peer)), "{ticket}");
    assert!(ticket.contains("&x="), "open relay session without a link secret: {ticket}");
    assert!(!ticket.contains("s=1"), "an open session must not ask for a passphrase: {ticket}");
    assert!(!ticket.contains("p2p-circuit"), "{ticket}");

    // The guest has no route but the relay.
    let guest = join(&staging);
    let ticket = relay_only(&ticket);
    assert!(guest.check_ticket(&ticket, ""));
    let checked = wait_until(Duration::from_secs(45), || status(&guest)["probe"]["ok"] == true);
    assert!(checked, "check through the relay never passed: {}", guest.status_json());

    assert!(guest.join_ticket(&ticket, ""), "join_ticket refused");
    let ready = wait_until(Duration::from_secs(60), || {
        let s = status(&guest);
        assert_ne!(s["state"], "failed", "relayed join failed: {}", s["message"]);
        s["state"] == "ready"
    });
    assert!(ready, "relayed join never completed: {}", guest.status_json());
    assert!(guest.relayed_connections() >= 1, "the join never went through the relay");
    let path = PathBuf::from(status(&guest)["project_path"].as_str().unwrap());
    assert_eq!(fs::read(&path).unwrap(), fs::read(project.join("Song.rpp")).unwrap());
    let media = path.parent().unwrap().join("Audio/kick.wav");
    assert_eq!(fs::read(&media).unwrap(), fs::read(project.join("Audio/kick.wav")).unwrap());

    // Live gossip works over it too: each sees the other in its roster, and
    // the host can tell the guest came in through the relay (or, once the
    // hole punch upgraded it, directly).
    let guest_id = guest.local_peer_id().to_string();
    let seen = wait_until(Duration::from_secs(20), || {
        let peers: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        peers.as_array().unwrap().iter().any(|p| {
            p["id"] == guest_id && (p["link"] == "relay" || p["link"] == "direct")
        })
    });
    assert!(seen, "host roster never showed the relayed guest: {}", owner.peers_json());

    drop(guest);
    drop(owner);
    reamesh::set_relays(&[]);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn bad_key_is_refused_and_nothing_is_reserved() {
    let relay = start_relay(KEY);
    let wrong = [9u8; 32];
    reamesh::set_loopback_only(true);
    let (outcome, _) = reamesh::relay_test_blocking(&relay.code(&wrong));
    assert_eq!(outcome, "bad_key");
    // A relay nobody listens on.
    let dead = TestRelay { peer: PeerId::random(), tcp: 9, quic: 9, stop: None, thread: None, said: Arc::default() };
    let (outcome, _) = reamesh::relay_test_blocking(&dead.code(&KEY));
    assert!(outcome == "unreachable" || outcome == "timeout", "{outcome}");
    assert_eq!(reamesh::relay_test_blocking("reamesh://relay/garbage").0, "invalid");

    let project = temp_dir("proj-badkey");
    make_project(&project);
    assert_eq!(reamesh::set_relays(&[relay.code(&wrong)]), 1);
    let owner = host("bad-key", &project, "hunter2");
    let refused = wait_until(Duration::from_secs(20), || {
        relay_state(&relay.peer).is_some_and(|s| s["state"] == "refused" && s["reason"] == "bad_key")
    });
    assert!(refused, "a wrong key was not refused: {}", reamesh::relay_status_json());
    // It stays refused: no retry with the same wrong key, no reservation.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(relay_state(&relay.peer).unwrap()["state"], "refused");

    drop(owner);
    reamesh::set_relays(&[]);
    assert_eq!(reamesh::relay_status_json(), "[]", "status must clear when hosting stops");
    let _ = fs::remove_dir_all(project);
}

#[test]
fn guest_without_relay_reachability_fails_cleanly() {
    let relay = start_relay(KEY);
    let project = temp_dir("proj-unreach");
    let staging = temp_dir("staging-unreach");
    make_project(&project);
    let (owner, ticket) = host_on_relay(&relay, "unreachable", &project, "hunter2");
    let ticket = relay_only(&ticket);
    // Host and relay both gone: the link's only route is dead.
    drop(owner);
    drop(relay);
    reamesh::set_relays(&[]);

    let guest = join(&staging);
    assert!(guest.join_ticket(&ticket, "hunter2"));
    let failed = wait_until(Duration::from_secs(45), || status(&guest)["state"] == "failed");
    assert!(failed, "relay-only join against a dead relay never failed: {}", guest.status_json());
    assert_eq!(status(&guest)["message"], "ticket host unreachable");
    assert!(guest.check_ticket(&ticket, "hunter2"));
    let resolved = wait_until(Duration::from_secs(45), || status(&guest)["probe"]["ok"] == false);
    assert!(resolved, "check against a dead relay never resolved");
    assert_eq!(status(&guest)["probe"]["reason"], "unreachable");

    drop(guest);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn open_session_over_relay_requires_the_link_secret() {
    let relay = start_relay(KEY);
    let project = temp_dir("proj-secret");
    let staging = temp_dir("staging-secret");
    let lan_staging = temp_dir("staging-secret-lan");
    make_project(&project);
    let (owner, ticket) = host_on_relay(&relay, "open-over-relay", &project, "");
    let stripped = without_secret(&relay_only(&ticket));

    // Through the relay without the secret: the host refuses it.
    // Not on the host's LAN: in this loopback test a hole-punched connection
    // would otherwise count as one, and hand it the LAN announce.
    let stranger = join(&staging);
    stranger.leave_lobby();
    assert!(stranger.check_ticket(&stripped, ""));
    let rejected = wait_until(Duration::from_secs(45), || status(&stranger)["probe"]["ok"] == false);
    assert!(rejected, "check without the secret never resolved: {}", stranger.status_json());
    assert_eq!(status(&stranger)["probe"]["reason"], "rejected");
    assert!(stranger.join_ticket(&stripped, ""));
    let failed = wait_until(Duration::from_secs(45), || status(&stranger)["state"] == "failed");
    assert!(failed, "join without the secret didn't fail: {}", stranger.status_json());
    let none: Value = serde_json::from_str(&owner.peers_json()).unwrap();
    assert_eq!(none.as_array().unwrap().len(), 1, "the stranger must not reach the roster");
    drop(stranger);

    // A LAN guest still joins the open session from the lobby without
    // typing anything: the LAN announce carries the secret.
    let lan = join(&lan_staging);
    let sid = status(&owner)["session_id"].as_str().unwrap().to_string();
    let found = wait_until(Duration::from_secs(20), || {
        for a in owner.listen_addrs() {
            lan.dial(&a);
        }
        lan.sessions_json().contains(&sid)
    });
    assert!(found, "LAN guest never saw the announce");
    let sessions: Value = serde_json::from_str(&lan.sessions_json()).unwrap();
    assert_eq!(sessions[0]["auth"], false, "an open session must stay open on the LAN");
    assert!(lan.join_session(&sid, ""));
    let ready = wait_until(Duration::from_secs(45), || {
        let s = status(&lan);
        assert_ne!(s["state"], "failed", "LAN join failed: {}", s["message"]);
        s["state"] == "ready"
    });
    assert!(ready, "LAN join never completed: {}", lan.status_json());
    assert_eq!(lan.relayed_connections(), 0);

    drop(lan);
    drop(owner);
    reamesh::set_relays(&[]);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
    let _ = fs::remove_dir_all(lan_staging);
}

/// What the app last heard about `relay`.
fn relay_info(relay: &PeerId) -> Option<Value> {
    let v: Value = serde_json::from_str(&reamesh::relay_ctl_message()).unwrap();
    v["info"].get(relay.to_string()).cloned()
}

#[test]
fn a_host_hears_what_its_relay_announces_and_tells_its_guests() {
    use reamesh_relay_proto::{InfoResponse, Notice};
    let relay = start_relay_with(KEY, true);
    let plain = InfoResponse {
        ok: true,
        version: "1.2.0".into(),
        level: reamesh_relay_proto::RELAY_LEVEL,
        mode: "full".into(),
        ips: vec!["127.0.0.1".parse().unwrap()],
        tcp: relay.tcp,
        quic: relay.quic,
        ..InfoResponse::default()
    };
    *relay.said.lock().unwrap() = Some(plain.clone());

    let project = temp_dir("info-proj");
    make_project(&project);
    reamesh::set_loopback_only(true);
    let (owner, ticket) = host_on_relay(&relay, "info-session", &project, "");
    assert!(
        wait_until(Duration::from_secs(20), || relay_info(&relay.peer).is_some_and(|i| i["version"] == "1.2.0")),
        "the relay's answer never arrived"
    );
    let info = relay_info(&relay.peer).unwrap();
    assert_eq!(info["outdated"], false);
    assert_eq!(info["notice"], Value::Null);
    assert!(info.get("new_code").is_none(), "the address didn't change: {info}");

    // Announced later: the held request hears it right away.
    let notice = Notice { text: "Moving servers".into(), from: 0, until: u64::MAX };
    *relay.said.lock().unwrap() = Some(InfoResponse { notice: Some(notice), ..plain.clone() });
    assert!(
        wait_until(Duration::from_secs(10), || {
            relay_info(&relay.peer).is_some_and(|i| i["notice"]["text"] == "Moving servers")
        }),
        "the notice never arrived"
    );

    // A guest learns it from the host, in the join preview.
    let guest = join(&temp_dir("info-staging"));
    assert!(guest.check_ticket(&ticket, ""));
    assert!(
        wait_until(Duration::from_secs(30), || status(&guest)["probe"]["ok"] == true),
        "the check never passed: {}",
        guest.status_json()
    );
    let probe = status(&guest)["probe"].clone();
    assert_eq!(
        probe["preview"]["meta"]["relay_notices"][0]["text"], "Moving servers",
        "{probe}"
    );
    drop(guest);

    // A new address comes with the code to keep.
    *relay.said.lock().unwrap() = Some(InfoResponse {
        ips: vec!["127.0.0.1".parse().unwrap(), "203.0.113.9".parse().unwrap()],
        ..plain.clone()
    });
    assert!(
        wait_until(Duration::from_secs(10), || {
            relay_info(&relay.peer).is_some_and(|i| i["new_code"].as_str().is_some_and(|c| c.contains("203.0.113.9")))
        }),
        "the new address never arrived"
    );

    // Going away: losing it is announced, not a problem.
    *relay.said.lock().unwrap() = Some(InfoResponse { stopping: true, ..plain.clone() });
    assert!(wait_until(Duration::from_secs(10), || {
        relay_info(&relay.peer).is_some_and(|i| i["stopping"] == true)
    }));
    let peer = relay.peer;
    drop(relay);
    assert!(
        wait_until(Duration::from_secs(20), || relay_state(&peer).is_some_and(|s| s["reason"] == "paused")),
        "{:?}",
        relay_state(&peer)
    );
    drop(owner);
    reamesh::set_relays(&[]);
}

#[test]
fn an_older_relay_counts_as_outdated_but_keeps_working() {
    let relay = start_relay(KEY);
    reamesh::set_loopback_only(true);
    let (outcome, _) = reamesh::relay_test_blocking(&relay.code(&KEY));
    assert_eq!(outcome, "ok");
    let info = relay_info(&relay.peer).expect("nothing filed about the relay");
    assert_eq!(info["outdated"], true);
    assert_eq!(info["version"], "");
}
