// macOS cursor layer: Core Animation layers in a click-through view on top of
// REAPER's track view (see cursor_overlay.h).
//
// Motion is split in two so the cursor stays glued to the arrange and still
// glides:
//  - the layer's model position is always the exact pixel of the peer's
//    semantic spot under the current zoom/scroll. It is rewritten (without
//    animation) in the same run-loop pass in which REAPER changes its view,
//    right before Core Animation commits, so it lands in the frame REAPER
//    repaints;
//  - each network update adds an additive spring animation from the previous
//    target to the new one. Those offsets run in the render server, so they
//    keep moving at display rate even while REAPER's main thread is busy, and
//    they ride on top of whatever the model position does.
//
// Prototype knobs (environment, read at attach):
//   REAMESH_CURSOR_SYNC=commit|timer|paint  when the glue pass runs (default commit)
//   REAMESH_CURSOR_DEMO=k          send a synthetic moving pointer (path phase k) instead of the mouse
//   REAMESH_CURSOR_FAKE=N          show N synthetic local peers (load test)
//   REAMESH_CURSOR_BUSY=ms:every   block the main thread for ms every `every` ms
//   REAMESH_CURSOR_KNOBS=<file>    re-read `busy=ms:every` from this file every second
//   REAMESH_CURSOR_SCROLLTEST=scroll|zoom|height|vscroll  animate the arrange view at 60 Hz
//   REAMESH_CURSOR_STATS=1         print timing stats to stderr every 5s
//   REAMESH_CURSOR_TRACE=1         log each local sample and each arrival (latency)
//   REAMESH_CURSOR_PARK=t:track:fy:#rrggbb  one synthetic peer parked at a spot

#import <Cocoa/Cocoa.h>
#import <QuartzCore/QuartzCore.h>
#import <objc/runtime.h>

#include <mach/mach_time.h>
#include <unistd.h>

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <map>
#include <string>
#include <vector>

#include "cursor_overlay.h"

namespace {

// ---------- every visual parameter, in one table ----------

struct Style {
  // Pointer: a soft arrowhead in the peer's colour with a thin light edge,
  // symmetric about its diagonal, with a curved notch at the back.
  double pointerSize;    // extent of the arrowhead along each axis, pt
  double pointerNotch;   // where the notch sits on the diagonal, 0..1 of the size
  double tipRadius, wingRadius, notchRadius;  // corner rounding, pt
  double outline;        // white edge, pt
  double shadowOpacity;
  double shadowRadius;
  double shadowOffsetY;  // pt, downwards
  // Name tag: a pill below-right of the tip.
  double tagFont;        // pt
  double tagHeight;
  double tagPadX;
  double tagDX, tagDY;   // tag origin relative to the tip
  double tagIdleAfter;   // s without movement before the tag fades
  double tagFadeOut, tagFadeIn;
  // Motion. A critically damped spring (mass 1) toward each new target.
  double springStiffness;
  double fadeIn, fadeOut;
  double staleAfter;     // s without any update before a cursor fades out
  double forgetAfter;    // s before a silent peer's layers are dropped
  // Click ripple.
  double rippleRadius, rippleLine, rippleScaleFrom, rippleScaleTo, rippleOpacity, rippleDuration;
  // Edge indicator for peers outside the visible arrange.
  double edgeInset, edgeHeight, edgeFont, edgePadX, edgeOpacity;
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
    .springStiffness = 800,
    .fadeIn = 0.18,
    .fadeOut = 0.35,
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
    .edgeOpacity = 0.92,
};

// ---------- state ----------

enum class Sync { Commit, Timer, Paint };
enum class Side { Left, Right, Top, Bottom };

struct Peer {
  std::string id, name, color;
  double t = 0, fy = 0;
  std::string guid;
  bool in = false;
  double lastMsg = 0, lastMove = 0;
  bool pendingSpring = false, pendingClick = false;

  CALayer* root = nil;  // positioned at the pointer tip
  CAShapeLayer* arrow = nil;
  CALayer* tag = nil;
  CATextLayer* tagText = nil;
  CALayer* edge = nil;
  CATextLayer* edgeText = nil;
  std::string tagLabel, edgeLabel;

