// In-app updates. Checks the signed release manifest (plugins.updater in
// tauri.conf.json), downloads a newer build in the background and holds it in
// memory; nothing is installed until the user clicks restart. The state lives
// here rather than in the webview so the native macOS settings window and the
// webview show the same thing.
//
// The manifest carries the release's `proto` next to the fields Tauri reads.
// A restart while a session is up is only offered when that number matches
// ours: the session runs in REAPER and survives the app restarting, but an app
// with another PROTO would refuse the extension REAPER still has loaded.
//
// Each release also publishes its extension on its own (`extension.json` next
// to the app artifacts, the same updater manifest format and signing key).
// Once the app update is downloaded, that copy is fetched and installed as
// soon as no REAPER has ReaMesh loaded (`crate::preinstall_extension`), so a
// REAPER started before the user restarts the app already runs the new
// extension and doesn't need another restart afterwards.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// First check after launch: late enough not to compete with the start-up.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(8);
/// Background re-check cadence while automatic checks are on.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// How long the webview gets to fade out before the process goes away.
const FADE_OUT: Duration = Duration::from_millis(220);
/// How often a pending pre-install looks again whether REAPER has quit. Short:
/// a quick REAPER restart must not slip through with the old extension.
const PREINSTALL_RETRY: Duration = Duration::from_secs(1);
/// The extension's manifest, next to the app artifacts of the same release.
const EXTENSION_MANIFEST: &str = "extension.json";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Never checked yet this run.
    #[default]
    Idle,
    Checking,
    /// The last check found nothing newer.
    Current,
    Downloading,
    /// Downloaded and verified, waiting for the user's restart.
    Ready,
    /// The last check or download failed; the next check starts over.
    Failed,
}

#[derive(Clone, Serialize, Default)]
pub struct UpdateState {
    pub phase: Phase,
    /// The version on offer ("" until a check found one).
    pub version: String,
    /// Download progress 0..1 (0 when the size is unknown).
    pub progress: f64,
    /// Restart is possible right now (not mid-session with another PROTO).
    pub can_restart: bool,
}

struct Inner {
    state: UpdateState,
    /// The downloaded update and its verified bytes.
    pending: Option<(Update, Vec<u8>)>,
    /// The offered release's PROTO (None when the manifest has none).
    proto: Option<u32>,
}

static INNER: Mutex<Inner> = Mutex::new(Inner {
    state: UpdateState {
        phase: Phase::Idle,
        version: String::new(),
        progress: 0.0,
        can_restart: false,
    },
    pending: None,
    proto: None,
});

/// Restart is allowed outside a session, or inside one when the new build
/// speaks the same PROTO as the running extension.
fn can_restart(inner: &Inner) -> bool {
    if inner.pending.is_none() {
        return false;
    }
    let in_session = crate::SESSION.lock().unwrap().1.is_some();
    !in_session || inner.proto == Some(crate::PROTO)
}

pub fn snapshot() -> UpdateState {
    let inner = INNER.lock().unwrap();
    let mut st = inner.state.clone();
    st.can_restart = can_restart(&inner);
    st
}

/// Tell the webview and the native settings window. Progress ticks only reach
/// the webview: rebuilding the native pane on every chunk would flicker.
fn publish(app: &AppHandle, native_too: bool) {
    let _ = app.emit("update-state", snapshot());
    if native_too {
        crate::refresh_native_settings(app);
    }
}

fn set(app: &AppHandle, f: impl FnOnce(&mut Inner)) {
    {
        let mut inner = INNER.lock().unwrap();
        f(&mut inner);
    }
    publish(app, true);
}

/// The session state changed (pushed by the webview): `can_restart` may flip.
pub fn session_changed(app: &AppHandle) {
    if INNER.lock().unwrap().pending.is_some() {
        publish(app, true);
    }
}

// ---------- the automatic-check preference ----------

fn auto_file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("update_check"))
}

/// Default on: absent file or anything but an explicit off flag.
pub fn load_auto(app: &AppHandle) -> bool {
    auto_file(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|s| !matches!(s.trim(), "0" | "false"))
        .unwrap_or(true)
}

pub fn store_auto(app: &AppHandle, enabled: bool) {
    let Some(p) = auto_file(app) else { return };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(p, if enabled { "1" } else { "0" });
    if enabled {
        spawn_check(app);
    }
    let _ = app.emit("update-auto-changed", enabled);
    crate::refresh_native_settings(app);
}

// ---------- checking and downloading ----------

/// Check now (and download whatever is newer). A no-op while a check or a
/// download is already running, or once an update is ready.
pub fn spawn_check(app: &AppHandle) {
    {
        let mut inner = INNER.lock().unwrap();
        if matches!(
            inner.state.phase,
            Phase::Checking | Phase::Downloading | Phase::Ready
        ) {
            return;
        }
        inner.state.phase = Phase::Checking;
    }
    publish(app, true);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = check_and_download(&app).await {
            eprintln!("[reamesh] update check failed: {e}");
            set(&app, |i| {
                i.state.phase = Phase::Failed;
                i.state.progress = 0.0;
            });
        }
    });
}

