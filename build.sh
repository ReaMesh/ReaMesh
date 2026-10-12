#!/bin/bash
# ReaMesh build: Rust core -> FFI header -> REAPER extension -> companion app.
# Runs on macOS and on Windows (from Git Bash / MSYS, which ships with Git).
set -euo pipefail
cd "$(dirname "$0")"

export PATH="$HOME/.cargo/bin:$PATH"

case "$(uname -s)" in
  Darwin)               PLATFORM=macos ;;
  MINGW*|MSYS*|CYGWIN*) PLATFORM=windows ;;
  *)                    PLATFORM=other ;;
esac

# cmake's Visual Studio generator is multi-config, so the DLL lands in a
# Release/ subfolder; the single-config generator used on macOS does not.
if [[ "$PLATFORM" == windows ]]; then
  EXT_OUT="extension/build/Release/reaper_reamesh.dll"
else
  EXT_OUT="extension/build/reaper_reamesh.dylib"
fi

# The companion app bundles the extension binary produced by step 3, so it has
# to run after it. Platforms without an app target stop at step 3.
STEPS=3
[[ "$PLATFORM" != other ]] && STEPS=4

# Link against the SDK of the selected Xcode, not whatever xcrun resolves to:
# a Command Line Tools SDK newer than the Xcode toolchain carries .tbd
# architectures that toolchain's ld rejects ("tapi error: unknown
# architecture"), and cmake's compiler check fails before the extension builds.
if [[ "$PLATFORM" == macos && -z "${SDKROOT:-}" ]]; then
  XCODE_SDK="$(xcode-select -p)/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
  [[ -d "$XCODE_SDK" ]] && export SDKROOT="$XCODE_SDK"
fi

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

echo "==> [3/$STEPS] Building $(basename "$EXT_OUT")"
if [[ "$PLATFORM" == windows ]]; then
  # Multi-config generator: the config is chosen at build time, not configure
  # time, and passing CMAKE_BUILD_TYPE here only earns an "unused variable".
  cmake -S extension -B extension/build >/dev/null
else
  cmake -S extension -B extension/build -DCMAKE_BUILD_TYPE=Release >/dev/null
fi
cmake --build extension/build --config Release

echo
echo "Built: $EXT_OUT"

if [[ "$PLATFORM" == other ]]; then
  echo "No companion app target on this platform."
  exit 0
fi

echo
echo "==> [4/$STEPS] Building the companion app"
(cd app && bun install && bun run tauri build)
echo
if [[ "$PLATFORM" == windows ]]; then
  echo "Built: app/src-tauri/target/release/bundle/nsis/ (installer)"
else
  echo "Built: app/src-tauri/target/release/bundle/macos/ReaMesh.app"
fi
echo "       (it installs the bundled extension itself on first launch)"
