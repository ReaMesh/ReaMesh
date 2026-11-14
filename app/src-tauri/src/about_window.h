#ifndef REAMESH_ABOUT_WINDOW_H
#define REAMESH_ABOUT_WINDOW_H

#include <stddef.h>
#include <stdint.h>

// The window's strings in the app's language, from the shared string tables.
typedef struct {
    const char *title;         // "About ReaMesh"
    const char *version;       // "Version"
    const char *tagline;
    const char *build;         // "Release build" or "Dev build"
    const char *protocol;      // "Protocol"
    const char *source;        // "Source code"
    const char *credits;       // "Acknowledgements"
    const char *notice;        // trademark notice
} AboutText;

// What the About window shows. All strings are owned by Rust and only valid
// for the duration of the open call: the window copies them immediately.
typedef struct {
    void *ctx;
    const char *version;       // "0.1.0"
    const char *commit;        // short git hash, "" when unknown
    int64_t build_time;        // unix seconds, 0 when unknown
    int32_t release;           // 1 = release build, 0 = dev build
    uint32_t proto;            // wire/ctl compatibility number
    const char *locale;        // the UI language's code, for the build date
    AboutText text;
    const uint8_t *icon_png;   // fallback icon when not running from a .app
    size_t icon_png_len;
    void (*show_credits)(void *ctx); // acknowledgements, main window
} AboutInfo;

// Opens (or rebuilds and re-focuses) the About window. Main thread only.
void reamesh_about_open(AboutInfo info);

#endif
