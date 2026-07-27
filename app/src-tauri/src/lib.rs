// The companion app's Tauri backend stays thin: all session state lives in
// the REAPER extension, which the frontend talks to directly over the
// localhost control WebSocket. The backend only does what the webview can't:
// filesystem access for installing the bundled extension into REAPER's
// UserPlugins folder, and the native menu (Settings…).
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};

use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};

#[cfg(target_os = "windows")]
mod overlay_panel_win;

#[cfg(target_os = "macos")]
const PLUGIN_NAME: &str = "reaper_reamesh.dylib";
#[cfg(target_os = "windows")]
const PLUGIN_NAME: &str = "reaper_reamesh.dll";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const PLUGIN_NAME: &str = "reaper_reamesh.so";

/// REAPER's default resource directory (where reaper.ini and UserPlugins
/// live). Portable installs keep it elsewhere: that's what the custom path
/// setting is for.
fn default_resource_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support/REAPER"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("REAPER"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/REAPER"))
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ReaperStatus {
    /// The resource directory exists on disk.
    reaper_found: bool,
    /// It also looks like a real REAPER resource dir (reaper.ini or
    /// UserPlugins inside): used to warn on a suspicious custom pick.
    is_reaper_dir: bool,
    resource_path: String,
    user_plugins: String,
    installed: bool,
    /// Installed copy is byte-identical to the bundled one.
    up_to_date: bool,
}

fn resource_dir(custom_path: &Option<String>) -> Result<PathBuf, String> {
    match custom_path {
        Some(p) if !p.is_empty() => Ok(PathBuf::from(p)),
        _ => default_resource_dir().ok_or_else(|| "could not resolve home directory".into()),
    }
}

fn bundled_plugin(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve(PLUGIN_NAME, BaseDirectory::Resource)
        .map_err(|e| e.to_string())
}

/// The custom REAPER folder override is persisted backend-side (not in the
/// webview's localStorage) so the native macOS settings window and the webview
/// share one source of truth. Absent file = use the default location.
fn custom_path_file(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("reaper_path"))
}

fn load_custom_path(app: &AppHandle) -> Option<String> {
    let p = custom_path_file(app)?;
    let s = fs::read_to_string(p).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn store_custom_path(app: &AppHandle, path: Option<String>) -> Result<(), String> {
    let p = custom_path_file(app).ok_or("could not resolve config directory")?;
    match path {
        Some(v) if !v.is_empty() => {
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&p, v).map_err(|e| e.to_string())
        }
        _ => {
            let _ = fs::remove_file(&p);
            Ok(())
        }
    }
}

fn files_equal(a: &Path, b: &Path) -> bool {
    match (fs::read(a), fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

// ---------- language ----------
//
// The app speaks English and German. The preference ("system" | "en" | "de")
// is persisted backend-side (like the REAPER folder) so the webview and the
// native macOS settings window share one source of truth; "system" resolves
// through the device locale, anything that isn't German falls back to English.

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    En,
    De,
}

fn lang_file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("language"))
}

/// The stored preference: "system" (default), "en" or "de".
fn load_lang_pref(app: &AppHandle) -> String {
    let stored = lang_file(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string());
    match stored.as_deref() {
        Some("en") => "en".into(),
        Some("de") => "de".into(),
        _ => "system".into(),
    }
}

fn store_lang_pref(app: &AppHandle, pref: &str) {
    let Some(p) = lang_file(app) else { return };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(p, pref);
}

// Overlay visibility toggles, persisted backend-side as a small bitmask so the
// native settings window AND the overlay webview share one source of truth.
// Bits: roster=1, tint=2 (must match settings_window.h's kReaMeshOverlay* and
// overlaySettings.ts). Bit 2 was the retired "activity feed" toggle, reused for
// the locked-item tint so existing installs keep it on without a migration.
// Bits 4/8 (peer playheads / time-selection) are retired and masked off.
const OVERLAY_ALL: u8 = 0b11;

fn overlay_file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("overlay"))
}

fn load_overlay(app: &AppHandle) -> u8 {
    overlay_file(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<u8>().ok())
        .map(|m| m & OVERLAY_ALL)
        .unwrap_or(OVERLAY_ALL) // default: everything on
}

