// Windows cursor layer: DirectComposition visuals on top of REAPER's track view
// (see cursor_overlay.h), the counterpart of cursor_overlay_mac.mm with the same
// style table, so both platforms look alike. Draws other people's cursors and
// an outline around every item someone else is editing.
//
// The visual tree is bound to the track view itself as a topmost composition
// target: there is no window of our own, so input keeps going to REAPER and the
// tree is clipped like the track view (a floating window on top covers it).
// Content is drawn with Direct2D/DirectWrite into composition surfaces at the
// window's DPI, once per look.
//
// Motion is split in two, as on the Mac:
//  - each visual's offset is the exact pixel of the peer's semantic spot under
//    the current zoom/scroll. It is rewritten right after REAPER's own WM_PAINT
//    of the track view (a window subclass) and committed there, so it lands in
//    the frame REAPER repaints;
//  - each network update starts a critically damped spring on top, as a
//    translate transform played by the compositor from cubic segments. It
//    keeps moving at display rate even while REAPER's main thread is busy.
// DirectComposition can't report where an animation is now, so every animated
// value is also a Curve evaluated here; a new animation starts from the old
// one's current value and velocity, like Core Animation's presentation layer.
//
// Everything here runs on REAPER's main thread.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <windowsx.h>
#include <commctrl.h>
#include <d2d1_1.h>
#include <d2d1_1helper.h>
#include <d2d1effects.h>
#include <d3d11.h>
#include <dcomp.h>
#include <dwrite.h>
#include <wrl/client.h>

#include <algorithm>
#include <cmath>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <map>
#include <set>
#include <string>
#include <utility>
#include <vector>

#include "cursor_overlay.h"

using Microsoft::WRL::ComPtr;

namespace {

constexpr double kPi = 3.14159265358979323846;

// ---------- every visual parameter, in one table (as cursor_overlay_mac.mm) ----------
//
// Lengths are in DIPs (1/96 inch, the Mac's points); the layer multiplies them
// by the track view's DPI scale.

struct Style {
  double pointerSize;
  double pointerNotch;
  double tipRadius, wingRadius, notchRadius;
  double outline;
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
  double parkInset, parkItemInset;
  double dragLine, dragDash, dragGap, dragFill;
  double revertHold, revertGlide, revertFade, revertOpacity;
  double followLine, followOpacity, followTagInset;
  double haloRadius, haloCentre, haloFill, haloLine, haloLineOpacity;
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
    .parkInset = 8,
    .parkItemInset = 10,
    .dragLine = 1.5,
    .dragDash = 5,
    .dragGap = 4,
    .dragFill = 0.16,
    .revertHold = 0.25,
    .revertGlide = 0.45,
    .revertFade = 0.3,
    .revertOpacity = 0.85,
    .followLine = 2,
    .followOpacity = 0.9,
    .followTagInset = 8,
    .haloRadius = 15,
    .haloCentre = 8,
    .haloFill = 0.16,
    .haloLine = 1.5,
    .haloLineOpacity = 0.45,
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

// ---------- basics ----------

double now() {
  static const double freq = [] {
    LARGE_INTEGER f{};
    QueryPerformanceFrequency(&f);
    return static_cast<double>(f.QuadPart);
  }();
  LARGE_INTEGER c{};
  QueryPerformanceCounter(&c);
  return static_cast<double>(c.QuadPart) / freq;
}

void trace(const char* fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  fprintf(stderr, "[reamesh-cursor] ");
  vfprintf(stderr, fmt, ap);
  fprintf(stderr, "\n");
  va_end(ap);
}

// DIPs to the track view's pixels.
double g_scale = 1;
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
  float r = 0, g = 0, b = 0;
};

Rgb parseColor(const std::string& hex) {
  unsigned v = 0x4c8bf5;
  if (hex.size() == 7 && hex[0] == '#') v = static_cast<unsigned>(strtoul(hex.c_str() + 1, nullptr, 16));
  return {((v >> 16) & 255) / 255.0f, ((v >> 8) & 255) / 255.0f, (v & 255) / 255.0f};
}

bool isLight(const std::string& hex) {
  const Rgb c = parseColor(hex);
  return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b > 0.62;
}

Rgb inkOn(const std::string& hex) { return isLight(hex) ? Rgb{0.1f, 0.1f, 0.1f} : Rgb{1, 1, 1}; }

D2D1_COLOR_F rgba(Rgb c, double a = 1) { return D2D1::ColorF(c.r, c.g, c.b, static_cast<float>(a)); }

std::wstring widen(const std::string& s) {
  if (s.empty()) return {};
  const int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), static_cast<int>(s.size()), nullptr, 0);
  std::wstring w(n > 0 ? n : 0, L'\0');
  if (n > 0) MultiByteToWideChar(CP_UTF8, 0, s.data(), static_cast<int>(s.size()), w.data(), n);
  return w;
}

// ---------- animated values ----------

// Core Animation's timing curves (cubic Bézier from 0,0 to 1,1).
struct Bezier {
  double x1, y1, x2, y2;
  static double at(double p1, double p2, double s) {
    const double m = 1 - s;
    return 3 * m * m * s * p1 + 3 * m * s * s * p2 + s * s * s;
  }
  static double d(double p1, double p2, double s) {
    const double m = 1 - s;
    return 3 * m * m * p1 + 6 * m * s * (p2 - p1) + 3 * s * s * (1 - p2);
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
  double slope(double u) const {
    const double s = std::clamp(param(u), 1e-3, 1 - 1e-3);
    return d(y1, y2, s) / std::max(d(x1, x2, s), 1e-9);
  }
};
constexpr Bezier kEaseOut{0, 0, 0.58, 1};
constexpr Bezier kEaseInOut{0.42, 0, 0.58, 1};

// One animated scalar, evaluable here at any time.
struct Curve {
  enum Kind { Const, Spring, Ease, Keys } kind = Const;
  double start = 0;
  double a = 0, b = 0;         // Const: a. Spring: offset a, velocity b. Ease: a -> b.
  double w = 0;                // Spring: sqrt(stiffness / mass)
  double delay = 0, dur = 0;   // Ease: holds at a for delay, then runs dur
  const Bezier* timing = nullptr;
  std::vector<std::pair<double, double>> keys;  // Keys: (s from start, value), linear between

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
      case Keys: return keys.empty() ? start : start + keys.back().first;
      default: return start;
    }
  }
  double last() const {
    switch (kind) {
      case Spring: return 0;
      case Ease: return b;
      case Keys: return keys.empty() ? a : keys.back().second;
      default: return a;
    }
  }
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
      case Keys: {
        if (keys.empty()) return a;
        if (s <= keys.front().first) return keys.front().second;
        for (size_t i = 1; i < keys.size(); ++i)
          if (s < keys[i].first) {
            const auto& [t0, v0] = keys[i - 1];
            const auto& [t1, v1] = keys[i];
            return t1 > t0 ? v0 + (v1 - v0) * (s - t0) / (t1 - t0) : v1;
          }
        return keys.back().second;
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
      case Keys: {
        for (size_t i = 1; i < keys.size(); ++i)
          if (s >= keys[i - 1].first && s < keys[i].first)
            return (keys[i].second - keys[i - 1].second) / (keys[i].first - keys[i - 1].first);
        return 0;
      }
      default: return 0;
    }
  }
};

// ---------- the composition device ----------

using CreateDevice2Fn = HRESULT(WINAPI*)(IUnknown*, REFIID, void**);

ComPtr<ID2D1Factory1> g_d2dFactory;
ComPtr<IDWriteFactory> g_dw;
ComPtr<ID3D11Device> g_d3d;
ComPtr<ID2D1Device> g_d2d;
ComPtr<ID2D1DeviceContext> g_rec;  // records shadow sources
ComPtr<IDCompositionDesktopDevice> g_dev;
ComPtr<IDCompositionTarget> g_target;
ComPtr<IDCompositionVisual2> g_rootV, g_markV;
bool g_dirty = false;  // something changed since the last Commit
bool g_lost = false;   // a draw failed: rebuild on the next tick

void commit() {
  if (!g_dirty || !g_dev) return;
  g_dirty = false;
  if (FAILED(g_dev->Commit())) g_lost = true;
}

// The compositor's animation for a curve, from time t on; null when the curve
// stands still (the caller sets its final value).
ComPtr<IDCompositionAnimation> animationOf(const Curve& c, double t) {
  if (c.kind == Curve::Const || t >= c.end()) return nullptr;
  ComPtr<IDCompositionAnimation> an;
  if (FAILED(g_dev->CreateAnimation(&an))) return nullptr;
  double off = 0, s = t;
  if (c.kind == Curve::Keys) {
    for (size_t i = 1; i < c.keys.size(); ++i) {
      const double k0 = c.start + c.keys[i - 1].first, k1 = c.start + c.keys[i].first;
      if (k1 <= s || k1 <= k0) continue;
      const double from = std::max(s, k0);
      const double sl = (c.keys[i].second - c.keys[i - 1].second) / (k1 - k0);
      an->AddCubic(off, static_cast<float>(c.value(from)), static_cast<float>(sl), 0, 0);
      off += k1 - from;
      s = k1;
    }
  } else {
    if (c.kind == Curve::Ease && s < c.start + c.delay) {
      an->AddCubic(0, static_cast<float>(c.a), 0, 0, 0);
      off = c.start + c.delay - s;
      s = c.start + c.delay;
    }
    // Hermite pieces with the curve's exact slopes: indistinguishable from the
    // curve at this length.
    const double end = c.end();
    while (s < end - 1e-9) {
      const double e = std::min(end, s + 1.0 / 40), h = e - s;
      const double p0 = c.value(s), p1 = c.value(e), m0 = c.slope(s), m1 = c.slope(e);
      const double q = 3 * (p1 - p0) / (h * h) - (2 * m0 + m1) / h;
      const double k = 2 * (p0 - p1) / (h * h * h) + (m0 + m1) / (h * h);
      an->AddCubic(off, static_cast<float>(p0), static_cast<float>(m0), static_cast<float>(q), static_cast<float>(k));
      off += h;
      s = e;
    }
  }
  an->End(off, static_cast<float>(c.last()));
  return an;
}

