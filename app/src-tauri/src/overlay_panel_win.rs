//! Native Windows behaviour for the presence overlay, the counterpart to
//! `overlay_panel.m` on macOS. The Tauri window is already frameless,
//! transparent, always-on-top and taskbar-less; what has to happen natively is:
//!
//!   * roster clicks must not steal focus from REAPER;
//!   * it shows only while REAPER is the foreground application;
//!   * it follows REAPER's window, keeping the offset the user dragged it to.
//!
//! macOS gets this from AppKit (a non-activating `NSPanel`, `NSWorkspace` for
//! the frontmost app, a `CADisplayLink` to follow). Windows has no panel
//! concept, so it is assembled from `WS_EX_NOACTIVATE`, `GetForegroundWindow`
//! and a polling thread.
//!
//! Unlike AppKit, Win32 window calls are safe to make from any thread, so the
//! follow loop lives on its own thread rather than hopping to the main one.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH, RECT, TRUE};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE,
    HWND_TOPMOST, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE, SWP_NOSIZE, SW_HIDE,
    SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// How often the follow loop looks at REAPER's window. 60 Hz is wasted on a
/// window that only moves when the user drags REAPER around; ~30 Hz keeps up
/// with a drag without showing as CPU in a DAW session.
const TICK: Duration = Duration::from_millis(33);

/// Where the overlay sits by default, relative to REAPER's window: this far in
/// from the right edge, and this far down from the top: clear of REAPER's own
/// toolbar. Logical pixels, scaled by the window's DPI factor at attach time.
/// Matches the macOS defaults in overlay_panel.m.
const DEFAULT_OFFSET_X: f64 = -16.0;
const DEFAULT_OFFSET_Y: f64 = 48.0;

static OVERLAY: AtomicIsize = AtomicIsize::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);


/// Offset of the overlay's top-right corner from REAPER's, in physical pixels.
/// Rewritten whenever the user drags the overlay somewhere else.
fn offset() -> &'static Mutex<(i32, i32)> {
    static OFFSET: OnceLock<Mutex<(i32, i32)>> = OnceLock::new();
    OFFSET.get_or_init(|| Mutex::new((0, 0)))
}

/// The last position we placed the overlay at, so the loop can tell its own
/// move from one the user made by dragging the HUD.
fn last_placed() -> &'static Mutex<Option<(i32, i32)>> {
    static LAST: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn rect_of(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    Some(r)
}

fn process_name(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; MAX_PATH as usize];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0), windows::core::PWSTR(buf.as_mut_ptr()), &mut len)
    };
    let _ = unsafe { CloseHandle(handle) };
    ok.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase())
}

fn is_reaper(hwnd: HWND) -> bool {
    process_name(hwnd).is_some_and(|p| p.ends_with("\\reaper.exe") || p.ends_with("/reaper.exe"))
}

struct Search {
    best: HWND,
    best_area: i64,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let search = &mut *(lparam.0 as *mut Search);
    // Only real, on-screen top-level windows: REAPER owns a swarm of hidden
    // helper windows, and the main one is simply the biggest visible.
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { IsIconic(hwnd) }.as_bool() {
        return TRUE;
    }
    if !is_reaper(hwnd) {
        return TRUE;
    }
    if let Some(r) = rect_of(hwnd) {
        let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
        if area > search.best_area {
            search.best_area = area;
            search.best = hwnd;
        }
    }
    TRUE
}

/// REAPER's main window, or None when REAPER isn't running (or is minimised).
/// Matched on the owning process rather than a window class, so it does not
/// break if Cockos renames the class in a future version.
fn find_reaper_window() -> Option<HWND> {
    let mut search = Search {
        best: HWND::default(),
        best_area: 0,
    };
    let _ = unsafe { EnumWindows(Some(enum_proc), LPARAM(&mut search as *mut Search as isize)) };
    (!search.best.is_invalid() && search.best_area > 0).then_some(search.best)
}

