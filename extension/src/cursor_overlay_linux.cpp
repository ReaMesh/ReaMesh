// Linux cursor layer (see cursor_overlay.h): the counterpart of
// cursor_overlay_mac.mm and cursor_overlay_win.cpp with the same style table,
// so every platform looks alike. Draws other people's cursors and an outline
// around every item someone else is editing.
//
// REAPER's UI on Linux is SWELL on GDK, always on X11 (XWayland on a Wayland
// desktop). Only top-level SWELL windows have an X window; the track view is a
// virtual child drawn into the main window's, so there is nothing to attach a
// layer to inside REAPER. The layer is therefore a window of its own: an ARGB
// override-redirect X window laid exactly over the track view's screen
// rectangle, with an empty input shape (every click and scroll goes through to
// REAPER), never focused, never in a task bar, typed as a drag icon so
// compositors leave it alone (no shadow, no effects). It needs a compositing
// manager to be transparent; without one attach() fails and the extension
// keeps its colour tint.
//
// Stacking: the window is restacked directly above REAPER's main frame
// whenever it isn't there, so REAPER's own floating windows (FX, dialogs,
// menus) and any other application above REAPER keep covering it, as on the
// other platforms. That holds on an X server, where the compositor paints in
// X stacking order. Under XWayland the Wayland compositor may keep
// override-redirect windows above everything, so there the layer also hides
// while no REAPER window has the focus, and cuts every X window above REAPER's
// frame out of its bounding shape.
//
// Nothing animates by itself here: every value is a Curve (as on Windows)
// evaluated on each frame, the scene is rendered with cairo into a client-side
// buffer, and only the rectangles that changed since the last frame are
// cleared, redrawn and sent to the window. A SWELL timer drives frames at
// display rate while something moves, the local pointer moves or the view
// settles, and stops otherwise; the extension's tick picks things up again.
// Presses on the track view come from its window procedure, subclassed
// through SWELL as on Windows.
//
// The X11, Xfixes, cairo, pango, GObject and GDK functions are looked up with
// dlopen/dlsym when the layer first attaches: they are all in REAPER's process
// already (SWELL loads GDK), and a system without one of them only loses the
// layer instead of the whole extension. SWELL's own functions come through
// SWELL_dllMain, which SWELL calls when it loads the plug-in.
//
// Debugging: REAMESH_CURSOR_DEBUG=1 logs geometry, frame pacing and stacking
// to stderr; REAMESH_CURSOR_DUMP=<file.png> writes the layer's current frame
// there at most twice a second (drawn even while the window is hidden);
// REAMESH_DEBUG_CURSOR=1 logs which glyph REAPER's cursor maps to.
//
// Everything here runs on REAPER's main thread.
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include "../WDL/swell/swell-types.h"

#include <dlfcn.h>
#include <time.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <map>
#include <memory>
#include <set>
#include <string>
#include <utility>
#include <vector>

#include "cursor_glyphs.h"
#include "cursor_overlay.h"

// Headers only: nothing below links against these libraries.
#include <cairo.h>
#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/Xfixes.h>
#include <X11/extensions/shape.h>

namespace {

constexpr double kPi = 3.14159265358979323846;

// ---------- every visual parameter, in one table (as cursor_overlay_mac.mm) ----------
//
// Lengths are in DIPs (the Mac's points); the layer multiplies them by the
// screen pixels per DIP (REAPER's UI scale times X pixels per SWELL unit).

struct Style {
  double pointerSize;
  double pointerNotch;
  double tipRadius, wingRadius, notchRadius;
  double outline;
  double morph;
  double shadowOpacity;
  double shadowRadius;
  double shadowOffsetY;
  double tagFont;
  double tagHeight;
  double tagPadX;
  double tagDX, tagDY;
  double tagIdleAfter;
  double tagFadeOut, tagFadeIn;
  double tagStiffness;
  double springStiffness;
  double predictLead, predictMax, predictStopAfter;
  double jumpCrossfade, crossfade;
  double fadeIn, fadeOut;
  double appearScale;
  double pressedScale, pressDuration;
  double idleDimAfter, idleOpacity, idleFade;
  double staleAfter;
  double forgetAfter;
  double rippleRadius, rippleLine, rippleScaleFrom, rippleScaleTo, rippleOpacity, rippleDuration;
  double edgeInset, edgeHeight, edgeFont, edgePadX, edgeArrow, edgeOpacity;
  double dragLine, dragDash, dragGap, dragFill;
  double followLine, followOpacity, followTagInset;
  double markLine, markRadius, markDim, markLinger, markFadeIn, markFadeOut;
  double markTagFont, markTagHeight, markTagPadX, markTagGap;
  double loadingVeil;
};

constexpr Style kStyle = {
    .pointerSize = 20,
    .pointerNotch = 0.8,
    .tipRadius = 1.6,
    .wingRadius = 1.8,
    .notchRadius = 2.4,
    .outline = 1.75,
    .morph = 0.26,
    .shadowOpacity = 0.22,
    .shadowRadius = 2.5,
    .shadowOffsetY = 1,
    .tagFont = 11,
    .tagHeight = 18,
    .tagPadX = 7,
    .tagDX = 12,
    .tagDY = 14,
    .tagIdleAfter = 2.0,
    .tagFadeOut = 0.6,
    .tagFadeIn = 0.15,
    .tagStiffness = 260,
    .springStiffness = 800,
    .predictLead = 0.045,
    .predictMax = 28,
    .predictStopAfter = 0.075,
    .jumpCrossfade = 360,
    .crossfade = 0.22,
    .fadeIn = 0.18,
    .fadeOut = 0.35,
    .appearScale = 0.9,
    .pressedScale = 0.86,
    .pressDuration = 0.12,
    .idleDimAfter = 10,
    .idleOpacity = 0.5,
    .idleFade = 0.8,
    .staleAfter = 8,
    .forgetAfter = 60,
    .rippleRadius = 9,
    .rippleLine = 1.5,
    .rippleScaleFrom = 0.3,
    .rippleScaleTo = 1.8,
    .rippleOpacity = 0.7,
    .rippleDuration = 0.5,
    .edgeInset = 6,
    .edgeHeight = 18,
    .edgeFont = 11,
    .edgePadX = 7,
    .edgeArrow = 7,
    .edgeOpacity = 0.92,
    .dragLine = 1.5,
    .dragDash = 5,
    .dragGap = 4,
    .dragFill = 0.16,
    .followLine = 2,
    .followOpacity = 0.9,
    .followTagInset = 8,
    .markLine = 1.5,
    .markRadius = 3,
    .markDim = 0.3,
    .markLinger = 0.6,
    .markFadeIn = 0.15,
    .markFadeOut = 0.3,
    .markTagFont = 10,
    .markTagHeight = 16,
    .markTagPadX = 6,
    .markTagGap = 3,
    .loadingVeil = 0.4,
};

// The frame timer's period. SWELL's timers run off REAPER's main loop, which
// rounds this up to its own step (about 60 frames a second here).
constexpr unsigned kFrameMs = 10;

// ---------- basics ----------

double now() {
  timespec ts{};
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return static_cast<double>(ts.tv_sec) + static_cast<double>(ts.tv_nsec) * 1e-9;
}

void trace(const char* fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  fprintf(stderr, "[reamesh-cursor] ");
  vfprintf(stderr, fmt, ap);
  fprintf(stderr, "\n");
  va_end(ap);
}

const bool g_debug = [] {
  const char* v = getenv("REAMESH_CURSOR_DEBUG");
  return v && *v && strcmp(v, "0") != 0;
}();

#define DEBUGF(...)                \
  do {                             \
    if (g_debug) trace(__VA_ARGS__); \
  } while (0)

// Screen pixels per DIP: REAPER's UI scale (arrange units per DIP) times the
// X pixels per SWELL unit.
double g_scale = 1;
double g_ui = 1;  // arrange units per DIP
double g_d = 1;   // X pixels per arrange (SWELL) unit
double S(double dip) { return dip * g_scale; }

struct Pt {
  double x = 0, y = 0;
};
struct Box {
  double x = 0, y = 0, w = 0, h = 0;
};
bool samePoint(Pt a, Pt b) { return std::fabs(a.x - b.x) < 0.01 && std::fabs(a.y - b.y) < 0.01; }
bool boxContains(const Box& r, Pt p) { return p.x >= r.x && p.x < r.x + r.w && p.y >= r.y && p.y < r.y + r.h; }

struct Rgb {
  double r = 0, g = 0, b = 0;
};
struct Rgba {
  double r = 0, g = 0, b = 0, a = 0;
  bool operator==(const Rgba&) const = default;
};

Rgb parseColor(const std::string& hex) {
  unsigned v = 0x4c8bf5;
  if (hex.size() == 7 && hex[0] == '#') {
    unsigned p = 0;
    bool ok = true;
    for (size_t i = 1; i < 7 && ok; ++i) {
      const char ch = hex[i];
      const int d = ch >= '0' && ch <= '9' ? ch - '0' : ch >= 'a' && ch <= 'f' ? ch - 'a' + 10 : ch >= 'A' && ch <= 'F' ? ch - 'A' + 10 : -1;
      ok = d >= 0;
      p = p << 4 | static_cast<unsigned>(d < 0 ? 0 : d);
    }
    if (ok) v = p;
  }
  return {((v >> 16) & 255) / 255.0, ((v >> 8) & 255) / 255.0, (v & 255) / 255.0};
}

bool isLight(const std::string& hex) {
  const Rgb c = parseColor(hex);
  return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b > 0.62;
}

Rgb inkOn(const std::string& hex) { return isLight(hex) ? Rgb{0.1, 0.1, 0.1} : Rgb{1, 1, 1}; }

Rgba rgba(Rgb c, double a = 1) { return {c.r, c.g, c.b, a}; }

// ---------- animated values ----------

// Core Animation's timing curves (cubic Bézier from 0,0 to 1,1).
struct Bezier {
  double x1, y1, x2, y2;
  static double at(double p1, double p2, double s) {
    const double m = 1 - s;
    return 3 * m * m * s * p1 + 3 * m * s * s * p2 + s * s * s;
  }
  double param(double u) const {
    double lo = 0, hi = 1;
    for (int i = 0; i < 24; ++i) {
      const double mid = (lo + hi) / 2;
      (at(x1, x2, mid) < u ? lo : hi) = mid;
    }
    return (lo + hi) / 2;
  }
  double value(double u) const { return at(y1, y2, param(u)); }
  static double d(double p1, double p2, double s) {
    const double m = 1 - s;
    return 3 * m * m * p1 + 6 * m * s * (p2 - p1) + 3 * s * s * (1 - p2);
  }
  double slope(double u) const {
    const double s = std::clamp(param(u), 1e-3, 1 - 1e-3);
    return d(y1, y2, s) / std::max(d(x1, x2, s), 1e-9);
  }
};
constexpr Bezier kEaseOut{0, 0, 0.58, 1};

// One animated scalar, evaluated here on every frame.
struct Curve {
  enum Kind { Const, Spring, Ease } kind = Const;
  double start = 0;
  double a = 0, b = 0;        // Const: a. Spring: offset a, velocity b. Ease: a -> b.
  double w = 0;               // Spring: sqrt(stiffness / mass)
  double delay = 0, dur = 0;  // Ease: holds at a for delay, then runs dur
  const Bezier* timing = nullptr;