template <class SetValue, class SetAnim>
void bindCurve(const Curve& c, double t, SetValue setValue, SetAnim setAnim) {
  if (ComPtr<IDCompositionAnimation> an = animationOf(c, t)) setAnim(an.Get());
  else setValue(static_cast<float>(c.kind == Curve::Const ? c.a : c.last()));
  g_dirty = true;
}

// ---------- nodes: one visual each, with its content and animations ----------
//
// v carries the model offset (pixels), the transforms (scale about a centre,
// optional rotation, the spring offset) and the opacity; c is a child that
// shows the drawn surface, shifted so the content's anchor sits at v's origin.

struct Node {
  ComPtr<IDCompositionVisual2> parent, v, c;
  ComPtr<IDCompositionEffectGroup> fx;
  ComPtr<IDCompositionTranslateTransform> tr;
  ComPtr<IDCompositionScaleTransform> sc;
  ComPtr<IDCompositionRotateTransform> rot;
  ComPtr<IDCompositionSurface> surf;
  int sw = 0, sh = 0;  // surface pixels
  Curve dx = Curve::constant(0), dy = Curve::constant(0);
  Curve sx = Curve::constant(1), sy = Curve::constant(1), op = Curve::constant(1);
  double ox = 0, oy = 0;  // model offset, px
  bool placed = false;    // the offset was set at least once
  double cx = -1, cy = -1, angle = 0;  // scale centre, rotation (degrees)
  double w = 0;           // drawn width, DIP (pills)
  std::string look;       // what the content shows; skips identical redraws
  explicit operator bool() const { return v != nullptr; }
};

enum class Z { Top, Bottom, Below };

Node makeNode(IDCompositionVisual2* parent, Z z = Z::Top, IDCompositionVisual2* ref = nullptr, bool rotate = false) {
  Node n;
  if (!g_dev || !parent) return n;
  bool ok = SUCCEEDED(g_dev->CreateVisual(&n.v)) && SUCCEEDED(g_dev->CreateVisual(&n.c)) &&
            SUCCEEDED(g_dev->CreateEffectGroup(&n.fx)) && SUCCEEDED(g_dev->CreateTranslateTransform(&n.tr)) &&
            SUCCEEDED(g_dev->CreateScaleTransform(&n.sc));
  if (ok && rotate) ok = SUCCEEDED(g_dev->CreateRotateTransform(&n.rot));
  ComPtr<IDCompositionTransform> group;
  if (ok) {
    // Applied in order: scale about the centre, rotate, then the spring offset.
    IDCompositionTransform* parts[3] = {n.sc.Get(), n.tr.Get(), nullptr};
    if (rotate) parts[1] = n.rot.Get(), parts[2] = n.tr.Get();
    ok = SUCCEEDED(g_dev->CreateTransformGroup(parts, rotate ? 3 : 2, &group));
  }
  if (ok) {
    n.v->SetTransform(group.Get());
    n.v->SetEffect(n.fx.Get());
    ok = SUCCEEDED(n.v->AddVisual(n.c.Get(), FALSE, nullptr));
  }
  if (ok) {
    // TRUE with no reference: below every sibling; FALSE: above every one.
    switch (z) {
      case Z::Top: ok = SUCCEEDED(parent->AddVisual(n.v.Get(), FALSE, nullptr)); break;
      case Z::Bottom: ok = SUCCEEDED(parent->AddVisual(n.v.Get(), TRUE, nullptr)); break;
      case Z::Below: ok = SUCCEEDED(parent->AddVisual(n.v.Get(), FALSE, ref)); break;
    }
  }
  if (!ok) {
    g_lost = true;
    return Node{};
  }
  n.parent = parent;
  g_dirty = true;
  return n;
}

void dropNode(Node& n) {
  if (n.parent && n.v) n.parent->RemoveVisual(n.v.Get());
  n = Node{};
  g_dirty = true;
}

// Visuals to remove once their last animation has played.
struct Doomed {
  ComPtr<IDCompositionVisual2> parent, v;
  double at = 0;
};
std::vector<Doomed> g_doomed;

void dropLater(Node& n, double at) {
  if (n.parent && n.v) g_doomed.push_back({n.parent, n.v, at});
  n = Node{};
}

void sweepDoomed(double t) {
  for (auto it = g_doomed.begin(); it != g_doomed.end();) {
    if (t < it->at) {
      ++it;
      continue;
    }
    it->parent->RemoveVisual(it->v.Get());
    g_dirty = true;
    it = g_doomed.erase(it);
  }
}

void bindMove(Node& n, double t) {
  bindCurve(n.dx, t, [&](float f) { n.tr->SetOffsetX(f); }, [&](IDCompositionAnimation* a) { n.tr->SetOffsetX(a); });
  bindCurve(n.dy, t, [&](float f) { n.tr->SetOffsetY(f); }, [&](IDCompositionAnimation* a) { n.tr->SetOffsetY(a); });
}
void bindScale(Node& n, double t) {
  bindCurve(n.sx, t, [&](float f) { n.sc->SetScaleX(f); }, [&](IDCompositionAnimation* a) { n.sc->SetScaleX(a); });
  bindCurve(n.sy, t, [&](float f) { n.sc->SetScaleY(f); }, [&](IDCompositionAnimation* a) { n.sc->SetScaleY(a); });
}
void bindOpacity(Node& n, double t) {
  bindCurve(n.op, t, [&](float f) { n.fx->SetOpacity(f); }, [&](IDCompositionAnimation* a) { n.fx->SetOpacity(a); });
}

void setOpacityNow(Node& n, double v) {
  if (!n) return;
  n.op = Curve::constant(v);
  bindOpacity(n, now());
}

// The model offset jumps; running springs keep riding on top.
void setPos(Node& n, Pt p) {
  if (!n || (n.placed && n.ox == p.x && n.oy == p.y)) return;
  n.placed = true;
  n.ox = p.x;
  n.oy = p.y;
  n.v->SetOffsetX(static_cast<float>(p.x));
  n.v->SetOffsetY(static_cast<float>(p.y));
  g_dirty = true;
}

// Additive spring from `from` to `to`: the model jumps, the offset (plus
// whatever was still moving) decays in the compositor.
void springTo(Node& n, Pt from, Pt to, double stiffness) {
  if (!n) return;
  const double t = now();
  n.dx = Curve::spring(t, n.dx.value(t) + from.x - to.x, n.dx.slope(t), stiffness);
  n.dy = Curve::spring(t, n.dy.value(t) + from.y - to.y, n.dy.slope(t), stiffness);
  setPos(n, to);
  bindMove(n, t);
}

// Drops the position springs (fades and scaling stay).
void snapTo(Node& n, Pt p) {
  if (!n) return;
  n.dx = n.dy = Curve::constant(0);
  setPos(n, p);
  bindMove(n, now());
}

// Where a node shows right now, springs included.
Pt shownAt(const Node& n, double t) { return {n.ox + n.dx.value(t), n.oy + n.dy.value(t)}; }

constexpr double kFromNow = -1;  // "start from the current value"

void fade(Node& n, double to, double dur, double from = kFromNow) {
  if (!n) return;
  const double t = now();
  n.op = Curve::ease(t, from == kFromNow ? n.op.value(t) : from, to, dur, kEaseOut);
  bindOpacity(n, t);
}

// A uniform scale about the node's scale centre.
void scaleTo(Node& n, double to, double dur, double from = kFromNow) {
  if (!n) return;
  const double t = now();
  n.sx = Curve::ease(t, from == kFromNow ? n.sx.value(t) : from, to, dur, kEaseOut);
  n.sy = n.sx;
  bindScale(n, t);
}

void setScaleCentre(Node& n, double x, double y) {
  if (!n || (n.cx == x && n.cy == y)) return;
  n.cx = x;
  n.cy = y;
  n.sc->SetCenterX(static_cast<float>(x));
  n.sc->SetCenterY(static_cast<float>(y));
  g_dirty = true;
}

// ---------- drawing ----------

ComPtr<ID2D1StrokeStyle> g_roundJoin, g_dashed;
ComPtr<ID2D1PathGeometry> g_pointer, g_edgeArrow;
std::map<int, ComPtr<IDWriteTextFormat>> g_formats;  // by size * 10
double g_capPerEm = 0.7;                              // Segoe UI Semibold cap height / em

