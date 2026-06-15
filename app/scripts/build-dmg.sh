#!/usr/bin/env bash
# One-shot installer build: compiles the ReaMesh .app bundle, then wraps it in
# the styled drag-to-Applications DMG. Use this when you want a fresh DMG from
# source in a single step.
#
# Run from app/ (or anywhere): bun run dmg:full   (or: bash scripts/build-dmg.sh)
# Needs a GUI session: make-dmg.sh scripts Finder for the icon layout.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Building ReaMesh.app (bun run tauri build)…"
bun run tauri build

echo "==> Packaging styled DMG (scripts/make-dmg.sh)…"
bash scripts/make-dmg.sh