  static Curve constant(double v) {
    Curve c;
    c.a = v;
    return c;
  }
  // A critically damped spring (mass 1) from offset x0 at velocity v0 to 0.
  static Curve spring(double t, double x0, double v0, double stiffness) {
    const double w = std::sqrt(stiffness);
    if (std::fabs(x0) < 0.05 && std::fabs(v0) < 0.5) return constant(0);
    Curve c;
    c.kind = Spring;
    c.start = t;
    c.a = x0;
    c.b = v0;
    c.w = w;
    const double k = std::fabs(v0 + w * x0);
    double T = 0;
    while (T < 2 && (std::fabs(x0) + k * T) * std::exp(-w * T) > 0.05) T += 0.01;
    c.dur = T;
    return c;
  }
  static Curve ease(double t, double from, double to, double dur, const Bezier& tm, double delay = 0) {
    if (dur <= 0) return constant(to);
    Curve c;
    c.kind = Ease;
    c.start = t;
    c.a = from;
    c.b = to;
    c.dur = dur;
    c.delay = delay;
    c.timing = &tm;
    return c;
  }
  double end() const {
    switch (kind) {
      case Spring: return start + dur;
      case Ease: return start + delay + dur;
      default: return start;
    }
  }
  bool running(double t) const { return kind != Const && t < end(); }
  double value(double t) const {
    const double s = t - start;
    switch (kind) {
      case Spring: {
        if (s <= 0) return a;
        if (s >= dur) return 0;
        return (a + (b + w * a) * s) * std::exp(-w * s);
      }
      case Ease: {
        const double u = s - delay;
        if (u <= 0) return a;
        if (u >= dur) return b;
        return a + (b - a) * timing->value(u / dur);
      }
      default: return a;
    }
  }
  double slope(double t) const {
    const double s = t - start;
    switch (kind) {
      case Spring: {
        if (s < 0 || s >= dur) return s < 0 ? b : 0;
        const double c = b + w * a;
        return (b - w * c * s) * std::exp(-w * s);
      }
      case Ease: {
        const double u = s - delay;
        if (u < 0 || u >= dur) return 0;
        return (b - a) * timing->slope(u / dur) / dur;
      }
      default: return 0;
    }
  }
};

// ---------- SWELL, through SWELL_dllMain ----------

using SwellGetFunc = void* (*)(const char* name);
SwellGetFunc g_swellGet = nullptr;

namespace sw {
HWND (*GetDlgItem)(HWND, int) = nullptr;
void (*GetClientRect)(HWND, RECT*) = nullptr;
void (*ClientToScreen)(HWND, POINT*) = nullptr;
bool (*IsWindowVisible)(HWND) = nullptr;
bool (*IsWindow)(HWND) = nullptr;
UINT_PTR (*SetTimer)(HWND, UINT_PTR, UINT, TIMERPROC) = nullptr;
BOOL (*KillTimer)(HWND, UINT_PTR) = nullptr;
void* (*SWELL_GetOSWindow)(HWND, const char*) = nullptr;
// Optional.
LONG_PTR (*GetWindowLong)(HWND, int) = nullptr;
LONG_PTR (*SetWindowLong)(HWND, int, LONG_PTR) = nullptr;
HCURSOR (*SWELL_GetCursor)() = nullptr;
int (*SWELL_GetScaling256)() = nullptr;
WORD (*GetAsyncKeyState)(int) = nullptr;
}  // namespace sw

bool g_swellLoaded = false;

bool loadSwell() {
  if (g_swellLoaded) return true;
  if (!g_swellGet) return false;
  bool ok = true;
  const auto need = [&ok](auto& fn, const char* name) {
    fn = reinterpret_cast<std::remove_reference_t<decltype(fn)>>(g_swellGet(name));
    if (!fn) {
      trace("SWELL has no %s", name);
      ok = false;
    }
  };
  const auto want = [](auto& fn, const char* name) {
    fn = reinterpret_cast<std::remove_reference_t<decltype(fn)>>(g_swellGet(name));
  };
  need(sw::GetDlgItem, "GetDlgItem");
  need(sw::GetClientRect, "GetClientRect");
  need(sw::ClientToScreen, "ClientToScreen");
  need(sw::IsWindowVisible, "IsWindowVisible");
  need(sw::IsWindow, "IsWindow");
  need(sw::SetTimer, "SetTimer");
  need(sw::KillTimer, "KillTimer");
  need(sw::SWELL_GetOSWindow, "SWELL_GetOSWindow");
  want(sw::GetWindowLong, "GetWindowLong");
  want(sw::SetWindowLong, "SetWindowLong");
  want(sw::SWELL_GetCursor, "SWELL_GetCursor");
  want(sw::SWELL_GetScaling256, "SWELL_GetScaling256");
  want(sw::GetAsyncKeyState, "GetAsyncKeyState");
  g_swellLoaded = ok;
  return ok;
}

// ---------- X11, Xfixes, cairo, pango, GObject, GDK: looked up at run time ----------

// The few GLib/pango/GDK types used here, declared by hand so no GLib header
// is needed (their ABI has been stable for over a decade).
struct GValueLike {
  unsigned long g_type = 0;
  union {
    long long i;
    double d;
    void* p;
  } data[2] = {};
};
constexpr unsigned long kGTypeBoolean = 5 << 2;  // G_TYPE_BOOLEAN
constexpr unsigned long kGTypeString = 16 << 2;  // G_TYPE_STRING
struct PLayout;
struct PContext;
struct PFontDesc;
struct PRect {
  int x, y, width, height;
};
constexpr int kPangoScale = 1024;
constexpr int kPangoWeightSemibold = 600;

#define REAMESH_X11_FUNCS(F)                                                                                   \
  F(XCreateWindow) F(XDestroyWindow) F(XMapWindow) F(XUnmapWindow) F(XMoveResizeWindow) F(XConfigureWindow)    \
  F(XQueryTree) F(XFree) F(XGetWindowAttributes) F(XQueryPointer) F(XGetSelectionOwner) F(XInternAtom)          \
  F(XChangeProperty) F(XCreateColormap) F(XFreeColormap) F(XMatchVisualInfo) F(XCreateGC) F(XFreeGC)           \
  F(XPutImage) F(XCreateImage) F(XFlush) F(XGetInputFocus) F(XGetWindowProperty) F(XQueryExtension)            \
  F(XScreenNumberOfScreen)
#define REAMESH_XFIXES_FUNCS(F)                                                                   \
  F(XFixesQueryExtension) F(XFixesCreateRegion) F(XFixesDestroyRegion) F(XFixesSetWindowShapeRegion) \
  F(XFixesSubtractRegion) F(XFixesGetCursorImage)
#define REAMESH_CAIRO_FUNCS(F)                                                                                     \
  F(cairo_image_surface_create) F(cairo_image_surface_get_data) F(cairo_image_surface_get_stride)                \
  F(cairo_surface_destroy) F(cairo_surface_flush) F(cairo_surface_mark_dirty) F(cairo_surface_status)              \
  F(cairo_surface_write_to_png) F(cairo_create) F(cairo_destroy) F(cairo_save) F(cairo_restore) F(cairo_translate) \
  F(cairo_scale) F(cairo_set_matrix) F(cairo_get_matrix) F(cairo_identity_matrix) F(cairo_set_source_rgba)         \
  F(cairo_set_source_surface) F(cairo_set_operator) F(cairo_paint) F(cairo_paint_with_alpha) F(cairo_mask_surface) \
  F(cairo_rectangle) F(cairo_clip) F(cairo_new_path) F(cairo_move_to) F(cairo_line_to) F(cairo_arc)               \
  F(cairo_close_path) F(cairo_new_sub_path) F(cairo_fill) F(cairo_fill_preserve) F(cairo_stroke)                  \
  F(cairo_set_line_width)                                                                                          \
  F(cairo_set_line_join) F(cairo_set_dash) F(cairo_pattern_set_filter) F(cairo_get_source) F(cairo_push_group)    \
  F(cairo_pop_group_to_source) F(cairo_font_options_create) F(cairo_font_options_set_antialias)                   \
  F(cairo_font_options_set_hint_style) F(cairo_font_options_set_hint_metrics)

struct Api {
#define REAMESH_DECL(f) decltype(&::f) f = nullptr;
  REAMESH_X11_FUNCS(REAMESH_DECL)
  REAMESH_XFIXES_FUNCS(REAMESH_DECL)
  REAMESH_CAIRO_FUNCS(REAMESH_DECL)
#undef REAMESH_DECL
  // pango / pangocairo
  PLayout* (*pango_cairo_create_layout)(cairo_t*) = nullptr;
  void (*pango_cairo_update_layout)(cairo_t*, PLayout*) = nullptr;
  void (*pango_cairo_show_layout)(cairo_t*, PLayout*) = nullptr;
  void (*pango_cairo_context_set_font_options)(PContext*, const cairo_font_options_t*) = nullptr;
  PContext* (*pango_layout_get_context)(PLayout*) = nullptr;
  void (*pango_layout_context_changed)(PLayout*) = nullptr;
  void (*pango_layout_set_text)(PLayout*, const char*, int) = nullptr;
  void (*pango_layout_set_font_description)(PLayout*, const PFontDesc*) = nullptr;
  void (*pango_layout_get_extents)(PLayout*, PRect*, PRect*) = nullptr;
  int (*pango_layout_get_baseline)(PLayout*) = nullptr;
  PFontDesc* (*pango_font_description_from_string)(const char*) = nullptr;
  void (*pango_font_description_free)(PFontDesc*) = nullptr;
  void (*pango_font_description_set_weight)(PFontDesc*, int) = nullptr;
  void (*pango_font_description_set_absolute_size)(PFontDesc*, double) = nullptr;
  // GObject
  void (*g_object_unref)(void*) = nullptr;
  int (*g_type_check_instance_is_a)(void*, unsigned long) = nullptr;
  GValueLike* (*g_value_init)(GValueLike*, unsigned long) = nullptr;
  void (*g_value_unset)(GValueLike*) = nullptr;
  int (*g_value_get_boolean)(const GValueLike*) = nullptr;
  const char* (*g_value_get_string)(const GValueLike*) = nullptr;
  // GDK (X11 backend)
  void* (*gdk_display_get_default)() = nullptr;
  void* (*gdk_screen_get_default)() = nullptr;
  int (*gdk_screen_get_setting)(void*, const char*, GValueLike*) = nullptr;
  unsigned long (*gdk_x11_display_get_type)() = nullptr;
  Display* (*gdk_x11_display_get_xdisplay)(void*) = nullptr;
  Window (*gdk_x11_window_get_xid)(void*) = nullptr;
  void (*gdk_x11_display_error_trap_push)(void*) = nullptr;
  int (*gdk_x11_display_error_trap_pop)(void*) = nullptr;
  void (*gdk_x11_display_error_trap_pop_ignored)(void*) = nullptr;
};
Api A;
bool g_apiTried = false, g_apiOk = false;

bool loadApi() {
  if (g_apiTried) return g_apiOk;
  g_apiTried = true;
  // Already in the process (SWELL loaded GDK, which brings the rest); dlopen
  // only takes a reference. Never closed: GDK keeps using them.
  void* x11 = dlopen("libX11.so.6", RTLD_NOW | RTLD_LOCAL);
  void* xfixes = dlopen("libXfixes.so.3", RTLD_NOW | RTLD_LOCAL);
  void* cairo = dlopen("libcairo.so.2", RTLD_NOW | RTLD_LOCAL);
  void* pango = dlopen("libpango-1.0.so.0", RTLD_NOW | RTLD_LOCAL);
  void* pangocairo = dlopen("libpangocairo-1.0.so.0", RTLD_NOW | RTLD_LOCAL);
  void* gobject = dlopen("libgobject-2.0.so.0", RTLD_NOW | RTLD_LOCAL);
  void* gdk = dlopen("libgdk-3.so.0", RTLD_NOW | RTLD_LOCAL);
  const struct {
    void* h;
    const char* name;
  } libs[] = {{x11, "libX11.so.6"},         {xfixes, "libXfixes.so.3"},
              {cairo, "libcairo.so.2"},     {pango, "libpango-1.0.so.0"},
              {pangocairo, "libpangocairo-1.0.so.0"}, {gobject, "libgobject-2.0.so.0"},
              {gdk, "libgdk-3.so.0"}};
  for (const auto& l : libs)
    if (!l.h) {
      trace("cannot load %s", l.name);
      return false;
    }
  bool ok = true;
  const auto get = [&ok](void* h, auto& fn, const char* name) {
    fn = reinterpret_cast<std::remove_reference_t<decltype(fn)>>(dlsym(h, name));
    if (!fn) {
      trace("missing %s", name);
      ok = false;
    }
  };
#define REAMESH_LOAD(lib) [&](auto& fn, const char* name) { get(lib, fn, name); }
  {
    auto f = REAMESH_LOAD(x11);
#define REAMESH_GET(fn) f(A.fn, #fn);
    REAMESH_X11_FUNCS(REAMESH_GET)
  }
  {
    auto f = REAMESH_LOAD(xfixes);
    REAMESH_XFIXES_FUNCS(REAMESH_GET)
  }
  {
    auto f = REAMESH_LOAD(cairo);
    REAMESH_CAIRO_FUNCS(REAMESH_GET)
  }
  {
    auto f = REAMESH_LOAD(pangocairo);
    REAMESH_GET(pango_cairo_create_layout)
    REAMESH_GET(pango_cairo_update_layout)
    REAMESH_GET(pango_cairo_show_layout)
    REAMESH_GET(pango_cairo_context_set_font_options)
  }
  {
    auto f = REAMESH_LOAD(pango);
    REAMESH_GET(pango_layout_get_context)
    REAMESH_GET(pango_layout_context_changed)
    REAMESH_GET(pango_layout_set_text)
    REAMESH_GET(pango_layout_set_font_description)
    REAMESH_GET(pango_layout_get_extents)
    REAMESH_GET(pango_layout_get_baseline)
    REAMESH_GET(pango_font_description_from_string)
    REAMESH_GET(pango_font_description_free)
    REAMESH_GET(pango_font_description_set_weight)
    REAMESH_GET(pango_font_description_set_absolute_size)
  }
  {
    auto f = REAMESH_LOAD(gobject);
    REAMESH_GET(g_object_unref)
    REAMESH_GET(g_type_check_instance_is_a)
    REAMESH_GET(g_value_init)
    REAMESH_GET(g_value_unset)
    REAMESH_GET(g_value_get_boolean)
    REAMESH_GET(g_value_get_string)
  }
  {
    auto f = REAMESH_LOAD(gdk);
    REAMESH_GET(gdk_display_get_default)
    REAMESH_GET(gdk_screen_get_default)
    REAMESH_GET(gdk_screen_get_setting)
    REAMESH_GET(gdk_x11_display_get_type)
    REAMESH_GET(gdk_x11_display_get_xdisplay)
    REAMESH_GET(gdk_x11_window_get_xid)
    REAMESH_GET(gdk_x11_display_error_trap_push)
    REAMESH_GET(gdk_x11_display_error_trap_pop)
    REAMESH_GET(gdk_x11_display_error_trap_pop_ignored)
  }
#undef REAMESH_GET
#undef REAMESH_LOAD
  g_apiOk = ok;
  return ok;
}

// ---------- the X window ----------

void* g_gdpy = nullptr;  // GdkDisplay*
Display* g_dpy = nullptr;
Window g_rootW = 0, g_mainX = 0, g_frame = 0, g_win = 0;
Colormap g_cmap = 0;
GC g_gc = nullptr;
Visual* g_visual = nullptr;
bool g_xwayland = false;
bool g_mapped = false;
bool g_mainViewable = false;
bool g_active = true;  // a REAPER window has the focus (only tracked under XWayland)
int g_winX = 0, g_winY = 0;
int g_viewW = 0, g_viewH = 0;  // the layer's size, X pixels
double g_frameCheckedAt = -1;
std::vector<XRectangle> g_cuts;  // what the bounding shape leaves out
bool g_cutsSet = false;
double g_cutsAt = -1;
bool g_ewmhHidden = false;  // minimised or on another workspace, yet mapped
double g_ewmhCheckedAt = -1;
double g_cmCheckedAt = 0;

// Every X request goes through GDK's connection inside one of GDK's error
// traps: an X error outside a trap ends REAPER.
struct Trap {
  bool popped = false;
  Trap() { A.gdk_x11_display_error_trap_push(g_gdpy); }
  int check() {
    popped = true;
    return A.gdk_x11_display_error_trap_pop(g_gdpy);
  }
  ~Trap() {
    if (!popped) A.gdk_x11_display_error_trap_pop_ignored(g_gdpy);
  }
  Trap(const Trap&) = delete;
  Trap& operator=(const Trap&) = delete;
};

Atom atom(const char* name) { return A.XInternAtom(g_dpy, name, False); }

// The top-level ancestor of an X window (the window manager's frame, if any).
Window frameOf(Window w) {
  for (int i = 0; i < 16 && w; ++i) {
    Window root = 0, parent = 0, *kids = nullptr;
    unsigned n = 0;
    if (!A.XQueryTree(g_dpy, w, &root, &parent, &kids, &n)) return 0;
    if (kids) A.XFree(kids);
    if (parent == root || !parent) return w;
    w = parent;
  }
  return 0;
}

// Whether a compositing manager runs on this screen: without one an ARGB
// window shows black where it should be clear.
bool composited(int screen) {
  char name[32];
  snprintf(name, sizeof name, "_NET_WM_CM_S%d", screen);
  return A.XGetSelectionOwner(g_dpy, atom(name)) != None;
}

// Whether the X input focus is in one of REAPER's windows (a toplevel of this
// process, found through _NET_WM_PID up the focus window's ancestry).
bool reaperFocused() {
  Window focus = 0;
  int revert = 0;
  A.XGetInputFocus(g_dpy, &focus, &revert);
  if (focus == None || focus == PointerRoot) return false;
  const Atom pid = atom("_NET_WM_PID");
  Window w = focus;
  for (int i = 0; i < 8 && w && w != g_rootW; ++i) {
    Atom type = 0;
    int format = 0;
    unsigned long n = 0, after = 0;
    unsigned char* data = nullptr;
    if (A.XGetWindowProperty(g_dpy, w, pid, 0, 1, False, XA_CARDINAL, &type, &format, &n, &after, &data) == Success &&
        data) {
      const bool have = n == 1 && format == 32;
      const long v = have ? *reinterpret_cast<long*>(data) : 0;
      A.XFree(data);
      if (have) return v == static_cast<long>(getpid());
    }
    Window root = 0, parent = 0, *kids = nullptr;
    unsigned nk = 0;
    if (!A.XQueryTree(g_dpy, w, &root, &parent, &kids, &nk)) return false;
    if (kids) A.XFree(kids);
    w = parent;
  }
  return false;
}

// ---------- the scene: nodes with their content and animations ----------
//
// A node carries the model offset (pixels), the transforms (scale about a
// centre, an optional rotation, the spring offset) and the opacity, plus
// either a drawn image (anchored so the content's anchor sits at the node's
// origin) or a box drawn straight into the frame. Children draw above the
// content, in order.

struct BoxSpec {
  double w = 0, h = 0;  // pixels
  Rgba fill, stroke;
  double line = 0;      // DIP
  bool dash = false, rounded = true;
};

struct SNode {
  int id = 0;
  SNode* parent = nullptr;
  std::vector<std::unique_ptr<SNode>> kids;
  Curve dx = Curve::constant(0), dy = Curve::constant(0);
  Curve sx = Curve::constant(1), sy = Curve::constant(1), op = Curve::constant(1);
  double ox = 0, oy = 0;  // model offset, px
  bool placed = false;    // the offset was set at least once
  double cx = 0, cy = 0, angle = 0;  // scale centre, rotation (degrees)
  double w = 0;           // drawn width, DIP (pills)
  std::string look;       // what the content shows; skips identical redraws
  cairo_surface_t* img = nullptr;
  int imgW = 0, imgH = 0;
  double imgX = 0, imgY = 0;  // where the image's top-left sits, px from the origin
  bool hasBox = false;
  BoxSpec box;
  unsigned ver = 0;        // bumps whenever the content changes
  double doomAt = -1;      // removed once this time has passed
  SNode() = default;
  SNode(const SNode&) = delete;
  SNode& operator=(const SNode&) = delete;
  ~SNode() {
    if (img) A.cairo_surface_destroy(img);
  }
};
using Node = SNode*;

int g_nextNodeId = 1;
unsigned g_nextVer = 1;
std::unique_ptr<SNode> g_scene;  // the root: positioned at the arrange's top-left
Node g_markN = nullptr;          // item marks, below everything else

enum class Z { Top, Bottom, Beneath };

Node makeNode(Node parent, Z z = Z::Top, Node ref = nullptr) {
  if (!parent) return nullptr;
  auto n = std::make_unique<SNode>();
  n->id = g_nextNodeId++;
  n->parent = parent;
  Node raw = n.get();
  auto& k = parent->kids;
  auto at = k.end();
  if (z == Z::Bottom) at = k.begin();
  if (z == Z::Beneath && ref)
    at = std::find_if(k.begin(), k.end(), [ref](const std::unique_ptr<SNode>& c) { return c.get() == ref; });
  k.insert(at, std::move(n));
  return raw;
}

void removeNode(Node n) {
  if (!n || !n->parent) return;
  auto& k = n->parent->kids;
  auto it = std::find_if(k.begin(), k.end(), [n](const std::unique_ptr<SNode>& c) { return c.get() == n; });
  if (it != k.end()) k.erase(it);
}

void dropNode(Node& n) {
  removeNode(n);
  n = nullptr;
}

// Removed once its last animation has played.
void dropLater(Node& n, double at) {
  if (n) n->doomAt = at;
  n = nullptr;
}

void sweepDoomed(SNode* n, double t) {
  auto& k = n->kids;
  k.erase(std::remove_if(k.begin(), k.end(),
                         [t](const std::unique_ptr<SNode>& c) { return c->doomAt >= 0 && t >= c->doomAt; }),
          k.end());
  for (auto& c : k) sweepDoomed(c.get(), t);
}

// Moves a node above all its siblings.
void raise(Node n) {
  if (!n || !n->parent) return;
  auto& k = n->parent->kids;
  auto it = std::find_if(k.begin(), k.end(), [n](const std::unique_ptr<SNode>& c) { return c.get() == n; });
  if (it == k.end() || it + 1 == k.end()) return;
  std::unique_ptr<SNode> own = std::move(*it);
  k.erase(it);
  k.push_back(std::move(own));
}

void setOpacityNow(Node n, double v) {
  if (n) n->op = Curve::constant(v);
}

// The model offset jumps; running springs keep riding on top.
void setPos(Node n, Pt p) {
  if (!n || (n->placed && n->ox == p.x && n->oy == p.y)) return;
  n->placed = true;
  n->ox = p.x;
  n->oy = p.y;
}

// Additive spring from `from` to `to`: the model jumps, the offset (plus
// whatever was still moving) decays.
void springTo(Node n, Pt from, Pt to, double stiffness) {
  if (!n) return;
  const double t = now();
  n->dx = Curve::spring(t, n->dx.value(t) + from.x - to.x, n->dx.slope(t), stiffness);
  n->dy = Curve::spring(t, n->dy.value(t) + from.y - to.y, n->dy.slope(t), stiffness);
  setPos(n, to);
}

// Drops the position springs (fades and scaling stay).
void snapTo(Node n, Pt p) {
  if (!n) return;
  n->dx = n->dy = Curve::constant(0);
  setPos(n, p);
}

// Where a node shows right now, springs included.
Pt shownAt(const Node n, double t) { return {n->ox + n->dx.value(t), n->oy + n->dy.value(t)}; }

constexpr double kFromNow = -1;  // "start from the current value"

void fade(Node n, double to, double dur, double from = kFromNow) {
  if (!n) return;
  const double t = now();
  n->op = Curve::ease(t, from == kFromNow ? n->op.value(t) : from, to, dur, kEaseOut);
}

// A uniform scale about the node's scale centre.
void scaleTo(Node n, double to, double dur, double from = kFromNow) {
  if (!n) return;
  const double t = now();
  n->sx = Curve::ease(t, from == kFromNow ? n->sx.value(t) : from, to, dur, kEaseOut);
  n->sy = n->sx;
}

void setScaleCentre(Node n, double x, double y) {
  if (!n) return;
  n->cx = x;
  n->cy = y;
}

// ---------- drawing ----------

std::map<int, PFontDesc*> g_fonts;  // by size * 10
std::string g_fontName = "Sans";    // the desktop's UI font, from GTK's settings
double g_capPerEm = 0.7;            // the UI font's cap height / em
cairo_surface_t* g_measureSurf = nullptr;
cairo_t* g_measureCr = nullptr;
cairo_font_options_t* g_fontOptions = nullptr;

PFontDesc* fontOf(double size) {
  const int key = static_cast<int>(std::lround(size * 10));
  auto it = g_fonts.find(key);
  if (it != g_fonts.end()) return it->second;
  PFontDesc* f = A.pango_font_description_from_string(g_fontName.c_str());
  if (!f) return nullptr;
  A.pango_font_description_set_weight(f, kPangoWeightSemibold);
  A.pango_font_description_set_absolute_size(f, size * kPangoScale);
  g_fonts[key] = f;
  return f;
}

// A layout of `text` in the UI font for drawing with `cr` (whose transform
// sets the hinting); the caller unrefs it.
PLayout* layoutOf(cairo_t* cr, const std::string& text, double size) {
  PFontDesc* f = fontOf(size);
  PLayout* l = f ? A.pango_cairo_create_layout(cr) : nullptr;
  if (!l) return nullptr;
  if (g_fontOptions) {
    A.pango_cairo_context_set_font_options(A.pango_layout_get_context(l), g_fontOptions);
    A.pango_layout_context_changed(l);
  }
  A.pango_layout_set_font_description(l, f);
  A.pango_layout_set_text(l, text.c_str(), static_cast<int>(text.size()));
  return l;
}

// The UI font from GTK's settings ("Cantarell 11", "Ubuntu 11"…): the family
// only, size and weight are ours.
void readUiFont() {
  if (!A.gdk_screen_get_default()) return;
  GValueLike v;
  A.g_value_init(&v, kGTypeString);
  if (A.gdk_screen_get_setting(A.gdk_screen_get_default(), "gtk-font-name", &v)) {
    const char* s = A.g_value_get_string(&v);
    if (s && *s) {
      // Drop a trailing size ("Cantarell 11" -> "Cantarell").
      std::string name = s;
      const size_t sp = name.find_last_of(' ');
      if (sp != std::string::npos && name.find_first_not_of("0123456789.", sp + 1) == std::string::npos)
        name.resize(sp);
      if (!name.empty()) g_fontName = name;
    }
  }
  A.g_value_unset(&v);
}

void readCapHeight() {
  PLayout* l = g_measureCr ? layoutOf(g_measureCr, "H", 100) : nullptr;
  if (!l) return;
  PRect ink{}, logical{};
  A.pango_layout_get_extents(l, &ink, &logical);
  A.g_object_unref(l);
  const double cap = static_cast<double>(ink.height) / kPangoScale / 100;
  if (cap > 0.4 && cap < 1) g_capPerEm = cap;
}

std::string scaleKey() {
  char b[32];
  snprintf(b, sizeof b, "@%.3f", g_scale);
  return b;
}

// Starts drawing a node's content: w x h DIP with `margin` DIP free on every
// side, the point (ax, ay) of the content at the node's origin. The context
// draws in DIPs from the content's top-left; nullptr when it can't.
cairo_t* beginDraw(Node n, double w, double h, double margin, double ax = 0, double ay = 0) {
  if (!n) return nullptr;
  const int m = static_cast<int>(std::ceil(margin * g_scale));
  const int needW = std::clamp(static_cast<int>(std::ceil(w * g_scale)) + 2 * m, 1, 8192);
  const int needH = std::clamp(static_cast<int>(std::ceil(h * g_scale)) + 2 * m, 1, 8192);
  if (!n->img || n->imgW != needW || n->imgH != needH) {
    if (n->img) A.cairo_surface_destroy(n->img);
    n->img = A.cairo_image_surface_create(CAIRO_FORMAT_ARGB32, needW, needH);
    if (A.cairo_surface_status(n->img) != CAIRO_STATUS_SUCCESS) {
      A.cairo_surface_destroy(n->img);
      n->img = nullptr;
      return nullptr;
    }
    n->imgW = needW;
    n->imgH = needH;
  }
  n->imgX = -m - std::round(ax * g_scale);
  n->imgY = -m - std::round(ay * g_scale);
  cairo_t* cr = A.cairo_create(n->img);
  A.cairo_set_operator(cr, CAIRO_OPERATOR_CLEAR);
  A.cairo_paint(cr);
  A.cairo_set_operator(cr, CAIRO_OPERATOR_OVER);
  A.cairo_translate(cr, m, m);
  A.cairo_scale(cr, g_scale, g_scale);
  return cr;
}

void endDraw(Node n, cairo_t* cr) {
  A.cairo_destroy(cr);
  A.cairo_surface_flush(n->img);
  n->hasBox = false;
  n->ver = g_nextVer++;
}

void setSource(cairo_t* cr, Rgba c) { A.cairo_set_source_rgba(cr, c.r, c.g, c.b, c.a); }

void roundedRect(cairo_t* cr, double x0, double y0, double x1, double y1, double r) {
  r = std::max(0.0, std::min({r, (x1 - x0) / 2, (y1 - y0) / 2}));
  A.cairo_new_sub_path(cr);
  A.cairo_arc(cr, x1 - r, y0 + r, r, -kPi / 2, 0);
  A.cairo_arc(cr, x1 - r, y1 - r, r, 0, kPi / 2);
  A.cairo_arc(cr, x0 + r, y1 - r, r, kPi / 2, kPi);
  A.cairo_arc(cr, x0 + r, y0 + r, r, kPi, 3 * kPi / 2);
  A.cairo_close_path(cr);
}

void polygon(cairo_t* cr, const float* pts, int count) {
  A.cairo_move_to(cr, pts[0], pts[1]);
  for (int i = 1; i < count; ++i) A.cairo_line_to(cr, pts[2 * i], pts[2 * i + 1]);
  A.cairo_close_path(cr);
}

// A Gaussian blur of an A8 image, in place (zero outside).
void blurA8(unsigned char* data, int stride, int w, int h, double sigma) {
  if (sigma < 0.3 || w <= 0 || h <= 0) return;
  const int r = static_cast<int>(std::ceil(3 * sigma));
  std::vector<float> k(2 * r + 1);
  float sum = 0;
  for (int i = -r; i <= r; ++i) sum += k[i + r] = static_cast<float>(std::exp(-0.5 * i * i / (sigma * sigma)));
  for (float& v : k) v /= sum;
  std::vector<float> tmp(static_cast<size_t>(w) * h);
  for (int y = 0; y < h; ++y)
    for (int x = 0; x < w; ++x) {
      float acc = 0;
      for (int i = std::max(-r, -x); i <= std::min(r, w - 1 - x); ++i) acc += k[i + r] * data[y * stride + x + i];
      tmp[static_cast<size_t>(y) * w + x] = acc;
    }
  for (int y = 0; y < h; ++y)
    for (int x = 0; x < w; ++x) {
      float acc = 0;
      for (int i = std::max(-r, -y); i <= std::min(r, h - 1 - y); ++i) acc += k[i + r] * tmp[static_cast<size_t>(y + i) * w + x];
      data[y * stride + x] = static_cast<unsigned char>(std::clamp(acc + 0.5f, 0.0f, 255.0f));
    }
}

// A soft drop shadow of the shape `path` draws, like a Core Animation
// shadowPath: filled, moved down by shadowOffsetY, blurred, painted in black.
template <class Path>
void drawShadow(Node n, cairo_t* cr, Path path, double opacity) {
  cairo_surface_t* a8 = A.cairo_image_surface_create(CAIRO_FORMAT_A8, n->imgW, n->imgH);
  if (A.cairo_surface_status(a8) != CAIRO_STATUS_SUCCESS) {
    A.cairo_surface_destroy(a8);
    return;
  }
  cairo_t* s = A.cairo_create(a8);
  cairo_matrix_t m;
  A.cairo_get_matrix(cr, &m);
  A.cairo_set_matrix(s, &m);
  A.cairo_translate(s, 0, kStyle.shadowOffsetY);
  path(s);
  A.cairo_set_source_rgba(s, 0, 0, 0, 1);
  A.cairo_fill(s);
  A.cairo_destroy(s);
  A.cairo_surface_flush(a8);
  // Core Animation's shadowRadius is about two standard deviations.
  blurA8(A.cairo_image_surface_get_data(a8), A.cairo_image_surface_get_stride(a8), n->imgW, n->imgH,
         S(kStyle.shadowRadius / 2));
  A.cairo_surface_mark_dirty(a8);
  A.cairo_save(cr);
  A.cairo_identity_matrix(cr);
  A.cairo_set_source_rgba(cr, 0, 0, 0, opacity);
  A.cairo_mask_surface(cr, a8, 0, 0);
  A.cairo_restore(cr);
  A.cairo_surface_destroy(a8);
}

// Room around the hotspot every glyph fits in, DIP.
constexpr double kGlyphLeft = 16, kGlyphTop = 16, kGlyphRight = 24, kGlyphBottom = 24;

// The pointer glyph (cursor_glyphs.h) as its morph stands at time t: per
// glyph layer the white edges of its parts, then their fills, over one soft
// shadow. While a morph runs it is drawn again on every frame.
void drawGlyph(Node n, const cursor_glyphs::Morph& m, const std::string& color, double t) {
  if (!n) return;
  const bool running = m.running(t);
  char key[64];
  snprintf(key, sizeof key, "glyph%d|%s|%.4f", m.glyph, color.c_str(), running ? t : -1.0);
  const std::string look = key + scaleKey();
  if (n->look == look) return;
  const double u = m.progress(t);
  struct Part {
    std::array<float, cursor_glyphs::kPoints * 2> pts;
    cursor_glyphs::Look look;
    double op;
    int layer;
  };
  std::vector<Part> parts;
  int layers = 0;
  for (const auto& sl : m.slots) {
    Part pt;
    cursor_glyphs::Morph::current(sl, u, pt.pts.data(), &pt.look, &pt.op);
    if (pt.op <= 0.002) continue;
    for (int i = 0; i < cursor_glyphs::kPoints * 2; i += 2) {
      pt.pts[i] += static_cast<float>(kGlyphLeft);
      pt.pts[i + 1] += static_cast<float>(kGlyphTop);
    }
    pt.layer = m.drawLayer(sl, t);
    layers = std::max(layers, pt.layer + 1);
    parts.push_back(pt);
  }
  cairo_t* cr = beginDraw(n, kGlyphLeft + kGlyphRight, kGlyphTop + kGlyphBottom, 6, kGlyphLeft, kGlyphTop);
  if (!cr) return;
  A.cairo_set_line_join(cr, CAIRO_LINE_JOIN_ROUND);
  drawShadow(
      n, cr,
      [&](cairo_t* s) {
        for (const Part& pt : parts)
          if (pt.look.white < 0.5) polygon(s, pt.pts.data(), cursor_glyphs::kPoints);
      },
      kStyle.shadowOpacity);
  const Rgb c = parseColor(color);
  for (int layer = 0; layer < layers; ++layer) {
    for (const Part& pt : parts) {
      if (pt.layer != layer) continue;
      if (pt.look.edgeFill > 0) {
        A.cairo_new_path(cr);
        polygon(cr, pt.pts.data(), cursor_glyphs::kPoints);
        A.cairo_set_source_rgba(cr, 1, 1, 1, pt.look.edgeFill * pt.op);
        A.cairo_fill(cr);
      }
      if (pt.look.edge > 0.01) {
        A.cairo_new_path(cr);
        polygon(cr, pt.pts.data(), cursor_glyphs::kPoints);
        A.cairo_set_source_rgba(cr, 1, 1, 1, pt.op);
        A.cairo_set_line_width(cr, pt.look.edge);
        A.cairo_stroke(cr);
      }
    }
    for (const Part& pt : parts) {
      if (pt.layer != layer) continue;
      const double w = pt.look.white;
      A.cairo_new_path(cr);
      polygon(cr, pt.pts.data(), cursor_glyphs::kPoints);
      A.cairo_set_source_rgba(cr, c.r + (1 - c.r) * w, c.g + (1 - c.g) * w, c.b + (1 - c.b) * w,
                              pt.look.fillAlpha * pt.op);
      A.cairo_fill(cr);
    }
  }
  endDraw(n, cr);
  n->look = look;
}

// A pill in `color` with `label` in the matching ink, `lead` DIP free before
// the text; returns its width in DIP.
double drawPill(Node n, const std::string& label, const std::string& color, double fontSize, double height,
                double padX, double lead = 0) {
  if (!n) return 0;
  char key[96];
  snprintf(key, sizeof key, "|%.1f|%.1f|%.1f|%.1f", fontSize, height, padX, lead);
  const std::string look = "pill" + label + "|" + color + key + scaleKey();
  if (n->look == look) return n->w;
  if (!g_measureCr) return n->w;
  // Measured at the drawing scale, so the hinting matches what is drawn.
  A.cairo_identity_matrix(g_measureCr);
  A.cairo_scale(g_measureCr, g_scale, g_scale);
  PLayout* ml = layoutOf(g_measureCr, label, fontSize);
  if (!ml) return n->w;
  PRect ink{}, logical{};
  A.pango_layout_get_extents(ml, &ink, &logical);
  const double baseline = static_cast<double>(A.pango_layout_get_baseline(ml)) / kPangoScale;
  A.g_object_unref(ml);
  const double tw = std::ceil(static_cast<double>(logical.width) / kPangoScale);
  const double w = tw + 2 * padX + lead;
  const auto shape = [&](cairo_t* c) { roundedRect(c, 0, 0, w, height, height / 2); };
  cairo_t* cr = beginDraw(n, w, height, 6);
  if (!cr) return n->w;
  drawShadow(n, cr, shape, kStyle.shadowOpacity * 0.6);
  A.cairo_new_path(cr);
  shape(cr);
  setSource(cr, rgba(parseColor(color)));
  A.cairo_fill(cr);
  // Centre the capitals, not the line box: the baseline sits half a cap
  // height below the pill's middle.
  const double top = (height + g_capPerEm * fontSize) / 2 - baseline;
  if (PLayout* l = layoutOf(cr, label, fontSize)) {
    A.cairo_move_to(cr, padX + lead, std::round(top * g_scale) / g_scale);
    setSource(cr, rgba(inkOn(color)));
    A.pango_cairo_show_layout(cr, l);
    A.g_object_unref(l);
  }
  endDraw(n, cr);
  n->look = look;
  n->w = w;
  return w;
}

// A small triangle pointing along +x, centred on the node's origin.
void drawEdgeArrow(Node n, const std::string& color) {
  if (!n) return;
  const std::string look = "edgearrow" + color + scaleKey();
  if (n->look == look) return;
  const double a = kStyle.edgeArrow;
  cairo_t* cr = beginDraw(n, a, a, 2, a / 2, a / 2);
  if (!cr) return;
  const double h = kStyle.edgeArrow / 2;
  A.cairo_translate(cr, a / 2, a / 2);
  A.cairo_move_to(cr, -h * 0.8, -h);
  A.cairo_line_to(cr, h, 0);
  A.cairo_line_to(cr, -h * 0.8, h);
  A.cairo_close_path(cr);
  setSource(cr, rgba(inkOn(color)));
  A.cairo_fill_preserve(cr);
  A.cairo_set_line_join(cr, CAIRO_LINE_JOIN_ROUND);
  A.cairo_set_line_width(cr, 1.5);
  A.cairo_stroke(cr);
  endDraw(n, cr);
  n->look = look;
}

// A rounded box of wPx x hPx: `fill` inside and, when line > 0, a `line` DIP
// border of `stroke` along its inside (dashed: centred on the edge instead).
// Drawn straight into the frame: boxes can be as large as the view.
void drawBox(Node n, double wPx, double hPx, Rgba fill, Rgba stroke, double line, bool dash) {
  if (!n) return;
  char key[160];
  snprintf(key, sizeof key, "box%.0fx%.0f|%.3f,%.3f,%.3f,%.3f|%.3f,%.3f,%.3f,%.3f|%.2f%d", wPx, hPx, fill.r, fill.g,
           fill.b, fill.a, stroke.r, stroke.g, stroke.b, stroke.a, line, dash ? 1 : 0);
  const std::string look = key + scaleKey();
  if (n->look == look) return;
  if (n->img) A.cairo_surface_destroy(n->img);
  n->img = nullptr;
  n->hasBox = true;
  n->box = {wPx, hPx, fill, stroke, line, dash, true};
  n->ver = g_nextVer++;
  n->look = look;
}

// A plain rectangle of wPx x hPx.
void drawFill(Node n, double wPx, double hPx, Rgba c) {
  if (!n) return;
  char key[96];
  snprintf(key, sizeof key, "fill%.0fx%.0f|%.3f,%.3f,%.3f,%.3f", wPx, hPx, c.r, c.g, c.b, c.a);
  const std::string look = key + scaleKey();
  if (n->look == look) return;
  if (n->img) A.cairo_surface_destroy(n->img);
  n->img = nullptr;
  n->hasBox = true;
  n->box = {wPx, hPx, c, {}, 0, false, false};
  n->ver = g_nextVer++;
  n->look = look;
}

void paintBox(cairo_t* cr, const BoxSpec& b, double alpha) {
  const bool group = alpha < 0.999;
  if (group) A.cairo_push_group(cr);
  A.cairo_new_path(cr);
  if (b.rounded) {
    const double r = std::min({S(kStyle.markRadius), b.w / 2, b.h / 2});
    roundedRect(cr, 0, 0, b.w, b.h, r);
    setSource(cr, b.fill);
    A.cairo_fill(cr);
    const double line = S(b.line);
    if (line > 0 && b.dash) {
      const double dashes[2] = {S(kStyle.dragDash), S(kStyle.dragGap)};
      roundedRect(cr, 0, 0, b.w, b.h, r);
      A.cairo_set_dash(cr, dashes, 2, 0);
      A.cairo_set_line_join(cr, CAIRO_LINE_JOIN_ROUND);
      A.cairo_set_line_width(cr, line);
      setSource(cr, b.stroke);
      A.cairo_stroke(cr);
      A.cairo_set_dash(cr, nullptr, 0, 0);
    } else if (line > 0 && b.w > line && b.h > line) {
      const double i = line / 2;
      roundedRect(cr, i, i, b.w - i, b.h - i, r - i);
      A.cairo_set_line_width(cr, line);
      setSource(cr, b.stroke);
      A.cairo_stroke(cr);
    }
  } else {
    A.cairo_rectangle(cr, 0, 0, b.w, b.h);
    setSource(cr, b.fill);
    A.cairo_fill(cr);
  }
  if (group) {
    A.cairo_pop_group_to_source(cr);
    A.cairo_paint_with_alpha(cr, alpha);
  }
}

// ---------- state ----------

enum class Side { Left, Right, Top, Bottom };

struct Peer {
  std::string id, name, color;
  double t = 0, fy = 0;
  std::string guid;
  bool in = false, down = false;
  double lastMsg = 0, lastMove = 0;
  bool pendingSpring = false, pendingClick = false;
  // Velocity from the last updates (project seconds and track fractions per
  // second), for prediction.
  double vt = 0, vfy = 0;
  bool predict = false;
  // Their item drag, and its visuals per item: the original marked, the
  // landing spot outlined.
  cursor_overlay::Drag drag;
  struct DragItem {
    Node origin = nullptr, ghost = nullptr;
    Box ghostRect;
    bool shown = false;
  };
  std::map<std::string, DragItem> dragItems;