// Like CGPathAddArcToPoint: a line towards `corner`, then an arc of radius r
// tangent to both edges, ending on the edge towards `next`.
void arcTo(ID2D1GeometrySink* s, D2D1_POINT_2F& cur, D2D1_POINT_2F corner, D2D1_POINT_2F next, double r) {
  const double ax = cur.x - corner.x, ay = cur.y - corner.y, bx = next.x - corner.x, by = next.y - corner.y;
  const double la = std::hypot(ax, ay), lb = std::hypot(bx, by);
  if (la < 1e-9 || lb < 1e-9) return;
  const double ang = std::acos(std::clamp((ax * bx + ay * by) / (la * lb), -1.0, 1.0));
  const double d = r / std::tan(ang / 2);
  const D2D1_POINT_2F t1 = D2D1::Point2F(static_cast<float>(corner.x + ax / la * d), static_cast<float>(corner.y + ay / la * d));
  const D2D1_POINT_2F t2 = D2D1::Point2F(static_cast<float>(corner.x + bx / lb * d), static_cast<float>(corner.y + by / lb * d));
  s->AddLine(t1);
  // y points down, so a positive cross product turns clockwise on screen.
  const double cross = (corner.x - cur.x) * (next.y - corner.y) - (corner.y - cur.y) * (next.x - corner.x);
  s->AddArc(D2D1::ArcSegment(t2, D2D1::SizeF(static_cast<float>(r), static_cast<float>(r)), 0,
                             cross > 0 ? D2D1_SWEEP_DIRECTION_CLOCKWISE : D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                             D2D1_ARC_SIZE_SMALL));
  cur = t2;
}

// Hotspot at (0,0), y down. Four corners: the tip, two wings and the notch
// between them, every one rounded. The rounding pulls the drawn tip inwards,
// so the shape is shifted back by that amount to keep the hotspot on it.
ID2D1PathGeometry* pointerGeometry() {
  if (g_pointer || !g_d2dFactory) return g_pointer.Get();
  const double z = kStyle.pointerSize;
  const double wing = 0.37 * z;
  const double half = std::atan2(z, wing) - kPi / 4;
  const double inset = (kStyle.tipRadius / std::sin(half) - kStyle.tipRadius) / std::sqrt(2.0);
  auto P = [inset](double x, double y) { return D2D1::Point2F(static_cast<float>(x - inset), static_cast<float>(y - inset)); };
  const double nn = kStyle.pointerNotch * (z + wing) / 2;
  const D2D1_POINT_2F tip = P(0, 0), left = P(wing, z), right = P(z, wing), notch = P(nn, nn);
  ComPtr<ID2D1PathGeometry> g;
  ComPtr<ID2D1GeometrySink> s;
  if (FAILED(g_d2dFactory->CreatePathGeometry(&g)) || FAILED(g->Open(&s))) return nullptr;
  D2D1_POINT_2F cur = D2D1::Point2F((tip.x + left.x) / 2, (tip.y + left.y) / 2);
  s->BeginFigure(cur, D2D1_FIGURE_BEGIN_FILLED);
  arcTo(s.Get(), cur, left, notch, kStyle.wingRadius);
  arcTo(s.Get(), cur, notch, right, kStyle.notchRadius);
  arcTo(s.Get(), cur, right, tip, kStyle.wingRadius);
  arcTo(s.Get(), cur, tip, left, kStyle.tipRadius);
  s->EndFigure(D2D1_FIGURE_END_CLOSED);
  if (FAILED(s->Close())) return nullptr;
  g_pointer = g;
  return g_pointer.Get();
}

// A small triangle pointing along +x, centred on (0,0).
ID2D1PathGeometry* edgeArrowGeometry() {
  if (g_edgeArrow || !g_d2dFactory) return g_edgeArrow.Get();
  const float a = static_cast<float>(kStyle.edgeArrow / 2);
  ComPtr<ID2D1PathGeometry> g;
  ComPtr<ID2D1GeometrySink> s;
  if (FAILED(g_d2dFactory->CreatePathGeometry(&g)) || FAILED(g->Open(&s))) return nullptr;
  s->BeginFigure(D2D1::Point2F(-a * 0.8f, -a), D2D1_FIGURE_BEGIN_FILLED);
  s->AddLine(D2D1::Point2F(a, 0));
  s->AddLine(D2D1::Point2F(-a * 0.8f, a));
  s->EndFigure(D2D1_FIGURE_END_CLOSED);
  if (FAILED(s->Close())) return nullptr;
  g_edgeArrow = g;
  return g_edgeArrow.Get();
}

ID2D1StrokeStyle* roundJoin() {
  if (!g_roundJoin && g_d2dFactory) {
    D2D1_STROKE_STYLE_PROPERTIES p = D2D1::StrokeStyleProperties();
    p.lineJoin = D2D1_LINE_JOIN_ROUND;
    g_d2dFactory->CreateStrokeStyle(p, nullptr, 0, &g_roundJoin);
  }
  return g_roundJoin.Get();
}

ID2D1StrokeStyle* dashed() {
  if (!g_dashed && g_d2dFactory) {
    // Dash lengths are in units of the line width.
    const float dashes[2] = {static_cast<float>(kStyle.dragDash / kStyle.dragLine),
                             static_cast<float>(kStyle.dragGap / kStyle.dragLine)};
    D2D1_STROKE_STYLE_PROPERTIES p = D2D1::StrokeStyleProperties();
    p.dashStyle = D2D1_DASH_STYLE_CUSTOM;
    p.lineJoin = D2D1_LINE_JOIN_ROUND;
    g_d2dFactory->CreateStrokeStyle(p, dashes, 2, &g_dashed);
  }
  return g_dashed.Get();
}

IDWriteTextFormat* textFormat(double size) {
  const int key = static_cast<int>(std::lround(size * 10));
  auto it = g_formats.find(key);
  if (it != g_formats.end()) return it->second.Get();
  ComPtr<IDWriteTextFormat> f;
  if (!g_dw || FAILED(g_dw->CreateTextFormat(L"Segoe UI", nullptr, DWRITE_FONT_WEIGHT_SEMI_BOLD,
                                             DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
                                             static_cast<float>(size), L"", &f)))
    return nullptr;
  f->SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
  g_formats[key] = f;
  return f.Get();
}

void readCapHeight() {
  ComPtr<IDWriteFontCollection> fonts;
  ComPtr<IDWriteFontFamily> family;
  ComPtr<IDWriteFont> font;
  UINT32 index = 0;
  BOOL exists = FALSE;
  if (!g_dw || FAILED(g_dw->GetSystemFontCollection(&fonts)) ||
      FAILED(fonts->FindFamilyName(L"Segoe UI", &index, &exists)) || !exists ||
      FAILED(fonts->GetFontFamily(index, &family)) ||
      FAILED(family->GetFirstMatchingFont(DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_FONT_STRETCH_NORMAL,
                                          DWRITE_FONT_STYLE_NORMAL, &font)))
    return;
  DWRITE_FONT_METRICS m{};
  font->GetMetrics(&m);
  if (m.designUnitsPerEm > 0 && m.capHeight > 0) g_capPerEm = static_cast<double>(m.capHeight) / m.designUnitsPerEm;
}

// Starts drawing a node's content: w x h DIP with `margin` DIP free on every
// side, the point (ax, ay) of the content at the node's origin. The context
// draws in DIPs from the content's top-left; nullptr when it can't.
ID2D1DeviceContext* beginDraw(Node& n, double w, double h, double margin, double ax = 0, double ay = 0) {
  if (!n) return nullptr;
  const int m = static_cast<int>(std::ceil(margin * g_scale));
  const int needW = std::clamp(static_cast<int>(std::ceil(w * g_scale)) + 2 * m, 1, 16384);
  const int needH = std::clamp(static_cast<int>(std::ceil(h * g_scale)) + 2 * m, 1, 16384);
  // Surfaces come in steps of 64 pixels and the unused rest stays clear, so a
  // box that resizes while the view zooms is drawn again, not reallocated.
  if (!n.surf || needW > n.sw || needH > n.sh || n.sw > 2 * needW + 64 || n.sh > 2 * needH + 64) {
    const int pw = std::min((needW + 63) & ~63, 16384), ph = std::min((needH + 63) & ~63, 16384);
    n.surf.Reset();
    if (FAILED(g_dev->CreateSurface(pw, ph, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED, &n.surf))) {
      g_lost = true;
      return nullptr;
    }
    n.sw = pw;
    n.sh = ph;
    n.c->SetContent(n.surf.Get());
  }
  n.c->SetOffsetX(static_cast<float>(-m - std::round(ax * g_scale)));
  n.c->SetOffsetY(static_cast<float>(-m - std::round(ay * g_scale)));
  ID2D1DeviceContext* dc = nullptr;
  POINT off{};
  if (FAILED(n.surf->BeginDraw(nullptr, __uuidof(ID2D1DeviceContext), reinterpret_cast<void**>(&dc), &off))) {
    g_lost = true;
    return nullptr;
  }
  dc->SetDpi(static_cast<float>(96 * g_scale), static_cast<float>(96 * g_scale));
  dc->SetTransform(D2D1::Matrix3x2F::Identity());
  // The surface may sit inside a larger atlas: clear just our pixels.
  const float x0 = static_cast<float>(off.x / g_scale), y0 = static_cast<float>(off.y / g_scale);
  dc->PushAxisAlignedClip(D2D1::RectF(x0, y0, x0 + static_cast<float>(n.sw / g_scale), y0 + static_cast<float>(n.sh / g_scale)),
                          D2D1_ANTIALIAS_MODE_ALIASED);
  dc->Clear(D2D1::ColorF(0, 0, 0, 0));
  dc->PopAxisAlignedClip();
  dc->SetTransform(D2D1::Matrix3x2F::Translation(x0 + static_cast<float>(m / g_scale), y0 + static_cast<float>(m / g_scale)));
  dc->SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
  dc->SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
  return dc;
}

