// Peer cursor glyphs and the morph between them, shared by both platform
// layers (cursor_overlay_mac.mm, cursor_overlay_win.cpp).
//
// Someone's cursor shows what REAPER shows them: the plain pointer, or a
// glyph for trimming an item edge, dragging a fade, a razor edit and so on.
// Each glyph is a few parts (closed polygons of kPoints points, see
// cursor_glyphs.inc); a part belongs to a layer, and a layer reads as one
// silhouette: the white edges of all its parts first, then all their fills.
//
// Switching glyphs morphs: every part of the old glyph is paired with the
// closest part of the new one and its points glide to the new shape, the
// leftovers shrink away, new parts grow from their middle. The morph always
// starts from where the previous one is right now, so a quick change of mind
// stays one continuous movement. The platform layers render a Morph: the Mac
// hands each part's start and end to Core Animation with the same timing
// curve, Windows evaluates it per frame.
#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <string>
#include <vector>

namespace cursor_glyphs {

constexpr int kPoints = 96;
constexpr double kUnit = 1.0 / 32;  // the stored coordinates are 1/32 pt

struct GlyphDef {
  const char* key;  // on the wire; "" is the plain pointer
  int first, count;
  float maxX, maxY;  // right and bottom extent, pt
};

#include "cursor_glyphs.inc"

constexpr int kGlyphCount = static_cast<int>(sizeof(kGlyphDefs) / sizeof(kGlyphDefs[0]));
enum Kind { Solid = 0, Tint = 1, Ink = 2 };

// The glyph for a wire key; unknown keys (a newer build's) show the pointer.
inline int glyphFor(const std::string& key) {
  for (int i = 0; i < kGlyphCount; ++i)
    if (key == kGlyphDefs[i].key) return i;
  return 0;
}

// The name tag's offset from the hotspot: below the glyph.
inline void tagOffset(int glyph, double baseX, double baseY, double* x, double* y) {
  *x = baseX;
  *y = glyph <= 0 ? baseY : std::max(baseY, kGlyphDefs[glyph].maxY + 4.0);
}

// How a part is painted, interpolated during a morph. The edge is a white
// stroke of `edge` pt behind the fill (so half of it shows outside), filled
// white too when `edgeFill` is 1; the fill is the person's colour at
// `fillAlpha`, blended towards white by `white`.
struct Look {
  double edge = 0, edgeFill = 0, fillAlpha = 1, white = 0;
};

// outline: the white edge width the pointer has always had (kStyle.outline).
inline Look lookOf(int kind, double outline) {
  switch (kind) {
    case Tint: return {1.2, 0, 0.3, 0};
    case Ink: return {0, 0, 1, 1};
    default: return {2 * outline, 1, 1, 0};
  }
}

inline Look mix(const Look& a, const Look& b, double u) {
  return {a.edge + (b.edge - a.edge) * u, a.edgeFill + (b.edgeFill - a.edgeFill) * u,
          a.fillAlpha + (b.fillAlpha - a.fillAlpha) * u, a.white + (b.white - a.white) * u};
}

// cubic-bezier(0.4, 0, 0.2, 1): leaves gently and lands softly. The Mac
// passes the same control points to Core Animation.
constexpr double kEaseX1 = 0.4, kEaseY1 = 0, kEaseX2 = 0.2, kEaseY2 = 1;

inline double ease(double x) {
  if (x <= 0) return 0;
  if (x >= 1) return 1;
  auto bez = [](double t, double p1, double p2) {
    const double u = 1 - t;
    return 3 * u * u * t * p1 + 3 * u * t * t * p2 + t * t * t;
  };
  double lo = 0, hi = 1, t = x;
  for (int i = 0; i < 40; ++i) {
    t = (lo + hi) / 2;
    if (bez(t, kEaseX1, kEaseX2) < x) lo = t;
    else hi = t;
  }
  return bez(t, kEaseY1, kEaseY2);
}

struct Slot {
  int id = 0;  // stable while the slot lives: the platform layer keys its drawing on it
  float from[kPoints * 2] = {}, to[kPoints * 2] = {};
  Look a, b;
  double opA = 0, opB = 1;
  int layer = 0;
  int kind = Solid;
  bool dying = false;
};

class Morph {
 public:
  std::vector<Slot> slots;
  int glyph = -1;
  double start = 0, dur = 0;

