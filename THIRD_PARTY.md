# Third-party code

ReaMesh is licensed under the GPL-3.0 (see [LICENSE](LICENSE)). It vendors
and depends on the following third-party work, each under its own licence.

## Vendored in this repository

| Path | Project | Licence |
| --- | --- | --- |
| `extension/sdk/` | REAPER extension SDK, Cockos Incorporated | Permissive (see header of `reaper_plugin.h`) |
| `extension/WDL/swell/` | SWELL headers from WDL, Cockos Incorporated | zlib-style (WDL licence) |
| `extension/include/json.hpp` | nlohmann/json, Niels Lohmann | MIT |

`extension/include/reamesh_ffi.h` is generated from this project's own Rust
source by cbindgen and is not third-party.

## Dependencies

The Rust crates (`rust/Cargo.lock`, `app/src-tauri/Cargo.lock`) and npm
packages (`app/bun.lock`) are under permissive licences (MIT, Apache-2.0, BSD,
ISC, Zlib, Unicode and similar). A few shipped crates use MPL-2.0, a weak
file-level copyleft that covers only their own files: `cssparser`, `selectors`
and `dtoa-short` (pulled in by Tauri), `attohttpc` and `option-ext`. None of the
code shipped in the extension or the companion app is under a strong-copyleft
licence. This is enforced in CI by `cargo deny check licenses` with the
allow-list in [deny.toml](deny.toml) and by `tools/check-npm-licenses.ts`. The
companion app lists every shipped package on the Acknowledgements page in its
settings.

Build-time-only tools that are not distributed with ReaMesh and carry
non-permissive terms: `lightningcss` and `@resvg/resvg-js` (MPL-2.0).

## Trademarks

REAPER is a trademark of Cockos Incorporated. ReaMesh is an independent
project and is not affiliated with or endorsed by Cockos.