void endDraw(Node& n, ID2D1DeviceContext* dc) {
  dc->Release();
  if (FAILED(n.surf->EndDraw())) g_lost = true;
  g_dirty = true;
}

ComPtr<ID2D1SolidColorBrush> brush(ID2D1DeviceContext* dc, D2D1_COLOR_F c) {
  ComPtr<ID2D1SolidColorBrush> b;
  dc->CreateSolidColorBrush(c, &b);
  return b;
}

// A soft drop shadow of `g`, like a Core Animation shadowPath.
void drawShadow(ID2D1DeviceContext* dc, ID2D1Geometry* g, double opacity) {
  if (!g_rec || !g) return;
  ComPtr<ID2D1CommandList> cl;
  if (FAILED(g_rec->CreateCommandList(&cl))) return;
  g_rec->SetTarget(cl.Get());
  g_rec->BeginDraw();
  g_rec->SetTransform(D2D1::Matrix3x2F::Identity());
  if (ComPtr<ID2D1SolidColorBrush> b = brush(g_rec.Get(), D2D1::ColorF(0, 0, 0, 1))) g_rec->FillGeometry(g, b.Get());
  const HRESULT hr = g_rec->EndDraw();
  g_rec->SetTarget(nullptr);
  if (FAILED(hr) || FAILED(cl->Close())) return;
  ComPtr<ID2D1Effect> fx;
  if (FAILED(dc->CreateEffect(CLSID_D2D1Shadow, &fx))) return;
  fx->SetInput(0, cl.Get());
  // Core Animation's shadowRadius is about two standard deviations.
  fx->SetValue(D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION, static_cast<float>(kStyle.shadowRadius / 2));
  fx->SetValue(D2D1_SHADOW_PROP_COLOR, D2D1::Vector4F(0, 0, 0, static_cast<float>(opacity)));
  D2D1_MATRIX_3X2_F m{};
  dc->GetTransform(&m);
  dc->SetTransform(D2D1::Matrix3x2F::Translation(0, static_cast<float>(kStyle.shadowOffsetY)) * m);
  dc->DrawImage(fx.Get());
  dc->SetTransform(m);
}

std::string scaleKey() {
  char b[32];
  snprintf(b, sizeof b, "@%.3f", g_scale);
  return b;
}

void drawArrow(Node& n, const std::string& color) {
  const std::string look = "arrow" + color + scaleKey();
  if (n.look == look) return;
  ID2D1Geometry* g = pointerGeometry();
  ID2D1DeviceContext* dc = g ? beginDraw(n, kStyle.pointerSize, kStyle.pointerSize, 6) : nullptr;
  if (!dc) return;
  drawShadow(dc, g, kStyle.shadowOpacity);
  dc->FillGeometry(g, brush(dc, rgba(parseColor(color))).Get());
  dc->DrawGeometry(g, brush(dc, D2D1::ColorF(1, 1, 1, 1)).Get(), static_cast<float>(kStyle.outline), roundJoin());
  endDraw(n, dc);
  n.look = look;
}

// The disc behind the followed person's pointer, centred on the arrow's body
// (not its tip, where a click ripples).
void drawHalo(Node& n, const std::string& color) {
  const std::string look = "halo" + color + scaleKey();
  if (n.look == look) return;
  const double R = kStyle.haloRadius, c = kStyle.haloCentre;
  ID2D1DeviceContext* dc = beginDraw(n, 2 * R, 2 * R, 2, R - c, R - c);
  if (!dc) return;
  const Rgb rgb = parseColor(color);
  const float line = static_cast<float>(kStyle.haloLine);
  const D2D1_ELLIPSE e = D2D1::Ellipse(D2D1::Point2F(static_cast<float>(R), static_cast<float>(R)),
                                       static_cast<float>(R) - line / 2, static_cast<float>(R) - line / 2);
  dc->FillEllipse(e, brush(dc, rgba(rgb, kStyle.haloFill)).Get());
  dc->DrawEllipse(e, brush(dc, rgba(rgb, kStyle.haloLineOpacity)).Get(), line);
  endDraw(n, dc);
  n.look = look;
}

