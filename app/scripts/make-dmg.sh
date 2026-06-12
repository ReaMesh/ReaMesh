#!/usr/bin/env bash
# Builds the styled ReaMesh installer DMG: app-themed background with a
# drag-to-Applications layout, volume icon, compressed UDZO image.
#
# Run from app/ (or anywhere): bun run dmg
# Needs a prior `bun run tauri build` (the .app bundle), a GUI session
# (Finder is scripted to set icon positions), and Xcode CLT for SetFile.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_BUNDLE="src-tauri/target/release/bundle/macos/ReaMesh.app"
[ -d "$APP_BUNDLE" ] || { echo "error: $APP_BUNDLE not found; run 'bun run tauri build' first" >&2; exit 1; }

VERSION=$(bun -e 'console.log(JSON.parse(await Bun.file("src-tauri/tauri.conf.json").text()).version)')
VOL="ReaMesh"
OUT_DIR="src-tauri/target/release/bundle/dmg"
OUT="$OUT_DIR/ReaMesh_${VERSION}.dmg"
mkdir -p "$OUT_DIR"

# 1. Background: SVG -> 2x PNG, stamped 144dpi so Finder draws it at 1x.
bun scripts/render-dmg-background.ts
sips -s dpiHeight 144 -s dpiWidth 144 scripts/.build/dmg-background.png >/dev/null

# 2. Stage the volume contents.
STAGE=$(mktemp -d /tmp/reamesh-dmg.XXXXXX)
RW="$STAGE-rw.dmg"
cleanup() {
  [ -n "${DEV:-}" ] && hdiutil detach "$DEV" -force >/dev/null 2>&1 || true
  rm -rf "$STAGE" "$RW"
}
trap cleanup EXIT

cp -R "$APP_BUNDLE" "$STAGE/ReaMesh.app"
ln -s /Applications "$STAGE/Applications"
mkdir "$STAGE/.background"
cp scripts/.build/dmg-background.png "$STAGE/.background/background.png"
# Placeholder so the styling pass below can record a parked position for
# ".VolumeIcon.icns" in the .DS_Store (records are keyed by name and outlive
# the file); the real icon replaces it after styling and inherits the spot.
touch "$STAGE/.VolumeIcon.icns"

# 3. Writable image; mount it for Finder styling.
if [ -d "/Volumes/$VOL" ]; then hdiutil detach "/Volumes/$VOL" -force >/dev/null || true; fi
hdiutil create -volname "$VOL" -srcfolder "$STAGE" -fs HFS+ -format UDRW -ov "$RW" >/dev/null
DEV=$(hdiutil attach -readwrite -noverify -noautoopen "$RW" | grep -oE '^/dev/disk[0-9]+' | head -1)
sleep 1

# 4. Finder: icon view, background picture, icon positions. Writes .DS_Store.
osascript <<EOF
tell application "Finder"
  tell disk "$VOL"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    try
      set pathbar visible of container window to false
    end try
    set the bounds of container window to {200, 120, 860, 548}
    set theViewOptions to the icon view options of container window
    set arrangement of theViewOptions to not arranged
    set icon size of theViewOptions to 110
    set text size of theViewOptions to 13
    set background picture of theViewOptions to file ".background:background.png"
    set position of item "ReaMesh.app" of container window to {165, 193}
    set position of item "Applications" of container window to {495, 193}
    try
      set extension hidden of item "ReaMesh.app" to true
    end try
    -- park dotfiles outside the visible area for people who show hidden files
    repeat with h in {".background", ".fseventsd", ".DS_Store", ".VolumeIcon.icns"}
      try
        set position of item (h as text) of container window to {990, 205}
      end try
    end repeat
    update without registering applications
    delay 1
    close
  end tell
end tell
EOF
sync
sleep 1

# Volume icon LAST: the macOS 26+ Finder strips .VolumeIcon.icns (and the
# custom-icon flag) from volumes it opens, so it must land after the
# scripted styling above, right before detach. SetFile ships with the
# Xcode command line tools.
cp src-tauri/icons/icon.icns "/Volumes/$VOL/.VolumeIcon.icns"
xcrun SetFile -a C "/Volumes/$VOL" 2>/dev/null || SetFile -a C "/Volumes/$VOL" || true
# These two only exist now, so they couldn't be parked above. The window
# stays closed here: reopening it makes Finder strip the volume icon again.
# Finder ejects the disk itself so the position edits flush to .DS_Store
# (a plain hdiutil detach races Finder's lazy write and loses them).
osascript -e 'tell application "Finder"
  tell disk "'"$VOL"'"
    repeat with h in {".VolumeIcon.icns", ".DS_Store"}
      try
        set position of item (h as text) of container window to {990, 205}
      end try
    end repeat
  end tell
  eject disk "'"$VOL"'"
end tell' || true
sync

# 5. Detach (no-op if Finder's eject above already did) and compress.
for i in 1 2 3 4 5; do
  hdiutil info | grep -q "^$DEV" || break
  hdiutil detach "$DEV" >/dev/null 2>&1 && break
  sleep 2
  [ "$i" = 5 ] && hdiutil detach "$DEV" -force >/dev/null 2>&1
done
DEV=""
hdiutil convert "$RW" -format UDZO -imagekey zlib-level=9 -ov -o "$OUT" >/dev/null

echo "built $OUT"
