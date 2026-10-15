//! In-process owner/client session tests: two complete peers (two Tokio
//! runtimes, two swarms, one authoritative doc + one replica) over loopback.
//!
//! Requires real sockets (mDNS may be unavailable; the tests fall back to
//! dialing the peer's listen addresses directly).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use reamesh::Reamesh;
use serde_json::Value;

const KICK_SIZE: usize = 2 * 1024 * 1024 + 12345; // >1 chunk, not chunk-aligned

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
    let dir = std::env::temp_dir().join(format!(
        "reamesh-test-{tag}-{}-{nanos:x}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A small but realistic project directory: .rpp, a subdirectory with a
/// multi-chunk media file, an empty file, and files that must NOT transfer
/// (a backup, a dotfile, and media lying in the folder that the project
/// never uses).
fn make_project(dir: &Path) {
    fs::write(
        dir.join("Song.rpp"),
        "<REAPER_PROJECT 0.1\n<TRACK\n  <ITEM\n    POSITION 0\n    <SOURCE WAVE\n      FILE \"Audio/kick.wav\"\n    >\n  >\n>\n<NOTES 0 2\n  |see notes.txt\n>\n>\n",
    )
    .unwrap();
    fs::create_dir_all(dir.join("Unused")).unwrap();
    fs::write(dir.join("Unused/outtake.wav"), b"not part of the song").unwrap();
    fs::create_dir_all(dir.join("Audio")).unwrap();
    let kick: Vec<u8> = (0..KICK_SIZE).map(|i| (i % 251) as u8).collect();
    fs::write(dir.join("Audio/kick.wav"), kick).unwrap();
    fs::write(dir.join("notes.txt"), b"").unwrap();
    fs::write(dir.join("Song.rpp-bak"), b"backup").unwrap();
    fs::write(dir.join(".DS_Store"), b"junk").unwrap();
}

fn session_id_of(owner: &Reamesh) -> String {
    let status: Value = serde_json::from_str(&owner.status_json()).unwrap();
    status["session_id"].as_str().unwrap().to_string()
}

fn interconnect(a: &Reamesh, b: &Reamesh) {
    for addr in b.listen_addrs() {
        a.dial(&addr);
    }
    for addr in a.listen_addrs() {
        b.dial(&addr);
    }
}

/// Host a session over `project_dir`, join it from a client, and wait until
/// the download finished and both sides see each other on the session topic.
fn establish(session_name: &str, project_dir: &Path, staging: &Path) -> (Box<Reamesh>, Box<Reamesh>) {
    establish_pass(session_name, project_dir, staging, "")
}

/// Like `establish`, but the session is passphrase-protected; the client joins
/// with `pass`. Exercises the authenticated download and (token-gated) edit
/// paths end to end.
fn establish_pass(
    session_name: &str,
    project_dir: &Path,
    staging: &Path,
    pass: &str,
) -> (Box<Reamesh>, Box<Reamesh>) {
    let owner = Reamesh::host(session_name, project_dir.to_str().unwrap(), "Song.rpp", pass)
        .expect("host failed");
    let client = Reamesh::join(staging.to_str().unwrap()).expect("join failed");
    let sid = session_id_of(&owner);

    // mDNS with explicit-dial fallback (sandboxed/multicast-less environments).
    let discovered = wait_until(Duration::from_secs(20), || {
        interconnect(&owner, &client);
        client.sessions_json().contains(&sid)
    });
    assert!(discovered, "client never saw the session announcement");

    assert!(client.join_session(&sid, pass), "join_session refused");
    let ready = wait_until(Duration::from_secs(30), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        assert_ne!(s["state"], "failed", "download failed: {}", s["message"]);
        s["state"] == "ready"
    });
    assert!(ready, "download never completed: {}", client.status_json());

    let meshed = wait_until(Duration::from_secs(15), || {
        owner.connected_peers() >= 1 && client.connected_peers() >= 1
    });
    assert!(meshed, "session topic mesh never formed");
    (owner, client)
}

/// Read tracks["<guid>"]["<field>"] out of a snapshot_json string.
fn field(snapshot_json: &str, guid: &str, field: &str) -> Value {
    let v: Value = serde_json::from_str(snapshot_json).unwrap();
    v["tracks"][guid][field].clone()
}

/// Apply every drained changeset to `state` (a tiny stand-in for REAPER) and
/// return whether anything arrived.
fn drain_into(peer: &Reamesh, state: &mut serde_json::Map<String, Value>) -> bool {
    let drained: Value = serde_json::from_str(&peer.drain_remote()).unwrap();
    let mut any = false;
    for cs in drained.as_array().unwrap() {
        for (guid, fields) in cs["tracks"].as_object().unwrap() {
            any = true;
            let entry = state
                .entry(guid.clone())
                .or_insert_with(|| Value::Object(Default::default()));
            for (k, v) in fields.as_object().unwrap() {
                entry[k] = v.clone();
            }
        }
    }
    any
}

#[test]
fn client_join_downloads_project_files() {
    let project = temp_dir("proj-files");
    let staging = temp_dir("staging-files");
    make_project(&project);

    let (owner, client) = establish("files-test", &project, &staging);

    let status: Value = serde_json::from_str(&client.status_json()).unwrap();
    let path = PathBuf::from(status["project_path"].as_str().unwrap());
    assert!(path.ends_with("Song.rpp"), "unexpected project path {path:?}");
    let dest = path.parent().unwrap();

    assert_eq!(
        fs::read(dest.join("Song.rpp")).unwrap(),
        fs::read(project.join("Song.rpp")).unwrap()
    );
    assert_eq!(
        fs::read(dest.join("Audio/kick.wav")).unwrap(),
        fs::read(project.join("Audio/kick.wav")).unwrap(),
        "media file corrupted in transfer"
    );
    assert_eq!(fs::read(dest.join("notes.txt")).unwrap(), b"");
    assert!(!dest.join("Song.rpp-bak").exists(), "backup file should not transfer");
    assert!(!dest.join(".DS_Store").exists(), "hidden file should not transfer");
    assert!(!dest.join("Unused/outtake.wav").exists(), "media the project doesn't use should not transfer");

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn ticket_join_works_without_lan_discovery() {
    let project = temp_dir("proj-ticket");
    let staging = temp_dir("staging-ticket");
    make_project(&project);

    // Passphrase-protected, so this also runs the sealed-gossip path.
    let owner = Reamesh::host("ticket-test", project.to_str().unwrap(), "Song.rpp", "hunter2")
        .expect("host failed");
    // The ticket appears once the host knows its listen addresses.
    let mut ticket = String::new();
    assert!(
        wait_until(Duration::from_secs(10), || {
            ticket = owner.session_ticket();
            !ticket.is_empty()
        }),
        "host never produced a ticket"
    );
    assert!(ticket.starts_with("reamesh://join/"), "unexpected ticket format: {ticket}");
    // A dialable IP and at least one port must be in there; the loopback the
    // host also listens on must not be.
    assert!(ticket.contains("&a=") && (ticket.contains("&t=") || ticket.contains("&q=")), "{ticket}");
    assert!(!ticket.contains("127.0.0.1"), "loopback leaked into the ticket: {ticket}");

    let client = Reamesh::join(staging.to_str().unwrap()).expect("join failed");
    assert_eq!(client.session_ticket(), "", "clients must not mint tickets");
    // Garbage is refused outright.
    assert!(!client.join_ticket("reamesh://join/garbage!", "hunter2"));
    assert!(!client.join_ticket("", "hunter2"));

    // Deliberately no interconnect(): the ticket alone must reach the host.
    // First the passphrase pre-check through the ticket, wrong then right.
    assert!(client.check_ticket(&ticket, "wrong-pass"));
    let wrong = wait_until(Duration::from_secs(45), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["probe"]["ok"] == false
    });
    assert!(wrong, "wrong-passphrase ticket check never resolved");
    assert!(client.check_ticket(&ticket, "hunter2"));
    let right = wait_until(Duration::from_secs(45), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["probe"]["ok"] == true
    });
    assert!(right, "right-passphrase ticket check never resolved");

    assert!(client.join_ticket(&ticket, "hunter2"), "join_ticket refused");
    let ready = wait_until(Duration::from_secs(45), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        assert_ne!(s["state"], "failed", "ticket join failed: {}", s["message"]);
        s["state"] == "ready"
    });
    assert!(ready, "ticket join never completed: {}", client.status_json());

    let status: Value = serde_json::from_str(&client.status_json()).unwrap();
    let path = PathBuf::from(status["project_path"].as_str().unwrap());
    assert_eq!(
        fs::read(&path).unwrap(),
        fs::read(project.join("Song.rpp")).unwrap(),
        "project file corrupted over ticket join"
    );

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn ticket_join_against_dead_host_fails_instead_of_hanging() {
    // Joining with a ticket from a stopped host must fail within the timeout.
    let project = temp_dir("proj-deadticket");
    let staging = temp_dir("staging-deadticket");
    make_project(&project);

    let owner = Reamesh::host("dead-ticket", project.to_str().unwrap(), "Song.rpp", "hunter2")
        .expect("host failed");
    let mut ticket = String::new();
    assert!(
        wait_until(Duration::from_secs(10), || {
            ticket = owner.session_ticket();
            !ticket.is_empty()
        }),
        "host never produced a ticket"
    );
    // Stop hosting: the ticket's addresses are now dead.
    drop(owner);

    let client = Reamesh::join(staging.to_str().unwrap()).expect("join failed");
    assert!(client.join_ticket(&ticket, "hunter2"), "join_ticket refused");
    // It must resolve to "failed" within the ticket-join timeout (+ margin),
    // not sit on "connecting" forever.
    let failed = wait_until(Duration::from_secs(40), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["state"] == "failed"
    });
    assert!(
        failed,
        "join against a dead ticket never failed: {}",
        client.status_json()
    );

    // The passphrase pre-check against a dead ticket must likewise resolve
    // (to ok:false), not hang.
    assert!(client.check_ticket(&ticket, "hunter2"));
    let resolved = wait_until(Duration::from_secs(40), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["probe"]["ok"] == false
    });
    assert!(resolved, "check against a dead ticket never resolved");

    drop(client);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn owner_edit_propagates_to_client() {
    let project = temp_dir("proj-owneredit");
    let staging = temp_dir("staging-owneredit");
    make_project(&project);
    let (owner, client) = establish("owneredit-test", &project, &staging);

    owner.push_local(
        r#"{"tracks":{"{T1}":{"present":true,"index":0,"name":"Drums","vol":0.8,"pan":0.0,"mute":false,"solo":false}}}"#,
    );

    let mut applied = serde_json::Map::new();
    let arrived = wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut applied);
        applied.get("{T1}").map(|t| t["name"] == "Drums").unwrap_or(false)
    });
    assert!(arrived, "owner edit never reached the client");
    assert_eq!(field(&client.snapshot_json(), "{T1}", "vol"), 0.8);

    // No echo afterwards.
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(client.drain_remote(), "[]");
    assert_eq!(owner.drain_remote(), "[]");

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn client_edit_proposes_and_propagates() {
    let project = temp_dir("proj-clientedit");
    let staging = temp_dir("staging-clientedit");
    make_project(&project);
    let (owner, client) = establish("clientedit-test", &project, &staging);

    // Owner creates the track, client renames it once it sees it.
    owner.push_local(
        r#"{"tracks":{"{T2}":{"present":true,"index":0,"name":"Vox","vol":1.0,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    let mut client_state = serde_json::Map::new();
    assert!(wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut client_state);
        client_state.contains_key("{T2}")
    }));

    // Track creation locks every field for the owner. Wait for those locks
    // to expire before proposing the client edit.
    std::thread::sleep(Duration::from_millis(3200));
    client.push_local(r#"{"tracks":{"{T2}":{"name":"Lead Vox"}}}"#);

    // The proposal reaches the owner's REAPER via its drain...
    let mut owner_state = serde_json::Map::new();
    let on_owner = wait_until(Duration::from_secs(10), || {
        drain_into(&owner, &mut owner_state);
        owner_state.get("{T2}").map(|t| t["name"] == "Lead Vox").unwrap_or(false)
    });
    assert!(on_owner, "client proposal never surfaced on the owner");

    // ...and is confirmed into the client's replica without echoing a
    // self-apply back to the client's host.
    assert!(wait_until(Duration::from_secs(10), || {
        let _ = drain_into(&client, &mut client_state);
        field(&client.snapshot_json(), "{T2}", "name") == "Lead Vox"
    }));
    assert_eq!(
        client_state["{T2}"]["name"], "Vox",
        "confirmed proposal must not echo back to the proposing client"
    );

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn authenticated_client_proposal_propagates() {
    // Proposals are gated on the session token (only token-holders may write
    // the doc); a correctly-joined client must still be able to propose. This
    // exercises the authenticated envelope round-trip end to end.
    let project = temp_dir("proj-authprop");
    let staging = temp_dir("staging-authprop");
    make_project(&project);
    let (owner, client) = establish_pass("authprop-test", &project, &staging, "hunter2");

    owner.push_local(
        r#"{"tracks":{"{T9}":{"present":true,"index":0,"name":"Vox","vol":1.0,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    let mut client_state = serde_json::Map::new();
    assert!(wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut client_state);
        client_state.contains_key("{T9}")
    }));

    // Wait for the locks acquired during track creation to expire.
    std::thread::sleep(Duration::from_millis(3200));
    client.push_local(r#"{"tracks":{"{T9}":{"name":"Lead Vox"}}}"#);

    let mut owner_state = serde_json::Map::new();
    let on_owner = wait_until(Duration::from_secs(10), || {
        drain_into(&owner, &mut owner_state);
        owner_state.get("{T9}").map(|t| t["name"] == "Lead Vox").unwrap_or(false)
    });
    assert!(on_owner, "authenticated client proposal never surfaced on the owner");

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

/// The extension captures (push_local) BEFORE it drains (drain_remote) on every
/// tick, so a client proposal the owner admitted in between lands in the doc
/// while the owner's own edit is still on its way in. The owner's push must
/// not re-baseline that admitted edit away: its REAPER would then never apply
/// it while every other peer (and the doc) has it: a silent split brain.
#[test]
fn admitted_proposal_survives_owner_push_before_drain() {
    let project = temp_dir("proj-pushdrain");
    let staging = temp_dir("staging-pushdrain");
    make_project(&project);
    let (owner, client) = establish("pushdrain-test", &project, &staging);

    owner.push_local(
        r#"{"tracks":{"{T12}":{"present":true,"index":0,"name":"Gtr","vol":1.0,"pan":0.0}}}"#,
    );
    let mut client_state = serde_json::Map::new();
    assert!(wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut client_state);
        client_state.contains_key("{T12}")
    }));
    // Creating the track locked every field for the owner; wait that out.
    std::thread::sleep(Duration::from_millis(3200));

    // The client renames the track. Wait until the owner's DOC holds the
    // admitted value: deliberately WITHOUT draining the owner, whose REAPER
    // has not ticked yet.
    client.push_local(r#"{"tracks":{"{T12}":{"name":"Lead Gtr"}}}"#);
    assert!(
        wait_until(Duration::from_secs(10), || {
            field(&owner.snapshot_json(), "{T12}", "name") == "Lead Gtr"
        }),
        "the client's proposal never reached the owner's doc"
    );

    // The owner's next tick: capture a local edit to a DIFFERENT field first
    // (the lock on `name` is the client's; `pan` is free)...
    owner.push_local(r#"{"tracks":{"{T12}":{"pan":0.25}}}"#);
    // ...then drain. The admitted rename must be in what the owner applies.
    let mut owner_state = serde_json::Map::new();
    drain_into(&owner, &mut owner_state);
    assert_eq!(
        owner_state.get("{T12}").map(|t| t["name"].clone()),
        Some(serde_json::json!("Lead Gtr")),
        "the owner's own push swallowed the admitted client edit"
    );
    // The owner's own pan edit is re-baselined and must not echo back.
    assert_ne!(
        owner_state.get("{T12}").map(|t| t["pan"].clone()),
        Some(serde_json::json!(0.25)),
        "the owner's own edit echoed back from drain_remote"
    );

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn owner_wins_live_conflict() {
    let project = temp_dir("proj-conflict");
    let staging = temp_dir("staging-conflict");
    make_project(&project);
    let (owner, client) = establish("conflict-test", &project, &staging);

    // Baseline track on both sides.
    owner.push_local(
        r#"{"tracks":{"{T3}":{"present":true,"index":0,"name":"Bass","vol":0.5,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    let mut client_state = serde_json::Map::new();
    assert!(wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut client_state);
        client_state.contains_key("{T3}")
    }));

    // Conflict: owner sets vol=0.9, client proposes vol=0.1 right after.
    owner.push_local(r#"{"tracks":{"{T3}":{"vol":0.9}}}"#);
    client.push_local(r#"{"tracks":{"{T3}":{"vol":0.1}}}"#);

    // Collect what each host would apply for long enough to cover the
    // proposal timeout + revert.
    let mut owner_state = serde_json::Map::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        drain_into(&owner, &mut owner_state);
        drain_into(&client, &mut client_state);
        std::thread::sleep(Duration::from_millis(100));
    }

    // The owner never applies the losing proposal...
    assert_ne!(
        owner_state.get("{T3}").map(|t| t["vol"].clone()),
        Some(serde_json::json!(0.1)),
        "owner applied a proposal it should have dropped"
    );
    // ...the override is counted so the UI can tell the user...
    let st: Value = serde_json::from_str(&client.status_json()).unwrap();
    assert!(
        st["reverts"].as_u64().unwrap_or(0) >= 1,
        "overridden proposal was not counted: {st}"
    );
    // ...the doc converges to the owner's value on both sides...
    assert_eq!(field(&owner.snapshot_json(), "{T3}", "vol"), 0.9);
    assert_eq!(field(&client.snapshot_json(), "{T3}", "vol"), 0.9);
    // ...and the client's host was told to revert to it.
    assert_eq!(
        client_state["{T3}"]["vol"],
        serde_json::json!(0.9),
        "client host never reverted to the owner's value"
    );

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

/// Lock lapse: once the owner's edit lock on a field expires (it stopped
/// touching it), a client editing that field becomes the new holder and its
/// proposal is admitted: collaboration hands the field over instead of the
/// owner statically winning forever.
#[test]
fn client_wins_after_owner_lock_lapses() {
    let project = temp_dir("proj-active");
    let staging = temp_dir("staging-active");
    make_project(&project);
    let (owner, client) = establish("active-test", &project, &staging);

    // Baseline, then an owner edit to vol the client receives.
    owner.push_local(
        r#"{"tracks":{"{T7}":{"present":true,"index":0,"name":"Keys","vol":0.5,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    owner.push_local(r#"{"tracks":{"{T7}":{"vol":0.9}}}"#);
    let mut client_state = serde_json::Map::new();
    assert!(wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut client_state);
        client_state.get("{T7}").map(|t| t["vol"].clone()) == Some(serde_json::json!(0.9))
    }));

    // Wait for the owner's lock on vol to lapse, as observed by the CLIENT,
    // not by a bare sleep, which a loaded machine can outrun. The sequence has
    // to be *held, then released*: polling only for "not held" is ambiguous,
    // because an empty table is also what the client has before the owner's
    // first lock broadcast ever arrives (and editing then races a lock that is
    // still very much alive).
    let vol_held_by_other = || {
        let locked: Value =
            serde_json::from_str(&client.locked_entities_json()).unwrap_or(Value::Null);
        locked
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e["id"] == "{T7}" && e["field"] == "vol"))
    };
    assert!(
        wait_until(Duration::from_secs(10), vol_held_by_other),
        "the client never saw the owner hold the lock on vol"
    );
    assert!(
        wait_until(Duration::from_secs(10), || !vol_held_by_other()),
        "the owner's lock on vol never lapsed from the client's point of view"
    );
    client.push_local(r#"{"tracks":{"{T7}":{"vol":0.1}}}"#);

    let mut owner_state = serde_json::Map::new();
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        drain_into(&owner, &mut owner_state);
        drain_into(&client, &mut client_state);
        std::thread::sleep(Duration::from_millis(100));
    }

    // The owner admitted the client's edit on both sides...
    assert_eq!(field(&owner.snapshot_json(), "{T7}", "vol"), 0.1);
    assert_eq!(field(&client.snapshot_json(), "{T7}", "vol"), 0.1);
    assert_eq!(
        owner_state.get("{T7}").map(|t| t["vol"].clone()),
        Some(serde_json::json!(0.1)),
        "owner never applied the client's later edit"
    );
    // ...and the client was not snapped back.
    let st: Value = serde_json::from_str(&client.status_json()).unwrap();
    assert_eq!(
        st["reverts"].as_u64().unwrap_or(0),
        0,
        "client's later edit was wrongly snapped back: {st}"
    );

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