fn reaper_is_foreground() -> bool {
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() {
        return false;
    }
    // Our own overlay counts as REAPER being in front: WS_EX_NOACTIVATE should
    // stop it ever becoming foreground, but if it somehow does, the HUD must
    // not blink itself out of existence.
    if fg.0 as isize == OVERLAY.load(Ordering::Relaxed) {
        return true;
    }
    is_reaper(fg)
}

/// Turn the Tauri window into a non-activating tool window and start following
/// REAPER. Idempotent: a second call just re-seeds the offset.
pub fn attach(hwnd: isize, scale: f64) {
    let overlay = HWND(hwnd as *mut _);
    unsafe {
        // NOACTIVATE: clicks on the roster never pull focus off REAPER.
        // TOOLWINDOW: keeps it out of Alt-Tab, matching the macOS panel.
        let ex = GetWindowLongPtrW(overlay, GWL_EXSTYLE);
        SetWindowLongPtrW(
            overlay,
            GWL_EXSTYLE,
            ex | (WS_EX_NOACTIVATE.0 as isize) | (WS_EX_TOOLWINDOW.0 as isize),
        );
        // Tauri creates the window visible; the HUD belongs on screen only
        // while REAPER is in front, so start hidden and let the loop decide.
        let _ = ShowWindow(overlay, SW_HIDE);
    }

    *offset().lock().unwrap() = (
        (DEFAULT_OFFSET_X * scale).round() as i32,
        (DEFAULT_OFFSET_Y * scale).round() as i32,
    );
    *last_placed().lock().unwrap() = None;
    OVERLAY.store(hwnd, Ordering::SeqCst);

    if RUNNING.swap(true, Ordering::SeqCst) {
        return; // a follow loop is already running against this static
    }
    std::thread::spawn(follow_loop);
}

/// Stop following. The window itself is closed by the caller.
pub fn detach() {
    RUNNING.store(false, Ordering::SeqCst);
    OVERLAY.store(0, Ordering::SeqCst);
}

fn follow_loop() {
    let mut shown = false;
    while RUNNING.load(Ordering::SeqCst) {
        std::thread::sleep(TICK);
        let raw = OVERLAY.load(Ordering::SeqCst);
        if raw == 0 {
            break;
        }
        let overlay = HWND(raw as *mut _);
        if !unsafe { IsWindow(Some(overlay)) }.as_bool() {
            break; // window went away underneath us
        }

        let Some(reaper) = find_reaper_window().filter(|_| reaper_is_foreground()) else {
            if shown {
                let _ = unsafe { ShowWindow(overlay, SW_HIDE) };
                shown = false;
            }
            continue;
        };
        let (Some(rr), Some(orr)) = (rect_of(reaper), rect_of(overlay)) else {
            continue;
        };

        // A position we didn't set means the user dragged the HUD: keep where
        // they put it by re-deriving the offset from REAPER's current frame.
        let placed = *last_placed().lock().unwrap();
        if placed.is_some_and(|(x, y)| (x, y) != (orr.left, orr.top)) {
            *offset().lock().unwrap() = (orr.right - rr.right, orr.top - rr.top);
        }

        let (off_x, off_y) = *offset().lock().unwrap();
        let width = orr.right - orr.left;
        let x = rr.right + off_x - width;
        let y = rr.top + off_y;

        let mut force = placed.is_none();
        if !shown {
            let _ = unsafe { ShowWindow(overlay, SW_SHOWNOACTIVATE) };
            shown = true;
            // Re-assert topmost each time REAPER comes back to the front:
            // another always-on-top window may have been raised over us while
            // we were hidden. Doing it here rather than every tick keeps it to
            // once per focus change instead of 30 times a second.
            force = true;
        }
        if force || (orr.left, orr.top) != (x, y) {
            let flags: SET_WINDOW_POS_FLAGS = SWP_NOSIZE | SWP_NOACTIVATE;
            let _ = unsafe { SetWindowPos(overlay, Some(HWND_TOPMOST), x, y, 0, 0, flags) };
        }
        *last_placed().lock().unwrap() = Some((x, y));
    }
    RUNNING.store(false, Ordering::SeqCst);
}
