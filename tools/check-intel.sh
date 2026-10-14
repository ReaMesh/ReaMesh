#!/bin/bash
# Checks the Intel (x86_64) half of the universal macOS build. On Apple silicon
# everything x86_64 runs under Rosetta 2; on an Intel Mac it runs natively.
#
#   tools/check-intel.sh            binaries + C++ tests + Rust tests (x86_64)
#   tools/check-intel.sh --quick    skip the Rust tests (they take a few minutes)
#   tools/check-intel.sh --reaper   afterwards, start a second REAPER as x86_64
#
# Needs a prior ./build.sh (the universal Rust library and extension). For a
# two-architecture session, run --reaper next to a normally started REAPER:
# the Rosetta instance loads the x86_64 slice of the same installed extension.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

QUICK=0
REAPER=0
for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=1 ;;
    --reaper) REAPER=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

[[ "$(uname -s)" == Darwin ]] || { echo "macOS only" >&2; exit 2; }
if ! arch -x86_64 /usr/bin/true 2>/dev/null; then
  echo "error: x86_64 code can't run here. Install Rosetta 2:" >&2
  echo "       softwareupdate --install-rosetta --agree-to-license" >&2
  exit 1
fi

# Same SDK pin as build.sh: a Command Line Tools SDK newer than the Xcode
# toolchain lists architectures that toolchain's ld rejects.
if [[ -z "${SDKROOT:-}" ]]; then
  XCODE_SDK="$(xcode-select -p)/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
  [[ -d "$XCODE_SDK" ]] && export SDKROOT="$XCODE_SDK"
fi

MIN=$(sed -nE 's/.*"minimumSystemVersion": *"([^"]+)".*/\1/p' app/src-tauri/tauri.conf.json)
DYLIB=extension/build/reaper_reamesh.dylib
RUST_LIB=rust/target/universal-apple-darwin/release/libreamesh.a
OUT=$(mktemp -d "${TMPDIR:-/tmp}/reamesh-intel.XXXXXX")
trap 'rm -rf "$OUT"' EXIT
FAILED=0

ok()   { echo "  ok    $*"; }
bad()  { echo "  FAIL  $*"; FAILED=1; }
step() { echo; echo "==> $*"; }

# Every Mach-O must carry both slices, each loadable on the app's minimum macOS.
check_binary() {
  local f="$1" archs minos
  archs=$(lipo -archs "$f" 2>/dev/null || true)
  if [[ " $archs " == *" arm64 "* && " $archs " == *" x86_64 "* ]]; then
    ok "$f: $archs"
  else
    bad "$f: not universal (${archs:-unreadable})"
  fi
  while read -r minos; do
    if [[ "$(printf '%s\n%s\n' "$minos" "$MIN" | sort -t. -k1,1n -k2,2n | tail -1)" != "$MIN" ]]; then
      bad "$f: needs macOS $minos, the app supports $MIN"
    fi
  done < <(vtool -show-build "$f" 2>/dev/null | awk '/minos/ {print $2}')
}

step "Architectures + minimum macOS ($MIN)"
[[ -f "$DYLIB" ]] || { echo "error: $DYLIB missing; run ./build.sh first" >&2; exit 1; }
[[ -f "$RUST_LIB" ]] || { echo "error: $RUST_LIB missing; run ./build.sh first" >&2; exit 1; }
check_binary "$DYLIB"
APP=app/src-tauri/target/universal-apple-darwin/release/bundle/macos/ReaMesh.app
if [[ -d "$APP" ]]; then
  check_binary "$APP/Contents/MacOS/app"
  check_binary "$APP/Contents/Resources/reaper_reamesh.dylib"
else
  echo "  (no universal app bundle yet; 'cd app && bun run build:universal' adds it)"
fi

step "Loading the extension as x86_64"
if arch -x86_64 /usr/bin/python3 -c "
import ctypes, sys
h = ctypes.CDLL(sys.argv[1])
assert hasattr(h, 'ReaperPluginEntry')
" "$DYLIB"; then
  ok "dlopen + ReaperPluginEntry"
else
  bad "the x86_64 slice does not load"
fi

LINK=(-framework CoreFoundation -framework Security -framework SystemConfiguration -lresolv)

step "Chunk parse/splice test (x86_64)"
clang++ -arch x86_64 -mmacosx-version-min="$MIN" -std=c++20 -Iextension/include -Iextension/sdk \
  tools/chunk_test.cpp "$RUST_LIB" "${LINK[@]}" -framework CoreGraphics -o "$OUT/chunk_test"
if arch -x86_64 "$OUT/chunk_test" >"$OUT/chunk.log" 2>&1; then
  ok "$(tail -1 "$OUT/chunk.log")"
else
  cat "$OUT/chunk.log"; bad "chunk_test"
fi

step "FFI smoke test (x86_64: host, join, transfer, edits, media fetch)"
clang++ -arch x86_64 -mmacosx-version-min="$MIN" -std=c++20 -Iextension/include \
  tools/ffi_smoke.cpp "$RUST_LIB" "${LINK[@]}" -o "$OUT/ffi_smoke"
# One retry: the two in-process peers time out now and then on a busy machine,
# natively just the same, and that says nothing about the architecture.
if arch -x86_64 "$OUT/ffi_smoke" >"$OUT/ffi.log" 2>&1; then
  ok "$(tail -1 "$OUT/ffi.log")"
elif arch -x86_64 "$OUT/ffi_smoke" >"$OUT/ffi.log" 2>&1; then
  ok "$(tail -1 "$OUT/ffi.log") (second attempt)"
else
  cat "$OUT/ffi.log"; bad "ffi_smoke"
fi

if [[ $QUICK == 0 ]]; then
  step "Rust tests (x86_64, serial)"
  # cargo runs the x86_64 test binaries through Rosetta on its own.
  if (cd rust && MACOSX_DEPLOYMENT_TARGET="$MIN" cargo test --release --target x86_64-apple-darwin -- --test-threads=1); then
    ok "cargo test (x86_64)"
  else
    bad "cargo test (x86_64)"
  fi
fi

echo
if [[ $FAILED == 1 ]]; then
  echo "Intel check FAILED"
  exit 1
fi
echo "Intel check passed"

if [[ $REAPER == 1 ]]; then
  REAPER_BIN=/Applications/REAPER.app/Contents/MacOS/REAPER
  [[ " $(lipo -archs "$REAPER_BIN") " == *" x86_64 "* ]] || { echo "error: this REAPER has no x86_64 slice" >&2; exit 1; }
  echo "Starting a second REAPER as x86_64 (Rosetta); Activity Monitor shows it as \"Intel\"."
  arch -x86_64 "$REAPER_BIN" -newinst >/dev/null 2>&1 &
  disown
fi