  bool running(double now) const { return now < start + dur; }
  double progress(double now) const { return dur <= 0 ? 1 : ease((now - start) / dur); }

  // A slot as it is at eased progress u.
  static void current(const Slot& s, double u, float* pts, Look* look, double* op) {
    for (int i = 0; i < kPoints * 2; ++i) pts[i] = static_cast<float>(s.from[i] + (s.to[i] - s.from[i]) * u);
    if (look) *look = mix(s.a, s.b, u);
    if (op) *op = s.opA + (s.opB - s.opA) * u;
  }

  // Morph to `next` over `seconds`, from wherever the shapes are at `now`
  // (seconds = 0: jump). False when it shows that glyph already.
  bool set(int next, double now, double seconds, double outline) {
    if (next < 0 || next >= kGlyphCount || next == glyph) return false;
    const double u = progress(now);
    for (Slot& s : slots) {
      float pts[kPoints * 2];
      Look look;
      double op = 0;
      current(s, u, pts, &look, &op);
      std::copy(pts, pts + kPoints * 2, s.from);
      s.a = look;
      s.opA = op;
    }
    const GlyphDef& g = kGlyphDefs[next];
    std::vector<int> partOf(slots.size(), -1);
    std::vector<bool> taken(g.count, false);
    // Greedy pairing, cheapest first: same family (ink with ink), close
    // centres, similar size. A slot already on its way out pays extra, so a
    // live part wins when both are close.
    for (;;) {
      double best = 1e18;
      int bs = -1, bp = -1;
      for (size_t si = 0; si < slots.size(); ++si) {
        if (partOf[si] >= 0) continue;
        const Slot& s = slots[si];
        double sx, sy, sa;
        centre(s.from, &sx, &sy, &sa);
        for (int pi = 0; pi < g.count; ++pi) {
          if (taken[pi]) continue;
          const int kind = kGlyphPartInfo[g.first + pi][0];
          if ((kind == Ink) != (s.kind == Ink)) continue;
          float pp[kPoints * 2];
          partPoints(g.first + pi, pp);
          double px, py, pa;
          centre(pp, &px, &py, &pa);
          const double ds = std::sqrt(std::fabs(sa)) - std::sqrt(std::fabs(pa));
          double c = (sx - px) * (sx - px) + (sy - py) * (sy - py) + 0.5 * ds * ds;
          if (s.dying) c += 40 + 40 * (1 - s.opA);
          if (c < best) {
            best = c;
            bs = static_cast<int>(si);
            bp = pi;
          }
        }
      }
      if (bs < 0) break;
      partOf[bs] = bp;
      taken[bp] = true;
    }
    for (size_t si = 0; si < slots.size(); ++si) {
      Slot& s = slots[si];
      if (partOf[si] >= 0) {
        const int part = g.first + partOf[si];
        float pp[kPoints * 2];
        partPoints(part, pp);
        aligned(s.from, pp, s.to);
        s.kind = kGlyphPartInfo[part][0];
        s.layer = kGlyphPartInfo[part][1];
        s.b = lookOf(s.kind, outline);
        s.opB = 1;
        s.dying = false;
      } else {
        shrunk(s.from, s.to);
        s.b = s.a;
        s.opB = 0;
        s.dying = true;
      }
    }
    for (int pi = 0; pi < g.count; ++pi) {
      if (taken[pi]) continue;
      Slot s;
      s.id = m_nextId++;
      const int part = g.first + pi;
      partPoints(part, s.to);
      shrunk(s.to, s.from);
      s.kind = kGlyphPartInfo[part][0];
      s.layer = kGlyphPartInfo[part][1];
      s.a = s.b = lookOf(s.kind, outline);
      s.opA = 0;
      s.opB = 1;
      slots.push_back(s);
    }
    glyph = next;
    start = now;
    dur = seconds;
    return true;
  }