// A pill in `color` with `label` in the matching ink, `lead` DIP free before
// the text; returns its width in DIP.
double drawPill(Node& n, const std::string& label, const std::string& color, double fontSize, double height,
                double padX, double lead = 0) {
  char key[96];
  snprintf(key, sizeof key, "|%.1f|%.1f|%.1f|%.1f", fontSize, height, padX, lead);
  const std::string look = "pill" + label + "|" + color + key + scaleKey();
  if (n.look == look) return n.w;
  IDWriteTextFormat* fmt = textFormat(fontSize);
  if (!fmt || !g_dw) return n.w;
  const std::wstring text = widen(label);
  ComPtr<IDWriteTextLayout> layout;
  if (FAILED(g_dw->CreateTextLayout(text.c_str(), static_cast<UINT32>(text.size()), fmt, 4096, 512, &layout)))
    return n.w;
  DWRITE_TEXT_METRICS tm{};
  layout->GetMetrics(&tm);
  DWRITE_LINE_METRICS lm{};
  UINT32 lines = 0;
  layout->GetLineMetrics(&lm, 1, &lines);
  const double tw = std::ceil(tm.widthIncludingTrailingWhitespace);
  const double w = tw + 2 * padX + lead;
  ComPtr<ID2D1RoundedRectangleGeometry> shape;
  const float r = static_cast<float>(height / 2);
  if (FAILED(g_d2dFactory->CreateRoundedRectangleGeometry(
          D2D1::RoundedRect(D2D1::RectF(0, 0, static_cast<float>(w), static_cast<float>(height)), r, r), &shape)))
    return n.w;
  ID2D1DeviceContext* dc = beginDraw(n, w, height, 6);
  if (!dc) return n.w;
  drawShadow(dc, shape.Get(), kStyle.shadowOpacity * 0.6);
  dc->FillGeometry(shape.Get(), brush(dc, rgba(parseColor(color))).Get());
  // Centre the capitals, not the line box: the baseline sits half a cap
  // height below the pill's middle.
  const double top = (height + g_capPerEm * fontSize) / 2 - (lines ? lm.baseline : fontSize * 0.8);
  dc->DrawTextLayout(D2D1::Point2F(static_cast<float>(padX + lead), static_cast<float>(std::round(top * g_scale) / g_scale)),
                     layout.Get(), brush(dc, rgba(inkOn(color))).Get(), D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
  endDraw(n, dc);
  n.look = look;
  n.w = w;
  return w;
}

void drawEdgeArrow(Node& n, const std::string& color) {
  const std::string look = "edgearrow" + color + scaleKey();
  if (n.look == look) return;
  const double a = kStyle.edgeArrow;
  ID2D1Geometry* g = edgeArrowGeometry();
  ID2D1DeviceContext* dc = g ? beginDraw(n, a, a, 2, a / 2, a / 2) : nullptr;
  if (!dc) return;
  D2D1_MATRIX_3X2_F m{};
  dc->GetTransform(&m);
  dc->SetTransform(D2D1::Matrix3x2F::Translation(static_cast<float>(a / 2), static_cast<float>(a / 2)) * m);
  ComPtr<ID2D1SolidColorBrush> ink = brush(dc, rgba(inkOn(color)));
  dc->FillGeometry(g, ink.Get());
  dc->DrawGeometry(g, ink.Get(), 1.5f, roundJoin());
  endDraw(n, dc);
  n.look = look;
}

// A rounded box of wPx x hPx: `fill` inside and, when line > 0, a `line` DIP
// border of `stroke` along its inside (dashed: centred on the edge instead).
void drawBox(Node& n, double wPx, double hPx, D2D1_COLOR_F fill, D2D1_COLOR_F stroke, double line, bool dash) {
  char key[160];
  snprintf(key, sizeof key, "box%.0fx%.0f|%.3f,%.3f,%.3f,%.3f|%.3f,%.3f,%.3f,%.3f|%.2f%d", wPx, hPx, fill.r, fill.g,
           fill.b, fill.a, stroke.r, stroke.g, stroke.b, stroke.a, line, dash ? 1 : 0);
  const std::string look = key + scaleKey();
  if (n.look == look) return;
  const double w = wPx / g_scale, h = hPx / g_scale;
  ID2D1DeviceContext* dc = beginDraw(n, w, h, 2);
  if (!dc) return;
  const double r = std::min({kStyle.markRadius, w / 2, h / 2});
  const auto rr = [](double x0, double y0, double x1, double y1, double rad) {
    const float f = static_cast<float>(std::max(rad, 0.0));
    return D2D1::RoundedRect(D2D1::RectF(static_cast<float>(x0), static_cast<float>(y0), static_cast<float>(x1),
                                         static_cast<float>(y1)),
                             f, f);
  };
  dc->FillRoundedRectangle(rr(0, 0, w, h, r), brush(dc, fill).Get());
  if (line > 0 && dash) {
    dc->DrawRoundedRectangle(rr(0, 0, w, h, r), brush(dc, stroke).Get(), static_cast<float>(line), dashed());
  } else if (line > 0 && w > line && h > line) {
    const double i = line / 2;
    dc->DrawRoundedRectangle(rr(i, i, w - i, h - i, r - i), brush(dc, stroke).Get(), static_cast<float>(line));
  }
  endDraw(n, dc);
  n.look = look;
}

// A plain rectangle of wPx x hPx.
void drawFill(Node& n, double wPx, double hPx, D2D1_COLOR_F c) {
  char key[96];
  snprintf(key, sizeof key, "fill%.0fx%.0f|%.3f,%.3f,%.3f,%.3f", wPx, hPx, c.r, c.g, c.b, c.a);
  const std::string look = key + scaleKey();
  if (n.look == look) return;
  ID2D1DeviceContext* dc = beginDraw(n, wPx / g_scale, hPx / g_scale, 0);
  if (!dc) return;
  dc->FillRectangle(D2D1::RectF(0, 0, static_cast<float>(wPx / g_scale), static_cast<float>(hPx / g_scale)),
                    brush(dc, c).Get());
  endDraw(n, dc);
  n.look = look;
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
  // An open FX window (setActivity, from presence): where the cursor waits
  // while away when the cursor messages say nothing more precise.
  std::string fxTrack, fxLabel;
  double fxAt = 0;
  // Where they work away from the arrange, from their cursor messages.
  cursor_overlay::Away away;
  // Their item drag, and its visuals per item: the original marked, the
  // landing spot outlined.
  cursor_overlay::Drag drag;
  struct DragItem {
    Node origin, ghost;
    Box ghostRect;
    bool shown = false;
  };
  std::map<std::string, DragItem> dragItems;
  std::string parkLabel;

  Node root;   // positioned at the pointer tip
  Node arrow;  // child of root
  Node halo;   // child of root, below the arrow: the person we follow
  bool followed = false;
  Node tag;    // a sibling of root, so it can trail it
  Node edge;
  Node edgeArrow;  // child of edge
  double edgeAx = 0, edgeAy = 0.5;  // which point of the edge pill sits on its spot

  bool cursorShown = false, tagShown = false, edgeShown = false, parked = false;
  bool idle = false, pressed = false;
  bool fresh = false;  // heard from within staleAfter (tick notices the change)
  Pt cursorModel, tagModel, edgeModel;
  Side edgeSide = Side::Left;
};

// One marked item.
struct MarkState {
  cursor_overlay::Mark m;
  Node box;
  double lastWanted = 0;  // when setMarks last asked for it
  bool shown = false;
  Box rect;  // in the view, unclipped; valid while shown
};

struct Geo {
  bool ok = false;
  double t0 = 0, spp = 0, W = 0, H = 0;
};

struct Place {
  enum Kind { Hidden, Cursor, Edge } kind = Hidden;
  Pt p;
  Side side = Side::Left;
  double angle = 0;     // Edge: direction from the pill to the real spot
  bool parked = false;  // waiting while they work away from the arrange
  std::string label;    // what they work in, for the name tag
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
// "Show animations in Windows" off: no springs, no scaling, cursors fade
// instead of travelling.
bool g_reduceMotion = false;
double g_reduceMotionChecked = 0;
// Item marks live in their own visual below the cursors.
std::map<std::string, MarkState> g_marks;
bool g_marksDirty = false;
// REAPER reports an item's rows as of its last draw, which can trail a zoom
// or scroll by a pass: after any change, marks are laid out on every pass
// until this time.
double g_marksSettleUntil = 0;
double g_markKey[5] = {};
unsigned long long g_markStamp = 0;
// The name of whoever edits the item under the local pointer.
Node g_hover;
bool g_hoverShown = false;
bool g_localIn = false;
Pt g_localPoint;
// The follow frame: four strips and the tag, children of one visual so they
// fade together.
Node g_follow, g_followTag, g_followEdge[4];
std::string g_followKey, g_followName, g_followColor, g_followId;
bool g_clickPending = false;
POINT g_clickPoint{};

constexpr UINT_PTR kSubclassId = 0x524D4331;  // "RMC1"
constexpr UINT_PTR kTimerId = 0x524D4354;     // "RMCT"

// ---------- peers ----------

std::string displayName(const Peer& p) {
  if (!p.name.empty()) return p.name;
  return p.id.size() > 6 ? p.id.substr(p.id.size() - 6) : p.id;
}

// The name, plus the plug-in while the cursor waits beside an FX window.
std::string displayLabel(const Peer& p) {
  std::string s = displayName(p);
  if (p.parked && !p.parkLabel.empty()) s += " \xC2\xB7 " + p.parkLabel;  // ·
  return s;
}

// Re-inserts a visual above all its siblings.
void raise(Node& n) {
  if (!n || !n.parent) return;
  n.parent->RemoveVisual(n.v.Get());
  n.parent->AddVisual(n.v.Get(), FALSE, nullptr);
  g_dirty = true;
}

void updateTag(Peer& p, double t);

// The person we follow stands out: the disc behind the pointer, the name tag
// always up, never dimmed, above everyone else's cursor.
void setFollowed(Peer& p, bool on) {
  if (on == p.followed && (!on || p.halo)) return;
  p.followed = on;
  if (on) {
    if (!p.halo && p.root) {
      p.halo = makeNode(p.root.v.Get(), Z::Bottom);
      setOpacityNow(p.halo, 0);
    }
    drawHalo(p.halo, p.color);
    fade(p.halo, 1, kStyle.fadeIn * 2);
    raise(p.edge);
    raise(p.tag);
    raise(p.root);
    if (p.idle) {
      fade(p.arrow, 1, kStyle.fadeIn);
      p.idle = false;
    }
  } else {
    fade(p.halo, 0, kStyle.fadeOut);
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
  p.edge = makeNode(g_rootV.Get());
  p.edgeArrow = p.edge ? makeNode(p.edge.v.Get(), Z::Top, nullptr, true) : Node{};
  p.tag = makeNode(g_rootV.Get());
  p.root = makeNode(g_rootV.Get());
  p.arrow = p.root ? makeNode(p.root.v.Get()) : Node{};
  setOpacityNow(p.edge, 0);
  setOpacityNow(p.tag, 0);
  setOpacityNow(p.root, 0);
  drawArrow(p.arrow, p.color);
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
  dropNode(p.root);
  p.halo = Node{};  // went with root
  dropNode(p.tag);
  dropNode(p.edge);
}

void refreshTag(Peer& p) {
  drawPill(p.tag, displayLabel(p), p.color, kStyle.tagFont, kStyle.tagHeight, kStyle.tagPadX);
  drawArrow(p.arrow, p.color);
  if (p.halo) drawHalo(p.halo, p.color);
}

void refreshEdge(Peer& p, Side side) {
  const double lead = kStyle.edgeArrow + 4;
  const double w = drawPill(p.edge, displayLabel(p), p.color, kStyle.edgeFont, kStyle.edgeHeight, kStyle.edgePadX, lead);
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
  return {std::round(at.x - p.edgeAx * S(p.edge.w)), std::round(at.y - p.edgeAy * S(kStyle.edgeHeight))};
}

void setEdgeAngle(Peer& p, double angle) {
  const double deg = angle * 180 / kPi;
  if (!p.edgeArrow.rot || p.edgeArrow.angle == deg) return;
  p.edgeArrow.angle = deg;
  p.edgeArrow.rot->SetAngle(static_cast<float>(deg));
  g_dirty = true;
}

// Appear: fade in while growing from appearScale. `fromZero` starts the fade
// at nothing even if the visual was visible a moment ago (a crossfade).
void popIn(Node& n, double opacity, bool fromZero = false) {
  fade(n, opacity, kStyle.fadeIn, fromZero ? 0 : kFromNow);
  if (g_reduceMotion) return;
  scaleTo(n, 1, kStyle.fadeIn * 1.6, fromZero ? kStyle.appearScale : kFromNow);
}

void popOut(Node& n) {
  fade(n, 0, kStyle.fadeOut);
  if (g_reduceMotion) return;
  scaleTo(n, kStyle.appearScale, kStyle.fadeOut);
}

void ripple(Peer& p) {
  if (!p.root) return;
  Node r = makeNode(p.root.v.Get(), Z::Bottom);
  // Drawn at its largest and scaled up to it, so the ring stays sharp.
  const double to = kStyle.rippleScaleTo, R = kStyle.rippleRadius * to;
  ID2D1DeviceContext* dc = beginDraw(r, 2 * R, 2 * R, 3, R, R);
  if (!dc) {
    dropNode(r);
    return;
  }
  const float line = static_cast<float>(kStyle.rippleLine * to);
  dc->DrawEllipse(D2D1::Ellipse(D2D1::Point2F(static_cast<float>(R), static_cast<float>(R)),
                                static_cast<float>(R - line / 2), static_cast<float>(R - line / 2)),
                  brush(dc, rgba(parseColor(p.color))).Get(), line);
  endDraw(r, dc);
  const double t = now();
  if (g_reduceMotion) r.sx = Curve::constant(1 / to);
  else r.sx = Curve::ease(t, kStyle.rippleScaleFrom / to, 1, kStyle.rippleDuration, kEaseOut);
  r.sy = r.sx;
  bindScale(r, t);
  r.op = Curve::ease(t, kStyle.rippleOpacity, 0, kStyle.rippleDuration, kEaseOut);
  bindOpacity(r, t);
  dropLater(r, t + kStyle.rippleDuration + 0.05);
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
  Node ghost = makeNode(g_rootV.Get(), Z::Below, p.root.v.Get());
  if (ghost) {
    drawArrow(ghost, p.color);
    setPos(ghost, shownAt(p.root, t));
    ghost.sx = ghost.sy = Curve::constant(p.arrow.sx.value(t) * p.root.sx.value(t));
    bindScale(ghost, t);
    fade(ghost, 0, kStyle.crossfade, p.root.op.value(t) * p.arrow.op.value(t));
    dropLater(ghost, t + kStyle.crossfade + 0.05);
  }
  snapTo(p.root, at);
  snapTo(p.tag, tagAt);
  popIn(p.root, 1, true);
  if (p.tagShown) fade(p.tag, 1, kStyle.crossfade, 0);
}

// ---------- geometry ----------

Geo readGeo() {
  Geo g;
  if (!g_attached || !g_host.ready || !g_host.ready()) return g;
  if (!g_host.timeAxis(&g.t0, &g.spp) || !(g.spp > 0)) return g;
  RECT rc{};
  GetClientRect(g_tv, &rc);
  g.W = rc.right - rc.left;
  g.H = rc.bottom - rc.top;
  g.ok = g.W > 0 && g.H > 0;
  return g;
}

Place placeOf(const Peer& p, const Geo& g, double t) {
  Place pl;
  if (!g.ok) return pl;
  const bool fresh = t - p.lastMsg <= kStyle.staleAfter;
  const bool live = p.in && fresh;
  // Away from the arrange: what their cursor messages say, else an FX window
  // their presence reports.
  const bool awayMsg = !p.in && fresh && !p.away.kind.empty();
  const bool parked = !live && (awayMsg || !p.fxTrack.empty());
  if (!live && !parked) return pl;
  pl.parked = parked;
  std::string guid = p.guid;
  double parkX = S(kStyle.parkInset), parkFy = 0.5;
  bool onItem = false;
  double iy = 0, ih = 0;
  if (parked) {
    const std::string& k = awayMsg ? p.away.kind : std::string("fx");
    guid = awayMsg ? p.away.track : p.fxTrack;
    if (k == "fx") pl.label = awayMsg ? p.away.label : p.fxLabel;
    else if (k == "midi") pl.label = "MIDI";
    else if (k == "mixer") pl.label = "Mixer";
    double i0 = 0, i1 = 0;
    std::string itemTrack;
    if (k == "midi" && g_host.itemSpan &&
        g_host.itemSpan(p.away.item.c_str(), &i0, &i1, &iy, &ih, &itemTrack)) {
      // In the MIDI editor: on the item they edit.
      guid = itemTrack;
      parkX = (i0 - g.t0) / g.spp + S(kStyle.parkItemInset);
      onItem = true;
    }
  }
  double ty = 0, th = 0;
  const bool found = !guid.empty() && g_host.trackSpan(guid.c_str(), &ty, &th);
  const bool yKnown = found && th > 0;
  double x = parked ? parkX : (p.t - g.t0) / g.spp;
  double y = yKnown ? (onItem ? iy + ih * 0.5 : ty + (parked ? parkFy : p.fy) * th) : 0;
  if (live && yKnown && p.predict && !g_reduceMotion) {
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
    pl.kind = Place::Cursor;
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

// The name tag shows while the cursor moves, and always while it waits
// beside an FX window or belongs to the person we follow.
void updateTag(Peer& p, double t) {
  const bool want = p.cursorShown && (p.parked || p.followed || t - p.lastMove < kStyle.tagIdleAfter);
  if (want == p.tagShown) return;
  fade(p.tag, want ? 1 : 0, want ? kStyle.tagFadeIn : kStyle.tagFadeOut);
  p.tagShown = want;
}

// A rectangle cut to the view (plus a little, so cut edges draw no border):
// items can be far wider than any surface.
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
  if (!g_host.trackSpan(track.c_str(), &ty, &th)) return false;
  *at = {std::round((t0 - g.t0) / g.spp), std::round(y), std::round((t1 - t0) / g.spp), std::round(h)};
  double dy = 0;
  if (dtr != 0 && g_host.trackShift && g_host.trackShift(track.c_str(), dtr, &dest)) {
    double dy0 = 0, dh = 0;
    if (g_host.trackSpan(dest.c_str(), &dy0, &dh) && dh > 0) dy = dy0 - ty;
  }
  *to = {std::round(at->x + dt / g.spp), std::round(y + dy), at->w, at->h};
  return true;
}

D2D1_COLOR_F dimFill() { return D2D1::ColorF(0, 0, 0, static_cast<float>(kStyle.markDim)); }

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
      d.origin = makeNode(g_markV.Get());
      d.ghost = makeNode(g_markV.Get());
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

// Our undone edits, waiting for the next pass to know where the item went.
std::vector<cursor_overlay::Revert> g_pendingReverts;

void runRevert(const cursor_overlay::Revert& r, const Geo& g) {
  double ty = 0, th = 0, t0 = 0, t1 = 0, y = 0, h = 0;
  if (!g.ok || !g_host.trackSpan(r.track.c_str(), &ty, &th) || th <= 0) return;
  if (!g_host.itemSpan || !g_host.itemSpan(r.item.c_str(), &t0, &t1, &y, &h, nullptr)) return;
  const Box from{std::round((r.t0 - g.t0) / g.spp), std::round(ty + r.yOff), std::round((r.t1 - r.t0) / g.spp),
                 std::round(r.h > 0 ? r.h : h)};
  const Box to{std::round((t0 - g.t0) / g.spp), std::round(y), std::round((t1 - t0) / g.spp), std::round(h)};
  if (from.x == to.x && from.y == to.y && from.w == to.w && from.h == to.h) return;
  const double ux0 = std::min(from.x, to.x), ux1 = std::max(from.x + from.w, to.x + to.w);
  const double uy0 = std::min(from.y, to.y), uy1 = std::max(from.y + from.h, to.y + to.h);
  if (ux1 <= 0 || ux0 >= g.W || uy1 <= 0 || uy0 >= g.H) return;
  // The copy glides by scaling surfaces, which an item wider than any surface
  // can't do; it isn't seen whole on screen anyway.
  const auto fits = [](const Box& b) { return b.w >= 1 && b.h >= 1 && b.w <= 8192 && b.h <= 8192; };
  if (!fits(from) || !fits(to)) return;

  // Hold at our version, glide into the item, fade. A box drawn at one size
  // and scaled to another would stretch its dashes, so a resized item gets
  // two: one drawn at our size, one at the item's, crossfading as they glide.
  const double t = now();
  const double hold = kStyle.revertHold, glide = g_reduceMotion ? 0 : kStyle.revertGlide;
  const double total = hold + glide + kStyle.revertFade;
  const double move = std::max(glide, 0.001);
  const bool resized = from.w != to.w || from.h != to.h;
  // `at` is the size the box is drawn at; it travels from `from` to `to`.
  const auto box = [&](const Box& at, std::vector<std::pair<double, double>> keys) {
    Node l = makeNode(g_markV.Get());
    if (!l) return;
    drawBox(l, at.w, at.h, D2D1::ColorF(1, 1, 1, 0.08f),
            D2D1::ColorF(1, 1, 1, static_cast<float>(kStyle.revertOpacity)), kStyle.dragLine, true);
    setPos(l, {to.x, to.y});
    l.dx = Curve::ease(t, from.x - to.x, 0, move, kEaseInOut, hold);
    l.dy = Curve::ease(t, from.y - to.y, 0, move, kEaseInOut, hold);
    l.sx = Curve::ease(t, from.w / at.w, to.w / at.w, move, kEaseInOut, hold);
    l.sy = Curve::ease(t, from.h / at.h, to.h / at.h, move, kEaseInOut, hold);
    bindMove(l, t);
    bindScale(l, t);
    l.op.kind = Curve::Keys;
    l.op.start = t;
    l.op.keys = std::move(keys);
    bindOpacity(l, t);
    dropLater(l, t + total + 0.05);
  };
  if (!resized) {
    box(to, {{0, 0}, {0.08, 1}, {hold + glide, 1}, {total, 0}});
    return;
  }
  box(from, {{0, 0}, {0.08, 1}, {hold, 1}, {hold + move, 0}});
  box(to, {{0, 0}, {hold, 0}, {hold + move, 1}, {total, 0}});
}

// Moves a peer's visuals to where its spot is now. With a network update
// pending they spring toward it (or fade across on a long jump); otherwise
// only the view moved and they snap (glued).
void layoutPeer(Peer& p, const Geo& g, double t) {
  const Place pl = placeOf(p, g, t);
  const bool net = p.pendingSpring;
  p.pendingSpring = false;
  if (pl.parked != p.parked || pl.label != p.parkLabel) {
    p.parked = pl.parked;
    p.parkLabel = pl.label;
    refreshTag(p);
  }
  if (!p.drag.items.empty() && t - p.lastMsg > kStyle.staleAfter) p.drag = {};
  layoutDrag(p, g, net);

  if (pl.kind == Place::Cursor) {
    const Pt tagAt{pl.p.x + std::round(S(kStyle.tagDX)), pl.p.y + std::round(S(kStyle.tagDY))};
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

int dpiOf(HWND h) {
  using Fn = UINT(WINAPI*)(HWND);
  static const Fn fn = reinterpret_cast<Fn>(GetProcAddress(GetModuleHandleW(L"user32.dll"), "GetDpiForWindow"));
  if (fn && h) return static_cast<int>(fn(h));
  HDC dc = GetDC(nullptr);
  const int d = dc ? GetDeviceCaps(dc, LOGPIXELSX) : 96;
  if (dc) ReleaseDC(nullptr, dc);
  return d;
}

// The window moved to a monitor with another scale: everything is drawn again
// at the new one (the look keys carry the scale) and laid out again.
void checkScale() {
  const double s = std::max(dpiOf(g_tv), 48) / 96.0;
  if (s == g_scale) return;
  g_scale = s;
  for (auto& [id, p] : g_peers) {
    refreshTag(p);
    if (p.edgeShown) refreshEdge(p, p.edgeSide);
    p.pendingSpring = true;
  }
  g_marksDirty = true;
  if (g_hover) g_hover.look.clear();
  layoutFollow();
}

// Whether nothing on screen can have moved since the last full pass: no
// update waiting and the view (time axis, size, track rows) unchanged. The
// timer and every repaint run the glue pass, and most of them change
// nothing. REAPER's own numbers can trail a change by a pass, so after one
// the full pass keeps running for a moment.
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
  if (t < g_viewSettleUntil || g_marksDirty || !g_pendingReverts.empty() || t < g_marksSettleUntil) return false;
  for (const auto& [id, p] : g_peers)
    if (p.pendingSpring || p.pendingClick) return false;
  return true;
}

// The glue pass: every peer to its current pixel (`onlyPending`: just the
// peers with a network update waiting).
void glue(bool onlyPending = false) {
  if (!g_attached) return;
  RECT rc{};
  GetClientRect(g_tv, &rc);
  if (rc.right != g_clipW || rc.bottom != g_clipH) {
    g_clipW = rc.right;
    g_clipH = rc.bottom;
    g_rootV->SetClip(D2D1::RectF(0, 0, static_cast<float>(g_clipW), static_cast<float>(g_clipH)));
    g_dirty = true;
    layoutFollow();
  }
  checkScale();
  if (g_peers.empty() && g_marks.empty() && g_pendingReverts.empty()) return;
  const double t = now();
  const Geo g = readGeo();
  // The project can't be read right now (a remote apply, a project opening;
  // a repaint or a nested modal loop can land in the middle of one): leave
  // everything where it is and the updates pending.
  if (!g.ok) return;
  if (!onlyPending && quiet(g, t)) return;
  if (!onlyPending && !g_pendingReverts.empty()) {
    for (const auto& r : g_pendingReverts) runRevert(r, g);
    g_pendingReverts.clear();
  }
  if (!onlyPending) layoutMarks(g);
  for (auto& [id, p] : g_peers)
    if (!onlyPending || p.pendingSpring || p.pendingClick) layoutPeer(p, g, t);
}

// The timer pass, ~60-100 times a second while attached: network in/out,
// then the glue pass.
void onTimer() {
  if (!g_attached) return;
  if (g_host.pump) {
    static double lastPump = 0;
    const double t = now();
    if (t - lastPump >= 0.004) {
      lastPump = t;
      g_host.pump();
    }
  }
  if (!g_attached) return;  // the pump may have ended the session
  const double t = now();
  sweepDoomed(t);
  if (!g_peers.empty() || !g_marks.empty() || !g_pendingReverts.empty()) {
    // Updates stopped: the pointer came to rest, so take the prediction back.
    for (auto& [id, p] : g_peers)
      if (p.predict && t - p.lastMsg > kStyle.predictStopAfter) {
        p.predict = false;
        p.vt = p.vfy = 0;
        p.pendingSpring = true;
      }
    glue();
  }
  commit();
}

// ---------- item marks ----------

// Where an item is in the view now; false when it is not visible.
bool markRect(const MarkState& ms, const Geo& g, Box* out) {
  double t0 = 0, t1 = 0, y = 0, h = 0;
  if (!g.ok || !g_host.itemSpan || !g_host.itemSpan(ms.m.item.c_str(), &t0, &t1, &y, &h, nullptr)) return false;
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
    g_hover = makeNode(g_markV.Get());
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
    drawBox(ms.box, clipped.w, clipped.h, D2D1::ColorF(0, 0, 0, static_cast<float>(kStyle.loadingVeil)),
            D2D1::ColorF(0, 0, 0, 0), 0, false);
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
  const D2D1_COLOR_F c = rgba(parseColor(g_followColor));
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
  if (!g.ok || !g_host.trackAt(vp.y, guid, fy)) return false;
  *tt = g.t0 + vp.x * g.spp;
  return true;
}

// ---------- the track view ----------

void releaseDevice();
void teardown();

LRESULT CALLBACK trackViewProc(HWND h, UINT msg, WPARAM wp, LPARAM lp, UINT_PTR, DWORD_PTR) {
  switch (msg) {
    case WM_TIMER:
      if (wp == kTimerId) {
        onTimer();
        return 0;
      }
      break;
    case WM_LBUTTONDOWN:
    case WM_LBUTTONDBLCLK:
      g_clickPending = true;
      g_clickPoint = {GET_X_LPARAM(lp), GET_Y_LPARAM(lp)};
      break;
    case WM_PAINT: {
      // After REAPER painted: our visuals move in the same frame.
      const LRESULT r = DefSubclassProc(h, msg, wp, lp);
      if (g_attached && h == g_tv) {
        glue();
        commit();
      }
      return r;
    }
    case WM_NCDESTROY:
      if (h == g_tv) teardown();
      break;
    default: break;
  }
  return DefSubclassProc(h, msg, wp, lp);
}

bool createDevice(HWND tv) {
  static const CreateDevice2Fn create = [] {
    HMODULE m = LoadLibraryExW(L"dcomp.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    return m ? reinterpret_cast<CreateDevice2Fn>(GetProcAddress(m, "DCompositionCreateDevice2")) : nullptr;
  }();
  if (!create) return false;
  if (!g_d2dFactory && FAILED(D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, g_d2dFactory.GetAddressOf())))
    return false;
  if (!g_dw) {
    if (FAILED(DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED, __uuidof(IDWriteFactory),
                                   reinterpret_cast<IUnknown**>(g_dw.GetAddressOf()))))
      return false;
    readCapHeight();
  }
  HRESULT hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr,
                                 0, D3D11_SDK_VERSION, &g_d3d, nullptr, nullptr);
  if (FAILED(hr))
    hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_WARP, nullptr, D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0,
                           D3D11_SDK_VERSION, &g_d3d, nullptr, nullptr);
  ComPtr<IDXGIDevice> dxgi;
  if (FAILED(hr) || FAILED(g_d3d.As(&dxgi))) return false;
  if (FAILED(g_d2dFactory->CreateDevice(dxgi.Get(), &g_d2d))) return false;
  if (FAILED(g_d2d->CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE, &g_rec))) return false;
  if (FAILED(create(g_d2d.Get(), __uuidof(IDCompositionDesktopDevice), reinterpret_cast<void**>(g_dev.GetAddressOf()))))
    return false;
  if (FAILED(g_dev->CreateTargetForHwnd(tv, TRUE, &g_target))) return false;
  if (FAILED(g_dev->CreateVisual(&g_rootV)) || FAILED(g_dev->CreateVisual(&g_markV))) return false;
  g_rootV->SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR);
  g_rootV->SetBorderMode(DCOMPOSITION_BORDER_MODE_SOFT);
  if (FAILED(g_rootV->AddVisual(g_markV.Get(), TRUE, nullptr)) || FAILED(g_target->SetRoot(g_rootV.Get())))
    return false;
  // The follow frame sits above the marks and below every cursor.
  g_follow = makeNode(g_rootV.Get());
  for (Node& e : g_followEdge) {
    e = g_follow ? makeNode(g_follow.v.Get()) : Node{};
    if (!e) return false;
  }
  g_followTag = g_follow ? makeNode(g_follow.v.Get()) : Node{};
  if (!g_followTag) return false;
  setOpacityNow(g_follow, 0);
  g_dirty = true;
  commit();
  return !g_lost;
}

