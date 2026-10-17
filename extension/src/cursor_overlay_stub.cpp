// Platforms without a cursor layer.
#include "cursor_overlay.h"

namespace cursor_overlay {

bool attach(void*, const Host&) { return false; }
void detach() {}
bool draws() { return false; }
void setShowCursors(bool) {}
void update(const Remote&) {}
void setMarks(const std::vector<Mark>&) {}
void tick() {}
bool sampleLocal(Local*) { return false; }

}  // namespace cursor_overlay
