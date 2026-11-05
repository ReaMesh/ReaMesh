//! The relay session list, end to end over loopback against the real relay
//! server (`reamesh-relay`): a host publishes, a member reads the list through
//! its own key and joins through the circuit alone.
//!
//! **Not covered:** real NATs, hole punching across the internet, two
//! machines. On loopback a hole punch always succeeds.

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId, futures::StreamExt, noise, ping, tcp, yamux};
use reamesh::Reamesh;
use reamesh_relay::config::{self, Config};
use reamesh_relay::server::{self, Options, RelayHandle};
use reamesh_relay_proto::{RelayAddr, RelayCode};
use serde_json::Value;

/// The configured relays and the watcher are process-global.
static SERIAL: Mutex<()> = Mutex::new(());

const WAIT: Duration = Duration::from_secs(30);

struct Relay {
    rt: tokio::runtime::Runtime,
    handle: Option<RelayHandle>,
    dir: tempfile::TempDir,
    peer: PeerId,
    tcp: u16,
    quic: u16,
}

impl Relay {
    fn start(session_list: bool) -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let dir = tempfile::tempdir().unwrap();
        config::init(dir.path()).unwrap();
        let path = dir.path().join(config::CONFIG_FILE);
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("tcp_port = 4001", "tcp_port = 0")
            .replace("quic_port = 4001", "quic_port = 0")
            .replace(r#"listen_addrs = ["0.0.0.0", "::"]"#, r#"listen_addrs = ["127.0.0.1"]"#)
            .replace(r#"# public_addrs = ["203.0.113.7"]"#, r#"public_addrs = ["127.0.0.1"]"#)
            .replace("session_list = false", &format!("session_list = {session_list}"));
        fs::write(path, text).unwrap();
        let mut handle = server::spawn(Options {
            reload_poll: Duration::from_millis(100),
            tick: Duration::from_millis(100),
            banner: false,
            ..Options::new(dir.path())
        })
        .unwrap();
        let addrs = rt.block_on(handle.listen_addrs());
        let port = |pick: fn(&Protocol) -> Option<u16>| {
            addrs.iter().flat_map(|a| a.iter().collect::<Vec<_>>()).find_map(|p| pick(&p)).unwrap()
        };
        let tcp = port(|p| if let Protocol::Tcp(t) = p { Some(*t) } else { None });
        let quic = port(|p| if let Protocol::Udp(u) = p { Some(*u) } else { None });
        let peer = handle.peer_id;
        Self { rt, handle: Some(handle), dir, peer, tcp, quic }
    }

    /// A relay code for a new named key.
    fn code(&self, name: &str) -> String {
        let key = match Config::load(self.dir.path()).unwrap().key(name) {
            Some(k) => k,
            None => config::add_key(self.dir.path(), name).unwrap(),
        };
        std::thread::sleep(Duration::from_millis(400)); // the relay re-reads its config
        self.code_with(key, name)
    }

    fn code_with(&self, key: [u8; 32], label: &str) -> String {
        RelayCode {
            addr: RelayAddr {
                peer: self.peer,
                ips: vec!["127.0.0.1".parse::<IpAddr>().unwrap()],
                dns: String::new(),
                tcp: self.tcp,
                quic: self.quic,
            },
            key,
            label: label.to_owned(),
            list: false,
        }
        .encode()
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = self.rt.block_on(h.stop());
        }
    }
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
    let dir = std::env::temp_dir().join(format!("reamesh-list-{tag}-{}-{nanos:x}", std::process::id()));
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
    let kick: Vec<u8> = (0..(3 * 1024 * 1024 + 17)).map(|i| (i % 251) as u8).collect();
    fs::write(dir.join("Audio/kick.wav"), kick).unwrap();
}

fn status(r: &Reamesh) -> Value {
    serde_json::from_str(&r.status_json()).unwrap()
}

/// This relay's list as the app gets it.
fn list(relay: &PeerId) -> Value {
    let all: Value = serde_json::from_str(&reamesh::relay_lists_json()).unwrap();
    all.get(relay.to_string()).cloned().unwrap_or(Value::Null)
}