  // Drops the slots that have finished shrinking away; returns their ids.
  std::vector<int> prune(double now) {
    std::vector<int> gone;
    if (running(now)) return gone;
    for (auto it = slots.begin(); it != slots.end();) {
      if (it->dying) {
        gone.push_back(it->id);
        it = slots.erase(it);
      } else {
        ++it;
      }
    }
    return gone;
  }

 private:
  int m_nextId = 1;

  static void partPoints(int part, float* out) {
    for (int i = 0; i < kPoints * 2; ++i) out[i] = static_cast<float>(kGlyphPoints[part][i] * kUnit);
  }

  // Centre (of the points) and signed area of a polygon.
  static void centre(const float* p, double* cx, double* cy, double* area) {
    double x = 0, y = 0, a = 0;
    for (int i = 0; i < kPoints; ++i) {
      const int j = (i + 1) % kPoints;
      x += p[2 * i];
      y += p[2 * i + 1];
      a += p[2 * i] * p[2 * j + 1] - p[2 * j] * p[2 * i + 1];
    }
    *cx = x / kPoints;
    *cy = y / kPoints;
    *area = a / 2;
  }

  // `to` rotated along its ring so that it lines up with `from` best: every
  // point travels as little as it can.
  static void aligned(const float* from, const float* to, float* out) {
    int best = 0;
    double bestD = 1e18;
    for (int k = 0; k < kPoints; ++k) {
      double d = 0;
      for (int i = 0; i < kPoints && d < bestD; ++i) {
        const int j = (i + k) % kPoints;
        const double dx = from[2 * i] - to[2 * j], dy = from[2 * i + 1] - to[2 * j + 1];
        d += dx * dx + dy * dy;
      }
      if (d < bestD) {
        bestD = d;
        best = k;
      }
    }
    for (int i = 0; i < kPoints; ++i) {
      const int j = (i + best) % kPoints;
      out[2 * i] = to[2 * j];
      out[2 * i + 1] = to[2 * j + 1];
    }
  }

  // A part drawn small about its own centre: where new parts grow from and
  // where leftovers shrink to.
  static void shrunk(const float* p, float* out) {
    double cx, cy, a;
    centre(p, &cx, &cy, &a);
    for (int i = 0; i < kPoints; ++i) {
      out[2 * i] = static_cast<float>(cx + (p[2 * i] - cx) * 0.2);
      out[2 * i + 1] = static_cast<float>(cy + (p[2 * i + 1] - cy) * 0.2);
    }
  }
};

// The pointer's look as REAPER shows it to the local user, matched against
// REAPER's own cursor files by name. Each platform layer finds the cursor
// REAPER shows right now and asks which key it stands for.
struct CursorName {
  const char* file;  // REAPER's cursor resource / file name
  const char* key;   // glyph key
};
inline constexpr CursorName kCursorNames[] = {
    {"size-left", "trim_l"},       {"border_left", "trim_l"},      {"size-border", "trim_m"},
    {"size-right", "trim_r"},      {"border_right", "trim_r"},     {"curve_fadein", "fade_in"},
    {"curve_fadeout", "fade_out"}, {"adjustfade2", "fade_curve"},  {"fade_width", "fade_len"},
    {"curve_fadeinout_l_r", "xfade"}, {"xfade_move", "xfade_move"}, {"arrowslip", "slip"},
    {"razor", "razor"},            {"razor_move", "razor_move"},   {"razor_copy", "razor_copy"},
};

}  // namespace cursor_glyphs
