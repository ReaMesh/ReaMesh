// macOS cursor layer: Core Animation layers in a click-through view on top of
// REAPER's track view (see cursor_overlay.h). Draws other people's cursors and
// an outline around every item someone else is editing.
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
//   REAMESH_CURSOR_DEMO=story      send a scripted 40s tour of every cursor state (see storyPoint)
//   REAMESH_CURSOR_IDLE=s          dim resting cursors after s seconds instead of 10
//   REAMESH_CURSOR_REDUCE_MOTION=1 behave as with the system's "Reduce motion"
//   REAMESH_CURSOR_DEMO_ITEM=<guid>  with DEMO: the synthetic pointer rides on that item
//   REAMESH_CURSOR_FAKE=N          show N synthetic local peers (load test)
//   REAMESH_CURSOR_BUSY=ms:every   block the main thread for ms every `every` ms
//   REAMESH_CURSOR_KNOBS=<file>    re-read `busy=ms:every` from this file every second
//   REAMESH_CURSOR_SCROLLTEST=scroll|zoom|height|vscroll  animate the arrange view at 60 Hz
//   REAMESH_CURSOR_STATS=1         print timing stats to stderr every 5s
//   REAMESH_CURSOR_TRACE=1         log each local sample and each arrival (latency)
//   REAMESH_CURSOR_PARK=t:track:fy:#rrggbb  one synthetic peer parked at a spot
//   REAMESH_CURSOR_SNAPSHOT=<file.png>  with STATS: render the layer tree there every 5s

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
#include <set>
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
  // Name tag: a pill below-right of the tip, on a softer spring of its own so
  // it trails the pointer a little.
  double tagFont;        // pt
  double tagHeight;
  double tagPadX;
  double tagDX, tagDY;   // tag origin relative to the tip
  double tagIdleAfter;   // s without movement before the tag fades
  double tagFadeOut, tagFadeIn;
  double tagStiffness;
  // Motion. A critically damped spring (mass 1) toward each new target.
  double springStiffness;
  // Prediction: the cursor is drawn ahead along its recent velocity, by this
  // much time, at most this far; the push is taken back once updates stop.
  double predictLead, predictMax, predictStopAfter;
  // A move further than this fades across instead of flying over the arrange.
  double jumpCrossfade, crossfade;
  double fadeIn, fadeOut;
  double appearScale;    // a cursor grows from this as it appears, shrinks to it as it goes
  double pressedScale, pressDuration;  // while the button is held
  double idleDimAfter, idleOpacity, idleFade;  // a cursor resting this long dims
  double staleAfter;     // s without any update before a cursor fades out
  double forgetAfter;    // s before a silent peer's layers are dropped
  // Click ripple.
  double rippleRadius, rippleLine, rippleScaleFrom, rippleScaleTo, rippleOpacity, rippleDuration;
  // Edge indicator for peers outside the visible arrange: a pill with their
  // name and a small arrow pointing where they are.
  double edgeInset, edgeHeight, edgeFont, edgePadX, edgeArrow, edgeOpacity;
  // Someone working away from the arrange (FX window, mixer, track panel):
  // their cursor waits this far in from the arrange's left edge, on that
  // track's row; in the MIDI editor, this far into the item they edit.
  double parkInset, parkItemInset;
  // Someone dragging items: a dashed outline in their colour where the items
  // would land; the originals are marked like a lock.
  double dragLine, dragDash, dragGap, dragFill;
  // Our own edit undone: a dashed copy of it holds, glides back into the
  // item, and fades.
  double revertHold, revertGlide, revertFade, revertOpacity;
  // Following someone: a frame round the arrange.
  double followLine, followOpacity, followTagInset;
  // Item someone else is editing: dimmed ("not yours right now") with a thin
  // outline in their colour; their name shows while the local pointer is over
  // it. A mark stays up a little after the lock lapses, so the pauses
  // between someone's edits don't make it flicker.
  double markLine, markRadius, markDim, markLinger, markFadeIn, markFadeOut;
  double markTagFont, markTagHeight, markTagPadX, markTagGap;
  // Placeholder whose media is still downloading: a dark veil, no outline.
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

// ---------- state ----------

enum class Sync { Commit, Timer, Paint };
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
  // Their item drag, and its layers per item: the original marked, the
  // landing spot outlined.
  cursor_overlay::Drag drag;
  struct DragItem {
    CALayer* origin = nil;
    CAShapeLayer* ghost = nil;
    CGRect ghostRect = CGRectZero;
    bool shown = false;
  };
  std::map<std::string, DragItem> dragItems;
  std::string parkLabel;

  CALayer* root = nil;  // positioned at the pointer tip
  CAShapeLayer* arrow = nil;
  CALayer* tag = nil;   // a sibling of root, so it can trail it
  CATextLayer* tagText = nil;
  CALayer* edge = nil;
  CATextLayer* edgeText = nil;
  CAShapeLayer* edgeArrow = nil;
  std::string tagLabel, edgeLabel;

  bool cursorShown = false, tagShown = false, edgeShown = false, parked = false;
  bool idle = false, pressed = false;
  CGPoint cursorModel = CGPointZero, tagModel = CGPointZero, edgeModel = CGPointZero;
  Side edgeSide = Side::Left;
};

// One marked item.
struct MarkState {
  cursor_overlay::Mark m;
  CALayer* box = nil;
  double lastWanted = 0;  // when setMarks last asked for it
  bool shown = false;
  CGRect rect = CGRectZero;  // in the view, unclipped; valid while shown
};

struct Geo {
  bool ok = false;
  double t0 = 0, spp = 0, W = 0, H = 0;
};

struct Place {
  enum Kind { Hidden, Cursor, Edge } kind = Hidden;
  CGPoint p = CGPointZero;
  Side side = Side::Left;
  double angle = 0;     // Edge: direction from the pill to the real spot
  bool parked = false;  // waiting while they work away from the arrange
  std::string label;    // what they work in, for the name tag
};