void releaseDevice() {
  g_doomed.clear();
  g_hover = Node{};
  g_follow = g_followTag = Node{};
  for (Node& e : g_followEdge) e = Node{};
  g_markV.Reset();
  if (g_target) g_target->SetRoot(nullptr);
  if (g_dev) g_dev->Commit();
  g_rootV.Reset();
  g_target.Reset();
  g_dev.Reset();
  g_rec.Reset();
  g_d2d.Reset();
  g_d3d.Reset();
  g_dirty = false;
  g_lost = false;
}

// Off the track view and every peer forgotten. g_main stays, so tick() can
// attach again; detach() clears it.
void teardown() {
  if (!g_attached) return;
  g_attached = false;
  if (IsWindow(g_tv)) KillTimer(g_tv, kTimerId);
  RemoveWindowSubclass(g_tv, trackViewProc, kSubclassId);
  g_peers.clear();
  g_pendingReverts.clear();
  g_marks.clear();
  g_marksDirty = false;
  g_hoverShown = false;
  g_localIn = false;
  g_followKey.clear();
  g_followName.clear();
  g_followColor.clear();
  g_followId.clear();
  g_clickPending = false;
  releaseDevice();
  g_tv = nullptr;
}

}  // namespace

namespace cursor_overlay {

bool attach(void* mainWindow, const Host& host) {
  if (g_attached) return true;
  // A failed attach creates and drops a graphics device: not on every tick.
  if (now() < g_attachRetryAt) return false;
  g_attachRetryAt = now() + 5;
  HWND main = static_cast<HWND>(mainWindow);
  HWND tv = main ? GetDlgItem(main, 1000) : nullptr;
  if (!tv) {
    if (!g_attachFailedLogged) trace("no track view (main window %p)", mainWindow);
    g_attachFailedLogged = true;
    return false;
  }
  g_host = host;
  g_main = main;
  g_scale = std::max(dpiOf(tv), 48) / 96.0;
  g_clipW = g_clipH = -1;
  if (!createDevice(tv)) {
    if (!g_attachFailedLogged) trace("no composition target on the track view");
    g_attachFailedLogged = true;
    releaseDevice();
    return false;
  }
  if (!SetWindowSubclass(tv, trackViewProc, kSubclassId, 0)) {
    releaseDevice();
    return false;
  }
  g_tv = tv;
  g_attachRetryAt = 0;
  // The timer drives the network pump and the glue pass between repaints;
  // like any WM_TIMER it only fires when REAPER's queue is empty.
  SetTimer(tv, kTimerId, USER_TIMER_MINIMUM, nullptr);
  g_reduceMotionChecked = 0;
  g_attached = true;
  glue();
  commit();
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
  commit();
}

void setActivity(const std::vector<Activity>& list) {
  if (!g_attached || !g_showCursors) return;
  const double t = now();
  std::map<std::string, const Activity*> by;
  for (const Activity& a : list)
    if (!a.id.empty() && !a.track.empty()) by.emplace(a.id, &a);
  for (const auto& [id, a] : by) {
    auto it = g_peers.find(id);
    if (it == g_peers.end()) {
      Peer p;
      p.id = id;
      p.name = a->name;
      p.color = a->color;
      it = g_peers.emplace(id, std::move(p)).first;
      buildLayers(it->second);
      refreshTag(it->second);
    }
    Peer& p = it->second;
    p.fxAt = t;
    if (p.fxTrack != a->track || p.fxLabel != a->label) {
      p.fxTrack = a->track;
      p.fxLabel = a->label;
      p.pendingSpring = true;
      refreshTag(p);
    }
  }
  for (auto& [id, p] : g_peers)
    if (!by.count(id) && !p.fxTrack.empty()) {
      p.fxTrack.clear();
      p.fxLabel.clear();
      p.pendingSpring = true;
      refreshTag(p);
    }
  commit();
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
  commit();
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
      ms.box = makeNode(g_markV.Get(), Z::Bottom);
      setOpacityNow(ms.box, 0);
      ms.lastWanted = t;
      g_marks.emplace(item, std::move(ms));
      g_marksDirty = true;
      continue;
    }
    it->second.lastWanted = t;
    if (it->second.m.color != m->color || it->second.m.name != m->name || it->second.m.loading != m->loading) {
      it->second.m = *m;
      if (g_hover) g_hover.look.clear();
      g_marksDirty = true;
    }
  }
  // Called right after the tick applied other people's edits, which move
  // items without touching anything the cheap check watches.
  if (!g_marks.empty()) g_marksDirty = true;
  if (g_marksDirty) glue();
  commit();
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
  const bool awayChanged = r.away.kind != p.away.kind || r.away.track != p.away.track ||
                           r.away.item != p.away.item || r.away.label != p.away.label;
  const bool dragChanged = r.drag.items != p.drag.items || r.drag.dt != p.drag.dt || r.drag.dtr != p.drag.dtr;
  p.away = r.away;
  p.drag = r.drag;
  if (awayChanged || dragChanged) p.pendingSpring = true;
  p.t = r.t;
  p.fy = r.fy;
  p.guid = r.guid;
  p.in = r.in;
  p.lastMsg = t;
  if (moved || r.click) p.lastMove = t;
  p.pendingSpring = p.pendingSpring || moved;
  p.pendingClick = p.pendingClick || r.click;
}