  bool cursorShown = false, tagShown = false, edgeShown = false;
  CGPoint cursorModel = CGPointZero, edgeModel = CGPointZero;
  Side edgeSide = Side::Left;
};

struct Geo {
  bool ok = false;
  double t0 = 0, spp = 0, W = 0, H = 0;
};

struct Place {
  enum Kind { Hidden, Cursor, Edge } kind = Hidden;
  CGPoint p = CGPointZero;
  Side side = Side::Left;
};

cursor_overlay::Host g_host{};
bool g_attached = false;
bool g_attachFailedLogged = false;
NSView* g_tv = nil;
NSView* g_view = nil;
CALayer* g_root = nil;
CGFloat g_scale = 2;
std::map<std::string, Peer> g_peers;
CFRunLoopObserverRef g_observer = nullptr;
id g_clickMonitor = nil;
Class g_tvOrigClass = Nil;
bool g_clickPending = false;
NSPoint g_clickPoint = NSZeroPoint;

Sync g_sync = Sync::Commit;
int g_demo = 0;  // 0 off, else the synthetic path's phase
int g_fake = 0;
double g_busyMs = 0, g_busyEvery = 0, g_lastBusy = 0;
std::string g_scrollTest;
NSTimer* g_scrollTimer = nil;
double g_scrollT0 = 0, g_scrollT1 = 0, g_scrollStart = 0;
double g_lastFake = 0;
bool g_stats = false;
bool g_trace = false;  // REAMESH_CURSOR_TRACE: log every sample/arrival (latency measurement)
std::string g_park;

// Stats: glue lag (geometry change seen → layers moved), pass cost.
double g_changeAt = 0;
std::vector<double> g_lags;
unsigned g_ripples = 0, g_clicksOut = 0;
unsigned g_observerPasses = 0, g_gluePasses = 0, g_paintPasses = 0, g_netUpdates = 0;
double g_observerCost = 0, g_glueCost = 0;
double g_lastStats = 0;

double now() { return CACurrentMediaTime(); }

void logf(const char* fmt, ...) __attribute__((format(printf, 1, 2)));
void logf(const char* fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  fprintf(stderr, "[reamesh-cursor] ");
  vfprintf(stderr, fmt, ap);
  fprintf(stderr, "\n");
  va_end(ap);
}

// ---------- drawing helpers ----------

NSColor* parseColor(const std::string& hex) {
  unsigned v = 0x4c8bf5;
  if (hex.size() == 7 && hex[0] == '#') v = (unsigned)strtoul(hex.c_str() + 1, nullptr, 16);
  return [NSColor colorWithSRGBRed:((v >> 16) & 255) / 255.0
                             green:((v >> 8) & 255) / 255.0
                              blue:(v & 255) / 255.0
                             alpha:1];
}

bool isLight(const std::string& hex) {
  NSColor* c = parseColor(hex);
  const double l = 0.2126 * c.redComponent + 0.7152 * c.greenComponent + 0.0722 * c.blueComponent;
  return l > 0.62;
}

CGPathRef pointerPath() {
  // Hotspot at (0,0), y down. Four corners: the tip, two wings and the notch
  // between them, every one rounded. The rounding pulls the drawn tip inwards,
  // so the shape is shifted back by that amount to keep the hotspot on it.
  static CGPathRef path = nullptr;
  if (!path) {
    const double z = kStyle.pointerSize;
    const double wing = 0.37 * z;  // how far each wing leans off its axis
    const CGPoint tip = CGPointMake(0, 0), left = CGPointMake(wing, z), right = CGPointMake(z, wing);
    const double n = kStyle.pointerNotch * (z + wing) / 2;
    const CGPoint notch = CGPointMake(n, n);
    // The rounded tip sits r/sin(a/2) - r in from the corner along the
    // diagonal (a = the angle between the two edges at the tip).
    const double half = std::atan2(z, wing) - M_PI / 4;
    const double inset = (kStyle.tipRadius / std::sin(half) - kStyle.tipRadius) / std::sqrt(2.0);
    CGAffineTransform t = CGAffineTransformMakeTranslation(-inset, -inset);
    CGMutablePathRef p = CGPathCreateMutable();
    CGPathMoveToPoint(p, &t, (tip.x + left.x) / 2, (tip.y + left.y) / 2);
    CGPathAddArcToPoint(p, &t, left.x, left.y, notch.x, notch.y, kStyle.wingRadius);
    CGPathAddArcToPoint(p, &t, notch.x, notch.y, right.x, right.y, kStyle.notchRadius);
    CGPathAddArcToPoint(p, &t, right.x, right.y, tip.x, tip.y, kStyle.wingRadius);
    CGPathAddArcToPoint(p, &t, tip.x, tip.y, left.x, left.y, kStyle.tipRadius);
    CGPathCloseSubpath(p);
    path = p;
  }
  return path;
}

NSFont* tagFont(double size) {
  return [NSFont systemFontOfSize:size weight:NSFontWeightSemibold];
}

// Sets a pill (background layer + text layer) to `label`; returns its width.
double setPill(CALayer* pill, CATextLayer* text, const std::string& label, const std::string& color,
               double fontSize, double height, double padX) {
  NSFont* f = tagFont(fontSize);
  NSColor* fg = isLight(color) ? [NSColor colorWithSRGBRed:0.1 green:0.1 blue:0.1 alpha:1]
                               : [NSColor whiteColor];
  NSString* s = [NSString stringWithUTF8String:label.c_str()] ?: @"";
  NSAttributedString* as = [[NSAttributedString alloc]
      initWithString:s
          attributes:@{NSFontAttributeName : f, NSForegroundColorAttributeName : fg}];
  const double tw = std::ceil(as.size.width);
  const double lh = std::ceil(f.ascender - f.descender);
  const double w = tw + 2 * padX;
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  pill.bounds = CGRectMake(0, 0, w, height);
  pill.cornerRadius = height / 2;
  pill.backgroundColor = parseColor(color).CGColor;
  text.string = as;
  // Centre the capitals, not the line box: the baseline sits half a cap
  // height below the pill's middle.
  const double top = (height + f.capHeight) / 2 - f.ascender;
  text.frame = CGRectMake(padX, std::round(top * g_scale) / g_scale, tw, lh);
  [CATransaction commit];
  return w;
}

void setPos(CALayer* l, CGPoint p) {
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.position = p;
  [CATransaction commit];
}

// Additive spring from `from` to `to`: the model jumps, the offset decays in
// the render server.
void springTo(CALayer* l, CGPoint from, CGPoint to) {
  CASpringAnimation* a = [CASpringAnimation animationWithKeyPath:@"position"];
  a.additive = YES;
  a.mass = 1;
  a.stiffness = kStyle.springStiffness;
  a.damping = 2 * std::sqrt(kStyle.springStiffness);
  a.fromValue = [NSValue valueWithPoint:NSMakePoint(from.x - to.x, from.y - to.y)];
  a.toValue = [NSValue valueWithPoint:NSZeroPoint];
  a.duration = a.settlingDuration;
  if (@available(macOS 12.0, *)) a.preferredFrameRateRange = CAFrameRateRangeMake(60, 120, 120);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.position = to;
  [l addAnimation:a forKey:nil];
  [CATransaction commit];
}

void snapTo(CALayer* l, CGPoint p) {
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  for (NSString* k in l.animationKeys ?: @[])
    if (![k isEqualToString:@"fade"]) [l removeAnimationForKey:k];
  l.position = p;
  [CATransaction commit];
}

void fade(CALayer* l, float to, double dur) {
  CALayer* pres = l.presentationLayer;
  CABasicAnimation* a = [CABasicAnimation animationWithKeyPath:@"opacity"];
  a.fromValue = @(pres ? pres.opacity : l.opacity);
  a.toValue = @(to);
  a.duration = dur;
  a.timingFunction = [CAMediaTimingFunction functionWithName:kCAMediaTimingFunctionEaseOut];
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.opacity = to;
  [l addAnimation:a forKey:@"fade"];
  [CATransaction commit];
}

void ripple(Peer& p) {
  ++g_ripples;
  CAShapeLayer* r = [CAShapeLayer layer];
  const double rr = kStyle.rippleRadius;
  CGPathRef circle = CGPathCreateWithEllipseInRect(CGRectMake(-rr, -rr, 2 * rr, 2 * rr), nullptr);
  r.path = circle;
  CGPathRelease(circle);
  r.fillColor = nil;
  r.strokeColor = parseColor(p.color).CGColor;
  r.lineWidth = kStyle.rippleLine;
  r.contentsScale = g_scale;
  r.opacity = 0;

  CABasicAnimation* sc = [CABasicAnimation animationWithKeyPath:@"transform.scale"];
  sc.fromValue = @(kStyle.rippleScaleFrom);
  sc.toValue = @(kStyle.rippleScaleTo);
  CABasicAnimation* op = [CABasicAnimation animationWithKeyPath:@"opacity"];
  op.fromValue = @(kStyle.rippleOpacity);
  op.toValue = @0;
  CAAnimationGroup* g = [CAAnimationGroup animation];
  g.animations = @[ sc, op ];
  g.duration = kStyle.rippleDuration;
  g.timingFunction = [CAMediaTimingFunction functionWithName:kCAMediaTimingFunctionEaseOut];
  if (@available(macOS 12.0, *)) g.preferredFrameRateRange = CAFrameRateRangeMake(60, 120, 120);

  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  [CATransaction setCompletionBlock:^{
    [r removeFromSuperlayer];
  }];
  [p.root insertSublayer:r atIndex:0];
  [r addAnimation:g forKey:@"ripple"];
  [CATransaction commit];
}

void applyShadow(CALayer* l, CGPathRef path) {
  l.shadowColor = [NSColor blackColor].CGColor;
  l.shadowOpacity = kStyle.shadowOpacity;
  l.shadowRadius = kStyle.shadowRadius;
  l.shadowOffset = CGSizeMake(0, kStyle.shadowOffsetY);
  if (path) l.shadowPath = path;
}

void buildLayers(Peer& p) {
  [CATransaction begin];
  [CATransaction setDisableActions:YES];

  p.root = [CALayer layer];
  p.root.anchorPoint = CGPointZero;
  p.root.bounds = CGRectZero;
  p.root.opacity = 0;

  p.arrow = [CAShapeLayer layer];
  p.arrow.path = pointerPath();
  p.arrow.fillColor = parseColor(p.color).CGColor;
  p.arrow.strokeColor = [NSColor whiteColor].CGColor;
  p.arrow.lineWidth = kStyle.outline;
  p.arrow.lineJoin = kCALineJoinRound;
  p.arrow.contentsScale = g_scale;
  applyShadow(p.arrow, pointerPath());
  [p.root addSublayer:p.arrow];

  p.tag = [CALayer layer];
  p.tag.anchorPoint = CGPointZero;
  p.tag.position = CGPointMake(kStyle.tagDX, kStyle.tagDY);
  p.tag.contentsScale = g_scale;
  applyShadow(p.tag, nullptr);
  p.tag.shadowOpacity = kStyle.shadowOpacity * 0.6;
  p.tagText = [CATextLayer layer];
  p.tagText.contentsScale = g_scale;
  p.tagText.alignmentMode = kCAAlignmentLeft;
  [p.tag addSublayer:p.tagText];
  [p.root addSublayer:p.tag];
  p.tagShown = true;

  p.edge = [CALayer layer];
  p.edge.opacity = 0;
  p.edge.contentsScale = g_scale;
  applyShadow(p.edge, nullptr);
  p.edge.shadowOpacity = kStyle.shadowOpacity * 0.6;
  p.edgeText = [CATextLayer layer];
  p.edgeText.contentsScale = g_scale;
  [p.edge addSublayer:p.edgeText];

  [g_root addSublayer:p.edge];
  [g_root addSublayer:p.root];
  [CATransaction commit];
}

std::string displayName(const Peer& p) {
  if (!p.name.empty()) return p.name;
  return p.id.size() > 6 ? p.id.substr(p.id.size() - 6) : p.id;
}

void refreshTag(Peer& p) {
  const std::string label = displayName(p);
  if (label == p.tagLabel) return;
  p.tagLabel = label;
  setPill(p.tag, p.tagText, label, p.color, kStyle.tagFont, kStyle.tagHeight, kStyle.tagPadX);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  p.arrow.fillColor = parseColor(p.color).CGColor;
  [CATransaction commit];
}

void refreshEdge(Peer& p, Side side) {
  const std::string name = displayName(p);
  std::string label;
  switch (side) {
    case Side::Left: label = "\xE2\x97\x80 " + name; break;    // ◀
    case Side::Right: label = name + " \xE2\x96\xB6"; break;   // ▶
    case Side::Top: label = "\xE2\x96\xB2 " + name; break;     // ▲
    case Side::Bottom: label = "\xE2\x96\xBC " + name; break;  // ▼
  }
  if (label != p.edgeLabel) {
    p.edgeLabel = label;
    setPill(p.edge, p.edgeText, label, p.color, kStyle.edgeFont, kStyle.edgeHeight, kStyle.edgePadX);
  }
  CGPoint anchor = CGPointMake(0, 0.5);
  switch (side) {
    case Side::Left: anchor = CGPointMake(0, 0.5); break;
    case Side::Right: anchor = CGPointMake(1, 0.5); break;
    case Side::Top: anchor = CGPointMake(0.5, 0); break;
    case Side::Bottom: anchor = CGPointMake(0.5, 1); break;
  }
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  p.edge.anchorPoint = anchor;
  [CATransaction commit];
}

// ---------- geometry ----------

Geo readGeo() {
  Geo g;
  if (!g_attached || !g_host.ready || !g_host.ready()) return g;
  if (!g_host.timeAxis(&g.t0, &g.spp) || !(g.spp > 0)) return g;
  const NSRect b = g_view.bounds;
  g.W = b.size.width;
  g.H = b.size.height;
  g.ok = g.W > 0 && g.H > 0;
  return g;
}

Place placeOf(const Peer& p, const Geo& g, double t) {
  Place pl;
  if (!g.ok || !p.in || t - p.lastMsg > kStyle.staleAfter) return pl;
  const double x = (p.t - g.t0) / g.spp;
  double ty = 0, th = 0;
  const bool found = !p.guid.empty() && g_host.trackSpan(p.guid.c_str(), &ty, &th);
  const bool yKnown = found && th > 0;
  const double y = yKnown ? ty + p.fy * th : 0;
  const bool xIn = x >= 0 && x <= g.W;
  const bool yIn = yKnown && y >= 0 && y <= g.H;
  if (xIn && yIn) {
    pl.kind = Place::Cursor;
    pl.p = CGPointMake(std::round(x * g_scale) / g_scale, std::round(y * g_scale) / g_scale);
    return pl;
  }
  pl.kind = Place::Edge;
  const double in = kStyle.edgeInset;
  const double half = kStyle.edgeHeight / 2;
  const double cy = yKnown ? std::clamp(y, in + half, std::max(in + half, g.H - in - half)) : g.H / 2;
  const double cx = std::clamp(x, in + 40, std::max(in + 40, g.W - in - 40));
  if (!xIn) {
    pl.side = x < 0 ? Side::Left : Side::Right;
    pl.p = CGPointMake(x < 0 ? in : g.W - in, cy);
  } else if (yKnown) {
    pl.side = y < 0 ? Side::Top : Side::Bottom;
    pl.p = CGPointMake(cx, y < 0 ? in : g.H - in);
  } else {
    // Hidden track, or one this project doesn't have: the time is known.
    pl.side = Side::Bottom;
    pl.p = CGPointMake(cx, g.H - in);
  }
  return pl;
}

bool samePoint(CGPoint a, CGPoint b) { return std::fabs(a.x - b.x) < 0.01 && std::fabs(a.y - b.y) < 0.01; }

// Moves a peer's layers to where its spot is now. With a network update
// pending they spring toward it; otherwise only the view moved and they snap
// (glued). Returns whether the view moved them.
bool layoutPeer(Peer& p, const Geo& g, double t) {
  const Place pl = placeOf(p, g, t);
  bool moved = false;
  const bool net = p.pendingSpring;
  p.pendingSpring = false;

  if (pl.kind == Place::Cursor) {
    if (!p.cursorShown) {
      snapTo(p.root, pl.p);
      fade(p.root, 1, kStyle.fadeIn);
      p.cursorShown = true;
      moved = true;
    } else if (!samePoint(pl.p, p.cursorModel)) {
      if (net) springTo(p.root, p.cursorModel, pl.p);
      else setPos(p.root, pl.p);
      moved = true;
    }
    p.cursorModel = pl.p;
    if (p.pendingClick) ripple(p);
  } else if (p.cursorShown) {
    fade(p.root, 0, kStyle.fadeOut);
    p.cursorShown = false;
  }
  p.pendingClick = false;

  if (net) moved = false;
  if (pl.kind == Place::Edge) {
    if (!p.edgeShown || pl.side != p.edgeSide) {
      refreshEdge(p, pl.side);
      snapTo(p.edge, pl.p);
      if (!p.edgeShown) fade(p.edge, kStyle.edgeOpacity, kStyle.fadeIn);
      p.edgeShown = true;
      p.edgeSide = pl.side;
      moved = true;
    } else if (!samePoint(pl.p, p.edgeModel)) {
      if (net) springTo(p.edge, p.edgeModel, pl.p);
      else setPos(p.edge, pl.p);
      moved = !net;
    }
    p.edgeModel = pl.p;
  } else if (p.edgeShown) {
    fade(p.edge, 0, kStyle.fadeOut);
    p.edgeShown = false;
  }
  return moved;
}

// Would any layer move if laid out now? (lag detection only)
bool anyChange(const Geo& g, double t) {
  for (auto& [id, p] : g_peers) {
    const Place pl = placeOf(p, g, t);
    if (pl.kind == Place::Cursor && p.cursorShown && !samePoint(pl.p, p.cursorModel)) return true;
    if (pl.kind == Place::Edge && p.edgeShown && pl.side == p.edgeSide && !samePoint(pl.p, p.edgeModel))
      return true;
  }
  return false;
}

void checkScale() {
  const CGFloat s = g_tv.window ? g_tv.window.backingScaleFactor : 2;
  if (s == g_scale) return;
  g_scale = s;
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  for (auto& [id, p] : g_peers) {
    for (CALayer* l in @[ p.arrow, p.tag, p.tagText, p.edge, p.edgeText ]) l.contentsScale = s;
  }
  [CATransaction commit];
}

// The glue pass: every peer to its current pixel (`onlyPending`: just the
// peers with a network update waiting).
void glue(const char* why, bool onlyPending = false) {
  if (!g_attached || g_peers.empty()) return;
  const uint64_t a = mach_absolute_time();
  const double t = now();
  if (!NSEqualRects(g_view.frame, g_tv.bounds)) g_view.frame = g_tv.bounds;
  checkScale();
  const Geo g = readGeo();
  bool moved = false;
  for (auto& [id, p] : g_peers)
    if (!onlyPending || p.pendingSpring || p.pendingClick) moved |= layoutPeer(p, g, t);
  if (moved && g_changeAt > 0) {
    g_lags.push_back((t - g_changeAt) * 1000.0);
    g_changeAt = 0;
  }
  ++g_gluePasses;
  mach_timebase_info_data_t tb;
  mach_timebase_info(&tb);
  g_glueCost += double(mach_absolute_time() - a) * tb.numer / tb.denom / 1000.0;
  (void)why;
}

void onObserver() {
  if (!g_attached) return;
  if (g_host.pump) {
    // Network in/out at most every 4ms; the observer itself runs ~1000x/s.
    static double lastPump = 0;
    const double t = now();
    if (t - lastPump >= 0.004) {
      lastPump = t;
      g_host.pump();
    }
  }
  if (g_peers.empty()) return;
  ++g_observerPasses;
  if (g_sync == Sync::Commit) {
    glue("commit");
    return;
  }
  // Other modes: only note when the arrange moved under the cursors, so the
  // stats can say how long it took the chosen pass to catch up.
  if (g_changeAt > 0) return;
  const uint64_t a = mach_absolute_time();
  const Geo g = readGeo();
  if (anyChange(g, now())) g_changeAt = now();
  mach_timebase_info_data_t tb;
  mach_timebase_info(&tb);
  g_observerCost += double(mach_absolute_time() - a) * tb.numer / tb.denom / 1000.0;
}

void printStats() {
  const double t = now();
  if (!g_stats || t - g_lastStats < 5) return;
  const double span = g_lastStats > 0 ? t - g_lastStats : 5;
  g_lastStats = t;
  std::vector<double> l = g_lags;
  std::sort(l.begin(), l.end());
  double mean = 0;
  for (double v : l) mean += v;
  if (!l.empty()) mean /= l.size();
  const double p95 = l.empty() ? 0 : l[std::min(l.size() - 1, size_t(l.size() * 0.95))];
  const double mx = l.empty() ? 0 : l.back();
  const char* mode = g_sync == Sync::Commit ? "commit" : g_sync == Sync::Timer ? "timer" : "paint";
  logf("mode=%s peers=%zu net=%.1f/s glue=%.1f/s (%.1fus avg) observer=%.1f/s (%.1fus avg) paint=%.1f/s "
       "lag n=%zu mean=%.2fms p95=%.2fms max=%.2fms ripples=%u clicks_out=%u",
       mode, g_peers.size(), g_netUpdates / span, g_gluePasses / span,
       g_gluePasses ? g_glueCost / g_gluePasses : 0, g_observerPasses / span,
       (g_observerPasses && g_sync != Sync::Commit) ? g_observerCost / g_observerPasses : 0,
       g_paintPasses / span, l.size(), mean, p95, mx, g_ripples, g_clicksOut);
  g_lags.clear();
  g_netUpdates = g_gluePasses = g_observerPasses = g_paintPasses = 0;
  g_glueCost = g_observerCost = 0;
}

// ---------- synthetic motion (demo sender, fake peers) ----------

// A slow Lissajous path over the arrange, in arrange pixels.
NSPoint synthPoint(double t, int k, const Geo& g) {
  const double ph = k * 1.7;
  return NSMakePoint(g.W * (0.5 + 0.36 * std::sin(2 * M_PI * 0.11 * t + ph)),
                     g.H * (0.42 + 0.34 * std::sin(2 * M_PI * 0.17 * t + ph * 1.3)));
}

bool semanticAt(NSPoint vp, const Geo& g, double* tt, std::string* guid, double* fy) {
  if (!g.ok || !g_host.trackAt(vp.y, guid, fy)) return false;
  *tt = g.t0 + vp.x * g.spp;
  return true;
}

void fakeTick() {
  if (g_fake <= 0) return;
  const double t = now();
  if (t - g_lastFake < 0.05) return;
  g_lastFake = t;
  static const char* names[] = {"Lena", "Sam", "Alex", "Ben", "Mia", "Noah", "Ida", "Tom"};
  static const char* colors[] = {"#e5484d", "#30a46c", "#8e4ec6", "#f76b15",
                                 "#0090ff", "#d6409f", "#12a594", "#ffc53d"};
  const Geo g = readGeo();
  for (int k = 0; k < g_fake; ++k) {
    cursor_overlay::Remote r;
    r.id = "fake-" + std::to_string(k);
    r.name = names[k % 8];
    r.color = colors[k % 8];
    r.in = semanticAt(synthPoint(t, k + 1, g), g, &r.t, &r.guid, &r.fy);
    r.click = std::fmod(t + k, 4.0) < 0.05;
    cursor_overlay::update(r);
  }
}

void parkTick() {
  if (g_park.empty()) return;
  static double lastPark = 0;
  if (now() - lastPark < 1.0) return;
  lastPark = now();
  double t = 0, fy = 0;
  int idx = 0;
  char color[16] = "#00e5ff";
  if (sscanf(g_park.c_str(), "%lf:%d:%lf:%15s", &t, &idx, &fy, color) < 3) return;
  cursor_overlay::Remote r;
  r.id = "park";
  r.name = "Alex";
  r.color = color;
  r.t = t;
  r.fy = fy;
  r.in = g_host.trackGuidAt && g_host.trackGuidAt(idx, &r.guid);
  cursor_overlay::update(r);
}

void busyTick() {
  static double lastKnobs = 0;
  if (const char* knobs = getenv("REAMESH_CURSOR_KNOBS"); knobs && now() - lastKnobs > 1) {
    lastKnobs = now();
    if (FILE* f = fopen(knobs, "r")) {
      double ms = 0, every = 0;
      if (fscanf(f, "busy=%lf:%lf", &ms, &every) == 2) g_busyMs = ms, g_busyEvery = every;
      fclose(f);
    }
  }
  if (g_busyMs <= 0) return;
  const double t = now();
  if ((t - g_lastBusy) * 1000 < g_busyEvery) return;
  g_lastBusy = t;
  usleep(useconds_t(g_busyMs * 1000));
}

void startScrollTest() {
  if (g_scrollTest.empty() || !g_host.setTimeAxis) return;
  g_scrollTimer = [NSTimer timerWithTimeInterval:1.0 / 60
                                         repeats:YES
                                           block:^(NSTimer*) {
                                             if (!g_host.ready || !g_host.ready()) return;
                                             double t0 = 0, spp = 0;
                                             if (g_scrollT1 <= g_scrollT0) {
                                               if (!g_host.timeAxis(&t0, &spp)) return;
                                               g_scrollT0 = t0;
                                               g_scrollT1 = t0 + spp * g_view.bounds.size.width;
                                               g_scrollStart = now();
                                             }
                                             const double span = g_scrollT1 - g_scrollT0;
                                             const double ph = std::sin(2 * M_PI * 0.25 * (now() - g_scrollStart));
                                             if (g_scrollTest == "height") {
                                               const int hgt = int(64 + 40 * ph);
                                               g_host.testNudge(0, 0, hgt);
                                               return;
                                             }
                                             if (g_scrollTest == "vscroll") {
                                               static int step = 0;
                                               ++step;
                                               if (step % 4 == 0) g_host.testNudge(1, 0, (step / 120) % 2 ? -1 : 1);
                                               return;
                                             }
                                             if (g_scrollTest == "zoom") {
                                               const double c = g_scrollT0 + span / 2;
                                               const double h = span / 2 * (1 + 0.35 * ph);
                                               g_host.setTimeAxis(c - h, c + h);
                                             } else {
                                               const double d = 0.25 * span * ph;
                                               g_host.setTimeAxis(g_scrollT0 + d, g_scrollT1 + d);
                                             }
                                           }];
  [[NSRunLoop mainRunLoop] addTimer:g_scrollTimer forMode:NSRunLoopCommonModes];
}

// ---------- the paint hook (REAMESH_CURSOR_SYNC=paint) ----------

void installPaintHook() {
  g_tvOrigClass = object_getClass(g_tv);
  NSString* name = [NSString stringWithFormat:@"ReaMeshCursorTV_%s", class_getName(g_tvOrigClass)];
  Class sub = NSClassFromString(name);
  if (!sub) {
    sub = objc_allocateClassPair(g_tvOrigClass, name.UTF8String, 0);
    Method m = class_getInstanceMethod(g_tvOrigClass, @selector(drawRect:));
    IMP orig = method_getImplementation(m);
    IMP imp = imp_implementationWithBlock(^(id self, NSRect r) {
      ((void (*)(id, SEL, NSRect))orig)(self, @selector(drawRect:), r);
      ++g_paintPasses;
      glue("paint");
    });
    class_addMethod(sub, @selector(drawRect:), imp, method_getTypeEncoding(m));
    objc_registerClassPair(sub);
  }
  object_setClass(g_tv, sub);
}

NSView* findTrackView(void* mainWindow) {
  id obj = (__bridge id)mainWindow;
  NSView* main = nil;
  if ([obj isKindOfClass:[NSWindow class]]) main = [(NSWindow*)obj contentView];
  else if ([obj isKindOfClass:[NSView class]]) main = (NSView*)obj;
  if (!main) return nil;
  for (NSView* v in main.subviews)
    if (v.tag == 1000) return v;
  return [main viewWithTag:1000];
}

}  // namespace