/// While a client holds a field's editing lock, an owner edit to the same
/// field must be rejected and reverted to the client's value.
#[test]
fn client_lock_protects_against_owner() {
    let project = temp_dir("proj-cl-lock");
    let staging = temp_dir("staging-cl-lock");
    make_project(&project);
    let (owner, client) = establish("cl-lock-test", &project, &staging);

    // Baseline from the owner; wait past LOCK_HOLD so the baseline's lock on
    // vol lapses and the client can take the field cleanly.
    owner.push_local(
        r#"{"tracks":{"{T5}":{"present":true,"index":0,"name":"Bass","vol":0.5,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    let mut client_state = serde_json::Map::new();
    assert!(wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut client_state);
        client_state.contains_key("{T5}")
    }));
    std::thread::sleep(Duration::from_millis(2400));

    // The client edits vol and the owner admits it -> the client now HOLDS vol.
    client.push_local(r#"{"tracks":{"{T5}":{"vol":0.2}}}"#);
    assert!(
        wait_until(Duration::from_secs(10), || {
            field(&owner.snapshot_json(), "{T5}", "vol") == 0.2
        }),
        "owner never admitted the client's edit (so the client never held the lock)"
    );

    // Immediately (well within LOCK_HOLD) the owner tries to grab the same
    // field. The client holds it, so the owner is rejected and snapped back.
    owner.push_local(r#"{"tracks":{"{T5}":{"vol":0.9}}}"#);

    let mut owner_state = serde_json::Map::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        drain_into(&owner, &mut owner_state);
        drain_into(&client, &mut client_state);
        std::thread::sleep(Duration::from_millis(100));
    }

    // The client's value stands everywhere; the owner's 0.9 never took.
    assert_eq!(field(&owner.snapshot_json(), "{T5}", "vol"), 0.2);
    assert_eq!(field(&client.snapshot_json(), "{T5}", "vol"), 0.2);
    // The owner's own REAPER was snapped back to the client's value.
    assert_eq!(
        owner_state.get("{T5}").map(|t| t["vol"].clone()),
        Some(serde_json::json!(0.2)),
        "owner's REAPER never snapped back off its rejected edit"
    );
    // The client's edit was not snapped back (it won the field).
    let cst: Value = serde_json::from_str(&client.status_json()).unwrap();
    assert_eq!(
        cst["reverts"].as_u64().unwrap_or(0),
        0,
        "the holding client was wrongly snapped back: {cst}"
    );

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn items_section_propagates_and_client_items_propose() {
    let project = temp_dir("proj-items");
    let staging = temp_dir("staging-items");
    make_project(&project);
    let (owner, client) = establish("items-test", &project, &staging);

    // Owner creates an item (chunk-shaped payload, as the extension sends).
    owner.push_local(
        r#"{"items":{"{I1}":{"present":true,"track":"{T1}","chunk":"<ITEM\nPOSITION 1.0\n>"}}}"#,
    );
    let drained_has = |state: &serde_json::Map<String, Value>, id: &str| {
        state.contains_key(id)
    };
    let mut client_items = serde_json::Map::new();
    let arrived = wait_until(Duration::from_secs(10), || {
        let drained: Value = serde_json::from_str(&client.drain_remote()).unwrap();
        for cs in drained.as_array().unwrap() {
            if let Some(items) = cs.get("items").and_then(|v| v.as_object()) {
                for (id, fields) in items {
                    client_items.insert(id.clone(), fields.clone());
                }
            }
        }
        drained_has(&client_items, "{I1}")
    });
    assert!(arrived, "owner item never reached the client");
    assert_eq!(client_items["{I1}"]["track"], "{T1}");

    // Client proposes an item move after the creation lock expires.
    std::thread::sleep(Duration::from_millis(3200));
    client.push_local(r#"{"items":{"{I1}":{"chunk":"<ITEM\nPOSITION 2.0\n>"}}}"#);
    let on_owner = wait_until(Duration::from_secs(10), || {
        let v: Value = serde_json::from_str(&owner.snapshot_json()).unwrap();
        v["items"]["{I1}"]["chunk"]
            .as_str()
            .is_some_and(|c| c.contains("POSITION 2.0"))
    });
    assert!(on_owner, "client item proposal never admitted by the owner");

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

// The local item-tint lock signal rides on `locked_entities_json`: an item a
// *peer* is editing must show up (attributed to that peer) so the extension can
// grey it, while our OWN held items must never appear (we don't tint our own).
#[test]
fn locked_entities_reports_other_peers_item_holds_not_own() {
    let project = temp_dir("proj-locked-ent");
    let staging = temp_dir("staging-locked-ent");
    make_project(&project);
    let (owner, client) = establish("locked-ent-test", &project, &staging);

    // Owner creates an item; the client receives it.
    owner.push_local(
        r#"{"items":{"{I1}":{"present":true,"track":"{T1}","chunk":"<ITEM\nPOSITION 1.0\n>"}}}"#,
    );
    let mut client_items = serde_json::Map::new();
    assert!(wait_until(Duration::from_secs(10), || {
        let drained: Value = serde_json::from_str(&client.drain_remote()).unwrap();
        for cs in drained.as_array().unwrap() {
            if let Some(items) = cs.get("items").and_then(|v| v.as_object()) {
                for (id, fields) in items {
                    client_items.insert(id.clone(), fields.clone());
                }
            }
        }
        client_items.contains_key("{I1}")
    }));

    // Wait for the creation lock to expire. locked_entities_json excludes
    // our own locks, so the owner cannot observe its expiry through that API.
    std::thread::sleep(Duration::from_millis(3200));
    // Nobody is editing now -> neither peer reports another peer's hold.
    assert_eq!(owner.locked_entities_json(), "[]");
    assert_eq!(client.locked_entities_json(), "[]");

    // The client edits the item; the owner admits it, so the client now HOLDS
    // the item's chunk lock: exactly what the extension tints from.
    client.push_local(r#"{"items":{"{I1}":{"chunk":"<ITEM\nPOSITION 2.0\n>"}}}"#);
    assert!(
        wait_until(Duration::from_secs(10), || {
            let v: Value = serde_json::from_str(&owner.snapshot_json()).unwrap();
            v["items"]["{I1}"]["chunk"]
                .as_str()
                .is_some_and(|c| c.contains("POSITION 2.0"))
        }),
        "owner never admitted the client item edit"
    );

    let client_id = client.local_peer_id().to_string();
    let owner_id = owner.local_peer_id().to_string();
    assert!(
        wait_until(Duration::from_secs(3), || {
            let v: Value = serde_json::from_str(&owner.locked_entities_json()).unwrap();
            v.as_array().unwrap().iter().any(|e| {
                e["section"] == "items"
                    && e["id"] == "{I1}"
                    && e["holder"] == serde_json::json!(client_id)
            })
        }),
        "owner never saw the client's item lock: {}",
        owner.locked_entities_json()
    );
    // The owner never lists a field it holds itself (no self-tint).
    let ow: Value = serde_json::from_str(&owner.locked_entities_json()).unwrap();
    assert!(
        ow.as_array()
            .unwrap()
            .iter()
            .all(|e| e["holder"] != serde_json::json!(owner_id)),
        "owner listed a self-held field: {ow}"
    );
    // The holding client must not list its OWN hold either (it doesn't tint it).
    let cl: Value = serde_json::from_str(&client.locked_entities_json()).unwrap();
    assert!(
        cl.as_array().unwrap().iter().all(|e| e["id"] != "{I1}"),
        "client listed its own held item: {cl}"
    );

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn mid_session_media_fetch() {
    let project = temp_dir("proj-fetch");
    let staging = temp_dir("staging-fetch");
    make_project(&project);
    let (owner, client) = establish("fetch-test", &project, &staging);

    let client_dir = {
        let status: Value = serde_json::from_str(&client.status_json()).unwrap();
        PathBuf::from(status["project_path"].as_str().unwrap())
            .parent()
            .unwrap()
            .to_path_buf()
    };

    // New media appears on the owner *after* the initial transfer (recorded /
    // imported mid-session). Multi-chunk to exercise chunked fetch.
    let snare: Vec<u8> = (0..KICK_SIZE).map(|i| (i % 241) as u8).collect();
    fs::write(project.join("Audio/snare.wav"), &snare).unwrap();

    // A file in the host's folder that the session doesn't use is refused.
    assert!(client.fetch_file("Unused/outtake.wav"));
    std::thread::sleep(Duration::from_secs(2));
    client.fetch_file("Unused/outtake.wav");
    std::thread::sleep(Duration::from_secs(1));
    assert!(!client_dir.join("Unused/outtake.wav").exists(), "unused file was served");

    // The owner puts the new file on the timeline: now it is part of the session.
    owner.push_local(
        r#"{"items":{"{SNARE}":{"present":true,"track":"{T1}","chunk":"<ITEM\n  <SOURCE WAVE\n    FILE \"Audio/snare.wav\"\n  >\n>\n"}}}"#,
    );

    // The client notices an item referencing it and fetches; idempotent calls
    // while missing must be safe.
    assert!(client.fetch_file("Audio/snare.wav"));
    let fetched = wait_until(Duration::from_secs(20), || {
        client.fetch_file("Audio/snare.wav");
        client_dir.join("Audio/snare.wav").exists()
    });
    assert!(fetched, "mid-session fetch never completed");
    assert_eq!(
        fs::read(client_dir.join("Audio/snare.wav")).unwrap(),
        snare,
        "fetched file corrupted"
    );

    // And the reverse direction: the client "records" a file, the owner
    // fetches it from the client. Not before the client put it on the
    // timeline, though: a guest's copy serves session files only.
    let vox: Vec<u8> = (0..123_456).map(|i| (i % 13) as u8).collect();
    fs::create_dir_all(client_dir.join("Audio")).unwrap();
    fs::write(client_dir.join("Audio/vox.wav"), &vox).unwrap();
    let leaked = wait_until(Duration::from_secs(3), || {
        owner.fetch_file("Audio/vox.wav");
        project.join("Audio/vox.wav").exists()
    });
    assert!(!leaked, "a guest's file nothing references was served");
    client.push_local(
        r#"{"items":{"{VOX}":{"present":true,"track":"{T1}","chunk":"<ITEM\n  <SOURCE WAVE\n    FILE \"Audio/vox.wav\"\n  >\n>\n"}}}"#,
    );
    let fetched_back = wait_until(Duration::from_secs(20), || {
        owner.fetch_file("Audio/vox.wav");
        project.join("Audio/vox.wav").exists()
    });
    assert!(fetched_back, "owner never fetched the client's file");
    assert_eq!(fs::read(project.join("Audio/vox.wav")).unwrap(), vox);

    // An item naming a file outside the project never enters the session.
    client.push_local(
        r#"{"items":{"{LEAK}":{"present":true,"track":"{T1}","chunk":"<ITEM\n  <SOURCE WAVE\n    FILE \"/etc/hosts\"\n  >\n>\n"}}}"#,
    );
    std::thread::sleep(Duration::from_secs(2));
    let v: Value = serde_json::from_str(&owner.snapshot_json()).unwrap();
    assert!(v["items"].get("{LEAK}").is_none(), "an out-of-project media path was admitted");

    // Path traversal is refused outright.
    assert!(!client.fetch_file("../escape.wav"));
    assert!(!client.fetch_file("/abs/path.wav"));

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

/// Progressive join: with enough media outstanding, the client reports `ready`
/// (with `background: true`) once the project file and the small files are on
/// disk, while the media keeps streaming; the flag drops when everything has
/// landed, verified. The extension opens the project at the first `ready`
/// with placeholder items and lets the deferred-item path fill them in.
#[test]
fn progressive_join_reports_ready_before_media() {
    let project = temp_dir("proj-progressive");
    let staging = temp_dir("staging-progressive");
    make_project(&project);
    // Well above PROGRESSIVE_MIN_BYTES (16 MiB), spread over three files (one
    // only a plugin uses), all of it held back behind the project file.
    const BIG: usize = 24 * 1024 * 1024;
    let stems: Vec<(String, Vec<u8>)> = (0..3u8)
        .map(|k| {
            let name = format!("Audio/stem{k}.wav");
            (name, (0..BIG).map(|i| ((i as u64 * (k as u64 + 3)) % 253) as u8).collect())
        })
        .collect();
    for (name, bytes) in &stems {
        fs::write(project.join(name), bytes).unwrap();
    }
    fs::write(
        project.join("Song.rpp"),
        // stem1 is loaded by a plugin (its state names it), not by an item.
        "<REAPER_PROJECT 0.1\n<TRACK\n  <FXCHAIN\n    <VST \"VSTi: Sampler\" s.vst 0\n      L1VzZXJzL21lL1NvbmcvQXVkaW8vc3RlbTEud2F2AA==\n    >\n  >\n  <ITEM\n    POSITION 30\n    <SOURCE WAVE\n      FILE \"Audio/stem0.wav\"\n    >\n  >\n  <ITEM\n    POSITION 10\n    <SOURCE WAVE\n      FILE \"Audio/stem2.wav\"\n    >\n  >\n>\n<NOTES 0 2\n  |notes.txt\n>\n>\n",
    )
    .unwrap();

    let owner = Reamesh::host("progressive-test", project.to_str().unwrap(), "Song.rpp", "")
        .expect("host failed");
    let client = Reamesh::join(staging.to_str().unwrap()).expect("join failed");
    let sid = session_id_of(&owner);
    let discovered = wait_until(Duration::from_secs(20), || {
        interconnect(&owner, &client);
        client.sessions_json().contains(&sid)
    });
    assert!(discovered, "client never saw the session announcement");
    assert!(client.join_session(&sid, ""), "join_session refused");

    // Poll tightly: the early `ready` must show up while media is still missing.
    let mut saw_early = false;
    let mut saw_media_missing = false;
    let mut client_dir: Option<PathBuf> = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        assert_ne!(s["state"], "failed", "download failed: {}", s["message"]);
        if s["state"] == "ready" {
            let dir = PathBuf::from(s["project_path"].as_str().unwrap()).parent().unwrap().to_path_buf();
            client_dir.get_or_insert(dir.clone());
            let present: Vec<&str> = stems
                .iter()
                .map(|(n, _)| n.as_str())
                .filter(|n| dir.join(n).exists())
                .collect();
            if s["background"] == true {
                saw_early = true;
                // Ready came with the project file + small files, media pending.
                assert!(dir.join("Song.rpp").exists());
                assert!(dir.join("notes.txt").exists());
                // The last file can land a moment before `background` drops,
                // so only some early report has to come with media missing.
                saw_media_missing |= present.len() < stems.len();
            } else {
                break; // finished
            }
        }
        assert!(Instant::now() < deadline, "download never finished: {}", client.status_json());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(saw_early, "never observed ready with background=true");
    assert!(saw_media_missing, "ready+background only ever with all media present");
    let dir = client_dir.unwrap();
    for (name, bytes) in &stems {
        assert_eq!(&fs::read(dir.join(name)).unwrap(), bytes, "{name} corrupted");
    }
    // The media queue is sorted by timeline position (media_order_from_rpp has
    // its own unit test). Which file lands first is not asserted here: pieces
    // of several files are in flight at once, over more than one connection.
    // The final status is the ordinary completed one.
    let s: Value = serde_json::from_str(&client.status_json()).unwrap();
    assert_eq!(s["state"], "ready");
    assert_eq!(s["background"], false);
    assert_eq!(s["files_done"], s["files_total"]);

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

/// The passphrase pre-check verifies against the owner without joining: a wrong
/// one reports `ok=false`, the right one `ok=true`, both via `status.probe`.
/// This is what lets the app show an inline "wrong password" before any join.
#[test]
fn passphrase_precheck_reports_right_and_wrong() {
    let project = temp_dir("proj-probe");
    let staging = temp_dir("staging-probe");
    make_project(&project);

    let owner = Reamesh::host("probe-test", project.to_str().unwrap(), "Song.rpp", "hunter2")
        .expect("host failed");
    let client = Reamesh::join(staging.to_str().unwrap()).expect("join failed");
    let sid = session_id_of(&owner);
    assert!(wait_until(Duration::from_secs(20), || {
        interconnect(&owner, &client);
        client.sessions_json().contains(&sid)
    }));

    let probe_is = |ok: bool| {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["probe"]["session_id"] == sid && s["probe"]["ok"] == Value::Bool(ok)
    };

    // Wrong passphrase -> ok=false, and the client never transitions (still
    // browsing: no failed/downloading state from a mere pre-check).
    assert!(client.check_passphrase(&sid, "nope"));
    assert!(
        wait_until(Duration::from_secs(12), || probe_is(false)),
        "wrong passphrase wasn't reported false: {}",
        client.status_json()
    );

    // Right passphrase -> ok=true.
    assert!(client.check_passphrase(&sid, "hunter2"));
    assert!(
        wait_until(Duration::from_secs(12), || probe_is(true)),
        "right passphrase wasn't accepted: {}",
        client.status_json()
    );

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn passphrase_gates_download() {
    let project = temp_dir("proj-pass");
    let staging_bad = temp_dir("staging-pass-bad");
    let staging_ok = temp_dir("staging-pass-ok");
    make_project(&project);

    let owner = Reamesh::host("pass-test", project.to_str().unwrap(), "Song.rpp", "hunter2")
        .expect("host failed");
    let sid = session_id_of(&owner);

    // Wrong passphrase: the owner refuses to serve the manifest, the download
    // fails rather than handing the project to an unauthorized peer.
    let wrong = Reamesh::join(staging_bad.to_str().unwrap()).expect("join failed");
    assert!(wait_until(Duration::from_secs(20), || {
        interconnect(&owner, &wrong);
        wrong.sessions_json().contains(&sid)
    }));
    // Find this session by id; other hosts may also appear in the LAN lobby.
    let listing: Value = serde_json::from_str(&wrong.sessions_json()).unwrap();
    let ours = listing
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == sid)
        .expect("our session in the listing");
    assert_eq!(ours["auth"], true);
    // The host's build rides the announce, so the lobby can grey out a
    // session from another PROTO before anyone tries to join it.
    assert_eq!(ours["proto"].as_u64(), Some(reamesh::PROTO as u64));
    assert_eq!(ours["version"], reamesh::VERSION);

    assert!(wrong.join_session(&sid, "wrong"));
    let failed = wait_until(Duration::from_secs(15), || {
        let s: Value = serde_json::from_str(&wrong.status_json()).unwrap();
        s["state"] == "failed"
    });
    assert!(failed, "download with wrong passphrase should fail");
    // The refused peer never joined the session topic, so it must never have
    // appeared in the owner's roster: no ghost "Guest" flashing up on the
    // host while an unauthorized join is being refused.
    assert_eq!(
        owner.connected_peers(),
        0,
        "wrong-passphrase peer reached the session topic"
    );
    let roster: Value = serde_json::from_str(&owner.peers_json()).unwrap();
    assert_eq!(
        roster.as_array().unwrap().len(),
        1,
        "owner roster should hold only the owner itself: {roster}"
    );
    drop(wrong);

    // Right passphrase: downloads normally.
    let good = Reamesh::join(staging_ok.to_str().unwrap()).expect("join failed");
    assert!(wait_until(Duration::from_secs(20), || {
        interconnect(&owner, &good);
        good.sessions_json().contains(&sid)
    }));
    assert!(good.join_session(&sid, "hunter2"));
    let ready = wait_until(Duration::from_secs(30), || {
        let s: Value = serde_json::from_str(&good.status_json()).unwrap();
        assert_ne!(s["state"], "failed", "correct passphrase should not fail");
        s["state"] == "ready"
    });
    assert!(ready, "download with correct passphrase never completed");

    drop(good);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging_bad);
    let _ = fs::remove_dir_all(staging_ok);
}

#[test]
fn track_delete_propagates_as_tombstone() {
    let project = temp_dir("proj-delete");
    let staging = temp_dir("staging-delete");
    make_project(&project);
    let (owner, client) = establish("delete-test", &project, &staging);

    owner.push_local(
        r#"{"tracks":{"{T4}":{"present":true,"index":0,"name":"Gtr","vol":1.0,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    let mut client_state = serde_json::Map::new();
    assert!(wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut client_state);
        client_state.contains_key("{T4}")
    }));

    owner.push_local(r#"{"tracks":{"{T4}":{"present":false}}}"#);
    assert!(wait_until(Duration::from_secs(10), || {
        drain_into(&client, &mut client_state);
        client_state["{T4}"]["present"] == false
    }));
    // Tombstone, not removal: identity must survive in the doc.
    assert_eq!(field(&client.snapshot_json(), "{T4}", "present"), false);

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn owner_shutdown_ends_session_for_clients() {
    let project = temp_dir("proj-ended");
    let staging = temp_dir("staging-ended");
    make_project(&project);
    let (owner, client) = establish("ended-test", &project, &staging);

    // Stopping the host must reach the client as an explicit session end (the
    // farewell broadcast, or (had that been lost) the owner-gone fallback).
    drop(owner);
    let ended = wait_until(Duration::from_secs(25), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["state"] == "ended"
    });
    assert!(
        ended,
        "client never learned the session ended: {}",
        client.status_json()
    );

    drop(client);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn moderation_kick_ends_client_and_allows_rejoin() {
    let project = temp_dir("proj-kick");
    let staging = temp_dir("staging-kick");
    make_project(&project);
    let (owner, client) = establish("kick-test", &project, &staging);
    let sid = session_id_of(&owner);
    let victim = client.local_peer_id().to_string();

    // Nobody but the owner may moderate, and never itself.
    assert!(!client.moderate(owner.local_peer_id(), false));
    assert!(!owner.moderate(owner.local_peer_id(), false));

    assert!(owner.moderate(&victim, false), "owner kick refused");
    let ended = wait_until(Duration::from_secs(10), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["state"] == "ended" && s["end_reason"] == "kicked"
    });
    assert!(ended, "kicked client never ended: {}", client.status_json());
    // Off the host's roster (the kicked peer left the topic on its own).
    let gone = wait_until(Duration::from_secs(6), || {
        let roster: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        owner.connected_peers() == 0 && roster.as_array().unwrap().len() == 1
    });
    assert!(gone, "kicked peer still on the roster: {}", owner.peers_json());
    // A kick is not a ban (the table is process-global, so judge by our peer
    // id rather than expecting it empty): the same device rejoins at once.
    assert!(!reamesh::bans_json().contains(&victim));
    assert!(client.join_session(&sid, ""), "rejoin refused");
    let back = wait_until(Duration::from_secs(30), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        assert_ne!(s["state"], "failed", "rejoin failed: {}", s["message"]);
        s["state"] == "ready"
    });
    assert!(back, "kicked client could not rejoin: {}", client.status_json());
    let meshed = wait_until(Duration::from_secs(15), || owner.connected_peers() >= 1);
    assert!(meshed, "rejoined client never reached the session topic");

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn moderation_ban_refuses_rejoin_until_revoked() {
    let project = temp_dir("proj-ban");
    let staging = temp_dir("staging-ban");
    make_project(&project);
    reamesh::set_profile("Banned Bob", "#123456");
    let (owner, client) = establish("ban-test", &project, &staging);
    let sid = session_id_of(&owner);
    let victim = client.local_peer_id().to_string();
    // A second guest: the ban table must reach every member, not just the
    // banned peer, because every member serves files and shows a roster.
    let staging2 = temp_dir("staging-ban-bystander");
    let bystander = Reamesh::join(staging2.to_str().unwrap()).expect("join failed");
    assert!(wait_until(Duration::from_secs(20), || {
        interconnect(&owner, &bystander);
        interconnect(&client, &bystander);
        bystander.sessions_json().contains(&sid)
    }));
    assert!(bystander.join_session(&sid, ""));
    assert!(wait_until(Duration::from_secs(30), || {
        let s: Value = serde_json::from_str(&bystander.status_json()).unwrap();
        s["state"] == "ready"
    }));
    assert!(wait_until(Duration::from_secs(15), || owner.connected_peers() >= 2));
    // Let the owner learn the client's name so the ban list can show it.
    assert!(wait_until(Duration::from_secs(10), || {
        let o: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        o.as_array().unwrap().iter().any(|p| p["id"] == victim.as_str() && p["name"] == "Banned Bob")
    }));

    assert!(owner.moderate(&victim, true), "owner ban refused");
    let ended = wait_until(Duration::from_secs(10), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["state"] == "ended" && s["end_reason"] == "banned"
    });
    assert!(ended, "banned client never ended: {}", client.status_json());
    // Off the host's roster and out of its member count; the bystander stays.
    let gone = wait_until(Duration::from_secs(6), || {
        let roster: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        owner.connected_peers() == 1
            && roster.as_array().unwrap().len() == 2
            && !roster.as_array().unwrap().iter().any(|p| p["id"] == victim.as_str())
    });
    assert!(gone, "banned peer still on the roster: {}", owner.peers_json());
    // The owner broadcast the table: the other guest applies it too.
    let learned = wait_until(Duration::from_secs(6), || {
        bystander.banned_peers() == vec![victim.clone()]
    });
    assert!(learned, "bystander never received the ban table: {:?}", bystander.banned_peers());
    assert_eq!(owner.banned_peers(), vec![victim.clone()]);

    // The ban table names the device, with the name it had.
    let bans: Value = serde_json::from_str(&reamesh::bans_json()).unwrap();
    let entry = bans
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == victim.as_str())
        .expect("banned peer listed");
    assert_eq!(entry["name"], "Banned Bob");
    assert_eq!(entry["color"], "#123456");
    assert!(entry["expires_at"].as_u64().unwrap() > 0);

    // A passphrase pre-check answers "banned", not "wrong password"...
    assert!(client.check_passphrase(&sid, ""));
    let probed = wait_until(Duration::from_secs(10), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["probe"]["ok"] == false && s["probe"]["reason"] == "banned"
    });
    assert!(probed, "probe from a banned peer: {}", client.status_json());
    // ...and a join is refused outright.
    assert!(client.join_session(&sid, ""));
    let refused = wait_until(Duration::from_secs(15), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["state"] == "failed" && s["message"].as_str().unwrap_or("").contains("banned")
    });
    assert!(refused, "banned peer was served: {}", client.status_json());
    assert_eq!(owner.connected_peers(), 1, "banned peer counted as a member");

    // Revoking the ban lets the same device straight back in.
    assert!(reamesh::unban(&victim));
    assert!(!reamesh::unban(&victim), "unban of an unbanned peer reported true");
    assert!(!reamesh::bans_json().contains(&victim));
    let forgotten = wait_until(Duration::from_secs(6), || bystander.banned_peers().is_empty());
    assert!(forgotten, "bystander kept a lifted ban: {:?}", bystander.banned_peers());
    assert!(client.join_session(&sid, ""), "rejoin refused");
    let back = wait_until(Duration::from_secs(30), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["state"] == "ready"
    });
    assert!(back, "unbanned client could not rejoin: {}", client.status_json());
    let meshed = wait_until(Duration::from_secs(15), || owner.connected_peers() >= 2);
    assert!(meshed, "unbanned client never reached the session topic");

    drop(bystander);
    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
    let _ = fs::remove_dir_all(staging2);
}

