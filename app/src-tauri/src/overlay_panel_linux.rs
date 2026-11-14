//! Native Linux behaviour for the presence overlay, the counterpart to
//! `overlay_panel.m` on macOS and `overlay_panel_win.rs` on Windows:
//!
//!   * roster clicks must not steal focus from REAPER (the window is built
//!     unfocusable, see `ensure_hud_overlay`);
//!   * it shows only while REAPER is the active application;
//!   * it follows REAPER's window, keeping the offset the user dragged it to.
//!
//! All of it goes through X11: REAPER for Linux is an X11 client everywhere (on
//! a Wayland desktop it runs under XWayland), and so is this app, which asks
//! GDK for its X11 backend at startup. What REAPER's window does is read over a
//! connection of this module's own, from a polling thread as on Windows; the
//! overlay itself is moved through Tauri, which hops to the GTK thread. Without
//! an X server (a session with no XWayland) nothing here can see another
//! application's windows: the HUD then stays an ordinary window where the
//! desktop put it, and the checks answer "can't tell".

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{PhysicalPosition, WebviewWindow};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, MapState, Window,
};
use x11rb::rust_connection::RustConnection;

/// How often the follow loop looks at REAPER's window (see the Windows
/// module: enough to keep up with a drag, invisible as CPU).
const TICK: Duration = Duration::from_millis(33);

/// Where the overlay sits by default, relative to REAPER's window: this far in
/// from the right edge and this far down from the top. Logical pixels, scaled
/// at attach time. X11 reports the window without the title bar the desktop
/// draws around it, so the same numbers as on the other platforms land a
/// little lower, still clear of REAPER's toolbar.
const DEFAULT_OFFSET_X: f64 = -16.0;
const DEFAULT_OFFSET_Y: f64 = 48.0;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_ACTIVE_WINDOW,
        _NET_CLIENT_LIST,
        _NET_WM_PID,
        _NET_WM_STATE,
        _NET_WM_STATE_HIDDEN,
    }
}

/// One connection to the X server with what every query needs.
struct X {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
}

impl X {
    fn connect() -> Option<X> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let atoms = Atoms::new(&conn).ok()?.reply().ok()?;
        Some(X { conn, root, atoms })
    }

    fn window_list(&self, window: Window, property: u32, len: u32) -> Vec<u32> {
        self.conn
            .get_property(false, window, property, AtomEnum::ANY, 0, len)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }

    /// The window the desktop says has the focus (0 = none, or a window
    /// manager that doesn't say).
    fn active_window(&self) -> Window {
        self.window_list(self.root, self.atoms._NET_ACTIVE_WINDOW, 1)
            .first()
            .copied()
            .unwrap_or(0)
    }

    fn pid_of(&self, window: Window) -> u32 {
        self.window_list(window, self.atoms._NET_WM_PID, 1)
            .first()
            .copied()
            .unwrap_or(0)
    }

    /// Mapped and not minimised.
    fn on_screen(&self, window: Window) -> bool {
        let viewable = self
            .conn
            .get_window_attributes(window)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|a| a.map_state == MapState::VIEWABLE);
        viewable
            && !self
                .window_list(window, self.atoms._NET_WM_STATE, 32)
                .contains(&self.atoms._NET_WM_STATE_HIDDEN)
    }

    /// A window's frame on the root window: left, top, width, height.
    fn rect_of(&self, window: Window) -> Option<(i32, i32, i32, i32)> {
        let geo = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let at = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some((at.dst_x as i32, at.dst_y as i32, geo.width as i32, geo.height as i32))
    }

    /// The applications' top-level windows. The window manager's client list
    /// when it keeps one; otherwise (Weston's XWayland, some minimal window
    /// managers) the root's children and, under a reparenting window manager,
    /// their children: whichever of those carry a pid.
    fn client_windows(&self) -> Vec<Window> {
        let listed = self.window_list(self.root, self.atoms._NET_CLIENT_LIST, 4096);
        if !listed.is_empty() {
            return listed;
        }
        let children = |w: Window| -> Vec<Window> {
            self.conn
                .query_tree(w)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|t| t.children)
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        for top in children(self.root) {
            if self.pid_of(top) != 0 {
                out.push(top);
            } else {
                out.extend(children(top).into_iter().filter(|&w| self.pid_of(w) != 0));
            }
        }
        out
    }

    /// The main window of the REAPER with this pid (0 = any REAPER): its
    /// largest top-level window. `minimised` also counts windows that are not
    /// on screen, which is what bringing REAPER back needs.
    fn reaper_window(&self, pid: u32, minimised: bool) -> Option<Window> {
        let mut best: Option<(i64, Window)> = None;
        for window in self.client_windows() {
            if !is_reaper(self.pid_of(window), pid) {
                continue;
            }
            let shown = self.on_screen(window);
            if !shown && !minimised {
                continue;
            }
            // A window that is not on screen still competes, as a tiny one.
            let area = match self.rect_of(window) {
                Some((_, _, w, h)) if shown => (w as i64 * h as i64).max(1),
                _ => 1,
            };
            if best.is_none_or(|(a, _)| area > a) {
                best = Some((area, window));
            }
        }
        best.map(|(_, w)| w)
    }

    /// Ask the window manager to bring a window to the front.
    fn activate(&self, window: Window) -> bool {
        // Source indication 2 ("a pager"): the request comes from the user
        // clicking in another application, which a plain application request
        // (1) would be refused as focus stealing for.
        let event = ClientMessageEvent::new(32, window, self.atoms._NET_ACTIVE_WINDOW, [2, 0, 0, 0, 0]);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        self.conn.send_event(false, self.root, mask, event).is_ok() && self.conn.flush().is_ok()
    }

    /// The connection is gone (the X server went away): time for a new one.
    fn broken(&self) -> bool {
        self.conn.get_input_focus().ok().and_then(|c| c.reply().ok()).is_none()
    }
}

