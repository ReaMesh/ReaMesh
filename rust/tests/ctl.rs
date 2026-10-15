//! Control-server round trip: this test plays the ReaMesh app, talking to
//! the same WebSocket server REAPER's extension starts.

use libp2p::futures::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[test]
fn ctl_server_round_trip() {
    let port = reamesh::reamesh_ctl_start();
    assert_ne!(port, 0, "ctl server failed to bind");
    // Idempotent: second start returns the same port.
    assert_eq!(reamesh::reamesh_ctl_start(), port);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let (mut ws, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}")),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");

        // Greeting arrives immediately.
        let hello = ws.next().await.unwrap().unwrap();
        let v: Value = serde_json::from_str(hello.to_text().unwrap()).unwrap();
        assert_eq!(v["type"], "hello");
        assert!(v["pid"].as_u64().unwrap() > 0);
        // The app checks these before driving anything.
        assert_eq!(v["version"], reamesh::VERSION);
        assert_eq!(v["proto"].as_u64(), Some(reamesh::PROTO as u64));

        // set_profile is handled server-side, not queued for the extension.
        ws.send(Message::Text(
            r##"{"cmd":"set_profile","name":"Ctl Tester","color":"#123456"}"##.into(),
        ))
        .await
        .unwrap();
        // A real command queues for the extension's poll.
        ws.send(Message::Text(r#"{"cmd":"browse"}"#.into()))
            .await
            .unwrap();

        // Drain like the extension tick would.
        let cmd = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(s) = ffi_poll() {
                    return s;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("command never queued");
        let v: Value = serde_json::from_str(&cmd).unwrap();
        assert_eq!(v["cmd"], "browse");
        assert!(ffi_poll().is_none(), "set_profile must not reach the queue");

        // State published by the extension reaches the app.
        let state = r#"{"type":"state","mode":"idle"}"#;
        let cs = std::ffi::CString::new(state).unwrap();
        reamesh::reamesh_ctl_publish(cs.as_ptr());
        let got = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("no state broadcast")
            .unwrap()
            .unwrap();
        assert_eq!(got.to_text().unwrap(), state);

        // A late joiner gets the last state replayed right after the hello.
        let (mut ws2, _) =
            tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
                .await
                .unwrap();
        let _hello = ws2.next().await.unwrap().unwrap();
        let replay = tokio::time::timeout(Duration::from_secs(5), ws2.next())
            .await
            .expect("no replay")
            .unwrap()
            .unwrap();
        assert_eq!(replay.to_text().unwrap(), state);
    });
}

fn ffi_poll() -> Option<String> {
    let p = reamesh::reamesh_ctl_poll();
    if p.is_null() {
        return None;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned();
    reamesh::reamesh_free_string(p);
    Some(s)
}
