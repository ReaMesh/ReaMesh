// Windows: no drawing layer yet (DirectComposition + Direct2D later). attach()
// finds REAPER's track view so the local pointer can be sampled and sent, and
// people on macOS see this person's cursor. Other people's cursors are not
// drawn here, and locked items keep their colour tint (draws() is false).
#include <windows.h>

#include "cursor_overlay.h"

namespace {

cursor_overlay::Host g_host{};
HWND g_tv = nullptr;
HHOOK g_hook = nullptr;
// A left click in the track view since the last sample, in screen pixels.
bool g_clickPending = false;
POINT g_clickPoint{};

// Thread-local mouse hook on REAPER's main thread: sees every button press
// before REAPER handles it, without taking it.
LRESULT CALLBACK mouseHook(int code, WPARAM wp, LPARAM lp) {
  if (code == HC_ACTION && (wp == WM_LBUTTONDOWN || wp == WM_LBUTTONDBLCLK) && g_tv) {
    const auto* m = reinterpret_cast<const MOUSEHOOKSTRUCT*>(lp);
    if (m->hwnd == g_tv) {
      g_clickPending = true;
      g_clickPoint = m->pt;
    }
  }
  return CallNextHookEx(g_hook, code, wp, lp);
}

}  // namespace

namespace cursor_overlay {

bool attach(void* mainWindow, const Host& host) {
  if (g_tv) return true;
  HWND tv = mainWindow ? GetDlgItem(static_cast<HWND>(mainWindow), 1000) : nullptr;
  if (!tv) return false;
  g_host = host;
  g_tv = tv;
  g_hook = SetWindowsHookExW(WH_MOUSE, mouseHook, nullptr, GetCurrentThreadId());
  return true;
}

void detach() {
  if (g_hook) UnhookWindowsHookEx(g_hook);
  g_hook = nullptr;
  g_tv = nullptr;
  g_clickPending = false;
}

bool draws() { return false; }
void setShowCursors(bool) {}
void update(const Remote&) {}
void setActivity(const std::vector<Activity>&) {}
void setFollow(const std::string&, const std::string&) {}
void setMarks(const std::vector<Mark>&) {}
void tick() {}
void revert(const Revert&) {}
std::string storyAway() { return {}; }

bool mouseHeld() { return (GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0; }

bool pointerOver(void* hwnd) {
  POINT p{};
  HWND h = static_cast<HWND>(hwnd);
  if (!h || !GetCursorPos(&p)) return false;
  HWND under = WindowFromPoint(p);
  return under && GetAncestor(under, GA_ROOT) == GetAncestor(h, GA_ROOT);
}

bool sampleLocal(Local* out) {
  if (!g_tv) return false;
  *out = Local{};
  if (!IsWindow(g_tv) || !IsWindowVisible(g_tv)) return true;
  if (!g_host.ready || !g_host.ready()) return true;
  double t0 = 0, spp = 0;
  if (!g_host.timeAxis(&t0, &spp) || !(spp > 0)) return true;

  POINT p{};
  if (g_clickPending) {
    p = g_clickPoint;
    g_clickPending = false;
    out->click = true;
  } else {
    // Only while the track view is what's under the pointer: not when a
    // floating window or menu covers it.
    if (!GetCursorPos(&p) || WindowFromPoint(p) != g_tv) return true;
  }
  ScreenToClient(g_tv, &p);
  RECT rc{};
  GetClientRect(g_tv, &rc);
  if (!PtInRect(&rc, p)) {
    out->click = false;
    return true;
  }
  out->down = (GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0;
  out->t = t0 + p.x * spp;
  out->in = g_host.trackAt(p.y, &out->guid, &out->fy);
  out->px = p.x;
  out->py = p.y;
  return true;
}

}  // namespace cursor_overlay
