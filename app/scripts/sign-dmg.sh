#!/usr/bin/env bash
# Signs a built ReaMesh installer DMG with your Apple Developer ID.
#
#   bun run dmg:sign                      # newest src-tauri/target/universal-apple-darwin/release/bundle/dmg/ReaMesh_*.dmg
#   bash scripts/sign-dmg.sh path/to.dmg  # a specific image
#
# Options
#   --identity <id>   certificate to sign with: a SHA-1 hash or the full
#                     "Developer ID Application: Name (TEAMID)" name. Defaults to
#                     $APPLE_SIGNING_IDENTITY, else the one "Developer ID
#                     Application" identity in your keychain.
#   --notarize        after signing, submit to Apple (xcrun notarytool), wait for
#                     the verdict and staple the ticket. Needs a keychain profile:
#                     xcrun notarytool store-credentials <name> --apple-id … --team-id …
#                     Pass it with --profile <name> or $NOTARY_PROFILE.
#   --force           sign even if the .app inside was not signed with a Developer
#                     ID (Gatekeeper will still refuse such an app on other Macs).
#
# To sign the app inside the image, build it with the same identity:
#   APPLE_SIGNING_IDENTITY=<id> bun run dmg:full
set -euo pipefail
cd "$(dirname "$0")/.."

OUT_DIR="src-tauri/target/universal-apple-darwin/release/bundle/dmg"
DMG=""
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
NOTARIZE=0
PROFILE="${NOTARY_PROFILE:-}"
FORCE=0

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --identity) IDENTITY="$2"; shift 2 ;;
    --identity=*) IDENTITY="${1#*=}"; shift ;;
    --notarize) NOTARIZE=1; shift ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --profile=*) PROFILE="${1#*=}"; shift ;;
    --force) FORCE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "error: unknown option $1" >&2; usage >&2; exit 2 ;;
    *) DMG="$1"; shift ;;
  esac
done

# 1. Which image: the argument, else the most recently built ReaMesh_*.dmg.
if [ -z "$DMG" ]; then
  DMG=$(ls -t "$OUT_DIR"/ReaMesh_*.dmg 2>/dev/null | head -1 || true)
  [ -n "$DMG" ] || { echo "error: no $OUT_DIR/ReaMesh_*.dmg found; run 'bun run dmg:full' first" >&2; exit 1; }
fi
[ -f "$DMG" ] || { echo "error: $DMG not found" >&2; exit 1; }

# 2. Which identity: explicit, else the single Developer ID Application cert.
if [ -z "$IDENTITY" ]; then
  MATCHES=$(security find-identity -v -p codesigning | grep '"Developer ID Application:' || true)
  COUNT=$(printf '%s' "$MATCHES" | grep -c . || true)
  if [ "$COUNT" -eq 0 ]; then
    cat >&2 <<'MSG'
error: no "Developer ID Application" certificate in your keychain.

  Create one at https://developer.apple.com/account/resources/certificates
  (type: Developer ID Application), download and double-click it so it lands
  in the login keychain, then re-run. Check with:
    security find-identity -v -p codesigning
  Or pass another identity explicitly: --identity <hash>
MSG
    exit 1
  elif [ "$COUNT" -gt 1 ]; then
    echo "error: several Developer ID Application certificates found; pick one with --identity <hash>:" >&2
    echo "$MATCHES" >&2
    exit 1
  fi
  IDENTITY=$(echo "$MATCHES" | awk '{print $2}')
  echo "==> Using $(echo "$MATCHES" | sed -E 's/^ *[0-9]+\) //')"
fi

# 3. Look at the .app inside: the container signature is only worth something
#    if the app it carries is Developer-ID-signed too.
MNT=$(mktemp -d /tmp/reamesh-sign.XXXXXX)
cleanup() { hdiutil detach "$MNT" -quiet >/dev/null 2>&1 || true; rmdir "$MNT" 2>/dev/null || true; }
trap cleanup EXIT
# (stderr captured: recent macOS prints a deprecation notice for hdiutil attach)
if ! ATTACH_OUT=$(hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "$MNT" "$DMG" 2>&1); then
  echo "$ATTACH_OUT" >&2; echo "error: could not mount $DMG" >&2; exit 1
fi
APP=$(find "$MNT" -maxdepth 1 -name '*.app' | head -1)
[ -n "$APP" ] || { echo "error: no .app inside $DMG" >&2; exit 1; }
APP_AUTH=$(codesign -dvv "$APP" 2>&1 | grep -m1 '^Authority=' | cut -d= -f2- || true)
hdiutil detach "$MNT" -quiet
rmdir "$MNT" 2>/dev/null || true
trap - EXIT
echo "==> $(basename "$APP") inside is signed by: ${APP_AUTH:-<unsigned>}"
case "$APP_AUTH" in
  "Developer ID Application:"*) ;;
  *)
    if [ "$FORCE" -eq 1 ]; then
      echo "warning: continuing (--force); other Macs will refuse this app" >&2
    else
      cat >&2 <<MSG
error: the app inside the image is not Developer-ID-signed, so signing the
  container alone will not get it past Gatekeeper. Rebuild with the same identity:
    APPLE_SIGNING_IDENTITY='$IDENTITY' bun run dmg:full
  then run this script again. Use --force to sign anyway.
MSG
      exit 1
    fi ;;
esac

# 4. Sign the container (secure timestamp so the signature outlives the cert).
echo "==> Signing $DMG"
codesign --force --sign "$IDENTITY" --timestamp "$DMG"
codesign --verify --verbose=2 --strict "$DMG"
codesign -dvv "$DMG" 2>&1 | grep -E '^(Authority|TeamIdentifier|Timestamp)=' | sed 's/^/    /'

# 5. Optional notarization + stapling.
if [ "$NOTARIZE" -eq 1 ]; then
  [ -n "$PROFILE" ] || { echo "error: --notarize needs --profile <keychain-profile> (or \$NOTARY_PROFILE)" >&2; exit 1; }
  echo "==> Notarizing (profile: $PROFILE); this waits for Apple's verdict…"
  xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait
  echo "==> Stapling ticket"
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl -a -t open --context context:primary-signature -v "$DMG"
fi

echo "signed $DMG"