fn store_overlay(app: &AppHandle, mask: u8) {
    let Some(p) = overlay_file(app) else { return };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(p, (mask & OVERLAY_ALL).to_string());
}

/// Persist the overlay toggles and tell every webview (overlay windows + the
/// in-app Settings screen) so they re-render. Shared by the webview command and
/// the native settings window callback.
fn apply_overlay(app: &AppHandle, mask: u8) {
    let mask = mask & OVERLAY_ALL;
    store_overlay(app, mask);
    use tauri::Emitter;
    let _ = app.emit("overlay-settings-changed", mask);
}

#[tauri::command]
fn get_overlay_settings(app: AppHandle) -> u8 {
    load_overlay(&app)
}

#[tauri::command]
fn set_overlay_settings(app: AppHandle, mask: u8) {
    apply_overlay(&app, mask);
}

// ---------- auto-install ----------
//
// When enabled (the default), the bundled extension is installed into REAPER's
// UserPlugins automatically: on app launch, after the REAPER folder changes,
// and the moment the toggle is switched on. Because "up to date" is a byte
// comparison against the bundled copy, this also silently *updates* the
// installed extension whenever a newer app build ships a newer dylib. Turning
// it off restores the old manual install/update-button workflow. Persisted
// backend-side (like the language/overlay prefs) so the webview Settings screen
// and the native macOS settings window share one source of truth.
fn auto_install_file(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("auto_install"))
}

/// Default on: absent file (fresh install) or anything but an explicit off flag.
fn load_auto_install(app: &AppHandle) -> bool {
    auto_install_file(app)
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|s| !matches!(s.trim(), "0" | "false"))
        .unwrap_or(true)
}

fn store_auto_install(app: &AppHandle, enabled: bool) {
    let Some(p) = auto_install_file(app) else {
        return;
    };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(p, if enabled { "1" } else { "0" });
}

/// Install/update the bundled extension when auto-install is on and the
/// installed copy is missing or stale. Best-effort and silent: onboarding and
/// the manual install button still cover the case where REAPER can't be written
/// (e.g. a folder that isn't there yet, or permissions).
fn maybe_auto_install(app: &AppHandle) {
    if !load_auto_install(app) {
        return;
    }
    let custom = load_custom_path(app);
    let Ok(st) = compute_status(app, &custom) else {
        return;
    };
    if st.reaper_found && (!st.installed || !st.up_to_date) {
        let _ = do_install(app, &custom);
    }
}

/// Persist the auto-install preference, install right away when turning it on,
/// and tell the webview(s) so the toggle and the install status re-render.
/// Shared by the webview command and the native settings window callback.
fn apply_auto_install(app: &AppHandle, enabled: bool) {
    store_auto_install(app, enabled);
    if enabled {
        maybe_auto_install(app);
    }
    use tauri::Emitter;
    let _ = app.emit("auto-install-changed", enabled);
    // Nudge the main window to re-read status (an enable may have just installed).
    let _ = app.emit("menu:settings-changed", ());
}

#[tauri::command]
fn get_auto_install(app: AppHandle) -> bool {
    load_auto_install(&app)
}

#[tauri::command]
fn set_auto_install(app: AppHandle, enabled: bool) {
    apply_auto_install(&app, enabled);
}

fn device_lang() -> Lang {
    match sys_locale::get_locale() {
        Some(l) if l.to_lowercase().starts_with("de") => Lang::De,
        _ => Lang::En,
    }
}

fn resolved_lang(app: &AppHandle) -> Lang {
    match load_lang_pref(app).as_str() {
        "en" => Lang::En,
        "de" => Lang::De,
        _ => device_lang(),
    }
}

/// Two-language string table for the native side (menu, quit dialog).
fn tr(lang: Lang, en: &'static str, de: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::De => de,
    }
}

#[tauri::command]
fn get_language(app: AppHandle) -> String {
    load_lang_pref(&app)
}

#[tauri::command]
fn set_language(app: AppHandle, pref: String) {
    apply_language(&app, &pref);
}

