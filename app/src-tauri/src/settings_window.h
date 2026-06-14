#ifndef REAMESH_SETTINGS_WINDOW_H
#define REAMESH_SETTINGS_WINDOW_H

#include <stdint.h>

// A snapshot of REAPER/extension state, mirrored from the Rust `ReaperStatus`.
// `resource_path` is owned by Rust and only valid for the duration of the
// get_status call: the window copies it immediately.
typedef struct {
    const char *resource_path;
    int32_t reaper_found;
    int32_t is_reaper_dir;
    int32_t installed;
    int32_t up_to_date;
    int32_t has_custom_path;
} ReaperStatusC;

// Language preference values shared with Rust (the stored *preference*, not
// the resolved language: "system" resolves through the device locale).
enum {
    kReaMeshLangSystem = 0,
    kReaMeshLangEnglish = 1,
    kReaMeshLangGerman = 2,
};

// Overlay toggle bits (a bitmask shared with Rust + overlaySettings.ts).
// Bit 2 was the retired "activity feed" toggle; it now drives the locked-item
// tint, so existing installs (activity default-on) keep the tint on. Bits 4/8
// (peer playheads / time-selection) are retired.
enum {
    kReaMeshOverlayRoster = 1,
    kReaMeshOverlayTint = 2,
};

// Callbacks the native window uses to read state and act. `ctx` is an opaque
// Rust pointer passed back into every call.
typedef struct {
    void *ctx;
    ReaperStatusC (*get_status)(void *ctx);
    int32_t (*install)(void *ctx);            // returns 1 on success, 0 on failure
    void (*set_path)(void *ctx, const char *path); // path == NULL => use default
    int32_t (*get_language)(void *ctx);       // kReaMeshLang* preference
    void (*set_language)(void *ctx, int32_t pref);
    int32_t (*get_overlay)(void *ctx);        // kReaMeshOverlay* bitmask
    void (*set_overlay)(void *ctx, int32_t mask);
} SettingsCallbacks;

// Opens (or re-focuses) the native settings window. Must be called on the
// main thread.
void reamesh_settings_open(SettingsCallbacks cb);

#endif
