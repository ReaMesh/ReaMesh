//! Control-server round trip: this test plays the ReaMesh app, talking to
//! the same WebSocket server REAPER's extension starts.

use libp2p::futures::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;

/// Read the run token the server wrote for `port`, like the app's backend.
fn ctl_token(port: u16) -> String {
    let path = std::env::temp_dir().join(format!("reamesh-ctl-{port}.token"));
    std::fs::read_to_string(path).expect("control token file missing")
}

/// A handshake request offering `protocol` as the WebSocket subprotocol.
fn request(port: u16, protocol: Option<&str>) -> Request {
    let mut req = format!("ws://127.0.0.1:{port}")
        .into_client_request()
        .unwrap();
    if let Some(p) = protocol {
        req.headers_mut()
            .insert("sec-websocket-protocol", HeaderValue::from_str(p).unwrap());
    }
    req
}

fn authed(port: u16) -> Request {
    request(port, Some(&format!("reamesh.{}", ctl_token(port))))
}

#[test]
fn ctl_server_refuses_clients_without_the_token() {
    let port = reamesh::reamesh_ctl_start();
    assert_ne!(port, 0, "ctl server failed to bind");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("reamesh-ctl-{port}.token"));
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "token file readable by others");
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let wrong = "reamesh.".to_string() + &"0".repeat(64);
        for proto in [None, Some("reamesh."), Some(wrong.as_str()), Some("chat")] {
            let res = tokio::time::timeout(
                Duration::from_secs(5),
                tokio_tungstenite::connect_async(request(port, proto)),
            )
            .await
            .expect("connect timed out");
            assert!(res.is_err(), "handshake offering {proto:?} was accepted");
        }
        // The right token gets in, and the server echoes the subprotocol.
        let (_ws, resp) = tokio_tungstenite::connect_async(authed(port))
            .await
            .expect("authenticated connect failed");
        let echoed = resp
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert_eq!(echoed, format!("reamesh.{}", ctl_token(port)));
    });
}

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
            tokio_tungstenite::connect_async(authed(port)),
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
            tokio_tungstenite::connect_async(authed(port))
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
