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
// owns height). Idempotent: calling again re-attaches to the same window.
void reamesh_overlay_attach(void *ns_window, double width);

// Stop following / observing and restore the window's original delegate.
// Called from close_overlay before the window is closed.
void reamesh_overlay_detach(void);

#endif