void tick() {
  // A lost graphics device (driver update, GPU reset): start over on a new
  // one. Peers come back with their next update. Any other failed draw is
  // only skipped (its look stays unset, so it is tried again).
  if (g_attached && g_lost) {
    g_lost = false;
    if (g_d3d && g_d3d->GetDeviceRemovedReason() != S_OK) {
      trace("graphics device lost; attaching again");
      teardown();
    }
  }
  // Detached by a lost device or the track view going away: attach again
  // (throttled); meanwhile draws() is false and the extension tints.
  if (!g_attached && g_main) attach(g_main, g_host);
  if (!g_attached) return;
  const double t = now();
  if (t - g_reduceMotionChecked > 2) {
    g_reduceMotionChecked = t;
    BOOL anim = TRUE;
    if (SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &anim, 0)) g_reduceMotion = !anim;
  }
  // Network updates that arrived through the extension's timer rather than
  // this layer's pump land here.
  glue(true);
  for (auto it = g_peers.begin(); it != g_peers.end();) {
    Peer& p = it->second;
    if (t - std::max(p.lastMsg, p.fxAt) > kStyle.forgetAfter) {
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
    // A cursor resting a while dims to half; waiting beside an FX window is
    // not resting, and the person we follow never dims.
    const bool idle = p.cursorShown && !p.parked && !p.followed && t - p.lastMove > kStyle.idleDimAfter;
    if (idle != p.idle) {
      fade(p.arrow, idle ? kStyle.idleOpacity : 1, idle ? kStyle.idleFade : kStyle.fadeIn);
      p.idle = idle;
    }
    ++it;
  }
  commit();
}

