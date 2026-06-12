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

/// Open (or reveal) the floating presence overlay for one REAPER instance.
/// It's a frameless, transparent, always-on-top window that doesn't take focus
/// because opening it must never steal keys from REAPER mid-take. The `port` pins it
/// to the same control server the main window is in a session on.
#[tauri::command]
fn open_overlay(app: AppHandle, port: u16) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.show();
        return Ok(());
    }
    let url = format!("index.html?overlay=1&port={port}");
    let win = tauri::WebviewWindowBuilder::new(
        &app,
        "overlay",
        tauri::WebviewUrl::App(url.into()),
    )
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

/// Close the presence overlay (on leaving/stopping a session).
#[tauri::command]
fn close_overlay(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("overlay") {
        // Stop the follow timer / observers before the window goes away, in the
        // same main-thread hop so detach always precedes close.
        #[cfg(target_os = "macos")]
        {
            let _ = app.run_on_main_thread(move || {
                native_overlay::detach();
                let _ = win.close();
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = win.close();
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

/// Native confirm shown when quitting while hosting. Returns true to proceed.
fn confirm_quit_while_hosting(app: &AppHandle) -> bool {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    app.dialog()
        .message("Quitting will stop your session and disconnect everyone connected to you.")
        .title("Stop hosting and quit?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Stop hosting".into(),
            "Cancel".into(),
        ))
        .blocking_show()
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
            set_session_state
        ])
        .setup(|app| {
            // macOS menu bar with a Settings… item (Cmd+,) that the webview
            // listens for. The Edit submenu is required for clipboard
            // shortcuts to work in inputs once a custom menu is set.
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
                let settings = MenuItemBuilder::with_id("settings", "Settings…")
                    .accelerator("Cmd+,")
                    .build(app)?;
                // Custom Quit (instead of the predefined item) so it routes
                // through on_menu_event → app.exit(0) → our ExitRequested handler,
                // which stops hosting before the app dies.
                let quit = MenuItemBuilder::with_id("quit", "Quit ReaMesh")
                    .accelerator("Cmd+Q")
                    .build(app)?;
                let app_menu = SubmenuBuilder::new(app, "ReaMesh")
                    .about(None)
                    .separator()
                    .item(&settings)
                    .separator()
                    .services()
                    .separator()
                    .hide()
                    .hide_others()
                    .show_all()
                    .separator()
                    .item(&quit)
                    .build()?;
                let edit = SubmenuBuilder::new(app, "Edit")
                    .undo()
                    .redo()
                    .separator()
                    .cut()
                    .copy()
                    .paste()
                    .select_all()
                    .build()?;
                let window = SubmenuBuilder::new(app, "Window")
                    .minimize()
                    .separator()
                    .close_window()
                    .build()?;
                let menu = MenuBuilder::new(app)
                    .items(&[&app_menu, &edit, &window])
                    .build()?;
                app.set_menu(menu)?;
            }
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
                use std::sync::atomic::{AtomicBool, Ordering};
                // Set once the leave has been sent, so the second exit() pass
                // (after we stop hosting) isn't intercepted again.
                static CONFIRMED: AtomicBool = AtomicBool::new(false);
                if CONFIRMED.load(Ordering::SeqCst) {
                    return;
                }
                let (hosting, port) = *SESSION.lock().unwrap();
                // Not in a session: quit immediately (the OS tears the overlay
                // down with the process).
                let Some(port) = port else { return };
                api.prevent_exit();
                // Hosts confirm; clients leave silently.
                if hosting && !confirm_quit_while_hosting(app) {
                    return; // user cancelled; stay running
                }
                let app = app.clone();
                std::thread::spawn(move || {
                    send_leave(port);
                    // Give the extension a tick to drain the leave so peers see
                    // us go before the process exits.
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    CONFIRMED.store(true, Ordering::SeqCst);
                    app.exit(0);
                });
            }
        });
}

/// Bridge to the native AppKit settings window (src/settings_window.m). The
/// window calls back into these `extern "C"` functions to read status, run the
/// install, and persist the REAPER-folder override. `ctx` is a leaked, stable
/// `AppHandle` pointer.
#[cfg(target_os = "macos")]
mod native_settings {
    use super::{compute_status, do_install, load_custom_path, store_custom_path, ReaperStatus};
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
    }

    pub fn attach(win: &WebviewWindow) {
        if let Ok(ns) = win.ns_window() {
            unsafe { reamesh_overlay_attach(ns, super::OVERLAY_WIDTH) };
        }
    }

    pub fn detach() {
        unsafe { reamesh_overlay_detach() };
    }
}