/// Persist the preference, rebuild the (localized) native menu, and tell every
/// webview window so the UI re-renders. Shared by the webview command and the
/// native settings window callback.
fn apply_language(app: &AppHandle, pref: &str) {
    let pref = match pref {
        "en" | "de" => pref,
        _ => "system",
    };
    store_lang_pref(app, pref);
    #[cfg(target_os = "macos")]
    {
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(e) = build_menu(&app2) {
                eprintln!("rebuilding menu failed: {e}");
            }
        });
    }
    use tauri::Emitter;
    let _ = app.emit("language-changed", load_lang_pref(app));
}

fn compute_status(app: &AppHandle, custom_path: &Option<String>) -> Result<ReaperStatus, String> {
    let dir = resource_dir(custom_path)?;
    let user_plugins = dir.join("UserPlugins");
    let dest = user_plugins.join(PLUGIN_NAME);
    let installed = dest.is_file();
    let up_to_date = installed
        && bundled_plugin(app)
            .map(|src| files_equal(&src, &dest))
            .unwrap_or(false);
    Ok(ReaperStatus {
        reaper_found: dir.is_dir(),
        is_reaper_dir: dir.join("reaper.ini").is_file() || user_plugins.is_dir(),
        resource_path: dir.to_string_lossy().into_owned(),
        user_plugins: user_plugins.to_string_lossy().into_owned(),
        installed,
        up_to_date,
    })
}

fn do_install(app: &AppHandle, custom_path: &Option<String>) -> Result<ReaperStatus, String> {
    let dir = resource_dir(custom_path)?;
    if !dir.is_dir() {
        return Err(format!("REAPER folder not found at {}", dir.display()));
    }
    let src = bundled_plugin(app)?;
    if !src.is_file() {
        return Err("the bundled extension is missing from the app resources".into());
    }
    let user_plugins = dir.join("UserPlugins");
    fs::create_dir_all(&user_plugins)
        .map_err(|e| format!("could not create {}: {e}", user_plugins.display()))?;
    // Copy via temp + rename so a running REAPER never loads a half-written file.
    let dest = user_plugins.join(PLUGIN_NAME);
    let tmp = user_plugins.join(format!(".{PLUGIN_NAME}.tmp"));
    fs::copy(&src, &tmp).map_err(|e| format!("copy failed: {e}"))?;
    displace_loaded_plugin(&user_plugins, &dest);
    fs::rename(&tmp, &dest).map_err(|e| format!("install failed: {e}"))?;
    compute_status(app, custom_path)
}

/// Windows refuses to overwrite or delete a DLL that a process has loaded, so
/// installing over a running REAPER (the normal case, since the app is used
/// alongside it) would fail with access denied. It does allow the loaded file
/// to be *renamed*, so park the old copy under a dotted name (REAPER only
/// loads `reaper_*.dll`, so it will never pick one up) and let the fresh one
/// take the real name. REAPER keeps running on the old code until it restarts,
/// exactly as on macOS. Stale copies are swept on the next install, once
/// REAPER has let go of them.
///
/// No-op on macOS: replacing a loaded dylib there is just a normal rename.
#[cfg(target_os = "windows")]
fn displace_loaded_plugin(user_plugins: &Path, dest: &Path) {
    for entry in fs::read_dir(user_plugins).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&format!(".{PLUGIN_NAME}.old-")) {
            let _ = fs::remove_file(entry.path());
        }
    }
    if !dest.exists() {
        return;
    }
    for n in 0..64 {
        let stale = user_plugins.join(format!(".{PLUGIN_NAME}.old-{n}"));
        if stale.exists() {
            continue; // still held by a running REAPER from an earlier update
        }
        let _ = fs::rename(dest, &stale);
        return;
    }
}

#[cfg(not(target_os = "windows"))]
fn displace_loaded_plugin(_user_plugins: &Path, _dest: &Path) {}

#[tauri::command]
fn reaper_status(app: AppHandle) -> Result<ReaperStatus, String> {
    compute_status(&app, &load_custom_path(&app))
}

#[tauri::command]
fn install_extension(app: AppHandle) -> Result<ReaperStatus, String> {
    do_install(&app, &load_custom_path(&app))
}