void revert(const Revert& r) {
  if (!g_attached || r.item.empty() || r.track.empty()) return;
  g_pendingReverts.push_back(r);
}

bool viewSize(double* w, double* h) {
  if (!g_attached) return false;
  RECT rc{};
  GetClientRect(g_tv, &rc);
  *w = rc.right - rc.left;
  *h = rc.bottom - rc.top;
  return *w > 0 && *h > 0;
}

double pixelScale() { return g_scale; }

// The logical left button: the right one when the buttons are swapped.
bool mouseHeld() {
  return (GetAsyncKeyState(GetSystemMetrics(SM_SWAPBUTTON) ? VK_RBUTTON : VK_LBUTTON) & 0x8000) != 0;
}

bool pointerOver(void* hwnd) {
  POINT p{};
  HWND h = static_cast<HWND>(hwnd);
  if (!h || !GetCursorPos(&p)) return false;
  HWND under = WindowFromPoint(p);
  return under && GetAncestor(under, GA_ROOT) == GetAncestor(h, GA_ROOT);
}

bool pointerOverApp() {
  POINT p{};
  if (!GetCursorPos(&p)) return false;
  HWND under = WindowFromPoint(p);
  DWORD pid = 0;
  if (under) GetWindowThreadProcessId(under, &pid);
  return under && pid == GetCurrentProcessId();
}

bool sampleLocal(Local* out) {
  if (!g_attached) return false;
  *out = Local{};
  const Geo g = readGeo();
  if (!g.ok) return true;
  // Only when the track view itself is under the pointer: a floating window
  // over the arrange covers it.
  POINT sp{}, cp{};
  bool over = false;
  if (IsWindowVisible(g_tv) && GetCursorPos(&sp) && WindowFromPoint(sp) == g_tv) {
    cp = sp;
    ScreenToClient(g_tv, &cp);
    RECT rc{};
    GetClientRect(g_tv, &rc);
    over = PtInRect(&rc, cp) != FALSE;
  }
  const Pt vp{static_cast<double>(cp.x), static_cast<double>(cp.y)};
  // Hovering a marked item shows who is editing it.
  if (over != g_localIn || (over && !samePoint(vp, g_localPoint))) {
    g_localIn = over;
    g_localPoint = vp;
    if (!g_marks.empty() || g_hoverShown) {
      layoutHover();
      commit();
    }
  }
  if (!over) {
    g_clickPending = false;
    return true;
  }
  const bool down = mouseHeld();
  Pt at = vp;
  if (g_clickPending) {
    at = {static_cast<double>(g_clickPoint.x), static_cast<double>(g_clickPoint.y)};
    out->click = true;
    g_clickPending = false;
  }
  out->in = semanticAt(at, g, &out->t, &out->guid, &out->fy);
  out->down = out->in && down;
  out->px = at.x;
  out->py = at.y;
  return true;
}

}  // namespace cursor_overlay
