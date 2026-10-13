# Proprietary assets

Everything in this folder is the ReaMesh brand: the logo, the app icons, the
installer artwork and the README screenshots. None of it is covered by the GPL that licenses the rest of
the repository. All rights reserved.

Modified versions must use a different name and logo (see
[License and trademark](../README.md#license-and-trademark)). To ship a fork,
replace the files here with your own under the same names; the build picks
everything up from this folder and the code needs no changes.

| Path | Used by |
| --- | --- |
| `logo/mark.svg` | The R in the app UI (`app/src/components/RMark.tsx` reads its paths at build time) |
| `logo/lockup-on-*.svg` | Source artwork for the lockup (R + wordmark) |
| `logo/lockup-*.png` | The README header |
| `screenshots/*.png`, `screenshots/*.webp` | README screenshots, the README animation and the social preview images |
| `icons/*.png`, `icons/icon.icns`, `icons/icon.ico` | App bundle and window icons (`bundle.icon` in `app/src-tauri/tauri.conf.json`) |
| `icons/Assets.car` | The macOS 26+ app icon, compiled from `icons/ReaMesh.icon` |
| `icons/ReaMesh.icon` | Icon Composer source; `bun run icons:mac` renders the legacy macOS icons from it |
| `icons/windows-mark.svg` | Windows icon source; `bun run icons:windows` renders `icon.ico` and the tiles from it |
| `dmg/background.svg` | The macOS installer window background (`bun run dmg`) |