cursor_overlay::Host g_host{};
bool g_attached = false;
bool g_attachFailedLogged = false;
NSView* g_tv = nil;
NSView* g_view = nil;
CALayer* g_root = nil;
CGFloat g_scale = 2;
std::map<std::string, Peer> g_peers;
bool g_showCursors = true;
// System setting "Reduce motion" (or REAMESH_CURSOR_REDUCE_MOTION): no
// springs, no scaling, cursors fade instead of travelling.
bool g_reduceMotion = false;
double g_reduceMotionChecked = 0;
double g_idleDimAfter = kStyle.idleDimAfter;  // REAMESH_CURSOR_IDLE overrides (demo)
// Item marks live in their own layer below the cursors.
CALayer* g_markRoot = nil;
std::map<std::string, MarkState> g_marks;
bool g_marksDirty = false;
// REAPER reports an item's rows as of its last draw, which can trail a zoom
// or scroll by a pass: after any change, marks are laid out on every pass
// until this time.
double g_marksSettleUntil = 0;
double g_markKey[5] = {};
unsigned long long g_markStamp = 0;
// The name of whoever edits the item under the local pointer.
CALayer* g_hoverTag = nil;
CATextLayer* g_hoverText = nil;
std::string g_hoverItem, g_hoverLabel;
bool g_hoverShown = false;
bool g_localIn = false;
NSPoint g_localPoint = NSZeroPoint;
// The follow frame.
CALayer* g_followFrame = nil;
CALayer* g_followTag = nil;
CATextLayer* g_followText = nil;
std::string g_followKey;
CFRunLoopObserverRef g_observer = nullptr;
id g_clickMonitor = nil;
Class g_tvOrigClass = Nil;
bool g_clickPending = false;
NSPoint g_clickPoint = NSZeroPoint;

Sync g_sync = Sync::Commit;
int g_demo = 0;  // 0 off, else the synthetic path's phase
bool g_story = false;  // REAMESH_CURSOR_DEMO=story: the scripted feature tour
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

