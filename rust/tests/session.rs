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
/// multi-chunk media file, an empty file, and files that must NOT transfer.
fn make_project(dir: &Path) {
    fs::write(dir.join("Song.rpp"), b"<REAPER_PROJECT 0.1\n>\n").unwrap();
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

    drop(client);
    drop(owner);
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

/// Active-editor-wins: when the owner edited a field a while ago and the client
/// edits it now, the client is the one working on it, so its proposal must be
/// admitted (not shadowed by stale owner priority). This is the case that made
/// collaboration feel like a fight before: the owner no longer statically wins.
#[test]
fn client_wins_when_it_edits_after_the_owner() {
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

    // Wait well past the tie window so the next edit is unambiguously "later",
    // then the client edits the same field. It is now the active editor.
    std::thread::sleep(Duration::from_millis(1800));
    client.push_local(r#"{"tracks":{"{T7}":{"vol":0.1}}}"#);

    let mut owner_state = serde_json::Map::new();
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        drain_into(&owner, &mut owner_state);
        drain_into(&client, &mut client_state);
        std::thread::sleep(Duration::from_millis(100));
    }

    // The owner admitted the client's later edit on both sides...
    assert_eq!(field(&owner.snapshot_json(), "{T7}", "vol"), 0.1);
    assert_eq!(field(&client.snapshot_json(), "{T7}", "vol"), 0.1);
    assert_eq!(
        owner_state.get("{T7}").map(|t| t["vol"].clone()),
        Some(serde_json::json!(0.1)),
        "owner never applied the client's later edit"
    );
    // ...and the client was not overridden, so nothing snapped back.
    let st: Value = serde_json::from_str(&client.status_json()).unwrap();
    assert_eq!(
        st["reverts"].as_u64().unwrap_or(0),
        0,
        "client's later edit was wrongly counted as overridden: {st}"
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
    // fetches it from the client.
    let vox: Vec<u8> = (0..123_456).map(|i| (i % 13) as u8).collect();
    fs::create_dir_all(client_dir.join("Audio")).unwrap();
    fs::write(client_dir.join("Audio/vox.wav"), &vox).unwrap();
    let fetched_back = wait_until(Duration::from_secs(20), || {
        owner.fetch_file("Audio/vox.wav");
        project.join("Audio/vox.wav").exists()
    });
    assert!(fetched_back, "owner never fetched the client's file");
    assert_eq!(fs::read(project.join("Audio/vox.wav")).unwrap(), vox);

    // Path traversal is refused outright.
    assert!(!client.fetch_file("../escape.wav"));
    assert!(!client.fetch_file("/abs/path.wav"));

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
