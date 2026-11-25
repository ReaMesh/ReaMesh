//! ReaMesh Setup: the Windows installer and uninstaller in one program.
//!
//! With a payload appended (src/payload.rs) it installs or updates ReaMesh;
//! without one (as the installed `uninstall.exe`) it removes it. Both run in
//! a window of their own drawn like the app, from `app/setup.html`.
//!
//! The command line follows the NSIS installer it replaces, which is what the
//! app's updater speaks: `/S` silent, `/P` passive (progress only),
//! `/UPDATE` (run by the updater), `/R` start ReaMesh afterwards with the
//! arguments after `/ARGS`, `/NS` no shortcuts, `/D=<folder>` install folder.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("ReaMesh Setup is a Windows program");

mod ops;
mod payload;
mod win;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

#[derive(Default, Debug)]
struct Args {
    silent: bool,
    passive: bool,
    update: bool,
    restart: bool,
    no_shortcuts: bool,
    dir: Option<PathBuf>,
    /// The raw text after `/ARGS`, handed to the app as is.
    app_args: Option<String>,
    /// Start as the uninstaller even with a payload (testing).
    uninstall: bool,
}

fn parse_args(raw: &str) -> Args {
    let mut a = Args::default();
    let mut rest = skip_program(raw);
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let upper = rest.to_ascii_uppercase();
        // Both take the rest of the line.
        if upper.starts_with("/ARGS") && upper[5..].chars().next().is_none_or(char::is_whitespace) {
            let tail = rest[5..].trim();
            a.app_args = (!tail.is_empty()).then(|| tail.to_string());
            break;
        }
        if upper.starts_with("/D=") {
            let d = rest[3..].trim().trim_matches('"');
            if !d.is_empty() {
                a.dir = Some(PathBuf::from(d));
            }
            break;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        match &upper[..end] {
            "/S" => a.silent = true,
            "/P" => a.passive = true,
            "/UPDATE" => a.update = true,
            "/R" => a.restart = true,
            "/NS" => a.no_shortcuts = true,
            "/UNINSTALL" => a.uninstall = true,
            _ => {}
        }
        rest = &rest[end..];
    }
    a
}

/// The command line minus the program path (quoted or not).
fn skip_program(raw: &str) -> &str {
    let raw = raw.trim_start();
    if let Some(r) = raw.strip_prefix('"') {
        match r.find('"') {
            Some(i) => &r[i + 1..],
            None => "",
        }
    } else {
        raw.find(char::is_whitespace).map_or("", |i| &raw[i..])
    }
}

/// What the window is for.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Mode {
    Install,
    Uninstall,
}

struct Ctx {
    mode: Mode,
    exe: PathBuf,
    payload: Option<(payload::Located, payload::Header)>,
    args: Args,
    /// The install folder the window currently targets.
    dir: Mutex<PathBuf>,
    busy: Mutex<bool>,
    /// Webview scratch folder, removed after exit.
    scratch: PathBuf,
    /// Set by a finished uninstall: what to remove once this process is gone.
    leftover: Mutex<Option<PathBuf>>,
}

