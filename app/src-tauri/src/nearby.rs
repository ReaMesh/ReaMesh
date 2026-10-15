//! Nearby sessions over Bluetooth LE: a discovery fallback for networks that
//! block LAN discovery (guest Wi-Fi filtering multicast), while the session
//! itself still runs over the network.
//!
//! A host advertises its dialable IPs and ports (taken from its invite link)
//! and nothing else; a scanner hands what it reads to the webview
//! (`nearby:found`), which passes it on to the extension as a dial `hint`.
//! The extension dials only addresses in its own subnets, and a session only
//! appears in the Lobby once its signed announce arrives over that
//! connection, so a Bluetooth payload is never trusted for anything.
//!
//! The preference (`nearby` file: "1" on, "0" off, absent = never decided)
//! gates everything: the CoreBluetooth managers, and with them the system's
//! permission prompt, only exist once the user turned it on. macOS only for
//! now (src/ble.m); elsewhere it reports `supported: false`.

use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

/// Largest payload either side handles (the characteristic value).
const MAX_PAYLOAD: usize = 512;
/// Most IPs one payload names, as in an invite link.
const MAX_IPS: usize = 8;

fn pref_file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("nearby"))
}

/// The stored choice: `None` until the user decided either way.
pub fn load_pref(app: &AppHandle) -> Option<bool> {
    let s = fs::read_to_string(pref_file(app)?).ok()?;
    match s.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

fn enabled(app: &AppHandle) -> bool {
    supported() && load_pref(app) == Some(true)
}

pub fn store_pref(app: &AppHandle, on: bool) {
    if let Some(p) = pref_file(app) {
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(p, if on { "1" } else { "0" });
    }
    if !on {
        ble::scan(false);
        ble::advertise(None);
    }
    // The webview re-sends scan/advertise for the screen it is on.
    let _ = app.emit("nearby-changed", ());
    crate::refresh_native_settings(app);
}

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// Bluetooth permission as the webview and settings window show it.
pub fn authorization() -> &'static str {
    match ble::authorization() {
        0 => "undetermined",
        1 => "restricted",
        2 => "denied",
        3 => "allowed",
        _ => "unsupported",
    }
}

/// A payload is `{"v":<proto>,"ips":[…],"tcp":n,"quic":n}`: re-serialize it
/// from its parts so only those keys, bounded, ever leave this function.
fn clean_payload(raw: &str) -> Option<String> {
    if raw.len() > MAX_PAYLOAD {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let port = |k: &str| v.get(k).and_then(|p| p.as_u64()).filter(|p| (1..=65535).contains(p));
    let ips: Vec<String> = v
        .get("ips")?
        .as_array()?
        .iter()
        .filter_map(|x| x.as_str())
        .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
        .take(MAX_IPS)
        .map(str::to_string)
        .collect();
    let (tcp, quic) = (port("tcp"), port("quic"));
    if ips.is_empty() || (tcp.is_none() && quic.is_none()) {
        return None;
    }
    let out = serde_json::json!({
        "v": v.get("v").and_then(|x| x.as_u64()).unwrap_or(0),
        "ips": ips,
        "tcp": tcp.unwrap_or(0),
        "quic": quic.unwrap_or(0),
    })
    .to_string();
    (out.len() <= MAX_PAYLOAD).then_some(out)
}

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

/// A payload read from a nearby host: validated, then handed to the main
/// window, which owns the connection to the extension.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn on_found(raw: &str) {
    let (Some(app), Some(clean)) = (APP.get(), clean_payload(raw)) else { return };
    let _ = app.emit_to("main", "nearby:found", clean);
}

pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    ble::init();
}

#[derive(Serialize)]
pub struct NearbyState {
    supported: bool,
    /// `null` while the user never decided.
    enabled: Option<bool>,
    auth: &'static str,
}

#[tauri::command]
pub fn nearby_state(app: AppHandle) -> NearbyState {
    NearbyState {
        supported: supported(),
        enabled: if supported() { load_pref(&app) } else { Some(false) },
        auth: authorization(),
    }
}

#[tauri::command]
pub fn set_nearby(app: AppHandle, on: bool) -> NearbyState {
    store_pref(&app, on);
    nearby_state(app)
}

/// Scan while the Lobby is on screen (no-op unless enabled).
#[tauri::command]
pub fn nearby_scan(app: AppHandle, on: bool) {
    ble::scan(on && enabled(&app));
}

/// Advertise this host's addresses while hosting; `None` stops.
#[tauri::command]
pub fn nearby_advertise(app: AppHandle, payload: Option<String>) {
    let clean = payload.as_deref().and_then(clean_payload).filter(|_| enabled(&app));
    ble::advertise(clean.as_deref());
}

#[cfg(target_os = "macos")]
mod ble {
    use std::ffi::{c_char, CStr, CString};

    extern "C" {
        fn reamesh_ble_init(found: extern "C" fn(*const c_char));
        fn reamesh_ble_authorization() -> i32;
        fn reamesh_ble_advertise(payload: *const c_char);
        fn reamesh_ble_scan(on: i32);
    }

    extern "C" fn found(p: *const c_char) {
        if p.is_null() {
            return;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        super::on_found(&s);
    }

    pub fn init() {
        unsafe { reamesh_ble_init(found) }
    }

    pub fn authorization() -> i32 {
        unsafe { reamesh_ble_authorization() }
    }

    pub fn advertise(payload: Option<&str>) {
        let c = payload.and_then(|p| CString::new(p).ok());
        unsafe { reamesh_ble_advertise(c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())) }
    }

    pub fn scan(on: bool) {
        unsafe { reamesh_ble_scan(on as i32) }
    }
}

#[cfg(not(target_os = "macos"))]
mod ble {
    pub fn init() {}
    pub fn authorization() -> i32 {
        -1
    }
    pub fn advertise(_: Option<&str>) {}
    pub fn scan(_: bool) {}
}

#[cfg(test)]
mod tests {
    use super::clean_payload;

    #[test]
    fn payload_is_rebuilt_from_its_parts() {
        let got = clean_payload(r#"{"v":2,"ips":["10.0.0.2","nope",3],"tcp":4001,"quic":4002,"x":"y"}"#)
            .map(|s| serde_json::from_str::<serde_json::Value>(&s).unwrap());
        assert_eq!(
            got,
            Some(serde_json::json!({"v": 2, "ips": ["10.0.0.2"], "tcp": 4001, "quic": 4002}))
        );
        assert_eq!(clean_payload(r#"{"ips":[],"tcp":1}"#), None);
        assert_eq!(clean_payload(r#"{"ips":["10.0.0.2"],"tcp":0}"#), None);
        assert_eq!(clean_payload("garbage"), None);
        assert_eq!(clean_payload(&"x".repeat(600)), None);
    }
}
