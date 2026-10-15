// Peer cursors drawn inside REAPER's arrange view (prototype).
//
// Other people's mouse cursors are drawn as compositor layers on top of
// REAPER's track view, inside REAPER's own process: Core Animation on macOS
// (cursor_overlay_mac.mm), DirectComposition + Direct2D on Windows later. The
// layer is display-only and click-through; it never takes input.
//
// Positions travel semantically ({project seconds, track GUID, 0..1 within the
// track}), so every peer sees a cursor at the same musical spot whatever their
// own zoom, scroll or track heights; the platform layer turns them into pixels
// with the geometry callbacks below on every pass in which REAPER repaints.
//
// Everything here runs on REAPER's main thread.
#pragma once

#include <string>

namespace cursor_overlay {

// Arrange geometry, answered by the extension from the REAPER API.
struct Host {
  // false while the project must not be read (opening, applying remote state,
  // not in a session).
  bool (*ready)();
  // Project seconds at the arrange's left edge and seconds per pixel.
  bool (*timeAxis)(double* t0, double* spp);
  // A track's vertical extent in the arrange (pixels from its top). false when
  // the GUID is not in this project; *h == 0 when the track is hidden.
  bool (*trackSpan)(const char* guid, double* y, double* h);
  // The visible track under an arrange y, with the 0..1 position inside it.
  // false over the empty area below the last track.
  bool (*trackAt)(double y, std::string* guid, double* fy);
  // Test scaffolding only: move the arrange to [t0, t1]; the GUID of the
  // track at an index.
  void (*setTimeAxis)(double t0, double t1);
  bool (*trackGuidAt)(int index, std::string* guid);
  // 0: set the height of track `index` to `v` px; 1: scroll vertically by `v` steps.
  void (*testNudge)(int kind, int index, int v);
};

// Attach to the arrange of REAPER's main window (an NSView* / HWND). Returns
// false where the platform has no layer yet; every other call is then a no-op.
bool attach(void* mainWindow, const Host& host);
// Remove the layer and forget every peer.
void detach();

// One cursor update from a peer, in arrival order.
struct Remote {
  std::string id, name, color;  // color "#rrggbb"
  double t = 0, fy = 0;
  std::string guid;
  bool in = false;     // false: the mouse left their arrange
  bool click = false;  // a mouse-down at this spot
};
void update(const Remote& r);

// Called from the extension timer: idle fades, staleness.
void tick();

// The local pointer over this arrange.
struct Local {
  bool in = false;
  double t = 0, fy = 0;
  std::string guid;
  bool click = false;
  double px = 0, py = 0;  // arrange pixels, for change detection only
};
// Samples the local pointer; false when there is nothing to sample (no layer).
bool sampleLocal(Local* out);

}  // namespace cursor_overlay