fn entries(relay: &PeerId) -> Vec<Value> {
    list(relay)["entries"].as_array().cloned().unwrap_or_default()
}

fn reserved(relay: &PeerId) -> bool {
    let status: Value = serde_json::from_str(&reamesh::relay_status_json()).unwrap();
    status.as_array().is_some_and(|a| {
        a.iter().any(|r| r["peer"] == relay.to_string() && r["state"] == "reserved")
    })
}

/// Hosts through `code`, listed unless the relay is in `hidden`.
fn host(code: &str, dir: &Path, pass: &str, hidden: &[String]) -> Box<Reamesh> {
    reamesh::set_loopback_only(true);
    assert_eq!(reamesh::set_relays(&[code.to_owned()]), 1, "relay code refused");
    reamesh::set_relay_listing(hidden);
    let r = Reamesh::host("Listed Song", dir.to_str().unwrap(), "Song.rpp", pass).expect("host failed");
    r
}

fn stop_watching() {
    reamesh::relay_watch("test", false);
    // Winding down takes a tick; the next test starts on a clean slate.
    std::thread::sleep(Duration::from_millis(400));
    reamesh::set_relays(&[]);
}

#[test]
fn a_member_with_another_key_sees_the_session_and_joins_through_the_relay_alone() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let relay = Relay::start(true);
    let project = temp_dir("proj");
    let staging = temp_dir("staging");
    make_project(&project);

    let owner = host(&relay.code("sam"), &project, "", &[]);
    assert!(wait_until(WAIT, || reserved(&relay.peer)), "never reserved: {}", reamesh::relay_status_json());

    // The member's app knows the relay by a key of its own. (The hosting run
    // keeps the list it started with.)
    reamesh::set_relays(&[relay.code("lena")]);
    reamesh::relay_watch("test", true);
    assert!(wait_until(WAIT, || entries(&relay.peer).len() == 1), "never listed: {}", reamesh::relay_lists_json());
    let entry = entries(&relay.peer).remove(0);
    let l = list(&relay.peer);
    assert_eq!(l["state"], "ok");
    assert_eq!(l["enabled"], true);
    // Not a check on `count`: the host in this process reads the same list and
    // leaves itself out of the count, so an answer of its own can turn it to 0.
    assert_eq!(entry["session_name"], "Listed Song");
    assert_eq!(entry["owner"], owner.local_peer_id());
    assert_eq!(entry["auth"], false);
    assert_eq!(entry["key_label"], "sam");
    assert_eq!(entry["files"], 2);
    assert_eq!(entry["proto"], reamesh::PROTO);
    assert!(entry.get("project_file").is_none(), "the file name stays off the list");
    let ticket = entry["ticket"].as_str().unwrap().to_owned();
    assert!(ticket.contains(&format!("&r={}~127.0.0.1~", relay.peer)), "{ticket}");
    assert!(ticket.contains("&x="), "an open session's entry needs its secret: {ticket}");
    assert!(ticket.contains("&a=&"), "the host's own addresses stay off the list: {ticket}");
    let key_hex: String = Config::load(relay.dir.path())
        .unwrap()
        .key("sam")
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(!ticket.contains(&key_hex), "a relay key leaked into the entry");

    // One click: no passphrase, only the relay as a way in.
    let guest = Reamesh::join(staging.to_str().unwrap()).expect("join failed");
    assert!(guest.check_ticket(&ticket, ""));
    let checked = wait_until(Duration::from_secs(45), || status(&guest)["probe"]["ok"] == true);
    assert!(checked, "check through the relay never passed: {}", guest.status_json());
    reamesh::relay_watch("test", false); // leaving the Lobby
    assert!(guest.join_ticket(&ticket, ""), "join_ticket refused");
    let ready = wait_until(Duration::from_secs(60), || {
        let s = status(&guest);
        assert_ne!(s["state"], "failed", "joining from the list failed: {}", s["message"]);
        s["state"] == "ready"
    });
    assert!(ready, "the join never completed: {}", guest.status_json());
    assert!(guest.relayed_connections() >= 1, "the join never went through the relay");
    let path = PathBuf::from(status(&guest)["project_path"].as_str().unwrap());
    assert_eq!(fs::read(&path).unwrap(), fs::read(project.join("Song.rpp")).unwrap());
    let media = path.parent().unwrap().join("Audio/kick.wav");
    assert_eq!(fs::read(&media).unwrap(), fs::read(project.join("Audio/kick.wav")).unwrap());
    let guest_id = guest.local_peer_id().to_string();
    let seen = wait_until(WAIT, || {
        let peers: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        peers.as_array().unwrap().iter().any(|p| p["id"] == guest_id)
    });
    assert!(seen, "the host's roster never showed the member: {}", owner.peers_json());

    // Watching ended with the Lobby: nothing is kept but whether there's a list.
    // Not a check on `state`: the host in this process reads the same list, and
    // an answer of its own can turn `idle` back into `ok`. It never adds entries.
    assert!(wait_until(WAIT, || entries(&relay.peer).is_empty()), "{}", reamesh::relay_lists_json());
    assert_eq!(list(&relay.peer)["enabled"], true);

    drop(guest);
    drop(owner);
    stop_watching();
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn a_stranger_without_a_key_sees_nothing_and_cannot_get_in() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let relay = Relay::start(true);
    let project = temp_dir("proj");
    make_project(&project);
    let owner = host(&relay.code("sam"), &project, "", &[]);
    assert!(wait_until(WAIT, || reserved(&relay.peer)));

    let wrong = relay.code_with([0x42; 32], "stranger");
    reamesh::set_relays(&[wrong.clone()]);
    reamesh::relay_watch("test", true);
    assert!(wait_until(WAIT, || list(&relay.peer)["state"] == "refused"), "{}", reamesh::relay_lists_json());
    assert_eq!(list(&relay.peer)["reason"], "bad_key");
    assert!(entries(&relay.peer).is_empty());
    let (outcome, _) = reamesh::relay_test_blocking(&wrong);
    assert_eq!(outcome, "bad_key");

    drop(owner);
    stop_watching();
    let _ = fs::remove_dir_all(project);
}