/// Locate reaper.exe on Windows. Check portable installs first, then the known
/// folder names in both Program Files directories.
#[cfg(target_os = "windows")]
fn reaper_executable(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = resource_dir(&load_custom_path(app)) {
        candidates.push(dir.join("reaper.exe"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(var) {
            let base = PathBuf::from(base);
            candidates.push(base.join("REAPER (x64)").join("reaper.exe"));
            candidates.push(base.join("REAPER").join("reaper.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Launch REAPER from the "waiting for REAPER" screen. Windows only: macOS
/// opens the bundle straight from the webview through the opener plugin.
#[tauri::command]
fn launch_reaper(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let exe = reaper_executable(&app).ok_or("could not find reaper.exe")?;
        std::process::Command::new(exe)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &app;
        Err("launch_reaper is only used on Windows".into())
    }
}

#[tauri::command]
fn get_reaper_path(app: AppHandle) -> Option<String> {
    load_custom_path(&app)
}

#[tauri::command]
fn set_reaper_path(app: AppHandle, path: Option<String>) -> Result<(), String> {
    store_custom_path(&app, path)?;
    // A freshly located REAPER folder should pick up the extension on its own
    // when auto-install is on (no-op in manual mode).
    maybe_auto_install(&app);
    Ok(())
}

/// Width of the floating presence overlay, in logical pixels. The webview
/// drives the height (it fits the window to the HUD card); width is fixed here
/// so the right edge stays put as the roster grows and shrinks.
const OVERLAY_WIDTH: f64 = 232.0;

/// Open the frameless presence HUD without stealing focus from REAPER. `port`
/// selects the same control server as the main window.
#[tauri::command]
// Keep this command async. On Windows, the window builder waits for the
// event loop to create the window; calling it on the main thread deadlocks.
async fn open_overlay(app: AppHandle, port: u16, pid: Option<u32>) -> Result<(), String> {
    ensure_hud_overlay(&app, port, pid.unwrap_or(0))
}

/// The port the currently-open HUD window was built for. The port is baked into
/// the window's URL, so re-targeting it means rebuilding it.
static OVERLAY_PORT: AtomicU16 = AtomicU16::new(0);

/// The top-right HUD window (roster + click-to-follow).
fn ensure_hud_overlay(app: &AppHandle, port: u16, pid: u32) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("overlay") {
        if OVERLAY_PORT.load(Ordering::SeqCst) == port {
            let _ = win.show();
            return Ok(());
        }
        // Different instance: the existing window is still wired to the
        // previous REAPER's control server (and, on Windows, following its
        // process). Tear it down so the build below binds to the new one.
        // Switching between two instances that are both in a session never
        // passes through a state where the frontend closes it for us.
        close_hud_overlay(app);
    }
    let url = format!("index.html?overlay=1&port={port}");
    let win = tauri::WebviewWindowBuilder::new(app, "overlay", tauri::WebviewUrl::App(url.into()))
        .title("ReaMesh overlay")
        .inner_size(OVERLAY_WIDTH, 120.0)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .shadow(false)
        .build()
        .map_err(|e| e.to_string())?;

    let _ = win.set_visible_on_all_workspaces(true);

    // Hand the window to the native panel layer: it becomes a non-activating
    // panel that follows REAPER's window and shows only when REAPER is
    // frontmost. AppKit mutation must run on the main thread; positioning is
    // now owned natively (no manual parking here).
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || native_overlay::attach(&win, pid));
    }
    // Win32 window calls are thread-safe, so this needs no main-thread hop.
    #[cfg(target_os = "windows")]
    {
        if let Ok(hwnd) = win.hwnd() {
            overlay_panel_win::attach(hwnd.0 as isize, win.scale_factor().unwrap_or(1.0), pid);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (&win, pid);
    }
    OVERLAY_PORT.store(port, Ordering::SeqCst);
    Ok(())
}

/// Close the presence overlay (on leaving/stopping a session).
#[tauri::command]
fn close_overlay(app: AppHandle) -> Result<(), String> {
    close_hud_overlay(&app);
    Ok(())
}

fn close_hud_overlay(app: &AppHandle) {
    OVERLAY_PORT.store(0, Ordering::SeqCst);
    let hud = app.get_webview_window("overlay");
    // Stop the follow timer / observers before the window goes away, in the same
    // main-thread hop so detach always precedes close.
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || {
            native_overlay::detach();
            if let Some(w) = hud {
                let _ = w.close();
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "windows")]
        overlay_panel_win::detach();
        if let Some(w) = hud {
            let _ = w.close();
        }
    }
}

// ---------- join attention ----------
//
// When someone joins the session while the user is neither in this app nor in
// REAPER (where the overlay roster already shows the join), the dock icon gets
// a badge counting the joins missed plus one polite attention bounce. The
// webview decides "window unfocused" (document.hasFocus()); the REAPER half of
// the check lives here because only the native side knows the frontmost app.

static UNSEEN_JOINS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[cfg(target_os = "macos")]
fn frontmost_is_reaper() -> bool {
    extern "C" {
        fn reamesh_frontmost_is_reaper() -> i32;
    }
    unsafe { reamesh_frontmost_is_reaper() != 0 }
}

/// Without a native frontmost check, only the webview's focus gate applies.
#[cfg(target_os = "windows")]
fn frontmost_is_reaper() -> bool {
    // Not the pinned-pid variant: this gate asks whether the user can see *a*
    // REAPER (and therefore the roster HUD floating over it), not whether it is
    // the instance the app currently has selected.
    overlay_panel_win::any_reaper_is_foreground()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn frontmost_is_reaper() -> bool {
    false
}

/// A peer joined while the main window was unfocused. Returns whether the user
/// was really away (badge set): only then does the webview queue the "who's
/// new" row flash for when they come back.
#[tauri::command]
fn notify_join(app: AppHandle) -> bool {
    use std::sync::atomic::Ordering;
    if frontmost_is_reaper() {
        return false;
    }
    let n = UNSEEN_JOINS.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_badge_count(Some(n));
        let _ = win.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
    true
}

/// The user is back (window focused, or the session ended): clear the badge.
#[tauri::command]
fn clear_join_badge(app: AppHandle) {
    use std::sync::atomic::Ordering;
    if UNSEEN_JOINS.swap(0, Ordering::SeqCst) == 0 {
        return; // Preserve badges not created by this feature.
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_badge_count(None);
    }
}

/// Whether this app is hosting (vs merely a client) and the control port it's
/// in a session on: pushed by the webview whenever the session mode changes.
/// Read by the quit handler to leave the session (and confirm, if hosting)
/// before the app exits.
static SESSION: std::sync::Mutex<(bool, Option<u16>)> = std::sync::Mutex::new((false, None));

#[tauri::command]
fn set_session_state(hosting: bool, port: Option<u16>) {
    *SESSION.lock().unwrap() = (hosting, port);
}

/// Tell the extension to leave/stop the session over its control server. A
/// no-Origin local WebSocket client is accepted by ctl.rs (it already runs as
/// the user). Best-effort, fire-and-forget.
fn send_leave(port: u16) {
    use tungstenite::Message;
    if let Ok((mut socket, _)) = tungstenite::connect(format!("ws://127.0.0.1:{port}")) {
        let _ = socket.send(Message::Text("{\"cmd\":\"leave\"}".to_string().into()));
        let _ = socket.flush();
    }
}

/// Allow the second exit pass after sending leave without another confirmation.
static QUIT_CONFIRMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Native confirm shown when quitting while hosting. Returns true to proceed.
/// MUST be called off the main thread: the dialog plugin then marshals the
/// NSAlert onto the main run loop and blocks *this* thread for the answer.
/// (Presenting it on the main thread, directly or via the run-loop ExitRequested
/// observer, throws an ObjC exception that aborts the process.)
fn confirm_quit_while_hosting(app: &AppHandle) -> bool {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let lang = resolved_lang(app);
    app.dialog()
        .message(tr(
            lang,
            "Quitting will stop your session and disconnect everyone connected to you.",
            "Beim Beenden wird deine Session gestoppt und alle Verbundenen werden getrennt.",
        ))
        .title(tr(lang, "Stop hosting and quit?", "Hosting beenden und schließen?"))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            tr(lang, "Stop hosting", "Hosting beenden").into(),
            tr(lang, "Cancel", "Abbrechen").into(),
        ))
        .blocking_show()
}

/// Build (or rebuild, after a language change) the macOS menu bar with a
/// Settings… item (Cmd+,) and a custom Quit that routes through the
/// ExitRequested handler. The Edit submenu is required for clipboard shortcuts
/// to work in inputs once a custom menu is set.
#[cfg(target_os = "macos")]
fn build_menu(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
    let lang = resolved_lang(app);
    let settings = MenuItemBuilder::with_id("settings", tr(lang, "Settings…", "Einstellungen…"))
        .accelerator("Cmd+,")
        .build(app)?;
    // Custom Quit (instead of the predefined item) so it routes through
    // on_menu_event → app.exit(0) → our ExitRequested handler, which stops
    // hosting before the app dies.
    let quit = MenuItemBuilder::with_id("quit", tr(lang, "Quit ReaMesh", "ReaMesh beenden"))
        .accelerator("Cmd+Q")
        .build(app)?;
    let app_menu = SubmenuBuilder::new(app, "ReaMesh")
        .item(&PredefinedMenuItem::about(
            app,
            Some(tr(lang, "About ReaMesh", "Über ReaMesh")),
            None,
        )?)
        .separator()
        .item(&settings)
        .separator()
        .item(&PredefinedMenuItem::services(
            app,
            Some(tr(lang, "Services", "Dienste")),
        )?)
        .separator()
        .item(&PredefinedMenuItem::hide(
            app,
            Some(tr(lang, "Hide ReaMesh", "ReaMesh ausblenden")),
        )?)
        .item(&PredefinedMenuItem::hide_others(
            app,
            Some(tr(lang, "Hide Others", "Andere ausblenden")),
        )?)
        .item(&PredefinedMenuItem::show_all(
            app,
            Some(tr(lang, "Show All", "Alle einblenden")),
        )?)
        .separator()
        .item(&quit)
        .build()?;
    let edit = SubmenuBuilder::new(app, tr(lang, "Edit", "Bearbeiten"))
        .item(&PredefinedMenuItem::undo(
            app,
            Some(tr(lang, "Undo", "Widerrufen")),
        )?)
        .item(&PredefinedMenuItem::redo(
            app,
            Some(tr(lang, "Redo", "Wiederholen")),
        )?)
        .separator()
        .item(&PredefinedMenuItem::cut(
            app,
            Some(tr(lang, "Cut", "Ausschneiden")),
        )?)
        .item(&PredefinedMenuItem::copy(
            app,
            Some(tr(lang, "Copy", "Kopieren")),
        )?)
        .item(&PredefinedMenuItem::paste(
            app,
            Some(tr(lang, "Paste", "Einsetzen")),
        )?)
        .item(&PredefinedMenuItem::select_all(
            app,
            Some(tr(lang, "Select All", "Alles auswählen")),
        )?)
        .build()?;
    let window = SubmenuBuilder::new(app, tr(lang, "Window", "Fenster"))
        .item(&PredefinedMenuItem::minimize(
            app,
            Some(tr(lang, "Minimize", "Im Dock ablegen")),
        )?)
        .separator()
        .item(&PredefinedMenuItem::close_window(
            app,
            Some(tr(lang, "Close Window", "Fenster schließen")),
        )?)
        .build()?;
    let menu = MenuBuilder::new(app)
        .items(&[&app_menu, &edit, &window])
        .build()?;
    app.set_menu(menu)?;
    Ok(())
}

/// Confirm quitting, leave the session, and allow the extension to process
/// the command before exiting. Confirmation runs on a background thread.
fn confirm_leave_then_exit(app: AppHandle, hosting: bool, port: u16) {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        if hosting && !confirm_quit_while_hosting(&app) {
            return; // user cancelled; stay running (exit already prevented)
        }
        send_leave(port);
        std::thread::sleep(std::time::Duration::from_millis(500));
        QUIT_CONFIRMED.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            reaper_status,
            install_extension,
            launch_reaper,
            open_overlay,
            close_overlay,
            get_reaper_path,
            set_reaper_path,
            get_language,
            set_language,
            get_overlay_settings,
            set_overlay_settings,
            get_auto_install,
            set_auto_install,
            set_session_state,
            notify_join,
            clear_join_badge
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            build_menu(app.handle())?;
            // Keep the bundled extension installed/updated automatically unless
            // the user opted into the manual workflow.
            maybe_auto_install(app.handle());
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == "settings" {
                // macOS opens the real AppKit settings window; other platforms
                // fall back to the in-webview Settings screen.
                #[cfg(target_os = "macos")]
                {
                    let app = app.clone();
                    let _ = app
                        .clone()
                        .run_on_main_thread(move || native_settings::open(&app));
                }
                #[cfg(not(target_os = "macos"))]
                {
                    use tauri::Emitter;
                    let _ = app.emit("menu:settings", ());
                }
            } else if event.id() == "quit" {
                // Funnel into the ExitRequested handler (stops hosting first).
                app.exit(0);
            }
        })
        .on_window_event(|window, event| {
            // Closing the main window quits the app (which closes the overlay);
            // the overlay window closing must not quit. Route through exit() so
            // the session is left/stopped first.
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                use std::sync::atomic::Ordering;
                if QUIT_CONFIRMED.load(Ordering::SeqCst) {
                    return; // The leave was sent; allow the confirmed exit.
                }
                let (hosting, port) = *SESSION.lock().unwrap();
                // Not in a session: quit immediately (the OS tears the overlay
                // down with the process).
                let Some(port) = port else { return };
                api.prevent_exit();
                // Confirm and leave on a background thread, then request exit again.
                // The dialog plugin dispatches to the main run loop; presenting
                // directly from this observer would throw an Objective-C exception.
                confirm_leave_then_exit(app.clone(), hosting, port);
            }
        });
}