/// The name of a running process, as the kernel has it.
fn process_name(pid: u32) -> Option<String> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(name.trim().to_string())
}

/// Is `pid` the REAPER we mean? With a pinned pid that is a plain compare;
/// only the unpinned case reads the process name.
fn is_reaper(pid: u32, want: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if want != 0 {
        return pid == want;
    }
    process_name(pid).is_some_and(|n| n == "reaper")
}

/// Whether any REAPER is running. Read from the process table, so it also
/// answers without an X server.
pub fn reaper_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else { return false };
    entries.flatten().any(|e| {
        e.file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
            .and_then(process_name)
            .is_some_and(|n| n == "reaper")
    })
}

/// Is *any* REAPER the active application? For the join-notification gate,
/// which asks whether the user can see a REAPER at all. False when it can't be
/// told.
pub fn any_reaper_is_foreground() -> bool {
    let Some(x) = X::connect() else { return false };
    is_reaper(x.pid_of(x.active_window()), 0)
}

/// Bring a REAPER instance to the front (pid 0 = any REAPER). A minimised
/// window comes back with the activation.
pub fn activate_reaper(pid: u32) -> bool {
    let Some(x) = X::connect() else { return false };
    x.reaper_window(pid, true).is_some_and(|w| x.activate(w))
}

static RUNNING: AtomicBool = AtomicBool::new(false);
/// Counts attaches, so a follow loop notices it has been replaced.
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// The REAPER process this HUD's roster comes from (0 = any REAPER).
static REAPER_PID: AtomicU32 = AtomicU32::new(0);

/// Offset of the overlay's top-right corner from REAPER's, in physical pixels.
fn offset() -> &'static Mutex<(i32, i32)> {
    static OFFSET: OnceLock<Mutex<(i32, i32)>> = OnceLock::new();
    OFFSET.get_or_init(|| Mutex::new((0, 0)))
}

/// Where the dragged offset is persisted while "remember HUD position" is on
/// (`None` = off: every session starts from the default anchor).
fn offset_store() -> &'static Mutex<Option<std::path::PathBuf>> {
    static STORE: OnceLock<Mutex<Option<std::path::PathBuf>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(None))
}

/// Turn position memory on (with the file to keep it in) or off. Off also
/// deletes the stored offset. Call before `attach`.
pub fn set_remember(path: Option<std::path::PathBuf>, on: bool) {
    if !on {
        if let Some(p) = &path {
            let _ = std::fs::remove_file(p);
        }
    }
    *offset_store().lock().unwrap() = if on { path } else { None };
}