#[test]
fn a_host_chooses_per_relay_and_taking_it_off_works_at_once() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let relay = Relay::start(true);
    let project = temp_dir("proj");
    make_project(&project);

    // Off from the start: the session runs, reserved, and nobody sees it.
    let hidden = vec![relay.peer.to_string()];
    let owner = host(&relay.code("sam"), &project, "", &hidden);
    assert!(wait_until(WAIT, || reserved(&relay.peer)));
    reamesh::set_relays(&[relay.code("lena")]);
    reamesh::relay_watch("test", true);
    assert!(wait_until(WAIT, || list(&relay.peer)["state"] == "ok"));
    std::thread::sleep(Duration::from_secs(2));
    assert!(entries(&relay.peer).is_empty(), "listed against the host's choice");

    // On during the session.
    reamesh::set_relay_listing(&[]);
    assert!(wait_until(WAIT, || entries(&relay.peer).len() == 1), "{}", reamesh::relay_lists_json());
    // Off again, without waiting for any republish interval.
    reamesh::set_relay_listing(&hidden);
    assert!(wait_until(Duration::from_secs(5), || entries(&relay.peer).is_empty()));
    // On once more, and ending the session takes it off.
    reamesh::set_relay_listing(&[]);
    assert!(wait_until(WAIT, || entries(&relay.peer).len() == 1));
    drop(owner);
    assert!(wait_until(Duration::from_secs(10), || entries(&relay.peer).is_empty()));

    stop_watching();
    let _ = fs::remove_dir_all(project);
}