  Node root = nullptr;   // positioned at the pointer tip
  Node arrow = nullptr;  // child of root: the glyph, scaled about the hotspot while pressed
  cursor_glyphs::Morph morph;
  double tagOffX = kStyle.tagDX, tagOffY = kStyle.tagDY;  // below the glyph, DIP
  bool followed = false;
  Node tag = nullptr;  // a sibling of root, so it can trail it
  Node edge = nullptr;
  Node edgeArrow = nullptr;          // child of edge
  double edgeAx = 0, edgeAy = 0.5;  // which point of the edge pill sits on its spot

  bool cursorShown = false, tagShown = false, edgeShown = false;
  bool idle = false, pressed = false;
  bool fresh = false;  // heard from within staleAfter (tick notices the change)
  Pt cursorModel, tagModel, edgeModel;
  Side edgeSide = Side::Left;
};

// One marked item.
struct MarkState {
  cursor_overlay::Mark m;
  Node box = nullptr;
  double lastWanted = 0;  // when setMarks last asked for it
  bool shown = false;
  Box rect;  // in the view, unclipped; valid while shown
};

struct Geo {
  bool ok = false;
  double t0 = 0, spp = 0, W = 0, H = 0;  // spp: seconds per X pixel
};

struct Place {
  enum Kind { Hidden, Pointer, Edge } kind = Hidden;
  Pt p;
  Side side = Side::Left;
  double angle = 0;  // Edge: direction from the pill to the real spot
};

cursor_overlay::Host g_host{};
bool g_attached = false;
bool g_attachFailedLogged = false;
double g_attachRetryAt = 0;
HWND g_main = nullptr;
HWND g_tv = nullptr;
double g_clipW = -1, g_clipH = -1;
std::map<std::string, Peer> g_peers;
bool g_showCursors = true;
// The desktop's "animations" switch off: no springs, no scaling, cursors fade
// instead of travelling.
bool g_reduceMotion = false;
double g_reduceMotionChecked = 0;
// Item marks live in their own node below the cursors.
std::map<std::string, MarkState> g_marks;
bool g_marksDirty = false;
// REAPER reports an item's rows as of its last draw, which can trail a zoom
// or scroll by a pass: after any change, marks are laid out on every pass
// until this time.
double g_marksSettleUntil = 0;
double g_markKey[5] = {};
unsigned long long g_markStamp = 0;
// The name of whoever edits the item under the local pointer.
Node g_hover = nullptr;
bool g_hoverShown = false;
bool g_localIn = false;
Pt g_localPoint;
bool g_localDown = false;  // the button as last sampled (a press is a click)
double g_localMovedAt = -1;
// The follow frame: four strips and the tag, children of one node so they
// fade together.
Node g_follow = nullptr, g_followTag = nullptr, g_followEdge[4] = {};
std::string g_followKey, g_followName, g_followColor, g_followId;

// Frames.
UINT_PTR g_timer = 0;
bool g_animating = false;    // something moved in the last frame
bool g_fullDirty = true;     // redraw the whole window next frame
double g_newsAt = -1;        // a cursor update that moves something arrived
double g_geomSettleUntil = 0;
double g_lastPump = 0;
int g_framesCounted = 0;
double g_framesSince = 0;

// ---------- peers ----------

std::string displayName(const Peer& p) {
  if (!p.name.empty()) return p.name;
  return p.id.size() > 6 ? p.id.substr(p.id.size() - 6) : p.id;
}

void updateTag(Peer& p, double t);

// The person we follow stands out: the name tag always up, never dimmed,
// above everyone else's cursor.
void setFollowed(Peer& p, bool on) {
  if (!on && !p.followed) return;
  p.followed = on;
  if (on) {
    raise(p.edge);
    raise(p.tag);
    raise(p.root);
    if (p.idle) {
      fade(p.arrow, 1, kStyle.fadeIn);
      p.idle = false;
    }
  }
  updateTag(p, now());
}

void raiseFollowed() {
  if (g_followId.empty()) return;
  auto it = g_peers.find(g_followId);
  if (it == g_peers.end()) return;
  raise(it->second.edge);
  raise(it->second.tag);
  raise(it->second.root);
}

void buildLayers(Peer& p) {
  p.edge = makeNode(g_scene.get());
  p.edgeArrow = makeNode(p.edge);
  p.tag = makeNode(g_scene.get());
  p.root = makeNode(g_scene.get());
  p.arrow = makeNode(p.root);
  setOpacityNow(p.edge, 0);
  setOpacityNow(p.tag, 0);
  setOpacityNow(p.root, 0);
  p.morph.set(0, now(), 0, kStyle.outline);
  drawGlyph(p.arrow, p.morph, p.color, now());
  // Built above every other cursor, which the followed one has to stay on.
  if (p.id == g_followId) setFollowed(p, true);
  else raiseFollowed();
}

void dropDragItem(Peer::DragItem& d) {
  dropNode(d.origin);
  dropNode(d.ghost);
}

void dropLayers(Peer& p) {
  for (auto& [item, d] : p.dragItems) dropDragItem(d);
  p.dragItems.clear();
  p.arrow = nullptr;  // goes with root
  p.edgeArrow = nullptr;  // goes with edge
  dropNode(p.root);
  dropNode(p.tag);
  dropNode(p.edge);
}

void refreshTag(Peer& p) {
  drawPill(p.tag, displayName(p), p.color, kStyle.tagFont, kStyle.tagHeight, kStyle.tagPadX);
  drawGlyph(p.arrow, p.morph, p.color, now());
}

void refreshEdge(Peer& p, Side side) {
  const double lead = kStyle.edgeArrow + 4;
  const double w = drawPill(p.edge, displayName(p), p.color, kStyle.edgeFont, kStyle.edgeHeight, kStyle.edgePadX, lead);
  setPos(p.edgeArrow, {std::round(S(kStyle.edgePadX + kStyle.edgeArrow / 2)), std::round(S(kStyle.edgeHeight / 2))});
  drawEdgeArrow(p.edgeArrow, p.color);
  switch (side) {
    case Side::Left: p.edgeAx = 0, p.edgeAy = 0.5; break;
    case Side::Right: p.edgeAx = 1, p.edgeAy = 0.5; break;
    case Side::Top: p.edgeAx = 0.5, p.edgeAy = 0; break;
    case Side::Bottom: p.edgeAx = 0.5, p.edgeAy = 1; break;
  }
  setScaleCentre(p.edge, p.edgeAx * S(w), p.edgeAy * S(kStyle.edgeHeight));
}

// Where the edge pill's node goes for its spot `at` (pill top-left).
Pt edgeOrigin(const Peer& p, Pt at) {
  const double w = p.edge ? p.edge->w : 0;
  return {std::round(at.x - p.edgeAx * S(w)), std::round(at.y - p.edgeAy * S(kStyle.edgeHeight))};
}

void setEdgeAngle(Peer& p, double angle) {
  if (p.edgeArrow) p.edgeArrow->angle = angle * 180 / kPi;
}

// Appear: fade in while growing from appearScale. `fromZero` starts the fade
// at nothing even if the node was visible a moment ago (a crossfade).
void popIn(Node n, double opacity, bool fromZero = false) {
  fade(n, opacity, kStyle.fadeIn, fromZero ? 0 : kFromNow);
  if (g_reduceMotion) return;
  scaleTo(n, 1, kStyle.fadeIn * 1.6, fromZero ? kStyle.appearScale : kFromNow);
}

void popOut(Node n) {
  fade(n, 0, kStyle.fadeOut);
  if (g_reduceMotion) return;
  scaleTo(n, kStyle.appearScale, kStyle.fadeOut);
}

void ripple(Peer& p) {
  if (!p.root) return;
  Node r = makeNode(p.root, Z::Bottom);
  // Drawn at its largest and scaled up to it, so the ring stays sharp.
  const double to = kStyle.rippleScaleTo, R = kStyle.rippleRadius * to;
  cairo_t* cr = beginDraw(r, 2 * R, 2 * R, 3, R, R);
  if (!cr) {
    dropNode(r);
    return;
  }
  const double line = kStyle.rippleLine * to;
  A.cairo_arc(cr, R, R, R - line / 2, 0, 2 * kPi);
  setSource(cr, rgba(parseColor(p.color)));
  A.cairo_set_line_width(cr, line);
  A.cairo_stroke(cr);
  endDraw(r, cr);
  const double t = now();
  if (g_reduceMotion) r->sx = Curve::constant(1 / to);
  else r->sx = Curve::ease(t, kStyle.rippleScaleFrom / to, 1, kStyle.rippleDuration, kEaseOut);
  r->sy = r->sx;
  r->op = Curve::ease(t, kStyle.rippleOpacity, 0, kStyle.rippleDuration, kEaseOut);
  dropLater(r, t + kStyle.rippleDuration + 0.05);
}

// The glyph REAPER's cursor stands for on their screen ("" the pointer).
// Hidden cursors switch at once; a visible one morphs, drawn per frame.
void setGlyph(Peer& p, const std::string& key) {
  const int g = cursor_glyphs::glyphFor(key);
  const double t = now();
  if (!p.morph.set(g, t, g_reduceMotion || !p.cursorShown ? 0 : kStyle.morph, kStyle.outline)) return;
  drawGlyph(p.arrow, p.morph, p.color, t);
  cursor_glyphs::tagOffset(g, kStyle.tagDX, kStyle.tagDY, &p.tagOffX, &p.tagOffY);
  p.pendingSpring = true;
}

// Every running morph one frame on; finished ones drop their leftovers.
bool stepMorphs(double t) {
  bool running = false;
  for (auto& [id, p] : g_peers) {
    // A moment past the end draws the final shape (then it is a cached look).
    if (t < p.morph.start + p.morph.dur + 0.1) {
      drawGlyph(p.arrow, p.morph, p.color, t);
      running = true;
    }
    p.morph.prune(t);
  }
  return running;
}

// The button is held (or let go): the pointer shrinks a little about its tip.
void setPressed(Peer& p, bool down) {
  if (down == p.pressed) return;
  p.pressed = down;
  scaleTo(p.arrow, down ? kStyle.pressedScale : 1.0, kStyle.pressDuration);
}

// The cursor fades across instead of flying: a copy fades out where it was,
// the cursor fades (and grows) in at its new spot.
void crossfadeTo(Peer& p, Pt at, Pt tagAt) {
  const double t = now();
  Node ghost = makeNode(g_scene.get(), Z::Beneath, p.root);
  if (ghost) {
    drawGlyph(ghost, p.morph, p.color, p.morph.start + p.morph.dur + 1);
    setPos(ghost, shownAt(p.root, t));
    ghost->sx = ghost->sy = Curve::constant(p.arrow->sx.value(t) * p.root->sx.value(t));
    fade(ghost, 0, kStyle.crossfade, p.root->op.value(t) * p.arrow->op.value(t));
    dropLater(ghost, t + kStyle.crossfade + 0.05);
  }
  snapTo(p.root, at);
  snapTo(p.tag, tagAt);
  popIn(p.root, 1, true);
  if (p.tagShown) fade(p.tag, 1, kStyle.crossfade, 0);
}

// ---------- geometry ----------
//
// REAPER answers in SWELL units; the layer works in X pixels (g_d per unit).

bool trackSpanPx(const char* guid, double* y, double* h) {
  if (!g_host.trackSpan(guid, y, h)) return false;
  *y *= g_d;
  *h *= g_d;
  return true;
}

Geo readGeo() {
  Geo g;
  if (!g_attached || !g_host.ready || !g_host.ready()) return g;
  double t0 = 0, spp = 0;
  if (!g_host.timeAxis(&t0, &spp) || !(spp > 0)) return g;
  g.t0 = t0;
  g.spp = spp / g_d;
  g.W = g_viewW;
  g.H = g_viewH;
  g.ok = g.W > 0 && g.H > 0;
  return g;
}

Place placeOf(const Peer& p, const Geo& g, double t) {
  Place pl;
  if (!g.ok) return pl;
  const bool fresh = t - p.lastMsg <= kStyle.staleAfter;
  if (!p.in || !fresh) return pl;
  double ty = 0, th = 0;
  const bool found = !p.guid.empty() && trackSpanPx(p.guid.c_str(), &ty, &th);
  const bool yKnown = found && th > 0;
  double x = (p.t - g.t0) / g.spp;
  double y = yKnown ? ty + p.fy * th : 0;
  if (yKnown && p.predict && !g_reduceMotion) {
    // Ahead along the velocity, but never further than predictMax and never
    // off the track the pointer is on.
    double dx = p.vt * kStyle.predictLead / g.spp, dy = p.vfy * kStyle.predictLead * th;
    const double d = std::hypot(dx, dy), most = S(kStyle.predictMax);
    if (d > most) dx *= most / d, dy *= most / d;
    x += dx;
    y = std::clamp(y + dy, ty, ty + th);
  }
  const bool xIn = x >= 0 && x <= g.W;
  const bool yIn = yKnown && y >= 0 && y <= g.H;
  if (xIn && yIn) {
    pl.kind = Place::Pointer;
    pl.p = {std::round(x), std::round(y)};
    return pl;
  }
  pl.kind = Place::Edge;
  const double in = S(kStyle.edgeInset);
  const double half = S(kStyle.edgeHeight) / 2;
  const double side = S(40);
  const double cy = yKnown ? std::clamp(y, in + half, std::max(in + half, g.H - in - half)) : g.H / 2;
  const double cx = std::clamp(x, in + side, std::max(in + side, g.W - in - side));
  if (!xIn) {
    pl.side = x < 0 ? Side::Left : Side::Right;
    pl.p = {x < 0 ? in : g.W - in, cy};
  } else if (yKnown) {
    pl.side = y < 0 ? Side::Top : Side::Bottom;
    pl.p = {cx, y < 0 ? in : g.H - in};
  } else {
    // Hidden track, or one this project doesn't have: the time is known.
    pl.side = Side::Bottom;
    pl.p = {cx, g.H - in};
  }
  // Which way the arrow in the pill points: from the pill to the real spot
  // (straight out of the edge when the height is unknown).
  const double ry = yKnown ? y : (pl.side == Side::Bottom ? g.H + S(100) : pl.p.y);
  pl.angle = std::atan2(ry - pl.p.y, x - pl.p.x);
  if (!xIn && !yKnown) pl.angle = x < 0 ? kPi : 0;
  return pl;
}

// The name tag shows while the cursor moves, and always for the person we
// follow.
void updateTag(Peer& p, double t) {
  const bool want = p.cursorShown && (p.followed || t - p.lastMove < kStyle.tagIdleAfter);
  if (want == p.tagShown) return;
  fade(p.tag, want ? 1 : 0, want ? kStyle.tagFadeIn : kStyle.tagFadeOut);
  p.tagShown = want;
}

// A rectangle cut to the view (plus a little, so cut edges draw no border):
// items can be far wider than the view.
Box clipToView(const Box& r, const Geo& g) {
  const double m = 8;
  const double x0 = std::max(r.x, -m), y0 = std::max(r.y, -m);
  const double x1 = std::min(r.x + r.w, g.W + m), y1 = std::min(r.y + r.h, g.H + m);
  return {x0, y0, std::max(x1 - x0, 1.0), std::max(y1 - y0, 1.0)};
}

// Where an item is, and where it would land moved by `dt` and `dtr` tracks.
bool itemRects(const std::string& item, double dt, int dtr, const Geo& g, Box* at, Box* to) {
  double t0 = 0, t1 = 0, y = 0, h = 0, ty = 0, th = 0;
  std::string track, dest;
  if (!g.ok || !g_host.itemSpan || !g_host.itemSpan(item.c_str(), &t0, &t1, &y, &h, &track)) return false;
  if (!trackSpanPx(track.c_str(), &ty, &th)) return false;
  y *= g_d;
  h *= g_d;
  *at = {std::round((t0 - g.t0) / g.spp), std::round(y), std::round((t1 - t0) / g.spp), std::round(h)};
  double dy = 0;
  if (dtr != 0 && g_host.trackShift && g_host.trackShift(track.c_str(), dtr, &dest)) {
    double dy0 = 0, dh = 0;
    if (trackSpanPx(dest.c_str(), &dy0, &dh) && dh > 0) dy = dy0 - ty;
  }
  *to = {std::round(at->x + dt / g.spp), std::round(y + dy), at->w, at->h};
  return true;
}

Rgba dimFill() { return {0, 0, 0, kStyle.markDim}; }

// The drag preview: each dragged item's original marked like a lock, and a
// dashed outline where it would land, moving with the person's cursor.
void layoutDrag(Peer& p, const Geo& g, bool net) {
  std::set<std::string> want(p.drag.items.begin(), p.drag.items.end());
  for (auto it = p.dragItems.begin(); it != p.dragItems.end();) {
    if (want.count(it->first)) {
      ++it;
      continue;
    }
    dropDragItem(it->second);
    it = p.dragItems.erase(it);
  }
  if (want.empty() || !g.ok) return;
  const Rgb c = parseColor(p.color);
  for (const std::string& item : want) {
    Peer::DragItem& d = p.dragItems[item];
    if (!d.ghost) {
      d.origin = makeNode(g_markN);
      d.ghost = makeNode(g_markN);
    }
    Box at, to;
    if (!itemRects(item, p.drag.dt, p.drag.dtr, g, &at, &to)) continue;
    const Box a = clipToView(at, g), b = clipToView(to, g);
    drawBox(d.origin, a.w, a.h, dimFill(), rgba(c), kStyle.markLine, false);
    setPos(d.origin, {a.x, a.y});
    drawBox(d.ghost, b.w, b.h, rgba(c, kStyle.dragFill), rgba(c), kStyle.dragLine, true);
    if (net && d.shown && !g_reduceMotion) springTo(d.ghost, {d.ghostRect.x, d.ghostRect.y}, {b.x, b.y}, kStyle.springStiffness);
    else setPos(d.ghost, {b.x, b.y});
    d.ghostRect = b;
    d.shown = true;
  }
}

// Moves a peer's visuals to where its spot is now. With a network update
// pending they spring toward it (or fade across on a long jump); otherwise
// only the view moved and they snap (glued).
void layoutPeer(Peer& p, const Geo& g, double t) {
  const Place pl = placeOf(p, g, t);
  const bool net = p.pendingSpring;
  p.pendingSpring = false;
  if (!p.drag.items.empty() && t - p.lastMsg > kStyle.staleAfter) p.drag = {};
  layoutDrag(p, g, net);

  if (pl.kind == Place::Pointer) {
    const Pt tagAt{pl.p.x + std::round(S(p.tagOffX)), pl.p.y + std::round(S(p.tagOffY))};
    if (!p.cursorShown) {
      snapTo(p.root, pl.p);
      snapTo(p.tag, tagAt);
      popIn(p.root, 1);
      p.cursorShown = true;
    } else if (!samePoint(pl.p, p.cursorModel)) {
      const double dist = std::hypot(pl.p.x - p.cursorModel.x, pl.p.y - p.cursorModel.y);
      if (net && dist > S(kStyle.jumpCrossfade)) {
        crossfadeTo(p, pl.p, tagAt);
      } else if (net && !g_reduceMotion) {
        springTo(p.root, p.cursorModel, pl.p, kStyle.springStiffness);
        springTo(p.tag, p.tagModel, tagAt, kStyle.tagStiffness);
      } else {
        setPos(p.root, pl.p);
        setPos(p.tag, tagAt);
      }
    } else if (!samePoint(tagAt, p.tagModel)) {
      // A new glyph moved the name tag below it.
      if (g_reduceMotion) setPos(p.tag, tagAt);
      else springTo(p.tag, p.tagModel, tagAt, kStyle.tagStiffness);
    }
    p.cursorModel = pl.p;
    p.tagModel = tagAt;
    if (p.pendingClick) ripple(p);
  } else if (p.cursorShown) {
    popOut(p.root);
    p.cursorShown = false;
  }
  p.pendingClick = false;
  updateTag(p, t);

  if (pl.kind == Place::Edge) {
    setEdgeAngle(p, pl.angle);
    if (!p.edgeShown || pl.side != p.edgeSide) {
      refreshEdge(p, pl.side);
      snapTo(p.edge, edgeOrigin(p, pl.p));
      popIn(p.edge, kStyle.edgeOpacity, p.edgeShown);
      p.edgeShown = true;
      p.edgeSide = pl.side;
    } else {
      const Pt before = edgeOrigin(p, p.edgeModel);
      refreshEdge(p, pl.side);
      const Pt after = edgeOrigin(p, pl.p);
      if (!samePoint(before, after)) {
        // Glides along the edge with the person.
        if (net && !g_reduceMotion) springTo(p.edge, before, after, kStyle.springStiffness);
        else setPos(p.edge, after);
      }
    }
    p.edgeModel = pl.p;
  } else if (p.edgeShown) {
    popOut(p.edge);
    p.edgeShown = false;
  }
}

void layoutMarks(const Geo& g);
void layoutFollow();

// REAPER's UI scale or the X pixels per SWELL unit changed: everything is
// drawn again at the new scale (the look keys carry it) and laid out again.
void checkScale() {
  double ui = 1;
  if (sw::SWELL_GetScaling256) {
    const int s = sw::SWELL_GetScaling256();
    if (s >= 128 && s <= 1024) ui = s / 256.0;
  }
  g_ui = ui;
  const double s = g_ui * g_d;
  if (s == g_scale) return;
  g_scale = s;
  for (auto& [id, p] : g_peers) {
    refreshTag(p);
    if (p.edgeShown) refreshEdge(p, p.edgeSide);
    p.pendingSpring = true;
  }
  g_marksDirty = true;
  if (g_hover) g_hover->look.clear();
  layoutFollow();
}

// Whether nothing on screen can have moved since the last full pass: no
// update waiting and the view (time axis, size, track rows) unchanged. Most
// passes change nothing. REAPER's own numbers can trail a change by a pass,
// so after one the full pass keeps running for a moment.
double g_viewKey[5] = {};
unsigned long long g_viewStamp = 0;
double g_viewSettleUntil = 0;

bool quiet(const Geo& g, double t) {
  const unsigned long long stamp = g.ok && g_host.rowsStamp ? g_host.rowsStamp() : 0;
  const double key[5] = {g.t0, g.spp, g.W, g.H, g.ok ? 1.0 : 0.0};
  if (stamp != g_viewStamp || !std::equal(key, key + 5, g_viewKey)) {
    g_viewStamp = stamp;
    std::copy(key, key + 5, g_viewKey);
    g_viewSettleUntil = t + 0.15;
    return false;
  }
  if (t < g_viewSettleUntil || g_marksDirty || t < g_marksSettleUntil) return false;
  for (const auto& [id, p] : g_peers)
    if (p.pendingSpring || p.pendingClick) return false;
  return true;
}

void syncWindow();

// The glue pass: every peer to its current pixel (`onlyPending`: just the
// peers with a network update waiting).
void glue(bool onlyPending = false) {
  if (!g_attached) return;
  syncWindow();
  if (g_viewW != g_clipW || g_viewH != g_clipH) {
    g_clipW = g_viewW;
    g_clipH = g_viewH;
    layoutFollow();
  }
  checkScale();
  if (g_peers.empty() && g_marks.empty()) return;
  const double t = now();
  const Geo g = readGeo();
  // The project can't be read right now (a remote apply, a project opening):
  // leave everything where it is and the updates pending.
  if (!g.ok) return;
  if (!onlyPending && quiet(g, t)) return;
  if (!onlyPending) layoutMarks(g);
  for (auto& [id, p] : g_peers)
    if (!onlyPending || p.pendingSpring || p.pendingClick) layoutPeer(p, g, t);
}

// ---------- item marks ----------

// Where an item is in the view now; false when it is not visible.
bool markRect(const MarkState& ms, const Geo& g, Box* out) {
  double t0 = 0, t1 = 0, y = 0, h = 0;
  if (!g.ok || !g_host.itemSpan || !g_host.itemSpan(ms.m.item.c_str(), &t0, &t1, &y, &h, nullptr)) return false;
  y *= g_d;
  h *= g_d;
  if (!(h > 0) || !(t1 > t0)) return false;
  const double x0 = (t0 - g.t0) / g.spp, x1 = (t1 - g.t0) / g.spp;
  if (x1 <= 0 || x0 >= g.W || y + h <= 0 || y >= g.H) return false;
  *out = {std::round(x0), std::round(y), std::max(std::round(x1) - std::round(x0), 1.0), std::round(y + h) - std::round(y)};
  return true;
}

// The name tag above the hovered item (inside it when there is no room above).
void layoutHover() {
  const MarkState* hit = nullptr;
  if (g_localIn)
    for (const auto& [id, ms] : g_marks)
      if (ms.shown && !ms.m.loading && boxContains(ms.rect, g_localPoint)) hit = &ms;
  if (!hit) {
    if (g_hoverShown) fade(g_hover, 0, kStyle.markFadeOut);
    g_hoverShown = false;
    return;
  }
  if (!g_hover) {
    g_hover = makeNode(g_markN);
    setOpacityNow(g_hover, 0);
  }
  const std::string label = hit->m.name.empty() ? "\xE2\x80\xA2" : hit->m.name;  // •
  drawPill(g_hover, label, hit->m.color, kStyle.markTagFont, kStyle.markTagHeight, kStyle.markTagPadX);
  const Box r = hit->rect;
  const double above = r.y - S(kStyle.markTagGap) - S(kStyle.markTagHeight);
  setPos(g_hover, {std::round(std::max(r.x, S(2))), std::round(above >= S(2) ? above : r.y + S(kStyle.markTagGap))});
  if (!g_hoverShown) fade(g_hover, 1, kStyle.markFadeIn);
  g_hoverShown = true;
}

void drawMark(MarkState& ms, const Box& clipped) {
  if (ms.m.loading)
    drawBox(ms.box, clipped.w, clipped.h, {0, 0, 0, kStyle.loadingVeil}, {}, 0, false);
  else
    drawBox(ms.box, clipped.w, clipped.h, dimFill(), rgba(parseColor(ms.m.color)), kStyle.markLine, false);
}

// Every mark to its item's current rectangle. Skipped while neither the time
// axis nor the rows moved, since that takes a few API calls per item.
void layoutMarks(const Geo& g) {
  if (g_marks.empty()) {
    if (g_hoverShown) layoutHover();
    return;
  }
  if (!g.ok) return;  // can't read the project right now: keep them where they are
  const unsigned long long stamp = g_host.rowsStamp ? g_host.rowsStamp() : 0;
  const double key[5] = {g.t0, g.spp, g.W, g.H, g.ok ? 1.0 : 0.0};
  // The settle window opens only when the view moved: setMarks marks the set
  // dirty on every extension tick, which needs one pass, not a window.
  const bool viewMoved = stamp != g_markStamp || !std::equal(key, key + 5, g_markKey);
  if (viewMoved) g_marksSettleUntil = now() + 0.15;
  else if (!g_marksDirty && now() > g_marksSettleUntil) return;
  g_marksDirty = false;
  g_markStamp = stamp;
  std::copy(key, key + 5, g_markKey);
  for (auto& [id, ms] : g_marks) {
    Box r;
    if (markRect(ms, g, &r)) {
      const Box c = clipToView(r, g);
      drawMark(ms, c);
      setPos(ms.box, {c.x, c.y});
      ms.rect = r;
      if (!ms.shown) fade(ms.box, 1, kStyle.markFadeIn);
      ms.shown = true;
    } else if (ms.shown) {
      fade(ms.box, 0, kStyle.markFadeOut);
      ms.shown = false;
    }
  }
  layoutHover();
}

// ---------- the follow frame ----------

void layoutFollow() {
  if (!g_attached || g_followKey.empty() || g_clipW <= 0 || g_clipH <= 0) return;
  const double W = g_clipW, H = g_clipH, L = std::max(1.0, std::round(S(kStyle.followLine)));
  const Rgba c = rgba(parseColor(g_followColor));
  const Box strips[4] = {{0, 0, W, L}, {0, H - L, W, L}, {0, L, L, H - 2 * L}, {W - L, L, L, H - 2 * L}};
  for (int i = 0; i < 4; ++i) {
    const Box& b = strips[i];
    drawFill(g_followEdge[i], b.w, std::max(b.h, 1.0), c);
    setPos(g_followEdge[i], {b.x, b.y});
  }
  const double tw = drawPill(g_followTag, g_followName, g_followColor, kStyle.tagFont, kStyle.tagHeight, kStyle.tagPadX);
  setPos(g_followTag, {std::round(W - S(kStyle.followTagInset) - S(tw)), std::round(S(kStyle.followTagInset))});
}

// ---------- the local pointer ----------

bool semanticAt(Pt vp, const Geo& g, double* tt, std::string* guid, double* fy) {
  if (!g.ok || !g_host.trackAt(vp.y / g_d, guid, fy)) return false;
  *tt = g.t0 + vp.x * g.spp;
  return true;
}

// The pointer as the X server has it: root position, the top-level window
// under it (input shapes respected, so never this layer) and the buttons.
struct PointerState {
  bool ok = false;
  int x = 0, y = 0;
  Window child = 0;
  unsigned mask = 0;
};
PointerState g_pointer;
double g_pointerAt = -1;

PointerState queryPointer() {
  const double t = now();
  if (t - g_pointerAt < 0.004) return g_pointer;
  g_pointerAt = t;
  PointerState p;
  if (g_dpy && g_rootW) {
    Trap trap;
    Window root = 0, child = 0;
    int wx = 0, wy = 0;
    p.ok = A.XQueryPointer(g_dpy, g_rootW, &root, &child, &p.x, &p.y, &wx, &wy, &p.mask) != 0;
    p.child = child;
  }
  g_pointer = p;
  return p;
}

// ---------- the X window: geometry, visibility, stacking ----------

cairo_surface_t* g_back = nullptr;  // the frame as shown, client side
XImage* g_ximage = nullptr;         // over g_back's pixels

void dropBackBuffer() {
  if (g_ximage) {
    g_ximage->data = nullptr;  // cairo's: not Xlib's to free
    XDestroyImage(g_ximage);
    g_ximage = nullptr;
  }
  if (g_back) A.cairo_surface_destroy(g_back);
  g_back = nullptr;
}

bool makeBackBuffer(int w, int h) {
  dropBackBuffer();
  g_back = A.cairo_image_surface_create(CAIRO_FORMAT_ARGB32, w, h);
  if (A.cairo_surface_status(g_back) != CAIRO_STATUS_SUCCESS) {
    A.cairo_surface_destroy(g_back);
    g_back = nullptr;
    return false;
  }
  // Depth-32 ZPixmap is cairo's ARGB32 (premultiplied, native byte order).
  g_ximage = A.XCreateImage(g_dpy, g_visual, 32, ZPixmap, 0,
                            reinterpret_cast<char*>(A.cairo_image_surface_get_data(g_back)), w, h, 32,
                            A.cairo_image_surface_get_stride(g_back));
  if (!g_ximage) {
    dropBackBuffer();
    return false;
  }
  g_fullDirty = true;
  return true;
}

// A CARDINAL property of a window.
bool cardinal(Window w, const char* prop, long* out) {
  Atom type = 0;
  int format = 0;
  unsigned long n = 0, after = 0;
  unsigned char* data = nullptr;
  if (A.XGetWindowProperty(g_dpy, w, atom(prop), 0, 1, False, XA_CARDINAL, &type, &format, &n, &after, &data) !=
          Success ||
      !data)
    return false;
  const bool ok = n == 1 && format == 32;
  if (ok) *out = *reinterpret_cast<long*>(data);
  A.XFree(data);
  return ok;
}

// Whether the window manager hides REAPER's main window while keeping it
// mapped (compositing ones do, for thumbnails): minimised, or on another
// workspace.
bool ewmhHidden() {
  Atom type = 0;
  int format = 0;
  unsigned long n = 0, after = 0;
  unsigned char* data = nullptr;
  bool hidden = false;
  if (A.XGetWindowProperty(g_dpy, g_mainX, atom("_NET_WM_STATE"), 0, 32, False, XA_ATOM, &type, &format, &n, &after,
                           &data) == Success &&
      data) {
    const Atom h = atom("_NET_WM_STATE_HIDDEN");
    if (format == 32)
      for (unsigned long i = 0; i < n; ++i) hidden = hidden || static_cast<Atom>(reinterpret_cast<long*>(data)[i]) == h;
    A.XFree(data);
  }
  if (hidden) return true;
  long desk = 0, current = 0;
  return cardinal(g_mainX, "_NET_WM_DESKTOP", &desk) && cardinal(g_rootW, "_NET_CURRENT_DESKTOP", &current) &&
         (desk & 0xffffffffL) != 0xffffffffL && desk != current;
}

// Restacks the layer directly above REAPER's main frame (if it isn't there
// already) and, under XWayland, cuts every window above that frame out of its
// bounding shape (`cuts`: look at those windows again).
void restack(bool cuts = true) {
  if (!g_win || !g_frame) return;
  Window root = 0, parent = 0, *kids = nullptr;
  unsigned n = 0;
  if (!A.XQueryTree(g_dpy, g_rootW, &root, &parent, &kids, &n)) return;
  int fi = -1, oi = -1;
  for (unsigned i = 0; i < n; ++i) {
    if (kids[i] == g_frame) fi = static_cast<int>(i);
    if (kids[i] == g_win) oi = static_cast<int>(i);
  }
  if (fi >= 0 && oi != fi + 1) {
    XWindowChanges c{};
    c.sibling = g_frame;
    c.stack_mode = Above;
    A.XConfigureWindow(g_dpy, g_win, CWSibling | CWStackMode, &c);
    DEBUGF("restacked above frame 0x%lx (was at %d, frame at %d)", g_frame, oi, fi);
  }
  if (g_xwayland && fi >= 0 && cuts) {
    std::vector<XRectangle> cut;
    for (unsigned i = static_cast<unsigned>(fi) + 1; i < n && cut.size() < 64; ++i) {
      if (kids[i] == g_win) continue;
      XWindowAttributes wa{};
      if (!A.XGetWindowAttributes(g_dpy, kids[i], &wa)) continue;
      if (wa.map_state != IsViewable || wa.c_class != InputOutput) continue;
      const int x0 = std::max(wa.x - g_winX, 0), y0 = std::max(wa.y - g_winY, 0);
      const int x1 = std::min(wa.x + wa.width + 2 * wa.border_width - g_winX, g_viewW);
      const int y1 = std::min(wa.y + wa.height + 2 * wa.border_width - g_winY, g_viewH);
      if (x1 <= x0 || y1 <= y0) continue;
      cut.push_back({static_cast<short>(x0), static_cast<short>(y0), static_cast<unsigned short>(x1 - x0),
                     static_cast<unsigned short>(y1 - y0)});
    }
    const auto same = [](const XRectangle& a, const XRectangle& b) {
      return a.x == b.x && a.y == b.y && a.width == b.width && a.height == b.height;
    };
    if (!g_cutsSet || cut.size() != g_cuts.size() || !std::equal(cut.begin(), cut.end(), g_cuts.begin(), same)) {
      if (cut.empty()) {
        A.XFixesSetWindowShapeRegion(g_dpy, g_win, ShapeBounding, 0, 0, None);
      } else {
        XRectangle all{0, 0, static_cast<unsigned short>(std::max(g_viewW, 1)),
                       static_cast<unsigned short>(std::max(g_viewH, 1))};
        XserverRegion r = A.XFixesCreateRegion(g_dpy, &all, 1);
        XserverRegion c = A.XFixesCreateRegion(g_dpy, cut.data(), static_cast<int>(cut.size()));
        A.XFixesSubtractRegion(g_dpy, r, r, c);
        A.XFixesSetWindowShapeRegion(g_dpy, g_win, ShapeBounding, 0, 0, r);
        A.XFixesDestroyRegion(g_dpy, c);
        A.XFixesDestroyRegion(g_dpy, r);
      }
      DEBUGF("bounding shape: %zu windows cut out", cut.size());
      g_cuts = std::move(cut);
      g_cutsSet = true;
    }
  }
  if (kids) A.XFree(kids);
}

void teardown();

// The checks that take X round trips, once per extension tick: whether
// REAPER's main window is still there and shown, the X pixels per SWELL
// unit, the frame around it, focus (XWayland) and stacking.
void checkWindows() {
  if (!g_attached) return;
  if (!sw::IsWindow(g_tv) || !sw::IsWindow(g_main)) {
    trace("track view gone; detaching");
    teardown();
    return;
  }
  Trap trap;
  XWindowAttributes wa{};
  if (!A.XGetWindowAttributes(g_dpy, g_mainX, &wa)) {
    trace("REAPER's main X window is gone; detaching");
    teardown();
    return;
  }
  g_mainViewable = wa.map_state == IsViewable;
  RECT mc{};
  sw::GetClientRect(g_main, &mc);
  if (mc.right > 0) {
    const double r = static_cast<double>(wa.width) / mc.right;
    const double q = std::round(r * 4) / 4;
    if (std::fabs(r - q) < 0.02 && q >= 0.5 && q <= 4 && q != g_d) {
      DEBUGF("X pixels per SWELL unit: %.2f", q);
      g_d = q;
    }
  }
  const double t = now();
  if (t - g_frameCheckedAt > 0.5 || !g_frame) {
    g_frameCheckedAt = t;
    const Window f = frameOf(g_mainX);
    if (f && f != g_frame) DEBUGF("REAPER's frame: 0x%lx (main 0x%lx)", f, g_mainX);
    if (f) g_frame = f;
  }
  if (t - g_ewmhCheckedAt > 0.25) {
    g_ewmhCheckedAt = t;
    const bool hidden = ewmhHidden();
    if (hidden != g_ewmhHidden) DEBUGF("REAPER's window is %s", hidden ? "minimised or elsewhere" : "back");
    g_ewmhHidden = hidden;
  }
  if (g_xwayland) {
    const bool active = reaperFocused();
    if (active != g_active) DEBUGF("REAPER %s the focus", active ? "has" : "lost");
    g_active = active;
  }
  // Cutting windows out takes a round trip per window above REAPER's.
  const bool cuts = !g_cutsSet || t - g_cutsAt > 0.1;
  if (cuts) g_cutsAt = t;
  restack(cuts);
}

// Lays the window over the track view and shows or hides it. Cheap (SWELL
// only, plus X requests when something changed): runs on every frame.
void syncWindow() {
  if (!g_attached || !g_win) return;
  RECT rc{};
  sw::GetClientRect(g_tv, &rc);
  POINT o{0, 0};
  sw::ClientToScreen(g_tv, &o);
  const int W = static_cast<int>(std::lround(rc.right * g_d)), H = static_cast<int>(std::lround(rc.bottom * g_d));
  const int X = static_cast<int>(std::lround(o.x * g_d)), Y = static_cast<int>(std::lround(o.y * g_d));
  const bool visible = g_mainViewable && !g_ewmhHidden && sw::IsWindowVisible(g_main) && sw::IsWindowVisible(g_tv) &&
                       W > 0 && H > 0 && (!g_xwayland || g_active);
  const bool resized = W != g_viewW || H != g_viewH || (!g_back && W > 0 && H > 0);
  if (resized || X != g_winX || Y != g_winY) {
    {
      Trap trap;
      A.XMoveResizeWindow(g_dpy, g_win, X, Y, static_cast<unsigned>(std::max(W, 1)), static_cast<unsigned>(std::max(H, 1)));
    }
    g_winX = X;
    g_winY = Y;
    g_viewW = W;
    g_viewH = H;
    g_geomSettleUntil = now() + 0.25;
    if (resized) {
      g_cutsSet = false;
      if (W > 0 && H > 0) makeBackBuffer(W, H);
      else dropBackBuffer();
    }
    DEBUGF("layer at %d,%d %dx%d", X, Y, W, H);
  }
  const bool show = visible && g_back;
  if (show != g_mapped) {
    Trap trap;
    if (show) {
      // Stacked first: mapping an override-redirect window keeps its place.
      restack();
      A.XMapWindow(g_dpy, g_win);
      g_fullDirty = true;
    } else {
      A.XUnmapWindow(g_dpy, g_win);
    }
    A.XFlush(g_dpy);
    g_mapped = show;
    DEBUGF("layer %s", show ? "shown" : "hidden");
  }
}

// ---------- presses on the track view ----------
//
// A press is caught where REAPER gets it, in the track view's window
// procedure, subclassed through SWELL as on Windows: sampling the button
// would miss a click shorter than a frame. Unhooked on detach unless someone
// hooked in after us; then the procedure stays in their chain and only passes
// messages on.

WNDPROC g_prevProc = nullptr;  // set while our procedure is in the chain
HWND g_hookedTv = nullptr;
bool g_hookActive = false;
bool g_clickPending = false;
Pt g_clickPoint;  // arrange units

LRESULT trackViewProc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
  const WNDPROC prev = g_prevProc;
  if (g_hookActive && h == g_hookedTv) {
    if (msg == WM_LBUTTONDOWN || msg == WM_LBUTTONDBLCLK) {
      g_clickPending = true;
      g_clickPoint = {static_cast<double>(GET_X_LPARAM(lp)), static_cast<double>(GET_Y_LPARAM(lp))};
    } else if (msg == WM_DESTROY) {
      teardown();
    }
  }
  return prev ? prev(h, msg, wp, lp) : 0;
}