/// Bridge to the native AppKit settings window (src/settings_window.m). The
/// window calls back into these `extern "C"` functions to read status, run the
/// install, and persist the REAPER-folder override. `ctx` is a leaked, stable
/// `AppHandle` pointer.
#[cfg(target_os = "macos")]
mod native_settings {
    use super::{
        apply_auto_install, apply_language, apply_overlay, compute_status, do_install,
        load_auto_install, load_custom_path, load_lang_pref, load_overlay, maybe_auto_install,
        store_custom_path, ReaperStatus,
    };
    use std::cell::RefCell;
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::sync::OnceLock;
    use tauri::{AppHandle, Emitter};

    #[repr(C)]
    struct ReaperStatusC {
        resource_path: *const c_char,
        reaper_found: i32,
        is_reaper_dir: i32,
        installed: i32,
        up_to_date: i32,
        has_custom_path: i32,
    }

    #[repr(C)]
    struct SettingsCallbacks {
        ctx: *mut c_void,
        get_status: extern "C" fn(*mut c_void) -> ReaperStatusC,
        install: extern "C" fn(*mut c_void) -> i32,
        set_path: extern "C" fn(*mut c_void, *const c_char),
        get_language: extern "C" fn(*mut c_void) -> i32,
        set_language: extern "C" fn(*mut c_void, i32),
        get_overlay: extern "C" fn(*mut c_void) -> i32,
        set_overlay: extern "C" fn(*mut c_void, i32),
        get_auto_install: extern "C" fn(*mut c_void) -> i32,
        set_auto_install: extern "C" fn(*mut c_void, i32),
    }

