//! In-process two-peer tests: CRDT + networking with zero REAPER dependency.
//!
//! Each test creates two full `Reamesh` instances (each with its own Tokio
//! runtime, libp2p swarm, and yrs doc) in one process and talks over
//! loopback. Discovery prefers mDNS; if multicast is unavailable (sandboxed
//! CI), the test falls back to dialing the peer's listen address directly.

use reamesh::Reamesh;
use std::time::{Duration, Instant};

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Wait until both peers see each other on the project topic, falling back to
/// an explicit dial when mDNS doesn't get there on its own.
fn connect(a: &Reamesh, b: &Reamesh) {
    let linked = |x: &Reamesh, y: &Reamesh| x.connected_peers() > 0 && y.connected_peers() > 0;
    if wait_until(Duration::from_secs(3), || linked(a, b)) {
        return;
    }
    let addrs = b.listen_addrs();
    assert!(!addrs.is_empty(), "peer B reported no listen addresses");
    for addr in &addrs {
        a.dial(addr);
    }
    assert!(
        wait_until(Duration::from_secs(10), || linked(a, b)),
        "peers failed to join the session even with a direct dial"
    );
}

fn drain_until_nonempty(peer: &Reamesh, timeout: Duration) -> Option<String> {
    let mut out = None;
    wait_until(timeout, || {
        let drained = peer.drain_remote();
        if drained != "[]" {
            out = Some(drained);
            true
        } else {
            false
        }
    });
    out
}

#[test]
fn edit_propagates_between_peers() {
    let a = Reamesh::new("reamesh-test-propagate").unwrap();
    let b = Reamesh::new("reamesh-test-propagate").unwrap();
    connect(&a, &b);

    // A adds a track.
    a.push_local(
        r#"{"tracks":{"{11111111-1111-1111-1111-111111111111}":
            {"present":true,"index":0,"name":"Drums","vol":1.0,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    let received = drain_until_nonempty(&b, Duration::from_secs(10))
        .expect("peer B never received A's changeset");
    assert!(received.contains("{11111111-1111-1111-1111-111111111111}"));
    assert!(received.contains("Drums"));

    // And the reverse direction: B mutes the track.
    b.push_local(r#"{"tracks":{"{11111111-1111-1111-1111-111111111111}":{"mute":true}}}"#);
    let received = drain_until_nonempty(&a, Duration::from_secs(10))
        .expect("peer A never received B's changeset");
    assert!(received.contains("mute"));
    assert!(received.contains("true"));

    // Edits already drained must not echo back.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(a.drain_remote(), "[]", "stale echo surfaced on A");
    assert_eq!(b.drain_remote(), "[]", "stale echo surfaced on B");
}

#[test]
fn concurrent_same_field_edits_converge() {
    let a = Reamesh::new("reamesh-test-lww").unwrap();
    let b = Reamesh::new("reamesh-test-lww").unwrap();
    connect(&a, &b);

    // Seed a track from A; wait until B has merged it.
    a.push_local(
        r#"{"tracks":{"{22222222-2222-2222-2222-222222222222}":
            {"present":true,"index":0,"name":"Bass","vol":1.0,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    drain_until_nonempty(&b, Duration::from_secs(10)).expect("seed track never reached B");

    // Concurrent conflicting writes to the same field.
    a.push_local(r#"{"tracks":{"{22222222-2222-2222-2222-222222222222}":{"vol":0.25}}}"#);
    b.push_local(r#"{"tracks":{"{22222222-2222-2222-2222-222222222222}":{"vol":0.75}}}"#);

    assert!(
        wait_until(Duration::from_secs(10), || {
            a.drain_remote();
            b.drain_remote();
            a.snapshot_json() == b.snapshot_json()
        }),
        "peers did not converge after a same-field conflict:\n A: {}\n B: {}",
        a.snapshot_json(),
        b.snapshot_json()
    );
    let converged = a.snapshot_json();
    assert!(
        converged.contains("0.25") || converged.contains("0.75"),
        "converged value is neither writer's value: {converged}"
    );
}

#[test]
fn concurrent_different_field_edits_both_survive() {
    let a = Reamesh::new("reamesh-test-merge").unwrap();
    let b = Reamesh::new("reamesh-test-merge").unwrap();
    connect(&a, &b);

    a.push_local(
        r#"{"tracks":{"{33333333-3333-3333-3333-333333333333}":
            {"present":true,"index":0,"name":"Keys","vol":1.0,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    drain_until_nonempty(&b, Duration::from_secs(10)).expect("seed track never reached B");

    // A renames; B pans. Both edits must survive on both peers.
    a.push_local(r#"{"tracks":{"{33333333-3333-3333-3333-333333333333}":{"name":"Synth"}}}"#);
    b.push_local(r#"{"tracks":{"{33333333-3333-3333-3333-333333333333}":{"pan":-0.5}}}"#);

    assert!(
        wait_until(Duration::from_secs(10), || {
            a.drain_remote();
            b.drain_remote();
            let sa = a.snapshot_json();
            sa == b.snapshot_json() && sa.contains("Synth") && sa.contains("-0.5")
        }),
        "edits to different fields did not both survive:\n A: {}\n B: {}",
        a.snapshot_json(),
        b.snapshot_json()
    );
}

#[test]
fn track_delete_propagates_as_tombstone() {
    let a = Reamesh::new("reamesh-test-delete").unwrap();
    let b = Reamesh::new("reamesh-test-delete").unwrap();
    connect(&a, &b);

    a.push_local(
        r#"{"tracks":{"{44444444-4444-4444-4444-444444444444}":
            {"present":true,"index":0,"name":"Gtr","vol":1.0,"pan":0.0,"mute":false,"solo":false}}}"#,
    );
    drain_until_nonempty(&b, Duration::from_secs(10)).expect("seed track never reached B");

    a.push_local(r#"{"tracks":{"{44444444-4444-4444-4444-444444444444}":{"present":false}}}"#);
    let received = drain_until_nonempty(&b, Duration::from_secs(10))
        .expect("delete tombstone never reached B");
    assert!(received.contains(r#""present":false"#), "got: {received}");
}
