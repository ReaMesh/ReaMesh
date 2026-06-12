#!/bin/bash
# ReaMesh build: Rust core -> FFI header -> REAPER extension dylib.
# On macOS, also builds the Tauri companion app (which bundles the dylib).
set -euo pipefail
cd "$(dirname "$0")"

export PATH="$HOME/.cargo/bin:$PATH"

# The companion app is only built on macOS; Windows is built in CI, and the
# app bundles the dylib produced by step 3, so it must run after it.
STEPS=3
[[ "$(uname)" == "Darwin" ]] && STEPS=4

echo "==> [1/$STEPS] Building Rust library (release)"
(cd rust && cargo build --release)

echo "==> [2/$STEPS] Generating extension/include/reamesh_ffi.h"
if command -v cbindgen >/dev/null 2>&1; then
  (cd rust && cbindgen --config cbindgen.toml --crate reamesh \
      --output ../extension/include/reamesh_ffi.h)
else
  echo "    cbindgen not found; keeping the checked-in header."
  echo "    (install with: cargo install cbindgen)"
fi

echo "==> [3/$STEPS] Building reaper_reamesh.dylib"
cmake -S extension -B extension/build -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build extension/build --config Release

echo
echo "Built: extension/build/reaper_reamesh.dylib"

if [[ "$(uname)" == "Darwin" ]]; then
  echo
  echo "==> [4/$STEPS] Building the companion app (ReaMesh.app)"
  (cd app && bun install && bun run tauri build)
  echo
  echo "Built: app/src-tauri/target/release/bundle/macos/ReaMesh.app"
  echo "       (it installs the bundled dylib itself on first launch)"
else
  echo "Install with:"
  echo "  cp extension/build/reaper_reamesh.dylib ~/Library/Application\\ Support/REAPER/UserPlugins/"
fi