WNDPROC procOf(HWND h) { return reinterpret_cast<WNDPROC>(sw::GetWindowLong(h, GWL_WNDPROC)); }

void hookTrackView(HWND tv) {
  if (!sw::GetWindowLong || !sw::SetWindowLong) return;
  if (g_prevProc) {
    // Still in the chain from an earlier attach: just listen again.
    g_hookActive = g_hookedTv == tv;
    return;
  }
  const WNDPROC cur = procOf(tv);
  if (!cur) return;
  g_prevProc = cur;
  g_hookedTv = tv;
  sw::SetWindowLong(tv, GWL_WNDPROC, reinterpret_cast<LONG_PTR>(&trackViewProc));
  if (procOf(tv) != &trackViewProc) {
    g_prevProc = nullptr;
    g_hookedTv = nullptr;
    return;
  }
  g_hookActive = true;
}

void unhookTrackView() {
  g_hookActive = false;
  g_clickPending = false;
  if (!g_prevProc) return;
  if (!g_hookedTv || !sw::IsWindow(g_hookedTv)) {
    g_prevProc = nullptr;
    g_hookedTv = nullptr;
  } else if (procOf(g_hookedTv) == &trackViewProc) {
    sw::SetWindowLong(g_hookedTv, GWL_WNDPROC, reinterpret_cast<LONG_PTR>(g_prevProc));
    g_prevProc = nullptr;
    g_hookedTv = nullptr;
  }
}