fn main() -> ExitCode {
    let args = parse_args(&win::raw_command_line());
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return ExitCode::FAILURE,
    };

    let located = match payload::locate(&exe) {
        Ok(l) => l,
        Err(e) => return fail(&args, &format!("{e}")),
    };
    let payload = match located {
        Some(at) => {
            // Only the header now; the whole blob is checked before installing.
            let header = payload::read_blob(&exe, &at).and_then(|b| payload::read_header(&b));
            match header {
                Ok(h) => Some((at, h)),
                Err(e) => return fail(&args, &e.to_string()),
            }
        }
        None => None,
    };
    let mode = if payload.is_some() && !args.uninstall { Mode::Install } else { Mode::Uninstall };

    let installed = ops::installed();
    let dir = match mode {
        Mode::Install => args
            .dir
            .clone()
            .or_else(|| installed.as_ref().map(|i| i.dir.clone()))
            .unwrap_or_else(ops::default_dir),
        // The uninstaller removes the install it sits in.
        Mode::Uninstall => exe
            .parent()
            .filter(|d| d.join(ops::MAIN_BINARY).is_file())
            .map(Path::to_path_buf)
            .or_else(|| installed.as_ref().map(|i| i.dir.clone()))
            .unwrap_or_else(ops::default_dir),
    };

    if args.silent {
        return run_silent(mode, &exe, payload.as_ref(), &args, &dir);
    }

    if win::webview2_version().is_none() {
        use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONINFORMATION, MB_YESNO};
        let ask = "ReaMesh needs the Microsoft Edge WebView2 Runtime, which this PC doesn't have yet.\n\nDownload and install it now?";
        if win::message_box(ask, MB_YESNO | MB_ICONINFORMATION) != IDYES {
            return ExitCode::FAILURE;
        }
        if let Err(e) = win::install_webview2() {
            return fail(&args, &format!("Microsoft Edge WebView2 could not be installed: {e}"));
        }
    }

    let scratch = std::env::temp_dir().join(format!("ReaMesh-Setup-{}", std::process::id()));
    let ctx = Ctx {
        mode,
        exe,
        payload,
        args,
        dir: Mutex::new(dir),
        busy: Mutex::new(false),
        scratch: scratch.clone(),
        leftover: Mutex::new(None),
    };
    run_window(ctx);
    ExitCode::SUCCESS
}

fn fail(args: &Args, msg: &str) -> ExitCode {
    if !args.silent {
        use windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR;
        win::message_box(msg, MB_ICONERROR);
    }
    ExitCode::FAILURE
}

fn run_silent(
    mode: Mode,
    exe: &Path,
    payload: Option<&(payload::Located, payload::Header)>,
    args: &Args,
    dir: &Path,
) -> ExitCode {
    let mut quiet = |_: ops::Phase, _: f64| {};
    match mode {
        Mode::Install => {
            let (at, _) = payload.expect("install mode has a payload");
            let fresh = ops::installed().is_none();
            let opts = ops::InstallOpts {
                dir: dir.to_path_buf(),
                start_menu: !args.no_shortcuts && !args.update,
                desktop: !args.no_shortcuts && !args.update && fresh,
                update: args.update,
            };
            match ops::install(exe, at, &opts, &mut quiet) {
                Ok(_) => {
                    if args.restart {
                        launch(dir, args.app_args.as_deref());
                    }
                    ExitCode::SUCCESS
                }
                Err(_) => ExitCode::FAILURE,
            }
        }
        Mode::Uninstall => {
            let opts = ops::UninstallOpts { dir: dir.to_path_buf(), remove_extension: false, remove_data: false };
            let ok = ops::uninstall(&opts, &mut quiet).is_ok();
            leave_after_uninstall(exe, dir, None);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

/// Starts the installed app, detached from Setup.
fn launch(dir: &Path, args: Option<&str>) -> bool {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(dir.join(ops::MAIN_BINARY));
    if let Some(a) = args {
        cmd.raw_arg(a);
    }
    cmd.current_dir(dir).spawn().is_ok()
}

fn leave_after_uninstall(exe: &Path, dir: &Path, scratch: Option<&Path>) {
    let me_inside = exe.parent().is_some_and(|p| win::same_path(p, dir));
    let files: Vec<PathBuf> = if me_inside { vec![exe.to_path_buf()] } else { vec![] };
    let trees: Vec<PathBuf> = scratch.map(Path::to_path_buf).into_iter().collect();
    ops::clean_up_after_exit(&files, &[dir.to_path_buf()], &trees);
}

// ---------- window ----------

fn run_window(ctx: Ctx) {
    let scratch = ctx.scratch.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ctx)
        .invoke_handler(tauri::generate_handler![
            setup_info,
            choose_dir,
            start_install,
            start_uninstall,
            launch_app,
            show_window,
            quit,
            get_language,
            set_language,
        ])
        .setup(move |app| {
            let ctx = app.state::<Ctx>();
            let title = match ctx.mode {
                Mode::Install => "ReaMesh Setup",
                Mode::Uninstall => "Uninstall ReaMesh",
            };
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("setup.html".into()))
                .title(title)
                .inner_size(440.0, 540.0)
                .resizable(false)
                .maximizable(false)
                .minimizable(true)
                .decorations(false)
                .shadow(true)
                .center()
                .visible(false)
                .background_color(tauri::window::Color(0x10, 0x10, 0x13, 0xff))
                .data_directory(scratch.clone())
                .build()?;
            // Shown by the page once it has painted; this is the fallback.
            let w = window.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(4));
                let _ = w.show();
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("ReaMesh Setup could not start");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let ctx = handle.state::<Ctx>();
            let leftover = ctx.leftover.lock().unwrap().clone();
            match leftover {
                Some(dir) => leave_after_uninstall(&ctx.exe, &dir, Some(&ctx.scratch)),
                None => ops::clean_up_after_exit(&[], &[], &[ctx.scratch.clone()]),
            }
        }
    });
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    mode: Mode,
    /// The version this Setup installs (install mode).
    version: Option<String>,
    installed_version: Option<String>,
    /// The installed copy compared with this one: "fresh", "update",
    /// "same", "downgrade".
    relation: &'static str,
    dir: String,
    default_dir: String,
    app_running: bool,
    /// `/P`: progress only, starts by itself and closes when done.
    passive: bool,
    update: bool,
    need_bytes: u64,
    free_bytes: Option<u64>,
    extension_installed: bool,
    data_exists: bool,
}

