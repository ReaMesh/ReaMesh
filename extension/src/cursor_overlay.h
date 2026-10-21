// Peer cursors and edit marks drawn inside REAPER's arrange view.
//
// Other people's mouse cursors, and an outline around the items someone else
// is editing, are drawn as compositor layers on top of REAPER's track view,
// inside REAPER's own process: Core Animation on macOS
// (cursor_overlay_mac.mm), DirectComposition on Windows
// (cursor_overlay_win.cpp). Where attach() fails (no track view, no
// composition device), the extension keeps marking locked items with its
// colour tint and cursors are neither drawn nor sent. The layer is
// display-only and click-through; it never takes input.
//
// Positions travel semantically ({project seconds, track GUID, 0..1 within the
// track}), so every peer sees a cursor at the same musical spot whatever their
// own zoom, scroll or track heights; the platform layer turns them into pixels
// with the geometry callbacks below on every pass in which REAPER repaints.
//
// Everything here runs on REAPER's main thread.
#pragma once

#include <string>
#include <vector>

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
  // An item's extent: project seconds [*t0, *t1) and its pixel rows in the
  // arrange, plus its track's GUID when `track` is given. false when the item
  // is not in this project or not visible.
  bool (*itemSpan)(const char* guid, double* t0, double* t1, double* y, double* h, std::string* track);
  // The track `by` places below (negative: above) the given one, in this
  // project's order. false past either end.
  bool (*trackShift)(const char* guid, int by, std::string* out);
  // Changes whenever item rows may have moved without the time axis moving:
  // local project edits, vertical scroll, track heights. Between two
  // setMarks calls, marks are laid out again only when this or the time axis
  // changed.
  unsigned long long (*rowsStamp)();
  // Exchange cursors with the session: feeds arrived updates to update() and
  // sends the local pointer. The layer calls it right before it draws, so a
  // cursor that arrived this pass is on screen in this frame.
  void (*pump)();
};

// Attach to the arrange of REAPER's main window (an NSView* / HWND). Returns
// false where the platform has no layer yet; every other call is then a no-op.
bool attach(void* mainWindow, const Host& host);
// Remove the layer and forget every peer.
void detach();
// Whether this platform draws (cursors and item marks). false where attach
// only samples the local pointer.
bool draws();

// Whether other people's cursors are shown (the app's setting). Off forgets
// them at once and ignores updates; item marks stay.
void setShowCursors(bool on);

// Where someone works while their mouse is away from the arrange.
struct Away {
  std::string kind;   // "fx", "midi", "mixer", "tcp"; empty: nowhere to show
  std::string track;  // track GUID (fx, mixer, tcp)
  std::string item;   // item GUID (midi)
  std::string label;  // plug-in name (fx)
};

// An item drag in progress: the items, how far in time and by how many
// tracks they have been dragged so far.
struct Drag {
  std::vector<std::string> items;
  double dt = 0;
  int dtr = 0;
};

// One cursor update from a peer, in arrival order.
struct Remote {
  std::string id, name, color;  // color "#rrggbb"
  double t = 0, fy = 0;
  std::string guid;
  bool in = false;     // false: the mouse left their arrange
  bool click = false;  // a mouse-down at this spot
  bool down = false;   // the button is held (dragging)
  Away away;           // while !in
  Drag drag;           // while dragging items
};
void update(const Remote& r);

// What each other person is doing outside the arrange, from their presence:
// an open FX window on a track. While their mouse is away from the arrange,
// their cursor waits at the left edge of that track's row, next to its FX
// button, labelled with the plug-in. Replaces the previous set.
struct Activity {
  std::string id, name, color;
  std::string track;  // track GUID
  std::string label;  // plug-in name
};
void setActivity(const std::vector<Activity>& list);

// Following someone: a thin frame in their colour around the arrange with
// their name in a corner, and their cursor (peer `id`) stands out from the
// others: a soft disc in their colour behind the pointer, the name tag always
// up, never dimmed while resting, drawn above every other cursor. An empty
// name removes both.
void setFollow(const std::string& id, const std::string& name, const std::string& color);

// Items to mark, replacing the previous set. Called every extension tick,
// after remote edits were applied: lays the marks out again each time.
struct Mark {
  std::string item;         // item GUID
  std::string name, color;  // who is editing it; color "#rrggbb"
  bool loading = false;     // a placeholder whose media is still on its way
};
void setMarks(const std::vector<Mark>& marks);

// Called from the extension timer: idle fades, staleness.
void tick();

// The local pointer over this arrange.
struct Local {
  bool in = false;
  double t = 0, fy = 0;
  std::string guid;
  bool click = false;
  bool down = false;
  double px = 0, py = 0;  // arrange pixels, for change detection only
};
// Samples the local pointer; false when there is nothing to sample (no layer).
bool sampleLocal(Local* out);

// The arrange's size in the units of Host::trackSpan (points on macOS,
// pixels on Windows); false without a layer.
bool viewSize(double* w, double* h);

// Arrange units per point: the factor that turns a distance meant in points
// into arrange coordinates (1 on macOS, where they are points; the DPI scale
// on Windows, where they are pixels).
double pixelScale();

// Whether the left mouse button is held anywhere.
bool mouseHeld();
// Whether the pointer is over the window that holds `hwnd` (an HWND / NSView*),
// with nothing covering it.
bool pointerOver(void* hwnd);
// Whether the topmost window under the pointer is one of REAPER's.
bool pointerOverApp();

}  // namespace cursor_overlay