bool openWindow(HWND main) {
  g_gdpy = A.gdk_display_get_default();
  if (!g_gdpy) {
    trace("no GDK display");
    return false;
  }
  if (!A.g_type_check_instance_is_a(g_gdpy, A.gdk_x11_display_get_type())) {
    trace("GDK is not on X11");
    g_gdpy = nullptr;
    return false;
  }
  g_dpy = A.gdk_x11_display_get_xdisplay(g_gdpy);
  void* gw = sw::SWELL_GetOSWindow(main, "GdkWindow");
  g_mainX = gw ? A.gdk_x11_window_get_xid(gw) : 0;
  if (!g_dpy || !g_mainX) {
    trace("no X window for REAPER's main window");
    return false;
  }
  Trap trap;
  XWindowAttributes wa{};
  if (!A.XGetWindowAttributes(g_dpy, g_mainX, &wa)) {
    trace("cannot read REAPER's main X window");
    return false;
  }
  g_rootW = wa.root;
  const int screen = A.XScreenNumberOfScreen(wa.screen);
  int op = 0, ev = 0, er = 0;
  const char* wl = getenv("WAYLAND_DISPLAY");
  g_xwayland = A.XQueryExtension(g_dpy, "XWAYLAND", &op, &ev, &er) || (wl && *wl);
  if (!A.XFixesQueryExtension(g_dpy, &ev, &er)) {
    trace("the X server has no XFixes");
    return false;
  }
  // XWayland windows are always composited by the Wayland compositor.
  if (!g_xwayland && !composited(screen)) {
    trace("no compositing manager: cursors are not drawn");
    return false;
  }
  XVisualInfo vi{};
  if (!A.XMatchVisualInfo(g_dpy, screen, 32, TrueColor, &vi)) {
    trace("no 32-bit visual");
    return false;
  }
  g_visual = vi.visual;
  g_cmap = A.XCreateColormap(g_dpy, g_rootW, g_visual, AllocNone);
  XSetWindowAttributes a{};
  a.override_redirect = True;
  a.background_pixel = 0;
  a.border_pixel = 0;
  a.colormap = g_cmap;
  a.event_mask = 0;
  g_win = A.XCreateWindow(g_dpy, g_rootW, -16, -16, 1, 1, 0, 32, InputOutput, g_visual,
                          CWOverrideRedirect | CWBackPixel | CWBorderPixel | CWColormap | CWEventMask, &a);
  if (!g_win) return false;
  // No input at all: every click and scroll goes to whatever is below.
  XserverRegion none = A.XFixesCreateRegion(g_dpy, nullptr, 0);
  A.XFixesSetWindowShapeRegion(g_dpy, g_win, ShapeInput, 0, 0, none);
  A.XFixesDestroyRegion(g_dpy, none);
  // A drag icon to compositors: no shadow, no animation.
  const Atom type = atom("_NET_WM_WINDOW_TYPE_DND");
  A.XChangeProperty(g_dpy, g_win, atom("_NET_WM_WINDOW_TYPE"), XA_ATOM, 32, PropModeReplace,
                    reinterpret_cast<const unsigned char*>(&type), 1);
  static const char kClass[] = "reamesh-cursors\0ReaMesh";
  A.XChangeProperty(g_dpy, g_win, XA_WM_CLASS, XA_STRING, 8, PropModeReplace,
                    reinterpret_cast<const unsigned char*>(kClass), sizeof kClass);
  static const char kName[] = "ReaMesh cursors";
  A.XChangeProperty(g_dpy, g_win, atom("_NET_WM_NAME"), atom("UTF8_STRING"), 8, PropModeReplace,
                    reinterpret_cast<const unsigned char*>(kName), sizeof kName - 1);
  g_gc = A.XCreateGC(g_dpy, g_win, 0, nullptr);
  if (trap.check() != 0 || !g_gc) {
    trace("creating the layer window failed");
    return false;
  }
  g_cmCheckedAt = now();
  DEBUGF("layer window 0x%lx on screen %d%s", g_win, screen, g_xwayland ? " (XWayland)" : "");
  return true;
}

