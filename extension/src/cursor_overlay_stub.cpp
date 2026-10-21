// Platforms without a cursor layer.
#include "cursor_overlay.h"

namespace cursor_overlay {

bool attach(void*, const Host&) { return false; }
void detach() {}
bool draws() { return false; }
void setShowCursors(bool) {}
void update(const Remote&) {}
void setFollow(const std::string&, const std::string&, const std::string&) {}
void setMarks(const std::vector<Mark>&) {}
void tick() {}
bool sampleLocal(Local*) { return false; }
bool viewSize(double*, double*) { return false; }
double pixelScale() { return 1; }
bool mouseHeld() { return false; }

}  // namespace cursor_overlay