async fn check_and_download(app: &AppHandle) -> Result<(), String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let Some(update) = update else {
        set(app, |i| {
            i.state.phase = Phase::Current;
            i.state.version.clear();
        });
        return Ok(());
    };
    let proto = update
        .raw_json
        .get("proto")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    set(app, |i| {
        i.state.phase = Phase::Downloading;
        i.state.version = update.version.clone();
        i.state.progress = 0.0;
        i.proto = proto;
    });
    let mut received: u64 = 0;
    let mut last_pct: i64 = -1;
    let bytes = update
        .download(
            |chunk, total| {
                received += chunk as u64;
                let progress = match total {
                    Some(t) if t > 0 => (received as f64 / t as f64).min(1.0),
                    _ => 0.0,
                };
                // One event per whole percent is plenty for a ring.
                let pct = (progress * 100.0) as i64;
                if pct != last_pct {
                    last_pct = pct;
                    INNER.lock().unwrap().state.progress = progress;
                    publish(app, false);
                }
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    let version = update.version.clone();
    let release = update.download_url.clone();
    set(app, |i| {
        i.state.phase = Phase::Ready;
        i.state.progress = 1.0;
        i.pending = Some((update, bytes));
    });
    // Best-effort: without it the extension simply updates with the app.
    match fetch_extension(app, &release, &version).await {
        Ok(ext) => spawn_preinstall(app, version, ext),
        Err(e) => eprintln!("[reamesh] update's extension not fetched: {e}"),
    }
    Ok(())
}

/// Download and verify the extension of the release whose app artifact lives
/// at `release`. The updater does the fetch and the signature check; the
/// comparator accepts the release unconditionally because it was already
/// chosen by the app check, and the version must match that one.
async fn fetch_extension(
    app: &AppHandle,
    release: &tauri::Url,
    version: &str,
) -> Result<Vec<u8>, String> {
    let manifest = release.join(EXTENSION_MANIFEST).map_err(|e| e.to_string())?;
    let ext = app
        .updater_builder()
        .endpoints(vec![manifest])
        .map_err(|e| e.to_string())?
        .version_comparator(|_, _| true)
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no extension in this release")?;
    if ext.version != version {
        return Err(format!("extension is {}, update is {version}", ext.version));
    }
    ext.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())
}

/// Keep trying to put the update's extension in place until it is there (or
/// nothing is to be done); this process ends at the restart anyway.
fn spawn_preinstall(app: &AppHandle, version: String, bytes: Vec<u8>) {
    let app = app.clone();
    std::thread::spawn(move || {
        while !crate::preinstall_extension(&app, &version, &bytes) {
            std::thread::sleep(PREINSTALL_RETRY);
        }
    });
}

/// Background loop: one check shortly after launch, then every few hours,
/// each only while automatic checks are on.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK_DELAY);
        loop {
            if load_auto(&app) {
                spawn_check(&app);
            }
            std::thread::sleep(CHECK_INTERVAL);
        }
    });
}

// ---------- restart ----------

/// Install the downloaded update and start the new build. The webview gets
/// `update:restarting` first so it can remember its screen and fade out; the
/// quit handler is bypassed because the session (if any) lives in REAPER and
/// must survive this restart. Windows: `install` hands over to the installer,
/// which quits this process and relaunches the app itself.
pub fn restart(app: &AppHandle) -> Result<(), String> {
    let (update, bytes) = {
        let inner = INNER.lock().unwrap();
        if !can_restart(&inner) {
            return Err("an update can't be installed right now".into());
        }
        inner.pending.clone().ok_or("no update downloaded")?
    };
    let _ = app.emit("update:restarting", ());
    std::thread::sleep(FADE_OUT);
    crate::QUIT_CONFIRMED.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Err(e) = update.install(&bytes) {
        crate::QUIT_CONFIRMED.store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = app.emit("update:restart-failed", e.to_string());
        set(app, |i| {
            i.state.phase = Phase::Failed;
            i.pending = None;
        });
        return Err(e.to_string());
    }
    app.restart();
}

// ---------- webview commands ----------

#[tauri::command]
pub fn update_state() -> UpdateState {
    snapshot()
}

#[tauri::command]
pub fn update_check(app: AppHandle) {
    spawn_check(&app);
}

#[tauri::command]
pub async fn update_restart(app: AppHandle) -> Result<(), String> {
    // Off the main thread: the fade-out wait must not block the event loop
    // that delivers `update:restarting` to the webview.
    restart(&app)
}

#[tauri::command]
pub fn get_update_auto(app: AppHandle) -> bool {
    load_auto(&app)
}

#[tauri::command]
pub fn set_update_auto(app: AppHandle, enabled: bool) {
    store_auto(&app, enabled);
}