    extern "C" {
        fn reamesh_settings_open(cb: SettingsCallbacks);
    }

    thread_local! {
        // Keeps the resource-path C string alive for the duration of the
        // get_status call (the window copies it into an NSString immediately).
        static PATH_BUF: RefCell<Option<CString>> = const { RefCell::new(None) };
    }

    fn handle<'a>(ctx: *mut c_void) -> &'a AppHandle {
        unsafe { &*(ctx as *const AppHandle) }
    }

    extern "C" fn get_status(ctx: *mut c_void) -> ReaperStatusC {
        let app = handle(ctx);
        let custom = load_custom_path(app);
        let st = compute_status(app, &custom).unwrap_or(ReaperStatus {
            reaper_found: false,
            is_reaper_dir: false,
            resource_path: String::new(),
            user_plugins: String::new(),
            installed: false,
            up_to_date: false,
        });
        let cstr = CString::new(st.resource_path).unwrap_or_default();
        let ptr = cstr.as_ptr();
        PATH_BUF.with(|b| *b.borrow_mut() = Some(cstr));
        ReaperStatusC {
            resource_path: ptr,
            reaper_found: st.reaper_found as i32,
            is_reaper_dir: st.is_reaper_dir as i32,
            installed: st.installed as i32,
            up_to_date: st.up_to_date as i32,
            has_custom_path: custom.is_some() as i32,
        }
    }

