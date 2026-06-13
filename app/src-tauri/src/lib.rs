// The companion app's Tauri backend stays thin: all session state lives in
// the REAPER extension, which the frontend talks to directly over the
// localhost control WebSocket. The backend only does what the webview can't:
// filesystem access for installing the bundled extension into REAPER's
// UserPlugins folder, and the native menu (Settings…).
use std::fs;
use std::path::{Path, PathBuf};

use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};

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
    let tmp = user_plugins.join(format!(".{PLUGIN_NAME}.tmp"));
    fs::copy(&src, &tmp).map_err(|e| format!("copy failed: {e}"))?;
    fs::rename(&tmp, user_plugins.join(PLUGIN_NAME)).map_err(|e| format!("install failed: {e}"))?;
    compute_status(app, custom_path)
}

#[tauri::command]
fn reaper_status(app: AppHandle) -> Result<ReaperStatus, String> {
    compute_status(&app, &load_custom_path(&app))
}

#[tauri::command]
fn install_extension(app: AppHandle) -> Result<ReaperStatus, String> {
    do_install(&app, &load_custom_path(&app))
}

#[tauri::command]
fn get_reaper_path(app: AppHandle) -> Option<String> {
    load_custom_path(&app)
}

#[tauri::command]
fn set_reaper_path(app: AppHandle, path: Option<String>) -> Result<(), String> {
    store_custom_path(&app, path)
}

/// Width of the floating presence overlay, in logical pixels. The webview
/// drives the height (it fits the window to the HUD card); width is fixed here
/// so the right edge stays put as the roster grows and shrinks.
const OVERLAY_WIDTH: f64 = 232.0;

/// Open (or reveal) the presence overlays for one REAPER instance: the
/// top-right HUD window, and a full-window, click-through "arrange" window that
/// draws peer playheads + selection ghosts over REAPER's arrange view. Both are
/// frameless, transparent, always-on-top, and never take focus: opening them
/// must never steal keys from REAPER mid-take. The `port` pins them to the same
/// control server the main window is in a session on.
#[tauri::command]
fn open_overlay(app: AppHandle, port: u16) -> Result<(), String> {
    ensure_hud_overlay(&app, port)?;
    ensure_arrange_overlay(&app, port)?;
    Ok(())
}

/// The top-right HUD window (roster + activity + click-to-follow).
fn ensure_hud_overlay(app: &AppHandle, port: u16) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.show();
        return Ok(());
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
        let _ = app.run_on_main_thread(move || native_overlay::attach(&win));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &win;
    }
    Ok(())
}

/// The full-window, click-through arrange overlay (peer playheads + ghosts).
/// The native layer sizes it to REAPER's window and shares the HUD's
/// frontmost-only visibility; the webview paints into it and lets every click
/// fall through to REAPER (`set_ignore_cursor_events`).
fn ensure_arrange_overlay(app: &AppHandle, port: u16) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("arrange") {
        let _ = win.show();
        return Ok(());
    }
    let url = format!("index.html?arrange=1&port={port}");
    let win = tauri::WebviewWindowBuilder::new(app, "arrange", tauri::WebviewUrl::App(url.into()))
        .title("ReaMesh arrange overlay")
        .inner_size(800.0, 600.0)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .shadow(false)
        .build()
        .map_err(|e| e.to_string())?;

    let _ = win.set_ignore_cursor_events(true);
    let _ = win.set_visible_on_all_workspaces(true);

    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || native_overlay::attach_arrange(&win));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &win;
    }
    Ok(())
}

/// Close both presence overlays (on leaving/stopping a session).
#[tauri::command]
fn close_overlay(app: AppHandle) -> Result<(), String> {
    let hud = app.get_webview_window("overlay");
    let arrange = app.get_webview_window("arrange");
    // Stop the follow timer / observers before the windows go away, in the same
    // main-thread hop so detach always precedes close.
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || {
            native_overlay::detach_arrange();
            native_overlay::detach();
            if let Some(w) = arrange {
                let _ = w.close();
            }
            if let Some(w) = hud {
                let _ = w.close();
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(w) = arrange {
            let _ = w.close();
        }
        if let Some(w) = hud {
            let _ = w.close();
        }
    }
    Ok(())
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
            open_overlay,
            close_overlay,
            get_reaper_path,
            set_reaper_path,
            get_language,
            set_language,
            set_session_state
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            build_menu(app.handle())?;
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
        apply_language, compute_status, do_install, load_custom_path, load_lang_pref,
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
        fn reamesh_overlay_attach(ns_window: *mut c_void, width: f64);
        fn reamesh_overlay_detach();
        fn reamesh_overlay_attach_arrange(ns_window: *mut c_void);
        fn reamesh_overlay_detach_arrange();
    }

    pub fn attach(win: &WebviewWindow) {
        if let Ok(ns) = win.ns_window() {
            unsafe { reamesh_overlay_attach(ns, super::OVERLAY_WIDTH) };
        }
    }

    pub fn detach() {
        unsafe { reamesh_overlay_detach() };
    }

    pub fn attach_arrange(win: &WebviewWindow) {
        if let Ok(ns) = win.ns_window() {
            unsafe { reamesh_overlay_attach_arrange(ns) };
        }
    }

    pub fn detach_arrange() {
        unsafe { reamesh_overlay_detach_arrange() };
    }
}
