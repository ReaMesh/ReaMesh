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

// Callbacks the native window uses to read state and act. `ctx` is an opaque
// Rust pointer passed back into every call.
typedef struct {
    void *ctx;
    ReaperStatusC (*get_status)(void *ctx);
    int32_t (*install)(void *ctx);            // returns 1 on success, 0 on failure
    void (*set_path)(void *ctx, const char *path); // path == NULL => use default
} SettingsCallbacks;

// Opens (or re-focuses) the native settings window. Must be called on the
// main thread.
void reamesh_settings_open(SettingsCallbacks cb);

#endif