#[test]
fn switching_the_relays_list_on_reaches_a_running_session() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let relay = Relay::start(false);
    let project = temp_dir("proj");
    make_project(&project);
    let code = relay.code("sam");
    let owner = host(&code, &project, "", &[]);
    assert!(wait_until(WAIT, || reserved(&relay.peer)));
    assert!(wait_until(WAIT, || list(&relay.peer)["enabled"] == false), "{}", reamesh::relay_lists_json());
    reamesh::set_relays(&[relay.code("lena")]);
    reamesh::relay_watch("test", true);
    assert!(wait_until(WAIT, || list(&relay.peer)["state"] == "ok"));
    assert!(entries(&relay.peer).is_empty());

    config::set_session_list(relay.dir.path(), true).unwrap();
    assert!(wait_until(WAIT, || entries(&relay.peer).len() == 1), "{}", reamesh::relay_lists_json());
    assert_eq!(list(&relay.peer)["enabled"], true);

    config::set_session_list(relay.dir.path(), false).unwrap();
    assert!(wait_until(WAIT, || entries(&relay.peer).is_empty() && list(&relay.peer)["enabled"] == false));

    drop(owner);
    stop_watching();
    let _ = fs::remove_dir_all(project);
}

#[test]
fn a_protected_session_is_listed_without_a_secret() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let relay = Relay::start(true);
    let project = temp_dir("proj");
    let staging = temp_dir("staging");
    make_project(&project);
    let owner = host(&relay.code("sam"), &project, "hunter22", &[]);
    assert!(wait_until(WAIT, || reserved(&relay.peer)));
    reamesh::set_relays(&[relay.code("lena")]);
    reamesh::relay_watch("test", true);
    assert!(wait_until(WAIT, || entries(&relay.peer).len() == 1), "{}", reamesh::relay_lists_json());
    let entry = entries(&relay.peer).remove(0);
    assert_eq!(entry["auth"], true);
    let ticket = entry["ticket"].as_str().unwrap();
    assert!(!ticket.contains("&x="), "{ticket}");
    assert!(ticket.contains("s=1"), "{ticket}");

    let guest = Reamesh::join(staging.to_str().unwrap()).expect("join failed");
    guest.leave_lobby();
    assert!(guest.check_ticket(ticket, "wrong"));
    assert!(
        wait_until(Duration::from_secs(45), || status(&guest)["probe"]["ok"] == false),
        "{}",
        guest.status_json()
    );
    assert!(guest.check_ticket(ticket, "hunter22"));
    assert!(wait_until(Duration::from_secs(45), || status(&guest)["probe"]["ok"] == true), "{}", guest.status_json());

    drop(guest);
    drop(owner);
    stop_watching();
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn a_relay_from_before_the_list_is_no_error() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // Answers ping and nothing of ours.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut swarm = {
        let _guard = rt.enter();
        libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)
            .unwrap()
            .with_behaviour(|_| ping::Behaviour::default())
            .unwrap()
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build()
    };
    let peer = *swarm.local_peer_id();
    let port = rt.block_on(async {
        swarm.listen_on("/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>().unwrap()).unwrap();
        loop {
            if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
                let port = address.iter().find_map(|p| if let Protocol::Tcp(t) = p { Some(t) } else { None });
                break port.unwrap();
            }
        }
    });
    rt.spawn(async move {
        loop {
            swarm.select_next_some().await;
        }
    });
    reamesh::set_loopback_only(true);
    let code = RelayCode {
        addr: RelayAddr { peer, ips: vec!["127.0.0.1".parse().unwrap()], dns: String::new(), tcp: port, quic: 0 },
        key: [1; 32],
        label: "Old".into(),
        list: false,
    }
    .encode();
    assert_eq!(reamesh::set_relays(&[code]), 1);
    reamesh::relay_watch("test", true);
    assert!(wait_until(WAIT, || list(&peer)["enabled"] == false), "{}", reamesh::relay_lists_json());
    assert_eq!(list(&peer)["state"], "ok");
    assert!(entries(&peer).is_empty());
    stop_watching();
    rt.shutdown_background();
}