    extern "C" fn install(ctx: *mut c_void) -> i32 {
        let app = handle(ctx);
        let ok = do_install(app, &load_custom_path(app)).is_ok();
        let _ = app.emit("menu:settings-changed", ());
        ok as i32
    }

    extern "C" fn set_path(ctx: *mut c_void, p: *const c_char) {
        let app = handle(ctx);
        let path = if p.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
        };
        let _ = store_custom_path(app, path);
        maybe_auto_install(app);
        let _ = app.emit("menu:settings-changed", ());
    }

    // Language preference, 0 = system / 1 = en / 2 = de (kReaMeshLang* in
    // settings_window.h).
    extern "C" fn get_language(ctx: *mut c_void) -> i32 {
        match load_lang_pref(handle(ctx)).as_str() {
            "en" => 1,
            "de" => 2,
            _ => 0,
        }
    }

    extern "C" fn set_language(ctx: *mut c_void, pref: i32) {
        let app = handle(ctx);
        let pref = match pref {
            1 => "en",
            2 => "de",
            _ => "system",
        };
        apply_language(app, pref);
    }

    // Overlay toggles as a bitmask (roster=1, tint=2: kReaMeshOverlay* in
    // settings_window.h).
    extern "C" fn get_overlay(ctx: *mut c_void) -> i32 {
        load_overlay(handle(ctx)) as i32
    }

    extern "C" fn set_overlay(ctx: *mut c_void, mask: i32) {
        apply_overlay(handle(ctx), mask as u8);
    }

    // Auto-install toggle (1 = keep the extension installed/updated automatically).
    extern "C" fn get_auto_install(ctx: *mut c_void) -> i32 {
        load_auto_install(handle(ctx)) as i32
    }

    extern "C" fn set_auto_install(ctx: *mut c_void, enabled: i32) {
        apply_auto_install(handle(ctx), enabled != 0);
    }

    // The AppHandle is leaked once so the pointer stays valid for the lifetime
    // of the (long-lived) native window.
    static APP_PTR: OnceLock<usize> = OnceLock::new();

    pub fn open(app: &AppHandle) {
        let ptr = *APP_PTR.get_or_init(|| Box::into_raw(Box::new(app.clone())) as usize);
        let cb = SettingsCallbacks {
            ctx: ptr as *mut c_void,
            get_status,
            install,
            set_path,
            get_language,
            set_language,
            get_overlay,
            set_overlay,
            get_auto_install,
            set_auto_install,
        };
        unsafe { reamesh_settings_open(cb) };
    }
}

/// Bridge to the native overlay panel (src/overlay_panel.m). It turns the Tauri
/// overlay window into a non-activating panel that follows REAPER's window and
/// shows only while REAPER is frontmost; all of that state lives on the ObjC
/// side. Must be called on the main thread.
#[cfg(target_os = "macos")]
mod native_overlay {
    use std::ffi::c_void;
    use tauri::WebviewWindow;

    extern "C" {
        fn reamesh_overlay_attach(ns_window: *mut c_void, width: f64, reaper_pid: i32);
        fn reamesh_overlay_detach();
    }

    /// `reaper_pid` pins the HUD to one REAPER instance (0 = follow whichever
    /// one is found first, the pre-instance-switcher behaviour).
    pub fn attach(win: &WebviewWindow, reaper_pid: u32) {
        if let Ok(ns) = win.ns_window() {
            unsafe { reamesh_overlay_attach(ns, super::OVERLAY_WIDTH, reaper_pid as i32) };
        }
    }

    pub fn detach() {
        unsafe { reamesh_overlay_detach() };
    }
}