fn load_offset() -> Option<(i32, i32)> {
    let path = offset_store().lock().unwrap().clone()?;
    let text = std::fs::read_to_string(path).ok()?;
    let (x, y) = text.trim().split_once(',')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn save_offset(off: (i32, i32)) {
    if let Some(path) = offset_store().lock().unwrap().clone() {
        let _ = std::fs::write(path, format!("{},{}", off.0, off.1));
    }
}

/// Start following REAPER with the HUD window. Without an X server the window
/// is left where the desktop put it, visible.
pub fn attach(overlay: WebviewWindow, reaper_pid: u32) {
    REAPER_PID.store(reaper_pid, Ordering::SeqCst);
    let Some(x) = X::connect() else { return };
    let scale = overlay.scale_factor().unwrap_or(1.0);
    *offset().lock().unwrap() = load_offset().unwrap_or((
        (DEFAULT_OFFSET_X * scale).round() as i32,
        (DEFAULT_OFFSET_Y * scale).round() as i32,
    ));
    // The window is built visible; the HUD belongs on screen only while
    // REAPER is in front, so start hidden and let the loop decide.
    let _ = overlay.hide();
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    RUNNING.store(true, Ordering::SeqCst);
    std::thread::spawn(move || follow_loop(x, overlay, generation));
}

/// Stop following. The window itself is closed by the caller.
pub fn detach() {
    RUNNING.store(false, Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    REAPER_PID.store(0, Ordering::SeqCst);
}

fn follow_loop(mut x: X, overlay: WebviewWindow, generation: u32) {
    let mut shown = false;
    // The REAPER window found last, checked again before it is trusted.
    let mut cached: Window = 0;
    // The position asked for last, and where the window then turned out to
    // be: a window manager may place it elsewhere (keeping it on screen), and
    // only a position that differs from what was *seen* is the user's drag.
    let mut target: Option<(i32, i32)> = None;
    let mut seen: Option<(i32, i32)> = None;
    let mut settling = false;
    let mut failures = 0u32;
    while RUNNING.load(Ordering::SeqCst) && GENERATION.load(Ordering::SeqCst) == generation {
        std::thread::sleep(TICK);
        let want = REAPER_PID.load(Ordering::Relaxed);

        // Active-window check first: while the user is elsewhere the HUD is
        // hidden anyway and there is nothing to place.
        let active = x.active_window();
        let reaper = if is_reaper(x.pid_of(active), want) {
            if cached == 0 || !is_reaper(x.pid_of(cached), want) || !x.on_screen(cached) {
                cached = x.reaper_window(want, false).unwrap_or(0);
            }
            (cached != 0).then_some(cached)
        } else {
            None
        };
        let Some(rect) = reaper.and_then(|w| x.rect_of(w)) else {
            if shown {
                let _ = overlay.hide();
                shown = false;
            }
            // Nothing answered at all: the X server may have gone.
            if active == 0 {
                failures += 1;
                if failures > 30 && x.broken() {
                    match X::connect() {
                        Some(fresh) => x = fresh,
                        None => std::thread::sleep(Duration::from_secs(2)),
                    }
                    failures = 0;
                }
            } else {
                failures = 0;
            }
            continue;
        };
        failures = 0;
        let (Ok(pos), Ok(size)) = (overlay.outer_position(), overlay.outer_size()) else {
            break; // the window went away underneath us
        };
        let (right, top) = (rect.0 + rect.2, rect.1);
        let width = size.width as i32;
        let here = (pos.x, pos.y);

        if settling {
            settling = false;
            seen = Some(here);
        } else if shown && seen.is_some_and(|s| s != here) {
            // The user dragged the HUD: keep it where they put it.
            let dragged = (here.0 + width - right, here.1 - top);
            *offset().lock().unwrap() = dragged;
            save_offset(dragged);
            seen = Some(here);
            target = None;
        }

        let (off_x, off_y) = *offset().lock().unwrap();
        let goal = (right + off_x - width, top + off_y);
        if !shown {
            // Placed before it appears, and again below once the window
            // manager has had its say about a freshly mapped window.
            let _ = overlay.set_position(PhysicalPosition::new(goal.0, goal.1));
            let _ = overlay.show();
            let _ = overlay.set_always_on_top(true);
            shown = true;
            target = None;
            settling = true;
            continue;
        }
        if target != Some(goal) && here != goal {
            let _ = overlay.set_position(PhysicalPosition::new(goal.0, goal.1));
            settling = true;
        }
        target = Some(goal);
    }
}
