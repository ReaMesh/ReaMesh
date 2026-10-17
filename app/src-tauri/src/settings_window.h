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

// The in-app updater's state, mirrored from the Rust `updater::UpdateState`.
// `version` is owned by Rust and only valid for the duration of the
// get_update call: the window copies it immediately.
typedef struct {
    int32_t phase;       // kReaMeshUpdate*
    double progress;     // 0..1 while downloading (0 = size unknown)
    const char *version; // the version on offer, "" until a check found one
    int32_t can_restart; // 1 = the restart button may install it now
} UpdateStatusC;

enum {
    kReaMeshUpdateIdle = 0,
    kReaMeshUpdateChecking = 1,
    kReaMeshUpdateCurrent = 2,
    kReaMeshUpdateDownloading = 3,
    kReaMeshUpdateReady = 4,
    kReaMeshUpdateFailed = 5,
};

// Update channels shared with Rust (updater::Channel).
enum {
    kReaMeshChannelBeta = 0,
    kReaMeshChannelStable = 1,
};

// Language preference values shared with Rust (the stored *preference*, not
// the resolved language: "system" resolves through the device locale).
enum {
    kReaMeshLangSystem = 0,
    kReaMeshLangEnglish = 1,
    kReaMeshLangGerman = 2,
};

// Overlay toggle bits (a bitmask shared with Rust + overlaySettings.ts).
// Bit 2 was the retired "activity feed" toggle; it now drives the locked-item
// tint. Bit 4 (once peer playheads) is "remember HUD position". Bit 8
// (time-selection) is retired. Bit 16 is set when the HUD is switched off
// altogether, bit 32 when other people's cursors are hidden in REAPER and bit
// 64 when ours is not shared (all inverted, so existing installs keep them on).
enum {
    kReaMeshOverlayRoster = 1,
    kReaMeshOverlayTint = 2,
    kReaMeshOverlayRememberPos = 4,
    kReaMeshOverlayHidden = 16,
    kReaMeshOverlayCursorsHidden = 32,
    kReaMeshOverlayCursorPrivate = 64,
};

// Callbacks the native window uses to read state and act. `ctx` is an opaque
// Rust pointer passed back into every call.
typedef struct {
    void *ctx;
    ReaperStatusC (*get_status)(void *ctx);
    int32_t (*install)(void *ctx);            // returns 1 on success, 0 on failure
    int32_t (*remove)(void *ctx);             // returns 1 on success, 0 on failure
    void (*set_path)(void *ctx, const char *path); // path == NULL => use default
    int32_t (*get_language)(void *ctx);       // kReaMeshLang* preference
    void (*set_language)(void *ctx, int32_t pref);
    int32_t (*get_overlay)(void *ctx);        // kReaMeshOverlay* bitmask
    void (*set_overlay)(void *ctx, int32_t mask);
    UpdateStatusC (*get_update)(void *ctx);
    void (*check_update)(void *ctx);          // check now (and download)
    void (*restart_update)(void *ctx);        // install the download, relaunch
    int32_t (*get_update_auto)(void *ctx);    // 1 = check automatically
    void (*set_update_auto)(void *ctx, int32_t enabled);
    void (*show_credits)(void *ctx);          // acknowledgements, main window
    int32_t (*reset_identity)(void *ctx);     // 1 = reset, 0 = none stored, -1 = failed
    void (*copy_diagnostics)(void *ctx);      // the webview assembles + copies them
    const char *(*get_version)(void *ctx);    // static, owned by Rust
    int32_t (*get_update_channel)(void *ctx); // kReaMeshChannel*
    void (*set_update_channel)(void *ctx, int32_t channel);
    int32_t (*stable_available)(void *ctx);   // 1 = Stable can be chosen
    void (*show_joined)(void *ctx);           // joined projects, main window
    int32_t (*get_nearby)(void *ctx);         // 1 = nearby sessions over Bluetooth on
    void (*set_nearby)(void *ctx, int32_t on);
    int32_t (*get_nearby_auth)(void *ctx);    // kReaMeshBle* (ble.h)
} SettingsCallbacks;

// Opens (or re-focuses) the native settings window. Must be called on the
// main thread.
void reamesh_settings_open(SettingsCallbacks cb);

// Rebuild the open window from current state (no-op when it was never
// opened). Main thread only.
void reamesh_settings_refresh(void);

// Put plain text on the general pasteboard. Main thread only.
void reamesh_pasteboard_set(const char *text);

#endif