#[test]
fn client_leave_clears_owner_roster_quickly() {
    let project = temp_dir("proj-leave");
    let staging = temp_dir("staging-leave");
    make_project(&project);
    let (owner, client) = establish("leave-test", &project, &staging);

    // A leaving client unsubscribes before shutdown, so the owner removes it
    // within a few ticks instead of waiting for the presence timeout.
    drop(client);
    let gone = wait_until(Duration::from_secs(4), || {
        let roster: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        owner.connected_peers() == 0 && roster.as_array().unwrap().len() == 1
    });
    assert!(
        gone,
        "client departure not noticed promptly: {}",
        owner.peers_json()
    );

    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn join_against_dead_owner_fails_instead_of_hanging() {
    let project = temp_dir("proj-deadowner");
    let staging = temp_dir("staging-deadowner");
    make_project(&project);

    let owner = Reamesh::host("deadowner-test", project.to_str().unwrap(), "Song.rpp", "")
        .expect("host failed");
    let sid = session_id_of(&owner);
    let client = Reamesh::join(staging.to_str().unwrap()).expect("join failed");
    assert!(wait_until(Duration::from_secs(20), || {
        interconnect(&owner, &client);
        client.sessions_json().contains(&sid)
    }));

    // The owner dies right before the join: the cached announcement still
    // names it, so the manifest request goes nowhere. The join must fail
    // (MANIFEST_TIMEOUT) rather than re-request forever on a dead screen.
    drop(owner);
    assert!(client.join_session(&sid, ""));
    let resolved = wait_until(Duration::from_secs(30), || {
        let s: Value = serde_json::from_str(&client.status_json()).unwrap();
        s["state"] == "failed" || s["state"] == "ended"
    });
    assert!(
        resolved,
        "join against a dead owner never resolved: {}",
        client.status_json()
    );

    drop(client);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn presence_names_propagate() {
    let project = temp_dir("proj-presence");
    let staging = temp_dir("staging-presence");
    make_project(&project);
    // The profile is process-global, so both in-process peers beacon the same
    // name; what this verifies is the wire round trip: each side must learn
    // the *other's* name from its presence messages, not from local state.
    reamesh::set_profile("Tester", "#a1b2c3");
    let (owner, client) = establish("presence-test", &project, &staging);

    let seen = wait_until(Duration::from_secs(10), || {
        let o: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        let c: Value = serde_json::from_str(&client.peers_json()).unwrap();
        let named = |v: &Value| {
            v.as_array().unwrap().iter().any(|p| {
                p["is_self"] == false && p["name"] == "Tester" && p["color"] == "#a1b2c3"
            })
        };
        named(&o) && named(&c)
    });
    assert!(seen, "presence beacons never arrived: {}", owner.peers_json());

    // Shape checks: self first, exactly one owner on each side.
    let o: Value = serde_json::from_str(&owner.peers_json()).unwrap();
    assert_eq!(o[0]["is_self"], true);
    assert_eq!(o[0]["is_owner"], true);
    let c: Value = serde_json::from_str(&client.peers_json()).unwrap();
    assert_eq!(c[0]["is_self"], true);
    assert_eq!(c[0]["is_owner"], false);
    assert!(c.as_array().unwrap().iter().any(|p| p["is_owner"] == true));

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn presence_selection_and_fx_propagate() {
    let project = temp_dir("proj-presence-detail");
    let staging = temp_dir("staging-presence-detail");
    make_project(&project);
    let (owner, client) = establish("presence-detail-test", &project, &staging);

    // Live presence is process-global, so both in-process peers beacon the
    // same detail; what matters is that each side learns the *other's*
    // selection + FX window from the wire, sanitized, not from local state.
    let fx = serde_json::json!({"track": "{T-1}", "item": "{I-1}", "take": 0, "fx": 2, "float": true});
    reamesh::set_presence(
        &serde_json::json!({ "sel": ["{I-1}", "{I-2}"], "fx": fx, "focus": "{T-1}" }).to_string(),
    );
    let seen = wait_until(Duration::from_secs(10), || {
        let check = |peer: &Reamesh| {
            let v: Value = serde_json::from_str(&peer.peers_json()).unwrap();
            v.as_array().unwrap().iter().any(|p| {
                p["is_self"] == false
                    && p["sel"] == serde_json::json!(["{I-1}", "{I-2}"])
                    && p["fx"] == fx
            })
        };
        check(&owner) && check(&client)
    });
    assert!(seen, "selection/fx detail never arrived: {}", owner.peers_json());

    // Clearing the detail clears it on the other side too (a closed FX
    // window must not stay "open" in a follower).
    reamesh::set_presence(&serde_json::json!({ "focus": "{T-1}" }).to_string());
    let cleared = wait_until(Duration::from_secs(10), || {
        let v: Value = serde_json::from_str(&client.peers_json()).unwrap();
        v.as_array().unwrap().iter().any(|p| {
            p["is_self"] == false && p["sel"] == serde_json::json!([]) && p["fx"].is_null()
        })
    });
    assert!(cleared, "cleared detail never propagated: {}", client.peers_json());

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}

#[test]
fn followed_peer_beacons_fast() {
    let project = temp_dir("proj-followfast");
    let staging = temp_dir("staging-followfast");
    make_project(&project);
    let (owner, client) = establish("followfast-test", &project, &staging);

    let self_id = |peer: &Reamesh| -> String {
        let v: Value = serde_json::from_str(&peer.peers_json()).unwrap();
        v[0]["id"].as_str().unwrap().to_string()
    };
    let owner_id = self_id(&owner);
    let client_id = self_id(&client);
    assert!(wait_until(Duration::from_secs(10), || {
        owner.beacon_count(&client_id) > 0 && client.beacon_count(&owner_id) > 0
    }));

    // Nobody follows anybody: ~1 beacon per second each way.
    let count_in = |window: Duration, from: &Reamesh, of: &str| -> u64 {
        let before = from.beacon_count(of);
        std::thread::sleep(window);
        from.beacon_count(of) - before
    };
    let idle = count_in(Duration::from_secs(2), &client, &owner_id);
    assert!((1..=4).contains(&idle), "idle beacon rate off: {idle} in 2s");

    // The client follows the owner. Live presence is process-global, so the
    // owner's own beacon *also* says "follow <owner>", which nobody acts on:
    // following yourself isn't being followed. Only the owner, whose id the
    // client's beacon names, must switch to the fast cadence.
    reamesh::set_presence(&serde_json::json!({ "follow": owner_id }).to_string());
    assert!(wait_until(Duration::from_secs(5), || {
        let o: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        o.as_array().unwrap().iter().any(|p| p["id"] == client_id && p["follow"] == owner_id)
    }));
    let fast = count_in(Duration::from_secs(2), &client, &owner_id);
    let other = count_in(Duration::from_secs(2), &owner, &client_id);
    assert!(fast >= 10, "followed owner did not beacon fast: {fast} in 2s");
    assert!(other <= 4, "the follower sped up too: {other} in 2s");

    // Stop following: once the client's beacon no longer names the owner, the
    // owner drops back to the idle cadence.
    reamesh::set_presence("{}");
    assert!(wait_until(Duration::from_secs(5), || {
        let o: Value = serde_json::from_str(&owner.peers_json()).unwrap();
        o.as_array().unwrap().iter().any(|p| p["id"] == client_id && p["follow"] == "")
    }));
    let slow = count_in(Duration::from_secs(2), &client, &owner_id);
    assert!(slow <= 4, "owner kept the fast cadence after unfollow: {slow} in 2s");

    drop(client);
    drop(owner);
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_dir_all(staging);
}
