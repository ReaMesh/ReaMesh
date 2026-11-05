//! The whole update path with the real binary: reading a channel, downloading,
//! checking, staging, a running relay handing over, and falling back when the
//! new build doesn't start. Debug builds only: only they accept a test key and
//! test channel from the environment.
#![cfg(all(unix, debug_assertions))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BUILD: &[u8] = include_bytes!("fixtures/update-build.sh");
const SIGNATURE: &str = include_str!("fixtures/update-build.sig");
const PUBKEY: &str = include_str!("fixtures/update-build.pub");
const WAIT: Duration = Duration::from_secs(20);

/// A tiny web server for the channel manifest and the build; returns the
/// manifest's URL.
fn serve_channel(signature: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let manifest = serde_json::json!({
        "version": "99.0.0",
        "platforms": {},
        "relay": { "test-platform": { "url": format!("{base}/build"), "signature": signature } },
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 2048];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]);
            let body: &[u8] = if request.starts_with("GET /build") {
                BUILD
            } else {
                manifest.as_bytes()
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    format!("{base}/stable.json")
}

struct Env {
    data: tempfile::TempDir,
    out: PathBuf,
    channel: String,
}

impl Env {
    fn new(signature: &'static str) -> Self {
        let data = tempfile::tempdir().unwrap();
        reamesh_relay::config::init(data.path()).unwrap();
        let path = data.path().join(reamesh_relay::config::CONFIG_FILE);
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
        std::fs::write(path, text).unwrap();
        let out = data.path().join("out");
        Self {
            data,
            out,
            channel: serve_channel(signature),
        }
    }

    fn relay(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_reamesh-relay"));
        c.arg("--data")
            .arg(self.data.path())
            .args(args)
            .env("REAMESH_RELAY_TEST_PUBKEY", PUBKEY.trim())
            .env("REAMESH_RELAY_TEST_PLATFORM", "test-platform")
            .env("REAMESH_RELAY_TEST_MANIFEST", &self.channel)
            .env("REAMESH_RELAY_TEST_POLL_MS", "100")
            .env("REAMESH_TEST_OUT", &self.out)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        c
    }

    fn update_dir(&self) -> PathBuf {
        self.data.path().join("update")
    }

    fn out(&self) -> String {
        std::fs::read_to_string(&self.out).unwrap_or_default()
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn running(data: &Path) -> bool {
    reamesh_relay::status::Status::read(data).is_some_and(|s| s.running)
}

fn stop(mut child: Child) {
    let _ = Command::new("kill").arg(child.id().to_string()).status();
    let _ = child.wait();
}

#[test]
fn a_running_relay_hands_over_to_a_downloaded_build_and_falls_back_if_it_fails() {
    let env = Env::new(SIGNATURE);

    // The relay runs; `update` fetches the build.
    let relay = env.relay(&["run"]).spawn().unwrap();
    wait_until("the relay to run", || running(env.data.path()));
    assert!(env.relay(&["update"]).status().unwrap().success());
    assert_eq!(
        std::fs::read_to_string(env.update_dir().join("version")).unwrap(),
        "99.0.0"
    );

    // Nothing is being relayed, so it stops and starts the new build, with its
    // own arguments, without any supervisor.
    wait_until("the new build to start", || env.out().contains("started"));
    assert!(
        env.out()
            .contains(&format!("--data {} run", env.data.path().display())),
        "{}",
        env.out()
    );
    let mut relay = relay;
    assert!(relay.wait().unwrap().success());

    // The stand-in never reached `ready`: the next start stays on this version
    // and won't take that build again.
    let relay = env.relay(&["run"]).spawn().unwrap();
    wait_until("the fallback", || {
        std::fs::read_to_string(env.update_dir().join("bad")).is_ok()
    });
    assert!(!env.update_dir().join("reamesh-relay").exists());
    std::fs::remove_file(env.data.path().join("status.json")).ok();
    wait_until("the relay to run again", || running(env.data.path()));
    assert_eq!(env.out().matches("started").count(), 1);
    assert!(env.relay(&["update"]).status().unwrap().success());
    assert!(
        !env.update_dir().join("reamesh-relay").exists(),
        "a bad version came back"
    );
    stop(relay);
}

#[test]
fn a_build_with_a_wrong_signature_is_refused() {
    // Valid form, signed for other bytes.
    let env = Env::new(include_str!("fixtures/other.sig"));
    assert!(!env.relay(&["update"]).status().unwrap().success());
    assert!(!env.update_dir().join("reamesh-relay").exists());
    assert!(!env.update_dir().join("version").exists());
    let mut relay = env.relay(&["run"]).spawn().unwrap();
    wait_until("the relay to run", || running(env.data.path()));
    assert_eq!(env.out(), "");
    assert!(relay.try_wait().unwrap().is_none());
    stop(relay);
}
