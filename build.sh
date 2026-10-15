#!/bin/bash
# ReaMesh build: Rust core -> FFI header -> REAPER extension -> ReaMesh app.
# Runs on macOS and on Windows (from Git Bash / MSYS, which ships with Git).
#
#   ./build.sh            everything
#   ./build.sh --no-app   stop after the extension (CI signs it before the app build)
set -euo pipefail
cd "$(dirname "$0")"

NO_APP=0
for arg in "$@"; do
  case "$arg" in
    --no-app) NO_APP=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

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

# The ReaMesh app bundles the extension binary produced by step 3, so it has
# to run after it. Platforms without an app target stop at step 3.
STEPS=3
[[ "$PLATFORM" != other && $NO_APP == 0 ]] && STEPS=4

# macOS builds are universal (Apple silicon + Intel) with one deployment
# target for every piece: the Rust core, the extension and the app. Without an
# explicit target, clang stamps the build machine's macOS version as the
# minimum and REAPER on any older macOS refuses to load the extension.
MAC_ARCHS=(aarch64-apple-darwin x86_64-apple-darwin)
MAC_MIN=11.0
if [[ "$PLATFORM" == macos ]]; then
  export MACOSX_DEPLOYMENT_TARGET="$MAC_MIN"
  if command -v rustup >/dev/null 2>&1; then
    INSTALLED="$(rustup target list --installed)"
    for t in "${MAC_ARCHS[@]}"; do
      grep -qx "$t" <<<"$INSTALLED" || rustup target add "$t"
    done
  fi
fi

# Link against the SDK of the selected Xcode, not whatever xcrun resolves to:
# a Command Line Tools SDK newer than the Xcode toolchain carries .tbd
# architectures that toolchain's ld rejects ("tapi error: unknown
# architecture"), and cmake's compiler check fails before the extension builds.
if [[ "$PLATFORM" == macos && -z "${SDKROOT:-}" ]]; then
  XCODE_SDK="$(xcode-select -p)/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
  [[ -d "$XCODE_SDK" ]] && export SDKROOT="$XCODE_SDK"
fi

echo "==> [1/$STEPS] Building Rust library (release)"
if [[ "$PLATFORM" == macos ]]; then
  (cd rust && cargo build --release "${MAC_ARCHS[@]/#/--target=}")
  RUST_LIB="$PWD/rust/target/universal-apple-darwin/release/libreamesh.a"
  mkdir -p "$(dirname "$RUST_LIB")"
  SLICES=()
  for t in "${MAC_ARCHS[@]}"; do SLICES+=("rust/target/$t/release/libreamesh.a"); done
  lipo -create "${SLICES[@]}" -output "$RUST_LIB"
else
  (cd rust && cargo build --release)
fi

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
elif [[ "$PLATFORM" == macos ]]; then
  # Passed explicitly so an older build dir's cached single-arch values
  # can't win over them.
  cmake -S extension -B extension/build -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_OSX_ARCHITECTURES="arm64;x86_64" \
    -DCMAKE_OSX_DEPLOYMENT_TARGET="$MAC_MIN" \
    -DREAMESH_RUST_LIB="$RUST_LIB" >/dev/null
else
  cmake -S extension -B extension/build -DCMAKE_BUILD_TYPE=Release >/dev/null
fi
cmake --build extension/build --config Release

echo
echo "Built: $EXT_OUT"
[[ "$PLATFORM" == macos ]] && echo "       ($(lipo -archs "$EXT_OUT"), macOS $MAC_MIN+)"

if [[ "$PLATFORM" == other ]]; then
  echo "No app target on this platform."
  exit 0
fi
[[ $NO_APP == 1 ]] && exit 0

echo
echo "==> [4/$STEPS] Building the ReaMesh app"
if [[ "$PLATFORM" == windows ]]; then
  (cd app && bun install && bun run tauri build)
  echo
  echo "Built: app/src-tauri/target/release/bundle/nsis/ (installer)"
else
  (cd app && bun install && bun run tauri build --target universal-apple-darwin)
  echo
  echo "Built: app/src-tauri/target/universal-apple-darwin/release/bundle/macos/ReaMesh.app"
fi
echo "       (it installs the bundled extension itself on first launch)"
