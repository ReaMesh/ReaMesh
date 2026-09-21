#ifndef REAMESH_OVERLAY_PANEL_H
#define REAMESH_OVERLAY_PANEL_H

// Native macOS behaviors for the presence overlay window, attached to the
// Tauri-created NSWindow. The Rust side passes the NSWindow pointer
// (WebviewWindow::ns_window()); everything below: non-activating panel,
// show-only-when-REAPER-is-frontmost fade, follow REAPER's window, and
// drag-to-reposition: lives natively in overlay_panel.m. See lib.rs
// `mod native_overlay`.

// Turn the Tauri NSWindow into a non-activating floating panel glued to
// REAPER's window. `width` is the overlay's fixed logical width (the webview
// owns height). `reaper_pid` pins the HUD to one REAPER instance (0 = follow
// whichever is found first). Idempotent: calling again re-attaches to the
// same window.
void reamesh_overlay_attach(void *ns_window, double width, int reaper_pid);

// Stop following / observing and restore the window's original delegate.
// Called from close_overlay before the window is closed.
void reamesh_overlay_detach(void);

// Let the overlay take the keyboard (the marker name field) or give it up.
// The panel is non-activating, so this never makes ReaMesh the active app:
// it only makes the panel key, which is what a text field needs.
void reamesh_overlay_set_interactive(int on);

// Keep the dragged offset across relaunches (1) or start from the default
// anchor every session and forget any stored offset (0). Call before attach.
void reamesh_overlay_set_remember(int on);

#endif