NSColor* inkOn(const std::string& hex) {
  return isLight(hex) ? [NSColor colorWithSRGBRed:0.1 green:0.1 blue:0.1 alpha:1] : [NSColor whiteColor];
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

// A small rounded triangle pointing along +x, centred on (0,0).
CGPathRef edgeArrowPath() {
  static CGPathRef path = nullptr;
  if (!path) {
    const double a = kStyle.edgeArrow / 2;
    CGMutablePathRef p = CGPathCreateMutable();
    CGPathMoveToPoint(p, nullptr, -a * 0.8, -a);
    CGPathAddLineToPoint(p, nullptr, a, 0);
    CGPathAddLineToPoint(p, nullptr, -a * 0.8, a);
    CGPathCloseSubpath(p);
    path = p;
  }
  return path;
}

NSFont* tagFont(double size) {
  return [NSFont systemFontOfSize:size weight:NSFontWeightSemibold];
}

// Sets a pill (background layer + text layer) to `label`, leaving `lead` pt
// free before the text; returns its width.
double setPill(CALayer* pill, CATextLayer* text, const std::string& label, const std::string& color,
               double fontSize, double height, double padX, double lead = 0) {
  NSFont* f = tagFont(fontSize);
  NSString* s = [NSString stringWithUTF8String:label.c_str()] ?: @"";
  NSAttributedString* as = [[NSAttributedString alloc]
      initWithString:s
          attributes:@{NSFontAttributeName : f, NSForegroundColorAttributeName : inkOn(color)}];
  const double tw = std::ceil(as.size.width);
  const double lh = std::ceil(f.ascender - f.descender);
  const double w = tw + 2 * padX + lead;
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  pill.bounds = CGRectMake(0, 0, w, height);
  pill.cornerRadius = height / 2;
  pill.backgroundColor = parseColor(color).CGColor;
  text.string = as;
  // Centre the capitals, not the line box: the baseline sits half a cap
  // height below the pill's middle.
  const double top = (height + f.capHeight) / 2 - f.ascender;
  text.frame = CGRectMake(padX + lead, std::round(top * g_scale) / g_scale, tw, lh);
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
void springTo(CALayer* l, CGPoint from, CGPoint to, double stiffness) {
  CASpringAnimation* a = [CASpringAnimation animationWithKeyPath:@"position"];
  a.additive = YES;
  a.mass = 1;
  a.stiffness = stiffness;
  a.damping = 2 * std::sqrt(stiffness);
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

// Drops the position springs (fades and scaling stay).
void snapTo(CALayer* l, CGPoint p) {
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  for (NSString* k in l.animationKeys ?: @[])
    if (![k isEqualToString:@"fade"] && ![k isEqualToString:@"pop"]) [l removeAnimationForKey:k];
  l.position = p;
  [CATransaction commit];
}

void fade(CALayer* l, float to, double dur, NSNumber* from = nil) {
  CALayer* pres = l.presentationLayer;
  CABasicAnimation* a = [CABasicAnimation animationWithKeyPath:@"opacity"];
  a.fromValue = from ?: @(pres ? pres.opacity : l.opacity);
  a.toValue = @(to);
  a.duration = dur;
  a.timingFunction = [CAMediaTimingFunction functionWithName:kCAMediaTimingFunctionEaseOut];
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.opacity = to;
  [l addAnimation:a forKey:@"fade"];
  [CATransaction commit];
}

// Animates a uniform scale (about the layer's anchor), under `key`.
void scaleTo(CALayer* l, double to, double dur, NSString* key, NSNumber* from = nil) {
  CALayer* pres = l.presentationLayer;
  CABasicAnimation* a = [CABasicAnimation animationWithKeyPath:@"transform.scale"];
  a.fromValue = from ?: [(pres ?: l) valueForKeyPath:@"transform.scale"];
  a.toValue = @(to);
  a.duration = dur;
  a.timingFunction = [CAMediaTimingFunction functionWithName:kCAMediaTimingFunctionEaseOut];
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.transform = CATransform3DMakeScale(to, to, 1);
  [l addAnimation:a forKey:key];
  [CATransaction commit];
}

// Appear: fade in while growing from appearScale. `fromZero` starts the fade
// at nothing even if the layer was visible a moment ago (a crossfade).
void popIn(CALayer* l, float opacity, bool fromZero = false) {
  fade(l, opacity, kStyle.fadeIn, fromZero ? @0 : nil);
  if (g_reduceMotion) return;
  scaleTo(l, 1, kStyle.fadeIn * 1.6, @"pop", fromZero ? @(kStyle.appearScale) : nil);
}

void popOut(CALayer* l) {
  fade(l, 0, kStyle.fadeOut);
  if (g_reduceMotion) return;
  scaleTo(l, kStyle.appearScale, kStyle.fadeOut, @"pop");
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
  sc.fromValue = @(g_reduceMotion ? 1.0 : kStyle.rippleScaleFrom);
  sc.toValue = @(g_reduceMotion ? 1.0 : kStyle.rippleScaleTo);
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

CAShapeLayer* makeArrow(const std::string& color) {
  CAShapeLayer* a = [CAShapeLayer layer];
  a.path = pointerPath();
  a.fillColor = parseColor(color).CGColor;
  a.strokeColor = [NSColor whiteColor].CGColor;
  a.lineWidth = kStyle.outline;
  a.lineJoin = kCALineJoinRound;
  a.contentsScale = g_scale;
  applyShadow(a, pointerPath());
  return a;
}

void buildLayers(Peer& p) {
  [CATransaction begin];
  [CATransaction setDisableActions:YES];

  p.root = [CALayer layer];
  p.root.anchorPoint = CGPointZero;
  p.root.bounds = CGRectZero;
  p.root.opacity = 0;
  // Scaling about the tip: the arrow is drawn around (0,0) of a zero-size layer.
  p.arrow = makeArrow(p.color);
  [p.root addSublayer:p.arrow];

  p.tag = [CALayer layer];
  p.tag.anchorPoint = CGPointZero;
  p.tag.opacity = 0;
  p.tag.contentsScale = g_scale;
  applyShadow(p.tag, nullptr);
  p.tag.shadowOpacity = kStyle.shadowOpacity * 0.6;
  p.tagText = [CATextLayer layer];
  p.tagText.contentsScale = g_scale;
  p.tagText.alignmentMode = kCAAlignmentLeft;
  [p.tag addSublayer:p.tagText];

  p.edge = [CALayer layer];
  p.edge.opacity = 0;
  p.edge.contentsScale = g_scale;
  applyShadow(p.edge, nullptr);
  p.edge.shadowOpacity = kStyle.shadowOpacity * 0.6;
  p.edgeText = [CATextLayer layer];
  p.edgeText.contentsScale = g_scale;
  [p.edge addSublayer:p.edgeText];
  p.edgeArrow = [CAShapeLayer layer];
  p.edgeArrow.path = edgeArrowPath();
  p.edgeArrow.lineJoin = kCALineJoinRound;
  p.edgeArrow.lineWidth = 1.5;
  p.edgeArrow.contentsScale = g_scale;
  [p.edge addSublayer:p.edgeArrow];

  [g_root addSublayer:p.edge];
  [g_root addSublayer:p.tag];
  [g_root addSublayer:p.root];
  [CATransaction commit];
}

void dropDragItem(Peer::DragItem& d);

void dropLayers(Peer& p) {
  for (auto& [item, d] : p.dragItems) dropDragItem(d);
  p.dragItems.clear();
  [p.root removeFromSuperlayer];
  [p.tag removeFromSuperlayer];
  [p.edge removeFromSuperlayer];
}

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

void refreshTag(Peer& p) {
  const std::string label = displayLabel(p);
  if (label == p.tagLabel) return;
  p.tagLabel = label;
  setPill(p.tag, p.tagText, label, p.color, kStyle.tagFont, kStyle.tagHeight, kStyle.tagPadX);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  p.arrow.fillColor = parseColor(p.color).CGColor;
  [CATransaction commit];
}

void refreshEdge(Peer& p, Side side) {
  const std::string label = displayLabel(p);
  if (label != p.edgeLabel) {
    p.edgeLabel = label;
    const double lead = kStyle.edgeArrow + 4;
    setPill(p.edge, p.edgeText, label, p.color, kStyle.edgeFont, kStyle.edgeHeight, kStyle.edgePadX, lead);
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    p.edgeArrow.position = CGPointMake(kStyle.edgePadX + kStyle.edgeArrow / 2, kStyle.edgeHeight / 2);
    p.edgeArrow.fillColor = inkOn(p.color).CGColor;
    p.edgeArrow.strokeColor = inkOn(p.color).CGColor;
    [CATransaction commit];
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

void setEdgeAngle(Peer& p, double angle) {
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  p.edgeArrow.transform = CATransform3DMakeRotation(angle, 0, 0, 1);
  [CATransaction commit];
}

// The button is held (or let go): the pointer shrinks a little about its tip.
void setPressed(Peer& p, bool down) {
  if (down == p.pressed) return;
  p.pressed = down;
  scaleTo(p.arrow, down ? kStyle.pressedScale : 1.0, kStyle.pressDuration, @"press");
}

// The cursor fades across instead of flying: a copy fades out where it was,
// the cursor fades (and grows) in at its new spot.
void crossfadeTo(Peer& p, CGPoint at, CGPoint tagAt) {
  CALayer* pres = p.root.presentationLayer;
  const CGPoint from = pres ? pres.position : p.cursorModel;
  const float op = (pres ? pres.opacity : 1) * p.arrow.opacity;
  CAShapeLayer* ghost = makeArrow(p.color);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  ghost.position = from;
  ghost.transform = p.arrow.transform;
  [g_root insertSublayer:ghost below:p.root];
  [CATransaction commit];
  [CATransaction begin];
  [CATransaction setCompletionBlock:^{
    [ghost removeFromSuperlayer];
  }];
  fade(ghost, 0, kStyle.crossfade, @(op));
  [CATransaction commit];
  snapTo(p.root, at);
  snapTo(p.tag, tagAt);
  popIn(p.root, 1, true);
  if (p.tagShown) fade(p.tag, 1, kStyle.crossfade, @0);
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
  double parkX = kStyle.parkInset, parkFy = 0.5;
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
      parkX = (i0 - g.t0) / g.spp + kStyle.parkItemInset;
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
    const double d = std::hypot(dx, dy);
    if (d > kStyle.predictMax) dx *= kStyle.predictMax / d, dy *= kStyle.predictMax / d;
    x += dx;
    y = std::clamp(y + dy, ty, ty + th);
  }
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
  // Which way the arrow in the pill points: from the pill to the real spot
  // (straight out of the edge when the height is unknown).
  const double ry = yKnown ? y : (pl.side == Side::Bottom ? g.H + 100 : pl.p.y);
  pl.angle = std::atan2(ry - pl.p.y, x - pl.p.x);
  if (!xIn && !yKnown) pl.angle = x < 0 ? M_PI : 0;
  return pl;
}

bool samePoint(CGPoint a, CGPoint b) { return std::fabs(a.x - b.x) < 0.01 && std::fabs(a.y - b.y) < 0.01; }

// The name tag shows while the cursor moves, and always while it waits
// beside an FX window.
void updateTag(Peer& p, double t) {
  const bool want = p.cursorShown && (p.parked || t - p.lastMove < kStyle.tagIdleAfter);
  if (want == p.tagShown) return;
  fade(p.tag, want ? 1 : 0, want ? kStyle.tagFadeIn : kStyle.tagFadeOut);
  p.tagShown = want;
}

CGPathRef roundedPath(CGSize sz) {
  return CGPathCreateWithRoundedRect(CGRectMake(0, 0, sz.width, sz.height),
                                     std::min(kStyle.markRadius, sz.width / 2),
                                     std::min(kStyle.markRadius, sz.height / 2), nullptr);
}

CAShapeLayer* dashedBox(NSColor* stroke, NSColor* fill) {
  CAShapeLayer* l = [CAShapeLayer layer];
  l.anchorPoint = CGPointZero;
  l.strokeColor = stroke.CGColor;
  l.fillColor = fill.CGColor;
  l.lineWidth = kStyle.dragLine;
  l.lineDashPattern = @[ @(kStyle.dragDash), @(kStyle.dragGap) ];
  l.contentsScale = g_scale;
  return l;
}

void setBox(CAShapeLayer* l, CGRect r) {
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  l.position = r.origin;
  if (!CGSizeEqualToSize(l.bounds.size, r.size)) {
    l.bounds = CGRectMake(0, 0, r.size.width, r.size.height);
    CGPathRef path = roundedPath(r.size);
    l.path = path;
    CGPathRelease(path);
  }
  [CATransaction commit];
}

// Where an item is, and where it would land moved by `dt` and `dtr` tracks.
bool itemRects(const std::string& item, double dt, int dtr, const Geo& g, CGRect* at, CGRect* to) {
  double t0 = 0, t1 = 0, y = 0, h = 0, ty = 0, th = 0;
  std::string track, dest;
  if (!g.ok || !g_host.itemSpan || !g_host.itemSpan(item.c_str(), &t0, &t1, &y, &h, &track)) return false;
  if (!g_host.trackSpan(track.c_str(), &ty, &th)) return false;
  *at = CGRectMake((t0 - g.t0) / g.spp, y, (t1 - t0) / g.spp, h);
  double dy = 0;
  if (dtr != 0 && g_host.trackShift && g_host.trackShift(track.c_str(), dtr, &dest)) {
    double dy0 = 0, dh = 0;
    if (g_host.trackSpan(dest.c_str(), &dy0, &dh) && dh > 0) dy = dy0 - ty;
  }
  *to = CGRectMake(at->origin.x + dt / g.spp, y + dy, at->size.width, h);
  return true;
}

void dropDragItem(Peer::DragItem& d) {
  [d.origin removeFromSuperlayer];
  [d.ghost removeFromSuperlayer];
}

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
  NSColor* c = parseColor(p.color);
  for (const std::string& item : want) {
    Peer::DragItem& d = p.dragItems[item];
    if (!d.ghost) {
      [CATransaction begin];
      [CATransaction setDisableActions:YES];
      d.origin = [CALayer layer];
      d.origin.anchorPoint = CGPointZero;
      d.origin.cornerRadius = kStyle.markRadius;
      d.origin.borderWidth = kStyle.markLine;
      d.origin.borderColor = c.CGColor;
      d.origin.backgroundColor = [NSColor colorWithWhite:0 alpha:kStyle.markDim].CGColor;
      d.ghost = dashedBox(c, [c colorWithAlphaComponent:kStyle.dragFill]);
      [g_markRoot addSublayer:d.origin];
      [g_markRoot addSublayer:d.ghost];
      [CATransaction commit];
    }
    CGRect at, to;
    if (!itemRects(item, p.drag.dt, p.drag.dtr, g, &at, &to)) continue;
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    d.origin.frame = at;
    [CATransaction commit];
    if (net && d.shown && !g_reduceMotion) {
      const CGPoint from = d.ghostRect.origin;
      setBox(d.ghost, to);
      springTo(d.ghost, from, to.origin, kStyle.springStiffness);
    } else {
      setBox(d.ghost, to);
    }
    d.ghostRect = to;
    d.shown = true;
  }
}

// Our undone edits, waiting for the next pass to know where the item went.
std::vector<cursor_overlay::Revert> g_pendingReverts;

void runRevert(const cursor_overlay::Revert& r, const Geo& g) {
  double ty = 0, th = 0, t0 = 0, t1 = 0, y = 0, h = 0;
  if (!g.ok || !g_host.trackSpan(r.track.c_str(), &ty, &th) || th <= 0) return;
  if (!g_host.itemSpan || !g_host.itemSpan(r.item.c_str(), &t0, &t1, &y, &h, nullptr)) return;
  const CGRect from = CGRectMake((r.t0 - g.t0) / g.spp, ty + r.yOff, (r.t1 - r.t0) / g.spp, r.h > 0 ? r.h : h);
  const CGRect to = CGRectMake((t0 - g.t0) / g.spp, y, (t1 - t0) / g.spp, h);
  if (CGRectEqualToRect(CGRectIntegral(from), CGRectIntegral(to))) return;
  if (!CGRectIntersectsRect(CGRectUnion(from, to), CGRectMake(0, 0, g.W, g.H))) return;
  CAShapeLayer* l = dashedBox([NSColor colorWithWhite:1 alpha:kStyle.revertOpacity],
                              [NSColor colorWithWhite:1 alpha:0.08]);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  setBox(l, to);
  l.opacity = 0;
  [g_markRoot addSublayer:l];
  [CATransaction commit];

  // Hold at our version, glide into the item, fade.
  const double hold = kStyle.revertHold, glide = g_reduceMotion ? 0 : kStyle.revertGlide;
  const double total = hold + glide + kStyle.revertFade;
  CABasicAnimation* pos = [CABasicAnimation animationWithKeyPath:@"position"];
  pos.fromValue = [NSValue valueWithPoint:NSPointFromCGPoint(from.origin)];
  pos.toValue = [NSValue valueWithPoint:NSPointFromCGPoint(to.origin)];
  pos.beginTime = hold;
  pos.duration = std::max(glide, 0.001);
  pos.fillMode = kCAFillModeBackwards;
  pos.timingFunction = [CAMediaTimingFunction functionWithName:kCAMediaTimingFunctionEaseInEaseOut];
  CGPathRef fromPath = roundedPath(from.size), toPath = roundedPath(to.size);
  CABasicAnimation* shape = [CABasicAnimation animationWithKeyPath:@"path"];
  shape.fromValue = (__bridge id)fromPath;
  shape.toValue = (__bridge id)toPath;
  shape.beginTime = hold;
  shape.duration = pos.duration;
  shape.fillMode = kCAFillModeBackwards;
  shape.timingFunction = pos.timingFunction;
  CAKeyframeAnimation* op = [CAKeyframeAnimation animationWithKeyPath:@"opacity"];
  op.values = @[ @0, @1, @1, @0 ];
  op.keyTimes = @[ @0, @(0.08 / total), @((hold + glide) / total), @1 ];
  op.duration = total;
  CAAnimationGroup* grp = [CAAnimationGroup animation];
  grp.animations = @[ pos, shape, op ];
  grp.duration = total;
  if (@available(macOS 12.0, *)) grp.preferredFrameRateRange = CAFrameRateRangeMake(60, 120, 120);
  [CATransaction begin];
  [CATransaction setCompletionBlock:^{
    [l removeFromSuperlayer];
  }];
  [l addAnimation:grp forKey:@"revert"];
  [CATransaction commit];
  CGPathRelease(fromPath);
  CGPathRelease(toPath);
}

// Moves a peer's layers to where its spot is now. With a network update
// pending they spring toward it (or fade across on a long jump); otherwise
// only the view moved and they snap (glued). Returns whether the view moved
// them.
bool layoutPeer(Peer& p, const Geo& g, double t) {
  const Place pl = placeOf(p, g, t);
  bool moved = false;
  const bool net = p.pendingSpring;
  p.pendingSpring = false;
  if (pl.parked != p.parked || pl.label != p.parkLabel) {
    p.parked = pl.parked;
    p.parkLabel = pl.label;
    refreshTag(p);
    p.edgeLabel.clear();
  }
  if (!p.drag.items.empty() && t - p.lastMsg > kStyle.staleAfter) p.drag = {};
  layoutDrag(p, g, net);

  if (pl.kind == Place::Cursor) {
    const CGPoint tagAt = CGPointMake(pl.p.x + kStyle.tagDX, pl.p.y + kStyle.tagDY);
    if (!p.cursorShown) {
      snapTo(p.root, pl.p);
      snapTo(p.tag, tagAt);
      popIn(p.root, 1);
      p.cursorShown = true;
      moved = true;
    } else if (!samePoint(pl.p, p.cursorModel)) {
      const double dist = std::hypot(pl.p.x - p.cursorModel.x, pl.p.y - p.cursorModel.y);
      if (net && dist > kStyle.jumpCrossfade) {
        crossfadeTo(p, pl.p, tagAt);
      } else if (net && !g_reduceMotion) {
        springTo(p.root, p.cursorModel, pl.p, kStyle.springStiffness);
        springTo(p.tag, p.tagModel, tagAt, kStyle.tagStiffness);
      } else {
        setPos(p.root, pl.p);
        setPos(p.tag, tagAt);
      }
      moved = true;
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

  if (net) moved = false;
  if (pl.kind == Place::Edge) {
    setEdgeAngle(p, pl.angle);
    if (!p.edgeShown || pl.side != p.edgeSide) {
      refreshEdge(p, pl.side);
      snapTo(p.edge, pl.p);
      popIn(p.edge, kStyle.edgeOpacity, p.edgeShown);
      p.edgeShown = true;
      p.edgeSide = pl.side;
      moved = true;
    } else {
      refreshEdge(p, pl.side);
      if (!samePoint(pl.p, p.edgeModel)) {
        // Glides along the edge with the person.
        if (net && !g_reduceMotion) springTo(p.edge, p.edgeModel, pl.p, kStyle.springStiffness);
        else setPos(p.edge, pl.p);
        moved = !net;
      }
    }
    p.edgeModel = pl.p;
  } else if (p.edgeShown) {
    popOut(p.edge);
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
    for (CALayer* l in @[ p.arrow, p.tag, p.tagText, p.edge, p.edgeText, p.edgeArrow ]) l.contentsScale = s;
  }
  for (CALayer* l in @[ g_hoverTag ?: [CALayer layer], g_hoverText ?: [CALayer layer], g_followTag ?: [CALayer layer],
                        g_followText ?: [CALayer layer] ])
    l.contentsScale = s;
  g_hoverLabel.clear();
  g_followKey.clear();
  [CATransaction commit];
}

void layoutMarks(const Geo& g);

// The glue pass: every peer to its current pixel (`onlyPending`: just the
// peers with a network update waiting).
void glue(const char* why, bool onlyPending = false) {
  if (!g_attached) return;
  if (!NSEqualRects(g_view.frame, g_tv.bounds)) g_view.frame = g_tv.bounds;
  if (g_peers.empty() && g_marks.empty() && g_pendingReverts.empty()) return;
  const uint64_t a = mach_absolute_time();
  const double t = now();
  checkScale();
  const Geo g = readGeo();
  if (!onlyPending && g.ok && !g_pendingReverts.empty()) {
    for (const auto& r : g_pendingReverts) runRevert(r, g);
    g_pendingReverts.clear();
  }
  if (!onlyPending) layoutMarks(g);
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
  if (g_peers.empty() && g_marks.empty() && g_pendingReverts.empty()) return;
  ++g_observerPasses;
  // Updates stopped: the pointer came to rest, so take the prediction back.
  const double t = now();
  for (auto& [id, p] : g_peers)
    if (p.predict && t - p.lastMsg > kStyle.predictStopAfter) {
      p.predict = false;
      p.vt = p.vfy = 0;
      p.pendingSpring = true;
    }
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
  size_t shownMarks = 0;
  for (const auto& [id, ms] : g_marks) shownMarks += ms.shown;
  logf("marks=%zu shown=%zu hover=%d", g_marks.size(), shownMarks, (int)g_hoverShown);
  if (const char* snap = getenv("REAMESH_CURSOR_SNAPSHOT")) {
    const CGSize sz = g_view.bounds.size;
    NSBitmapImageRep* rep = [[NSBitmapImageRep alloc]
        initWithBitmapDataPlanes:nullptr pixelsWide:NSInteger(sz.width * g_scale) pixelsHigh:NSInteger(sz.height * g_scale)
                   bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO
                  colorSpaceName:NSCalibratedRGBColorSpace bytesPerRow:0 bitsPerPixel:0];
    NSGraphicsContext* ctx = [NSGraphicsContext graphicsContextWithBitmapImageRep:rep];
    CGContextRef cg = ctx.CGContext;
    CGContextScaleCTM(cg, g_scale, g_scale);
    CGContextTranslateCTM(cg, 0, sz.height);
    CGContextScaleCTM(cg, 1, -1);
    [g_root renderInContext:cg];
    [[rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:@(snap) atomically:YES];
  }
  g_lags.clear();
  g_netUpdates = g_gluePasses = g_observerPasses = g_paintPasses = 0;
  g_glueCost = g_observerCost = 0;
}

// ---------- item marks ----------

// Removes a layer once its fade-out has finished (unless it came back).
void fadeAndDrop(CALayer* l) {
  [CATransaction begin];
  [CATransaction setCompletionBlock:^{
    if (l.opacity == 0) [l removeFromSuperlayer];
  }];
  fade(l, 0, kStyle.markFadeOut);
  [CATransaction commit];
}

void styleMark(MarkState& ms) {
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  CALayer* b = ms.box;
  b.cornerRadius = kStyle.markRadius;
  if (ms.m.loading) {
    b.borderWidth = 0;
    b.backgroundColor = [NSColor colorWithWhite:0 alpha:kStyle.loadingVeil].CGColor;
  } else {
    b.borderWidth = kStyle.markLine;
    b.borderColor = parseColor(ms.m.color).CGColor;
    b.backgroundColor = [NSColor colorWithWhite:0 alpha:kStyle.markDim].CGColor;
  }
  [CATransaction commit];
}

// Where an item is in the view now; false when it is not visible.
bool markRect(const MarkState& ms, const Geo& g, CGRect* out) {
  double t0 = 0, t1 = 0, y = 0, h = 0;
  if (!g.ok || !g_host.itemSpan || !g_host.itemSpan(ms.m.item.c_str(), &t0, &t1, &y, &h, nullptr)) return false;
  if (!(h > 0) || !(t1 > t0)) return false;
  const double x0 = (t0 - g.t0) / g.spp, x1 = (t1 - g.t0) / g.spp;
  if (x1 <= 0 || x0 >= g.W || y + h <= 0 || y >= g.H) return false;
  auto px = [](double v) { return std::round(v * g_scale) / g_scale; };
  *out = CGRectMake(px(x0), px(y), std::max(px(x1) - px(x0), 1.0 / g_scale), px(y + h) - px(y));
  return true;
}

// The name tag above the hovered item (inside it when there is no room above).
void layoutHover() {
  const MarkState* hit = nullptr;
  if (g_localIn)
    for (const auto& [id, ms] : g_marks)
      if (ms.shown && !ms.m.loading && CGRectContainsPoint(ms.rect, g_localPoint)) hit = &ms;
  if (!hit) {
    if (g_hoverShown) fade(g_hoverTag, 0, kStyle.markFadeOut);
    g_hoverShown = false;
    g_hoverItem.clear();
    return;
  }
  if (!g_hoverTag) {
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    g_hoverTag = [CALayer layer];
    g_hoverTag.anchorPoint = CGPointZero;
    g_hoverTag.opacity = 0;
    g_hoverTag.contentsScale = g_scale;
    applyShadow(g_hoverTag, nullptr);
    g_hoverTag.shadowOpacity = kStyle.shadowOpacity * 0.6;
    g_hoverText = [CATextLayer layer];
    g_hoverText.contentsScale = g_scale;
    [g_hoverTag addSublayer:g_hoverText];
    [g_markRoot addSublayer:g_hoverTag];
    [CATransaction commit];
  }
  const std::string label = hit->m.name.empty() ? "\xE2\x80\xA2" : hit->m.name;  // •
  const std::string key = label + hit->m.color;
  if (key != g_hoverLabel) {
    g_hoverLabel = key;
    setPill(g_hoverTag, g_hoverText, label, hit->m.color, kStyle.markTagFont, kStyle.markTagHeight,
            kStyle.markTagPadX);
  }
  const CGRect r = hit->rect;
  const double above = r.origin.y - kStyle.markTagGap - kStyle.markTagHeight;
  const CGPoint at = CGPointMake(std::max(r.origin.x, 2.0),
                                 above >= 2 ? above : r.origin.y + kStyle.markTagGap);
  setPos(g_hoverTag, at);
  if (!g_hoverShown) fade(g_hoverTag, 1, kStyle.markFadeIn);
  g_hoverShown = true;
  g_hoverItem = hit->m.item;
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
  const bool changed = g_marksDirty || stamp != g_markStamp || !std::equal(key, key + 5, g_markKey);
  if (changed) g_marksSettleUntil = now() + 0.15;
  else if (now() > g_marksSettleUntil) return;
  g_marksDirty = false;
  g_markStamp = stamp;
  std::copy(key, key + 5, g_markKey);
  for (auto& [id, ms] : g_marks) {
    CGRect r;
    if (markRect(ms, g, &r)) {
      if (!CGRectEqualToRect(r, ms.rect) || !ms.shown) {
        [CATransaction begin];
        [CATransaction setDisableActions:YES];
        ms.box.frame = r;
        [CATransaction commit];
      }
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

// ---------- synthetic motion (demo sender, fake peers) ----------

// A slow Lissajous path over the arrange, in arrange pixels.
NSPoint synthPoint(double t, int k, const Geo& g) {
  const double ph = k * 1.7;
  return NSMakePoint(g.W * (0.5 + 0.36 * std::sin(2 * M_PI * 0.11 * t + ph)),
                     g.H * (0.42 + 0.34 * std::sin(2 * M_PI * 0.17 * t + ph * 1.3)));
}

// REAMESH_CURSOR_DEMO=story: a scripted pointer showing every cursor state
// in turn, on a 50s loop aligned to the wall clock (so a test script in the
// same instance can open an FX window and drop the dragged item in step).
// Returns false while the pointer is meant to be away from the arrange.
double storyPhase() { return std::fmod([[NSDate date] timeIntervalSince1970], 50.0); }

bool storyPoint(const Geo& g, NSPoint* vp, bool* down) {
  const double ph = storyPhase();
  static double lastPh = 0;
  const bool clickNow = lastPh < 10.0 && ph >= 10.0;
  lastPh = ph;
  const double W = g.W, H = g.H;
  auto lerp = [](double a, double b, double u) { return a + (b - a) * std::clamp(u, 0.0, 1.0); };
  auto ease = [](double u) { u = std::clamp(u, 0.0, 1.0); return u * u * (3 - 2 * u); };
  // The drag: grab the demo item (REAMESH_CURSOR_DEMO_ITEM) and carry it 4s
  // later and one track down. The start is fixed when the drag begins.
  static NSPoint grab = NSZeroPoint, drop = NSZeroPoint;
  if (ph < 5) {
    double t0 = 0, t1 = 0, y = 0, h = 0, ty = 0, th = 0;
    std::string track;
    if (const char* item = getenv("REAMESH_CURSOR_DEMO_ITEM");
        item && g_host.itemSpan && g_host.itemSpan(item, &t0, &t1, &y, &h, &track) &&
        g_host.trackSpan(track.c_str(), &ty, &th)) {
      grab = NSMakePoint((t0 + (t1 - t0) * 0.4 - g.t0) / g.spp, y + h * 0.5);
      drop = NSMakePoint(grab.x + 4.0 / g.spp, grab.y + th);
    } else {
      grab = NSMakePoint(0.3 * W, 0.3 * H);
      drop = NSMakePoint(0.45 * W, 0.4 * H);
    }
  }
  *down = false;
  if (ph < 4) {  // gliding about
    *vp = synthPoint(now(), 1, g);
  } else if (ph < 6) {  // over to the item
    const NSPoint a = synthPoint(now() - (ph - 4), 1, g);
    const double u = ease((ph - 4) / 2);
    *vp = NSMakePoint(lerp(a.x, grab.x, u), lerp(a.y, grab.y, u));
  } else if (ph < 9) {  // dragging it: button held
    const double u = ease((ph - 6.3) / 2.4);
    *vp = NSMakePoint(lerp(grab.x, drop.x, u), lerp(grab.y, drop.y, u));
    *down = true;
  } else if (ph < 11) {  // let go; a click
    *vp = drop;
    if (clickNow) {
      g_clickPending = true;
      g_clickPoint = *vp;
    }
  } else if (ph < 13.5) {  // a long jump, then a little circling
    const double a = (ph - 11) * 2.4;
    *vp = NSMakePoint(0.82 * W + 30 * std::sin(a), 0.74 * H + 22 * std::cos(a));
  } else if (ph < 18) {  // past the right edge, moving down the tracks
    *vp = NSMakePoint(W * 1.25, lerp(0.15 * H, 0.85 * H, ease((ph - 13.5) / 4.5)));
  } else if (ph < 20) {  // back in
    const double u = ease((ph - 18) / 2);
    *vp = NSMakePoint(lerp(W * 1.02, 0.6 * W, u), lerp(0.85 * H, 0.5 * H, u));
  } else if (ph < 28) {  // resting
    *vp = NSMakePoint(0.6 * W, 0.5 * H);
  } else if (ph < 43) {  // away: FX window, MIDI editor, mixer
    return false;
  } else {
    *vp = synthPoint(now(), 1, g);
  }
  return true;
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
    g_story = getenv("REAMESH_CURSOR_DEMO") && !strcmp(getenv("REAMESH_CURSOR_DEMO"), "story");
    if (const char* idle = getenv("REAMESH_CURSOR_IDLE")) g_idleDimAfter = std::max(0.5, atof(idle));
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
    g_markRoot = [CALayer layer];
    g_markRoot.anchorPoint = CGPointZero;
    g_markRoot.geometryFlipped = NO;
    [root addSublayer:g_markRoot];
    // The follow frame sits above the marks and below every cursor; it
    // resizes with the view.
    g_followFrame = [CALayer layer];
    g_followFrame.frame = root.bounds;
    g_followFrame.autoresizingMask = kCALayerWidthSizable | kCALayerHeightSizable;
    g_followFrame.opacity = 0;
    g_followTag = [CALayer layer];
    g_followTag.anchorPoint = CGPointMake(1, 0);
    g_followTag.autoresizingMask = kCALayerMinXMargin;
    g_followTag.contentsScale = g_scale;
    g_followText = [CATextLayer layer];
    g_followText.contentsScale = g_scale;
    [g_followTag addSublayer:g_followText];
    [g_followFrame addSublayer:g_followTag];
    [root addSublayer:g_followFrame];
    g_reduceMotionChecked = 0;

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
    for (auto& [id, p] : g_peers) p.dragItems.clear();
    g_peers.clear();
    g_pendingReverts.clear();
    g_marks.clear();
    g_marksDirty = false;
    g_hoverTag = nil;
    g_hoverText = nil;
    g_hoverItem.clear();
    g_hoverLabel.clear();
    g_hoverShown = false;
    g_localIn = false;
    g_markRoot = nil;
    g_followFrame = g_followTag = nil;
    g_followText = nil;
    g_followKey.clear();
    [g_view removeFromSuperview];
    g_view = nil;
    g_root = nil;
    g_tv = nil;
    g_attached = false;
  }
}

bool draws() { return true; }

void setShowCursors(bool on) {
  if (on == g_showCursors) return;
  g_showCursors = on;
  if (on || !g_attached) return;
  @autoreleasepool {
    for (auto& [id, p] : g_peers) dropLayers(p);
    g_peers.clear();
  }
}

void setActivity(const std::vector<Activity>& list) {
  if (!g_attached || !g_showCursors) return;
  @autoreleasepool {
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
        p.tagLabel.clear();
        p.edgeLabel.clear();
        refreshTag(p);
      }
    }
    for (auto& [id, p] : g_peers)
      if (!by.count(id) && !p.fxTrack.empty()) {
        p.fxTrack.clear();
        p.fxLabel.clear();
        p.pendingSpring = true;
        p.tagLabel.clear();
        p.edgeLabel.clear();
        refreshTag(p);
      }
  }
}

void setFollow(const std::string& name, const std::string& color) {
  if (!g_attached) return;
  @autoreleasepool {
    const std::string key = name.empty() ? "" : name + color;
    if (key == g_followKey) return;
    const bool was = !g_followKey.empty();
    g_followKey = key;
    if (key.empty()) {
      if (was) fade(g_followFrame, 0, kStyle.fadeOut);
      return;
    }
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    g_followFrame.frame = g_root.bounds;
    g_followFrame.borderWidth = kStyle.followLine;
    g_followFrame.borderColor = parseColor(color).CGColor;
    [CATransaction commit];
    setPill(g_followTag, g_followText, name, color, kStyle.tagFont, kStyle.tagHeight, kStyle.tagPadX);
    setPos(g_followTag, CGPointMake(g_root.bounds.size.width - kStyle.followTagInset, kStyle.followTagInset));
    if (!was) fade(g_followFrame, kStyle.followOpacity, kStyle.fadeIn * 2);
  }
}

void setMarks(const std::vector<Mark>& marks) {
  if (!g_attached) return;
  @autoreleasepool {
    std::map<std::string, const Mark*> want;
    for (const Mark& m : marks)
      if (!m.item.empty()) want.emplace(m.item, &m);
    const double t = now();
    for (auto it = g_marks.begin(); it != g_marks.end();) {
      if (want.count(it->first) || t - it->second.lastWanted < kStyle.markLinger) {
        ++it;
        continue;
      }
      fadeAndDrop(it->second.box);
      it = g_marks.erase(it);
      g_marksDirty = true;
    }
    for (const auto& [item, m] : want) {
      auto it = g_marks.find(item);
      if (it == g_marks.end()) {
        MarkState ms;
        ms.m = *m;
        [CATransaction begin];
        [CATransaction setDisableActions:YES];
        ms.box = [CALayer layer];
        ms.box.opacity = 0;
        [g_markRoot insertSublayer:ms.box atIndex:0];
        [CATransaction commit];
        styleMark(ms);
        ms.lastWanted = t;
        g_marks.emplace(item, std::move(ms));
        g_marksDirty = true;
        continue;
      }
      it->second.lastWanted = t;
      if (it->second.m.color != m->color || it->second.m.name != m->name ||
          it->second.m.loading != m->loading) {
        it->second.m = *m;
        styleMark(it->second);
        g_hoverLabel.clear();
      }
    }
    // Called right after the tick applied other people's edits, which move
    // items without touching anything the observer's cheap check watches.
    if (!g_marks.empty()) g_marksDirty = true;
    if (g_marksDirty) glue("marks");
  }
}

void update(const Remote& r) {
  if (!g_attached || !g_showCursors || r.id.empty()) return;
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
    if (t - g_reduceMotionChecked > 2) {
      g_reduceMotionChecked = t;
      g_reduceMotion = getenv("REAMESH_CURSOR_REDUCE_MOTION") ||
                       NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceMotion;
    }
    // Network updates land here, with a spring; in timer mode the view glue
    // rides along. In commit mode the observer would also catch them before
    // this pass commits.
    glue("tick", g_sync != Sync::Timer);
    for (auto it = g_peers.begin(); it != g_peers.end();) {
      Peer& p = it->second;
      if (t - std::max(p.lastMsg, p.fxAt) > kStyle.forgetAfter) {
        dropLayers(p);
        it = g_peers.erase(it);
        continue;
      }
      updateTag(p, t);
      // A cursor resting a while dims to half; waiting beside an FX window is
      // not resting.
      const bool idle = p.cursorShown && !p.parked && t - p.lastMove > g_idleDimAfter;
      if (idle != p.idle) {
        fade(p.arrow, idle ? kStyle.idleOpacity : 1, idle ? kStyle.idleFade : kStyle.fadeIn);
        p.idle = idle;
      }
      ++it;
    }
    printStats();
  }
}

void revert(const Revert& r) {
  if (!g_attached || r.item.empty() || r.track.empty()) return;
  g_pendingReverts.push_back(r);
}

bool mouseHeld() { return ([NSEvent pressedMouseButtons] & 1) != 0; }

bool pointerOver(void* hwnd) {
  if (!hwnd) return false;
  @autoreleasepool {
    id obj = (__bridge id)hwnd;
    NSWindow* w = [obj isKindOfClass:[NSView class]]     ? [(NSView*)obj window]
                  : [obj isKindOfClass:[NSWindow class]] ? (NSWindow*)obj
                                                         : nil;
    if (!w || !w.visible) return false;
    return [NSWindow windowNumberAtPoint:[NSEvent mouseLocation] belowWindowWithWindowNumber:0] ==
           w.windowNumber;
  }
}

std::string storyAway() {
  if (!g_story) return {};
  const double ph = storyPhase();
  if (ph >= 28 && ph < 33) return "fx";
  if (ph >= 33 && ph < 38) return "midi";
  if (ph >= 38 && ph < 43) return "mixer";
  return {};
}

bool sampleLocal(Local* out) {
  if (!g_attached) return false;
  @autoreleasepool {
    *out = Local{};
    const Geo g = readGeo();
    if (!g.ok) return true;
    NSPoint vp = NSZeroPoint;
    bool over = false;
    if (NSWindow* w = g_tv.window; w && w.visible) {
      const NSPoint sp = [NSEvent mouseLocation];
      if ([NSWindow windowNumberAtPoint:sp belowWindowWithWindowNumber:0] == w.windowNumber) {
        vp = [g_view convertPoint:[w convertPointFromScreen:sp] fromView:nil];
        over = NSPointInRect(vp, g_view.bounds);
      }
    }
    // Hovering a marked item shows who is editing it.
    if (over != g_localIn || (over && !NSEqualPoints(vp, g_localPoint))) {
      g_localIn = over;
      g_localPoint = vp;
      if (!g_marks.empty() || g_hoverShown) layoutHover();
    }
    bool down = over && ([NSEvent pressedMouseButtons] & 1);
    double it0 = 0, it1 = 0, iy = 0, ih = 0;
    if (g_story) {
      over = storyPoint(g, &vp, &down);
    } else if (const char* item = getenv("REAMESH_CURSOR_DEMO_ITEM");
        g_demo && item && g_host.itemSpan && g_host.itemSpan(item, &it0, &it1, &iy, &ih, nullptr)) {
      // Rides on an item, as if dragging it.
      const double w = 0.06 * std::sin(2 * M_PI * 0.7 * now());
      vp = NSMakePoint((it0 + (it1 - it0) * (0.4 + w) - g.t0) / g.spp, iy + ih * 0.45);
      over = true;
    } else if (g_demo) {
      vp = synthPoint(now(), g_demo * 3, g);
      over = true;
      if (std::fmod(now(), 3.0) < 0.04) {
        g_clickPending = true;
        g_clickPoint = vp;
      }
    }
    if (!over) return true;
    if (g_clickPending) {
      vp = g_clickPoint;
      out->click = true;
      g_clickPending = false;
      ++g_clicksOut;
    }
    out->in = semanticAt(vp, g, &out->t, &out->guid, &out->fy);
    out->down = out->in && down;
    if (g_trace && out->in)
      fprintf(stderr, "[trace] tx %.3f %.9f\n", [[NSDate date] timeIntervalSince1970] * 1000, out->t);
    out->px = vp.x;
    out->py = vp.y;
  }
  return true;
}

}  // namespace cursor_overlay