void closeWindow() {
  dropBackBuffer();
  if (g_dpy && g_gdpy) {
    Trap trap;
    if (g_gc) A.XFreeGC(g_dpy, g_gc);
    if (g_win) A.XDestroyWindow(g_dpy, g_win);
    if (g_cmap) A.XFreeColormap(g_dpy, g_cmap);
    A.XFlush(g_dpy);
  }
  g_gc = nullptr;
  g_win = 0;
  g_cmap = 0;
  g_mapped = false;
  g_frame = 0;
  g_frameCheckedAt = -1;
  g_cuts.clear();
  g_cutsSet = false;
  g_cutsAt = g_ewmhCheckedAt = -1;
  g_ewmhHidden = false;
  g_winX = g_winY = g_viewW = g_viewH = 0;
}

// ---------- frames ----------

struct Mat {
  double a = 1, b = 0, c = 0, d = 1, e = 0, f = 0;  // x' = a x + c y + e, y' = b x + d y + f
  Mat operator*(const Mat& o) const {
    return {a * o.a + c * o.b, b * o.a + d * o.b, a * o.c + c * o.d, b * o.c + d * o.d, a * o.e + c * o.f + e,
            b * o.e + d * o.f + f};
  }
  static Mat translate(double x, double y) { return {1, 0, 0, 1, x, y}; }
  static Mat scale(double x, double y) { return {x, 0, 0, y, 0, 0}; }
  static Mat rotate(double rad) {
    const double cs = std::cos(rad), sn = std::sin(rad);
    return {cs, sn, -sn, cs, 0, 0};
  }
  Pt apply(Pt p) const { return {a * p.x + c * p.y + e, b * p.x + d * p.y + f}; }
  bool near(const Mat& o) const {
    return std::fabs(a - o.a) < 1e-3 && std::fabs(b - o.b) < 1e-3 && std::fabs(c - o.c) < 1e-3 &&
           std::fabs(d - o.d) < 1e-3 && std::fabs(e - o.e) < 0.01 && std::fabs(f - o.f) < 0.01;
  }
};

