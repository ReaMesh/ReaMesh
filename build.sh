#!/bin/bash
# ReaMesh build: Rust core -> FFI header -> REAPER extension dylib.
set -euo pipefail
cd "$(dirname "$0")"

export PATH="$HOME/.cargo/bin:$PATH"

echo "==> [1/3] Building Rust library (release)"
(cd rust && cargo build --release)

echo "==> [2/3] Generating extension/include/reamesh_ffi.h"
if command -v cbindgen >/dev/null 2>&1; then
  (cd rust && cbindgen --config cbindgen.toml --crate reamesh \
      --output ../extension/include/reamesh_ffi.h)
else
  echo "    cbindgen not found; keeping the checked-in header."
  echo "    (install with: cargo install cbindgen)"
fi

echo "==> [3/3] Building reaper_reamesh.dylib"
cmake -S extension -B extension/build -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build extension/build --config Release

echo
echo "Built: extension/build/reaper_reamesh.dylib"
echo "Install with:"
echo "  cp extension/build/reaper_reamesh.dylib ~/Library/Application\\ Support/REAPER/UserPlugins/"
