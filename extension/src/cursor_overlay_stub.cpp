// Platforms without a cursor layer yet (Windows: DirectComposition + Direct2D).
#include "cursor_overlay.h"

namespace cursor_overlay {

bool attach(void*, const Host&) { return false; }
void detach() {}
void update(const Remote&) {}
void tick() {}
bool sampleLocal(Local*) { return false; }

}  // namespace cursor_overlay
