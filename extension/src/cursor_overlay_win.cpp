// Windows: no arrange layer yet (DirectComposition + Direct2D to come), so
// attach() declines and the extension keeps marking locked items with its
// colour tint, and neither draws nor sends cursors. The pointer helpers below
// are live: the live-values stream and the capture's gesture pause use them.
#include <windows.h>

#include "cursor_overlay.h"

namespace cursor_overlay {

bool attach(void*, const Host&) { return false; }
void detach() {}
bool draws() { return false; }
void setShowCursors(bool) {}
void update(const Remote&) {}
void setActivity(const std::vector<Activity>&) {}
void setFollow(const std::string&, const std::string&) {}
void setMarks(const std::vector<Mark>&) {}
void tick() {}
void revert(const Revert&) {}
bool sampleLocal(Local*) { return false; }

bool mouseHeld() { return (GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0; }

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

}  // namespace cursor_overlay
