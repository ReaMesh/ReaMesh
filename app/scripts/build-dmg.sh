#!/usr/bin/env bash
# One-shot installer build: compiles the ReaMesh .app bundle at the current
# version, then wraps it in the styled drag-to-Applications DMG:
#   src-tauri/target/release/bundle/dmg/ReaMesh_<version>.dmg
#
# Run from app/ (or anywhere): bun run dmg:full   (or: bash scripts/build-dmg.sh)
# Needs a GUI session: make-dmg.sh scripts Finder for the icon layout.
#
# Signing: by default `tauri build` signs the .app ad-hoc (signingIdentity "-"
# in src-tauri/tauri.conf.json) and the image stays unsigned, so a download
# asks for "Open Anyway" once, for the app. With a Developer ID, build with it
# instead and then sign (and notarize) the image with scripts/sign-dmg.sh:
#   APPLE_SIGNING_IDENTITY=<hash or "Developer ID Application: …"> bun run dmg:full
#   bun run dmg:sign
set -euo pipefail
cd "$(dirname "$0")/.."

# The version comes from tauri.conf.json (that's what names the .app and DMG);
# package.json and Cargo.toml are supposed to carry the same number.
VERSION=$(bun -e 'console.log(JSON.parse(await Bun.file("src-tauri/tauri.conf.json").text()).version)')
PKG_VERSION=$(bun -e 'console.log(JSON.parse(await Bun.file("package.json").text()).version)')
CARGO_VERSION=$(sed -nE 's/^version *= *"([^"]+)".*/\1/p' src-tauri/Cargo.toml | head -1)
echo "==> ReaMesh v$VERSION"
for pair in "package.json:$PKG_VERSION" "src-tauri/Cargo.toml:$CARGO_VERSION"; do
  [ "${pair#*:}" = "$VERSION" ] || echo "warning: ${pair%%:*} says ${pair#*:}, tauri.conf.json says $VERSION" >&2
done

BUILD_ARGS=()
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "==> Signing the .app with: $APPLE_SIGNING_IDENTITY"
  # --config merges over tauri.conf.json + tauri.macos.conf.json, overriding the
  # ad-hoc signingIdentity for this build only.
  BUILD_ARGS+=(--config "{\"bundle\":{\"macOS\":{\"signingIdentity\":\"$APPLE_SIGNING_IDENTITY\"}}}")
fi

echo "==> Building ReaMesh.app (bun run tauri build)…"
bun run tauri build "${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"}"

echo "==> Packaging styled DMG (scripts/make-dmg.sh)…"
bash scripts/make-dmg.sh

echo
echo "Done: src-tauri/target/release/bundle/dmg/ReaMesh_${VERSION}.dmg"
echo "Sign it for distribution with: bun run dmg:sign"