#[tauri::command]
fn setup_info(ctx: State<'_, Ctx>) -> Info {
    let dir = ctx.dir.lock().unwrap().clone();
    let installed = ops::installed();
    let version = ctx.payload.as_ref().map(|(_, h)| h.version.clone());
    let installed_version = installed.as_ref().and_then(|i| i.version.clone());
    let relation = match (&installed, &version) {
        (None, _) => "fresh",
        (Some(i), Some(v)) => match i.version.as_deref().map(|iv| ops::compare_versions(v, iv)) {
            Some(std::cmp::Ordering::Equal) => "same",
            Some(std::cmp::Ordering::Less) => "downgrade",
            _ => "update",
        },
        (Some(_), None) => "same",
    };
    Info {
        mode: ctx.mode,
        version,
        installed_version,
        relation,
        dir: dir.display().to_string(),
        default_dir: ops::default_dir().display().to_string(),
        app_running: ops::app_running(&dir),
        passive: ctx.args.passive,
        update: ctx.args.update,
        need_bytes: ctx.payload.as_ref().map_or(0, |(_, h)| h.total()),
        free_bytes: win::free_space(&dir),
        extension_installed: ctx.mode == Mode::Uninstall && ops::extension_installed(),
        data_exists: ctx.mode == Mode::Uninstall && ops::data_exists(),
    }
}