// The click-through, layer-hosting overlay view.
@interface ReaMeshCursorView : NSView
@end
@implementation ReaMeshCursorView
- (NSView*)hitTest:(NSPoint)point {
  return nil;
}
- (BOOL)isFlipped {
  return YES;
}
- (BOOL)acceptsFirstResponder {
  return NO;
}
@end

namespace cursor_overlay {

bool attach(void* mainWindow, const Host& host) {
  if (g_attached) return true;
  @autoreleasepool {
    NSView* tv = mainWindow ? findTrackView(mainWindow) : nil;
    if (!tv) {
      if (!g_attachFailedLogged) logf("no track view (main window %p)", mainWindow);
      g_attachFailedLogged = true;
      return false;
    }
    g_host = host;
    g_tv = tv;
    g_scale = tv.window ? tv.window.backingScaleFactor : 2;

    const char* sync = getenv("REAMESH_CURSOR_SYNC");
    g_sync = !sync ? Sync::Commit
             : !strcmp(sync, "timer") ? Sync::Timer
             : !strcmp(sync, "paint") ? Sync::Paint
                                      : Sync::Commit;
    g_demo = getenv("REAMESH_CURSOR_DEMO") ? std::max(1, atoi(getenv("REAMESH_CURSOR_DEMO"))) : 0;
    g_fake = getenv("REAMESH_CURSOR_FAKE") ? atoi(getenv("REAMESH_CURSOR_FAKE")) : 0;
    if (const char* b = getenv("REAMESH_CURSOR_BUSY")) sscanf(b, "%lf:%lf", &g_busyMs, &g_busyEvery);
    g_scrollTest = getenv("REAMESH_CURSOR_SCROLLTEST") ? getenv("REAMESH_CURSOR_SCROLLTEST") : "";
    g_stats = getenv("REAMESH_CURSOR_STATS") != nullptr;
    g_trace = getenv("REAMESH_CURSOR_TRACE") != nullptr;
    g_park = getenv("REAMESH_CURSOR_PARK") ? getenv("REAMESH_CURSOR_PARK") : "";

    ReaMeshCursorView* v = [[ReaMeshCursorView alloc] initWithFrame:tv.bounds];
    CALayer* root = [CALayer layer];
    root.masksToBounds = YES;
    v.layer = root;  // layer-hosting: AppKit leaves the sublayers alone
    v.wantsLayer = YES;
    root.geometryFlipped = YES;
    v.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    [tv addSubview:v positioned:NSWindowAbove relativeTo:nil];
    g_view = v;
    g_root = root;

    // Before Core Animation's commit (order 2000000): whatever REAPER did to
    // its view in this pass is on screen in the same frame as our layers.
    g_observer = CFRunLoopObserverCreateWithHandler(
        kCFAllocatorDefault, kCFRunLoopBeforeWaiting | kCFRunLoopExit, true, 1000,
        ^(CFRunLoopObserverRef, CFRunLoopActivity) {
          @autoreleasepool {
            onObserver();
          }
        });
    CFRunLoopAddObserver(CFRunLoopGetMain(), g_observer, kCFRunLoopCommonModes);

    g_clickMonitor = [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskLeftMouseDown
                                                           handler:^NSEvent*(NSEvent* e) {
                                                             if (g_view && e.window == g_tv.window) {
                                                               const NSPoint vp = [g_view convertPoint:e.locationInWindow
                                                                                              fromView:nil];
                                                               if (NSPointInRect(vp, g_view.bounds)) {
                                                                 g_clickPending = true;
                                                                 g_clickPoint = vp;
                                                               }
                                                             }
                                                             return e;
                                                           }];
    if (g_sync == Sync::Paint) installPaintHook();
    g_attached = true;
    startScrollTest();
    logf("attached to %s %.0fx%.0f flipped=%d layer=%s scale=%.0f sync=%s demo=%d fake=%d",
         class_getName(object_getClass(tv)), tv.bounds.size.width, tv.bounds.size.height, tv.isFlipped,
         tv.layer ? class_getName([tv.layer class]) : "none", g_scale, sync ? sync : "commit", g_demo, g_fake);
  }
  return true;
}

void detach() {
  if (!g_attached) return;
  @autoreleasepool {
    if (g_scrollTimer) [g_scrollTimer invalidate];
    g_scrollTimer = nil;
    if (g_observer) {
      CFRunLoopRemoveObserver(CFRunLoopGetMain(), g_observer, kCFRunLoopCommonModes);
      CFRelease(g_observer);
      g_observer = nullptr;
    }
    if (g_clickMonitor) [NSEvent removeMonitor:g_clickMonitor];
    g_clickMonitor = nil;
    if (g_tvOrigClass && g_tv) object_setClass(g_tv, g_tvOrigClass);
    g_tvOrigClass = Nil;
    g_peers.clear();
    [g_view removeFromSuperview];
    g_view = nil;
    g_root = nil;
    g_tv = nil;
    g_attached = false;
  }
}

void update(const Remote& r) {
  if (!g_attached || r.id.empty()) return;
  @autoreleasepool {
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
    if (r.color != p.color && !r.color.empty()) {
      p.color = r.color;
      p.tagLabel.clear();
      p.edgeLabel.clear();
    }
    if (!r.name.empty()) p.name = r.name;
    refreshTag(p);
    const bool moved = r.t != p.t || r.guid != p.guid || r.fy != p.fy || r.in != p.in;
    p.t = r.t;
    p.fy = r.fy;
    p.guid = r.guid;
    p.in = r.in;
    p.lastMsg = t;
    if (moved || r.click) p.lastMove = t;
    p.pendingSpring = p.pendingSpring || moved;
    p.pendingClick = p.pendingClick || r.click;
    ++g_netUpdates;
    if (g_trace)
      fprintf(stderr, "[trace] rx %.3f %.9f\n", [[NSDate date] timeIntervalSince1970] * 1000, r.t);
  }
}

void tick() {
  if (!g_attached) return;
  @autoreleasepool {
    busyTick();
    fakeTick();
    parkTick();
    const double t = now();
    // Network updates land here, with a spring; in timer mode the view glue
    // rides along. In commit mode the observer would also catch them before
    // this pass commits.
    glue("tick", g_sync != Sync::Timer);
    for (auto it = g_peers.begin(); it != g_peers.end();) {
      Peer& p = it->second;
      if (t - p.lastMsg > kStyle.forgetAfter) {
        [p.root removeFromSuperlayer];
        [p.edge removeFromSuperlayer];
        it = g_peers.erase(it);
        continue;
      }
      const bool wantTag = t - p.lastMove < kStyle.tagIdleAfter;
      if (wantTag != p.tagShown) {
        fade(p.tag, wantTag ? 1 : 0, wantTag ? kStyle.tagFadeIn : kStyle.tagFadeOut);
        p.tagShown = wantTag;
      }
      ++it;
    }
    printStats();
  }
}

bool sampleLocal(Local* out) {
  if (!g_attached) return false;
  @autoreleasepool {
    *out = Local{};
    const Geo g = readGeo();
    if (!g.ok) return true;
    NSPoint vp;
    if (g_demo) {
      vp = synthPoint(now(), g_demo * 3, g);
      if (std::fmod(now(), 3.0) < 0.04) {
        g_clickPending = true;
        g_clickPoint = vp;
      }
    } else {
      NSWindow* w = g_tv.window;
      if (!w || !w.visible) return true;
      const NSPoint sp = [NSEvent mouseLocation];
      if ([NSWindow windowNumberAtPoint:sp belowWindowWithWindowNumber:0] != w.windowNumber) return true;
      vp = [g_view convertPoint:[w convertPointFromScreen:sp] fromView:nil];
      if (!NSPointInRect(vp, g_view.bounds)) return true;
    }
    if (g_clickPending) {
      vp = g_clickPoint;
      out->click = true;
      g_clickPending = false;
      ++g_clicksOut;
    }
    out->in = semanticAt(vp, g, &out->t, &out->guid, &out->fy);
    if (g_trace && out->in)
      fprintf(stderr, "[trace] tx %.3f %.9f\n", [[NSDate date] timeIntervalSince1970] * 1000, out->t);
    out->px = vp.x;
    out->py = vp.y;
  }
  return true;
}

}  // namespace cursor_overlay