// One thing to draw in a frame.
struct Op {
  int id = 0;
  unsigned ver = 0;
  Mat m;
  double alpha = 1;
  Box bb;  // pixels it can touch, in the window
  int pred = 0;  // the op drawn just before (an order change redraws)
  Node n = nullptr;  // valid while the frame is being built
};
std::vector<Op> g_prevOps;

Box boundsOf(const Mat& m, double x0, double y0, double x1, double y1) {
  const Pt c[4] = {m.apply({x0, y0}), m.apply({x1, y0}), m.apply({x0, y1}), m.apply({x1, y1})};
  double lx = c[0].x, hx = c[0].x, ly = c[0].y, hy = c[0].y;
  for (const Pt& p : c) {
    lx = std::min(lx, p.x), hx = std::max(hx, p.x);
    ly = std::min(ly, p.y), hy = std::max(hy, p.y);
  }
  lx = std::floor(lx) - 1, ly = std::floor(ly) - 1, hx = std::ceil(hx) + 1, hy = std::ceil(hy) + 1;
  return {lx, ly, hx - lx, hy - ly};
}

Box intersect(const Box& a, const Box& b) {
  const double x0 = std::max(a.x, b.x), y0 = std::max(a.y, b.y);
  const double x1 = std::min(a.x + a.w, b.x + b.w), y1 = std::min(a.y + a.h, b.y + b.h);
  return {x0, y0, std::max(0.0, x1 - x0), std::max(0.0, y1 - y0)};
}

bool overlaps(const Box& a, const Box& b, double slack = 0) {
  return a.x < b.x + b.w + slack && b.x < a.x + a.w + slack && a.y < b.y + b.h + slack && b.y < a.y + a.h + slack;
}

Box unite(const Box& a, const Box& b) {
  const double x0 = std::min(a.x, b.x), y0 = std::min(a.y, b.y);
  return {x0, y0, std::max(a.x + a.w, b.x + b.w) - x0, std::max(a.y + a.h, b.y + b.h) - y0};
}

void collect(SNode* n, const Mat& parent, double alpha, double t, const Box& view, std::vector<Op>* ops, bool* anim) {
  if (n->doomAt >= 0) {
    *anim = true;  // removed soon: frames until then
    if (t >= n->doomAt) return;
  }
  const double a = alpha * std::clamp(n->op.value(t), 0.0, 1.0);
  if (n->op.running(t) || n->dx.running(t) || n->dy.running(t) || n->sx.running(t) || n->sy.running(t)) *anim = true;
  if (a <= 0.002) return;
  const double sx = n->sx.value(t), sy = n->sy.value(t);
  Mat local = Mat::translate(n->ox + n->dx.value(t), n->oy + n->dy.value(t));
  if (n->angle != 0) local = local * Mat::rotate(n->angle * kPi / 180);
  if (sx != 1 || sy != 1) local = local * Mat::translate(n->cx, n->cy) * Mat::scale(sx, sy) * Mat::translate(-n->cx, -n->cy);
  const Mat m = parent * local;
  if (n->img || n->hasBox) {
    Op op;
    op.id = n->id;
    op.ver = n->ver;
    op.alpha = a;
    op.n = n;
    if (n->img) {
      op.m = m * Mat::translate(n->imgX, n->imgY);
      op.bb = boundsOf(op.m, 0, 0, n->imgW, n->imgH);
    } else {
      op.m = m;
      const double pad = S(n->box.line) + 1;
      op.bb = boundsOf(op.m, -pad, -pad, n->box.w + pad, n->box.h + pad);
    }
    op.bb = intersect(op.bb, view);
    if (op.bb.w > 0 && op.bb.h > 0) {
      op.pred = ops->empty() ? 0 : ops->back().id;
      ops->push_back(op);
    }
  }
  for (auto& k : n->kids) collect(k.get(), m, a, t, view, ops, anim);
}

void drawOp(cairo_t* cr, const Op& op) {
  const cairo_matrix_t m{op.m.a, op.m.b, op.m.c, op.m.d, op.m.e, op.m.f};
  A.cairo_save(cr);
  A.cairo_set_matrix(cr, &m);
  if (op.n->img) {
    A.cairo_rectangle(cr, 0, 0, op.n->imgW, op.n->imgH);
    A.cairo_clip(cr);
    A.cairo_set_source_surface(cr, op.n->img, 0, 0);
    A.cairo_pattern_set_filter(A.cairo_get_source(cr), CAIRO_FILTER_BILINEAR);
    A.cairo_paint_with_alpha(cr, op.alpha);
  } else {
    paintBox(cr, op.n->box, op.alpha);
  }
  A.cairo_restore(cr);
}

// Debugging aid: the frame as a PNG (REAMESH_CURSOR_DUMP), at most twice a
// second; a frame drawn in between is written by a later tick.
const char* g_dumpPath = getenv("REAMESH_CURSOR_DUMP");
double g_dumpedAt = -1;
bool g_dumpPending = false;

void dumpFrame(double t) {
  if (!g_dumpPending || !g_back || t - g_dumpedAt < 0.5) return;
  g_dumpPending = false;
  g_dumpedAt = t;
  A.cairo_surface_write_to_png(g_back, g_dumpPath);
}

// Draws what changed since the last frame and sends it to the window.
void render() {
  if (!g_attached) return;
  syncWindow();
  const double t = now();
  // With a dump file set, frames are drawn while hidden too (not sent).
  const bool draw = g_mapped || (g_dumpPath && *g_dumpPath);
  if (!draw || !g_back || !g_ximage) {
    g_prevOps.clear();
    g_fullDirty = true;
    g_animating = false;
    return;
  }
  const Box view{0, 0, static_cast<double>(g_viewW), static_cast<double>(g_viewH)};
  std::vector<Op> ops;
  ops.reserve(g_prevOps.size() + 8);
  bool anim = false;
  collect(g_scene.get(), Mat{}, 1, t, view, &ops, &anim);
  g_animating = anim;

  std::vector<Box> dirty;
  if (g_fullDirty) {
    dirty.push_back(view);
  } else {
    std::map<int, const Op*> prev;
    for (const Op& o : g_prevOps) prev[o.id] = &o;
    for (const Op& o : ops) {
      auto it = prev.find(o.id);
      if (it == prev.end()) {
        dirty.push_back(o.bb);
        continue;
      }
      const Op& p = *it->second;
      if (p.ver != o.ver || !p.m.near(o.m) || std::fabs(p.alpha - o.alpha) > 0.002 || p.pred != o.pred) {
        dirty.push_back(p.bb);
        dirty.push_back(o.bb);
      }
      prev.erase(it);
    }
    for (const auto& [id, p] : prev) dirty.push_back(p->bb);
  }
  g_prevOps = ops;
  g_fullDirty = false;
  // Merge what touches; past a few dozen pieces one box is cheaper.
  for (bool merged = true; merged;) {
    merged = false;
    for (size_t i = 0; i < dirty.size() && !merged; ++i)
      for (size_t j = i + 1; j < dirty.size(); ++j)
        if (overlaps(dirty[i], dirty[j], 4)) {
          dirty[i] = unite(dirty[i], dirty[j]);
          dirty.erase(dirty.begin() + static_cast<long>(j));
          merged = true;
          break;
        }
  }
  if (dirty.size() > 24) {
    Box all = dirty[0];
    for (const Box& b : dirty) all = unite(all, b);
    dirty.assign(1, all);
  }
  for (Box& b : dirty) {
    b = intersect(b, view);
    b.x = std::floor(b.x), b.y = std::floor(b.y), b.w = std::ceil(b.w), b.h = std::ceil(b.h);
  }
  dirty.erase(std::remove_if(dirty.begin(), dirty.end(), [](const Box& b) { return b.w <= 0 || b.h <= 0; }),
              dirty.end());
  if (dirty.empty()) return;

  cairo_t* cr = A.cairo_create(g_back);
  for (const Box& b : dirty) A.cairo_rectangle(cr, b.x, b.y, b.w, b.h);
  A.cairo_clip(cr);
  A.cairo_set_operator(cr, CAIRO_OPERATOR_CLEAR);
  A.cairo_paint(cr);
  A.cairo_set_operator(cr, CAIRO_OPERATOR_OVER);
  for (const Op& o : ops) {
    bool hit = false;
    for (const Box& b : dirty) hit = hit || overlaps(o.bb, b);
    if (hit) drawOp(cr, o);
  }
  A.cairo_destroy(cr);
  A.cairo_surface_flush(g_back);
  if (!g_mapped) {
    g_fullDirty = true;  // sent whole once shown
  } else {
    Trap trap;
    for (const Box& b : dirty) {
      const int x = static_cast<int>(b.x), y = static_cast<int>(b.y);
      A.XPutImage(g_dpy, g_win, g_gc, g_ximage, x, y, x, y, static_cast<unsigned>(b.w), static_cast<unsigned>(b.h));
    }
    A.XFlush(g_dpy);
  }
  if (g_dumpPath && *g_dumpPath) g_dumpPending = true;
  dumpFrame(t);
}

bool wantFrames(double t) {
  if (!g_attached) return false;
  // The local pointer moving: sampled (and sent) at frame rate, shown or not.
  if (t - g_localMovedAt < 0.5) return true;
  if (!g_mapped && !(g_dumpPath && *g_dumpPath)) return false;
  if (g_animating || t < g_viewSettleUntil || t < g_marksSettleUntil || t < g_geomSettleUntil) return true;
  if (t - g_newsAt < 0.4) return true;
  for (const auto& [id, p] : g_peers)
    if (p.pendingSpring || p.pendingClick || t < p.morph.start + p.morph.dur + 0.1) return true;
  return false;
}

void frame();

void onFrameTimer(HWND, UINT, UINT_PTR, DWORD) {
  try {
    frame();
  } catch (...) {
  }
}

void stopFrames() {
  if (!g_timer) return;
  sw::KillTimer(nullptr, g_timer);
  g_timer = 0;
  const double d = now() - g_framesSince;
  DEBUGF("frames stop: %d in %.2fs (%.1f/s)", g_framesCounted, d, d > 0 ? g_framesCounted / d : 0.0);
}

// Frames at display rate while something moves; otherwise the extension's
// tick is enough.
void scheduleFrames() {
  if (g_timer || !wantFrames(now())) return;
  g_timer = sw::SetTimer(nullptr, 0, kFrameMs, onFrameTimer);
  g_framesCounted = 0;
  g_framesSince = now();
  DEBUGF("frames start");
}

// One frame: network in/out, layout, drawing.
void frame() {
  if (!g_attached) {
    stopFrames();
    return;
  }
  if (g_host.pump) {
    const double t = now();
    if (t - g_lastPump >= 0.004) {
      g_lastPump = t;
      g_host.pump();
    }
  }
  if (!g_attached) return;  // the pump may have ended the session
  const double t = now();
  ++g_framesCounted;
  sweepDoomed(g_scene.get(), t);
  stepMorphs(t);
  if (!g_peers.empty() || !g_marks.empty()) {
    // Updates stopped: the pointer came to rest, so take the prediction back.
    for (auto& [id, p] : g_peers)
      if (p.predict && t - p.lastMsg > kStyle.predictStopAfter) {
        p.predict = false;
        p.vt = p.vfy = 0;
        p.pendingSpring = true;
      }
  }
  glue();
  render();
  if (!wantFrames(now())) stopFrames();
}

void buildScene() {
  g_scene = std::make_unique<SNode>();
  g_scene->id = g_nextNodeId++;
  g_markN = makeNode(g_scene.get());
  // The follow frame sits above the marks and below every cursor.
  g_follow = makeNode(g_scene.get());
  for (Node& e : g_followEdge) e = makeNode(g_follow);
  g_followTag = makeNode(g_follow);
  setOpacityNow(g_follow, 0);
}

void releaseScene() {
  g_hover = nullptr;
  g_follow = g_followTag = nullptr;
  for (Node& e : g_followEdge) e = nullptr;
  g_markN = nullptr;
  g_scene.reset();
  g_prevOps.clear();
}

// Off the track view and every peer forgotten. g_main stays, so tick() can
// attach again; detach() clears it.
void teardown() {
  if (!g_attached) return;
  g_attached = false;
  stopFrames();
  g_peers.clear();
  g_marks.clear();
  g_marksDirty = false;
  g_hoverShown = false;
  g_localIn = false;
  g_localDown = false;
  g_followKey.clear();
  g_followName.clear();
  g_followColor.clear();
  g_followId.clear();
  unhookTrackView();
  releaseScene();
  closeWindow();
  g_tv = nullptr;
}

void readReduceMotion() {
  void* screen = A.gdk_screen_get_default();
  if (!screen) return;
  GValueLike v;
  A.g_value_init(&v, kGTypeBoolean);
  if (A.gdk_screen_get_setting(screen, "gtk-enable-animations", &v)) g_reduceMotion = !A.g_value_get_boolean(&v);
  A.g_value_unset(&v);
}

}  // namespace