/// A folder from the picker; returns where ReaMesh would actually go.
#[tauri::command]
fn choose_dir(ctx: State<'_, Ctx>, path: String) -> Result<String, String> {
    if ctx.mode != Mode::Install || *ctx.busy.lock().unwrap() {
        return Err("busy".into());
    }
    let picked = PathBuf::from(path.trim());
    if !picked.is_absolute() {
        return Err("not a folder".into());
    }
    let dir = ops::normalize_dir(&picked);
    *ctx.dir.lock().unwrap() = dir.clone();
    Ok(dir.display().to_string())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    phase: ops::Phase,
    value: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DoneEvent {
    ok: bool,
    error: Option<String>,
    restart_reaper: bool,
}

fn claim(ctx: &Ctx) -> Result<(), String> {
    let mut busy = ctx.busy.lock().unwrap();
    if *busy {
        return Err("already running".into());
    }
    *busy = true;
    Ok(())
}

/// Throttles progress to what the eye can see (and the IPC wants to carry).
fn emitter(app: AppHandle) -> impl FnMut(ops::Phase, f64) {
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    move |phase, value| {
        let now = std::time::Instant::now();
        if value >= 1.0 || value <= 0.0 || now - last >= std::time::Duration::from_millis(30) {
            last = now;
            let _ = app.emit("setup:progress", ProgressEvent { phase, value });
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallArgs {
    desktop: bool,
}

#[tauri::command]
fn start_install(app: AppHandle, ctx: State<'_, Ctx>, args: InstallArgs) -> Result<(), String> {
    if ctx.mode != Mode::Install {
        return Err("not an installer".into());
    }
    claim(&ctx)?;
    let dir = ctx.dir.lock().unwrap().clone();
    let opts = ops::InstallOpts {
        dir,
        start_menu: !ctx.args.no_shortcuts && !ctx.args.update,
        desktop: !ctx.args.no_shortcuts && args.desktop,
        update: ctx.args.update,
    };
    std::thread::spawn(move || {
        let ctx = app.state::<Ctx>();
        let (at, _) = ctx.payload.as_ref().expect("install mode has a payload");
        let mut progress = emitter(app.clone());
        let r = ops::install(&ctx.exe, at, &opts, &mut progress);
        *ctx.busy.lock().unwrap() = false;
        let _ = app.emit(
            "setup:done",
            DoneEvent { ok: r.is_ok(), error: r.err(), restart_reaper: false },
        );
    });
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UninstallArgs {
    remove_extension: bool,
    remove_data: bool,
}

#[tauri::command]
fn start_uninstall(app: AppHandle, ctx: State<'_, Ctx>, args: UninstallArgs) -> Result<(), String> {
    if ctx.mode != Mode::Uninstall {
        return Err("not an uninstaller".into());
    }
    claim(&ctx)?;
    let dir = ctx.dir.lock().unwrap().clone();
    let opts = ops::UninstallOpts { dir: dir.clone(), remove_extension: args.remove_extension, remove_data: args.remove_data };
    std::thread::spawn(move || {
        let ctx = app.state::<Ctx>();
        let mut progress = emitter(app.clone());
        let r = ops::uninstall(&opts, &mut progress);
        *ctx.busy.lock().unwrap() = false;
        *ctx.leftover.lock().unwrap() = Some(dir);
        let (ok, error, restart_reaper) = match r {
            Ok(r) => (true, None, r.restart_reaper),
            Err(e) => (false, Some(e), false),
        };
        let _ = app.emit("setup:done", DoneEvent { ok, error, restart_reaper });
    });
    Ok(())
}

/// Opens the freshly installed app and closes Setup. For an updater run
/// (`/R`), with the arguments the app was running with.
#[tauri::command]
fn launch_app(app: AppHandle, ctx: State<'_, Ctx>) {
    let dir = ctx.dir.lock().unwrap().clone();
    let args = if ctx.args.restart { ctx.args.app_args.clone() } else { None };
    launch(&dir, args.as_deref());
    app.exit(0);
}

#[tauri::command]
fn show_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[tauri::command]
fn quit(app: AppHandle, ctx: State<'_, Ctx>) {
    if *ctx.busy.lock().unwrap() {
        return;
    }
    app.exit(0);
}

/// The app's language preference, so Setup speaks the language the user
/// picked there (an update or a reinstall); "system" otherwise.
#[tauri::command]
fn get_language() -> String {
    win::appdata()
        .and_then(|a| std::fs::read_to_string(a.join(ops::APP_ID).join("language")).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() < 16 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or_else(|| "system".into())
}

/// Setup has no language switch of its own; never writes the app's file.
#[tauri::command]
fn set_language(_pref: String) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_command_line() {
        let a = parse_args(r#""C:\Temp\x y\setup.exe" /S /UPDATE /R /ARGS "C:\dir with space" --flag"#);
        assert!(a.silent && a.update && a.restart && !a.passive);
        assert_eq!(a.app_args.as_deref(), Some(r#""C:\dir with space" --flag"#));
    }

    #[test]
    fn plain_and_dir() {
        let a = parse_args(r"setup.exe /P /NS /D=C:\Program Files\ReaMesh");
        assert!(a.passive && a.no_shortcuts && !a.silent);
        assert_eq!(a.dir, Some(PathBuf::from(r"C:\Program Files\ReaMesh")));
        let b = parse_args(r#""C:\setup.exe""#);
        assert!(!b.silent && b.app_args.is_none() && b.dir.is_none());
        let c = parse_args(r#""C:\setup.exe" /ARGS"#);
        assert!(c.app_args.is_none());
        let d = parse_args(r#""C:\setup.exe" /ARGSX /s"#);
        assert!(d.silent && d.app_args.is_none());
    }
}