namespace cursor_overlay {

bool attach(void* mainWindow, const Host& host) {
  if (g_attached) return true;
  // A failed attach opens and closes an X window: not on every tick.
  if (now() < g_attachRetryAt) return false;
  g_attachRetryAt = now() + 5;
  const auto fail = [](const char* why) {
    if (!g_attachFailedLogged && why) trace("%s", why);
    g_attachFailedLogged = true;
    return false;
  };
  if (!loadSwell()) return fail("SWELL's functions are not available");
  if (!loadApi()) return fail("cursors are not drawn");
  HWND main = static_cast<HWND>(mainWindow);
  HWND tv = main ? sw::GetDlgItem(main, 1000) : nullptr;
  if (!tv) return fail("no track view");
  g_host = host;
  g_main = main;
  if (!openWindow(main)) {
    closeWindow();
    return fail(nullptr);
  }
  if (!g_measureCr) {
    g_measureSurf = A.cairo_image_surface_create(CAIRO_FORMAT_ARGB32, 1, 1);
    g_measureCr = A.cairo_create(g_measureSurf);
    g_fontOptions = A.cairo_font_options_create();
    A.cairo_font_options_set_antialias(g_fontOptions, CAIRO_ANTIALIAS_GRAY);
    A.cairo_font_options_set_hint_style(g_fontOptions, CAIRO_HINT_STYLE_SLIGHT);
    A.cairo_font_options_set_hint_metrics(g_fontOptions, CAIRO_HINT_METRICS_OFF);
    readUiFont();
    readCapHeight();
    DEBUGF("UI font '%s', cap height %.3f em", g_fontName.c_str(), g_capPerEm);
  }
  g_tv = tv;
  g_attachRetryAt = 0;
  g_attachFailedLogged = false;
  g_clipW = g_clipH = -1;
  g_active = true;
  g_mainViewable = true;
  g_fullDirty = true;
  g_reduceMotionChecked = 0;
  buildScene();
  hookTrackView(tv);
  DEBUGF("track view %s", g_hookActive ? "subclassed" : "not subclassed: presses are sampled");
  g_attached = true;
  checkWindows();
  if (!g_attached) return false;
  checkScale();
  glue();
  render();
  scheduleFrames();
  return true;
}

void detach() {
  g_main = nullptr;
  g_attachRetryAt = 0;
  teardown();
}

bool draws() { return g_attached; }

void setShowCursors(bool on) {
  if (on == g_showCursors) return;
  g_showCursors = on;
  if (on || !g_attached) return;
  for (auto& [id, p] : g_peers) dropLayers(p);
  g_peers.clear();
  render();
}

void setFollow(const std::string& id, const std::string& name, const std::string& color) {
  if (!g_attached) return;
  const std::string who = name.empty() ? "" : id;
  if (who != g_followId) {
    if (auto it = g_peers.find(g_followId); it != g_peers.end()) setFollowed(it->second, false);
    g_followId = who;
    if (auto it = g_peers.find(g_followId); it != g_peers.end()) setFollowed(it->second, true);
  }
  const std::string key = name.empty() ? "" : name + color;
  if (key == g_followKey) return;
  const bool was = !g_followKey.empty();
  g_followKey = key;
  g_followName = name;
  g_followColor = color;
  if (key.empty()) {
    if (was) fade(g_follow, 0, kStyle.fadeOut);
  } else {
    layoutFollow();
    if (!was) fade(g_follow, kStyle.followOpacity, kStyle.fadeIn * 2);
  }
  render();
  scheduleFrames();
}

void setMarks(const std::vector<Mark>& marks) {
  if (!g_attached) return;
  std::map<std::string, const Mark*> want;
  for (const Mark& m : marks)
    if (!m.item.empty()) want.emplace(m.item, &m);
  const double t = now();
  for (auto it = g_marks.begin(); it != g_marks.end();) {
    if (want.count(it->first) || t - it->second.lastWanted < kStyle.markLinger) {
      ++it;
      continue;
    }
    fade(it->second.box, 0, kStyle.markFadeOut);
    dropLater(it->second.box, t + kStyle.markFadeOut + 0.05);
    it = g_marks.erase(it);
    g_marksDirty = true;
  }
  for (const auto& [item, m] : want) {
    auto it = g_marks.find(item);
    if (it == g_marks.end()) {
      MarkState ms;
      ms.m = *m;
      ms.box = makeNode(g_markN, Z::Bottom);
      setOpacityNow(ms.box, 0);
      ms.lastWanted = t;
      g_marks.emplace(item, std::move(ms));
      g_marksDirty = true;
      continue;
    }
    it->second.lastWanted = t;
    if (it->second.m.color != m->color || it->second.m.name != m->name || it->second.m.loading != m->loading) {
      it->second.m = *m;
      if (g_hover) g_hover->look.clear();
      g_marksDirty = true;
    }
  }
  // Called right after the tick applied other people's edits, which move
  // items without touching anything the cheap check watches.
  if (!g_marks.empty()) g_marksDirty = true;
  if (g_marksDirty) glue();
  render();
  scheduleFrames();
}

void update(const Remote& r) {
  if (!g_attached || !g_showCursors || r.id.empty()) return;
  auto it = g_peers.find(r.id);
  if (it == g_peers.end()) {
    Peer p;
    p.id = r.id;
    p.color = r.color;
    p.name = r.name;
    it = g_peers.emplace(r.id, std::move(p)).first;
    buildLayers(it->second);
  }
  Peer& p = it->second;
  const double t = now();
  if (r.color != p.color && !r.color.empty()) p.color = r.color;
  if (!r.name.empty()) p.name = r.name;
  refreshTag(p);
  const bool moved = r.t != p.t || r.guid != p.guid || r.fy != p.fy || r.in != p.in;
  // Velocity for the prediction, only between close updates on one track.
  const double dt = t - p.lastMsg;
  if (r.in && p.in && r.guid == p.guid && dt > 0.004 && dt < 0.25) {
    p.vt = 0.5 * p.vt + 0.5 * (r.t - p.t) / dt;
    p.vfy = 0.5 * p.vfy + 0.5 * (r.fy - p.fy) / dt;
    p.predict = moved;
  } else {
    p.vt = p.vfy = 0;
    p.predict = false;
  }
  setPressed(p, r.in && r.down);
  setGlyph(p, r.in ? r.tool : "");
  const bool dragChanged = r.drag.items != p.drag.items || r.drag.dt != p.drag.dt || r.drag.dtr != p.drag.dtr;
  p.drag = r.drag;
  if (dragChanged) p.pendingSpring = true;
  p.t = r.t;
  p.fy = r.fy;
  p.guid = r.guid;
  p.in = r.in;
  p.lastMsg = t;
  if (moved || r.click) p.lastMove = t;
  p.pendingSpring = p.pendingSpring || moved;
  p.pendingClick = p.pendingClick || r.click;
  if (p.pendingSpring || p.pendingClick || r.down) g_newsAt = t;
}

void tick() {
  // Detached because REAPER's windows went away: attach again (throttled);
  // meanwhile draws() is false and the extension tints.
  if (!g_attached && g_main) attach(g_main, g_host);
  if (!g_attached) return;
  const double t = now();
  if (t - g_reduceMotionChecked > 2) {
    g_reduceMotionChecked = t;
    readReduceMotion();
  }
  // The compositing manager stopping turns the layer black: back to the tint
  // until one runs again.
  if (!g_xwayland && t - g_cmCheckedAt > 2) {
    g_cmCheckedAt = t;
    bool cm = true;
    {
      Trap trap;
      XWindowAttributes wa{};
      if (A.XGetWindowAttributes(g_dpy, g_rootW, &wa)) cm = composited(A.XScreenNumberOfScreen(wa.screen));
    }
    if (!cm) {
      trace("the compositing manager stopped; cursors are not drawn");
      teardown();
      return;
    }
  }
  checkWindows();
  if (!g_attached) return;
  // Network updates that arrived through the extension's timer rather than
  // this layer's pump land here.
  glue(true);
  if (!g_attached) return;
  sweepDoomed(g_scene.get(), t);
  for (auto it = g_peers.begin(); it != g_peers.end();) {
    Peer& p = it->second;
    if (t - p.lastMsg > kStyle.forgetAfter) {
      dropLayers(p);
      it = g_peers.erase(it);
      continue;
    }
    // Gone quiet (or back): lay it out again, the glue pass skips peers
    // without news.
    const bool fresh = t - p.lastMsg <= kStyle.staleAfter;
    if (fresh != p.fresh) {
      p.fresh = fresh;
      p.pendingSpring = true;
    }
    updateTag(p, t);
    // A cursor resting a while dims to half; the person we follow never
    // dims.
    const bool idle = p.cursorShown && !p.followed && t - p.lastMove > kStyle.idleDimAfter;
    if (idle != p.idle) {
      fade(p.arrow, idle ? kStyle.idleOpacity : 1, idle ? kStyle.idleFade : kStyle.fadeIn);
      p.idle = idle;
    }
    ++it;
  }
  // Between frames the view (scroll, zoom, track heights) is watched here.
  if (!g_timer) glue();
  render();
  dumpFrame(t);
  scheduleFrames();
}

bool viewSize(double* w, double* h) {
  if (!g_attached) return false;
  RECT rc{};
  sw::GetClientRect(g_tv, &rc);
  *w = rc.right - rc.left;
  *h = rc.bottom - rc.top;
  return *w > 0 && *h > 0;
}

double pixelScale() { return g_attached ? g_ui : 1; }

// The logical left button (X applies the button mapping).
bool mouseHeld() {
  if (g_attached) {
    const PointerState p = queryPointer();
    if (p.ok) return (p.mask & Button1Mask) != 0;
  }
  if (loadSwell() && sw::GetAsyncKeyState) return (sw::GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0;
  return false;
}

// ---------- what REAPER's own cursor shows ----------
//
// REAPER sets its cursors from the .cur files in its Resources folder, as
// SWELL cursors (GdkCursors) it loads once and reuses. The one it shows is
// found by its handle (SWELL_GetCursor); a handle seen for the first time is
// matched once against those files by the X server's image of the current
// cursor (XFixes): its opaque pixels on a 32 x 32 grid and its hotspot, as
// the Mac matches NSCursor images.

struct CursorRef {
  std::string key;
  std::vector<uint8_t> masks[2];  // opaque pixels: AND mask clear; and AND clear or XOR set
  double hx = 0, hy = 0;          // in the 32 x 32 grid
};
std::vector<CursorRef> g_cursorRefs;
bool g_cursorRefsLoaded = false;
// What each SWELL cursor was matched to. A match is final; "none of REAPER's
// cursors" is checked again after a while, in case the X server still showed
// another cursor then (it follows REAPER's cursor only while REAPER has the
// focus).
struct CursorKey {
  std::string key;
  double at = 0;
};
std::map<HCURSOR, CursorKey> g_cursorKeys;
const bool g_debugCursor = [] {
  const char* v = getenv("REAMESH_DEBUG_CURSOR");
  return v && *v && strcmp(v, "0") != 0;
}();

// A 32 x 32 grid from a w x h image, nearest pixel.
template <class Opaque>
std::vector<uint8_t> grid32(int w, int h, Opaque opaque) {
  std::vector<uint8_t> g(32 * 32, 0);
  for (int y = 0; y < 32; ++y)
    for (int x = 0; x < 32; ++x) g[y * 32 + x] = opaque(std::min(w - 1, x * w / 32), std::min(h - 1, y * h / 32)) ? 1 : 0;
  return g;
}

// One .cur file (the first image of it, an uncompressed DIB).
bool readCur(const std::string& path, CursorRef* out) {
  std::ifstream f(path, std::ios::binary);
  const std::vector<uint8_t> b((std::istreambuf_iterator<char>(f)), std::istreambuf_iterator<char>());
  const auto u16 = [&b](size_t o) { return o + 2 <= b.size() ? static_cast<unsigned>(b[o] | b[o + 1] << 8) : 0u; };
  const auto u32 = [&b](size_t o) {
    return o + 4 <= b.size() ? static_cast<uint32_t>(b[o] | b[o + 1] << 8 | b[o + 2] << 16 | static_cast<uint32_t>(b[o + 3]) << 24)
                             : 0u;
  };
  if (b.size() < 22 || u16(0) != 0 || u16(2) != 2 || u16(4) < 1) return false;
  const double dw = b[6] ? b[6] : 256, dh = b[7] ? b[7] : 256;
  const size_t off = u32(18);
  if (off + 40 > b.size() || u32(off) < 40) return false;
  const int w = static_cast<int>(static_cast<int32_t>(u32(off + 4)));
  const int h2 = static_cast<int>(static_cast<int32_t>(u32(off + 8)));
  const int bpp = static_cast<int>(u16(off + 14));
  if (u32(off + 16) != 0 || w <= 0 || w > 256 || h2 == 0) return false;  // compressed / PNG
  const bool bottomUp = h2 > 0;
  const int h = std::abs(h2) / 2;
  if (h <= 0 || h > 256 || (bpp != 1 && bpp != 4 && bpp != 8 && bpp != 24 && bpp != 32)) return false;
  const size_t colors = bpp <= 8 ? (u32(off + 32) ? u32(off + 32) : (1u << bpp)) : 0;
  const size_t pal = off + u32(off), xorOff = pal + 4 * colors;
  const size_t xorStride = ((static_cast<size_t>(w) * bpp + 31) / 32) * 4, andStride = ((static_cast<size_t>(w) + 31) / 32) * 4;
  const size_t andOff = xorOff + xorStride * h;
  if (andOff + andStride * h > b.size()) return false;
  const auto row = [&](int y) { return bottomUp ? h - 1 - y : y; };
  const auto andBit = [&](int x, int y) { return (b[andOff + row(y) * andStride + x / 8] >> (7 - x % 8)) & 1; };
  const auto xorSet = [&](int x, int y) {
    const size_t r = xorOff + row(y) * xorStride;
    uint32_t c = 0;
    if (bpp <= 8) {
      const unsigned idx = (b[r + x * bpp / 8] >> (8 - bpp - (x * bpp) % 8)) & ((1u << bpp) - 1);
      c = idx < colors ? u32(pal + 4 * idx) & 0xffffff : 0;
    } else {
      const size_t p = r + static_cast<size_t>(x) * (bpp / 8);
      c = static_cast<uint32_t>(b[p] | b[p + 1] << 8 | b[p + 2] << 16);
    }
    return c != 0;
  };
  out->masks[0] = grid32(w, h, [&](int x, int y) { return !andBit(x, y); });
  out->masks[1] = grid32(w, h, [&](int x, int y) { return !andBit(x, y) || xorSet(x, y); });
  out->hx = u16(10) * 32 / dw;
  out->hy = u16(12) * 32 / dh;
  return true;
}

void loadCursorRefs() {
  g_cursorRefsLoaded = true;
  char exe[4096];
  const ssize_t n = readlink("/proc/self/exe", exe, sizeof exe - 1);
  if (n <= 0) return;
  exe[n] = 0;
  std::string dir = exe;
  dir.resize(dir.find_last_of('/') + 1);
  dir += "Resources/";
  for (const auto& c : cursor_glyphs::kCursorNames) {
    CursorRef r;
    if (!readCur(dir + c.file + ".cur", &r)) continue;
    r.key = c.key;
    g_cursorRefs.push_back(std::move(r));
  }
  if (g_cursorRefs.empty()) trace("no REAPER cursor files found in %s", dir.c_str());
}

// The X server's current cursor image: its glyph key and serial (which
// changes with the cursor shown). False when there is none.
bool keyOfShownCursor(std::string* out, unsigned long* serial) {
  XFixesCursorImage* ci = nullptr;
  {
    Trap trap;
    ci = A.XFixesGetCursorImage(g_dpy);
  }
  if (!ci) return false;
  *serial = ci->cursor_serial;
  std::string key;
  if (ci->width > 0 && ci->height > 0 && ci->pixels) {
    const int w = ci->width, h = ci->height;
    const std::vector<uint8_t> m =
        grid32(w, h, [ci, w](int x, int y) { return ((ci->pixels[y * w + x] >> 24) & 0xff) >= 128; });
    const double hx = ci->xhot * 32.0 / w, hy = ci->yhot * 32.0 / h;
    int best = 40;  // of 1024 pixels: a rounding difference, never another cursor
    for (const auto& r : g_cursorRefs) {
      if (std::fabs(r.hx - hx) > 1.5 || std::fabs(r.hy - hy) > 1.5) continue;
      for (const auto& rm : r.masks) {
        int diff = 0;
        for (size_t i = 0; i < m.size(); ++i) diff += m[i] != rm[i];
        if (diff < best) {
          best = diff;
          key = r.key;
        }
      }
    }
  }
  if (g_debugCursor)
    trace("cursor serial %lu %ux%u hot %u,%u -> '%s'", ci->cursor_serial, ci->width, ci->height, ci->xhot, ci->yhot,
          key.c_str());
  A.XFree(ci);
  *out = key;
  return true;
}

// The X cursor last matched for each SWELL cursor, by serial: a SWELL cursor
// is only matched once the X server shows a cursor no other SWELL cursor was
// matched to (SWELL may hand the new cursor to the window a moment later).
std::map<unsigned long, HCURSOR> g_cursorSerials;

std::string currentCursorKey() {
  if (!sw::SWELL_GetCursor) return "";
  if (!g_cursorRefsLoaded) loadCursorRefs();
  HCURSOR c = sw::SWELL_GetCursor();
  if (!c) return "";
  const double t = now();
  auto it = g_cursorKeys.find(c);
  if (it != g_cursorKeys.end() && (!it->second.key.empty() || t - it->second.at < 2)) return it->second.key;
  std::string key;
  unsigned long serial = 0;
  if (!keyOfShownCursor(&key, &serial)) return "";
  auto s = g_cursorSerials.find(serial);
  if (s != g_cursorSerials.end() && s->second != c) return "";  // still the previous cursor on screen
  if (g_cursorKeys.size() > 256) {
    g_cursorKeys.clear();
    g_cursorSerials.clear();
  }
  g_cursorSerials[serial] = c;
  g_cursorKeys[c] = {key, t};
  if (g_debugCursor) trace("SWELL cursor %p is '%s'", static_cast<void*>(c), key.c_str());
  return key;
}

bool sampleLocal(Local* out) {
  if (!g_attached) return false;
  *out = Local{};
  const Geo g = readGeo();
  if (!g.ok) return true;
  // Only when the track view itself is under the pointer: a floating window
  // (REAPER's or another application's) over the arrange covers it.
  const PointerState ptr = queryPointer();
  const Pt vp{static_cast<double>(ptr.x - g_winX), static_cast<double>(ptr.y - g_winY)};
  const bool over = ptr.ok && ptr.child && ptr.child == g_frame && g_mainViewable && sw::IsWindowVisible(g_tv) &&
                    vp.x >= 0 && vp.y >= 0 && vp.x < g_viewW && vp.y < g_viewH;
  const bool down = ptr.ok && (ptr.mask & Button1Mask) != 0;
  const bool pressed = down && !g_localDown;
  g_localDown = down;
  // Hovering a marked item shows who is editing it.
  if (over != g_localIn || (over && !samePoint(vp, g_localPoint))) {
    g_localIn = over;
    g_localPoint = vp;
    g_localMovedAt = now();
    if (!g_marks.empty() || g_hoverShown) {
      layoutHover();
      render();
    }
    scheduleFrames();
  }
  if (!over) {
    g_clickPending = false;
    return true;
  }
  Pt at = vp;
  if (g_hookActive) {
    if (g_clickPending) {
      at = {g_clickPoint.x * g_d, g_clickPoint.y * g_d};
      out->click = true;
      g_clickPending = false;
    }
  } else {
    out->click = pressed;
  }
  out->in = semanticAt(at, g, &out->t, &out->guid, &out->fy);
  out->down = out->in && down;
  out->px = at.x / g_d;
  out->py = at.y / g_d;
  if (out->in) out->tool = currentCursorKey();
  return true;
}

}  // namespace cursor_overlay

// SWELL hands a plug-in its functions through this when it loads it: getFunc
// returns any SWELL function by name. 1 keeps the plug-in loaded.
extern "C" __attribute__((visibility("default"))) int SWELL_dllMain(HINSTANCE, DWORD callMode, LPVOID getFunc) {
  if (callMode == DLL_PROCESS_ATTACH && getFunc) g_swellGet = reinterpret_cast<SwellGetFunc>(getFunc);
  return 1;
}
