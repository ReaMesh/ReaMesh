# ReaMesh

Work on a REAPER project together. ReaMesh shares project files and keeps edits
in sync between computers on a local network. No account or server needed.

One person hosts a project, the others join and get a local copy. Everyone can
then edit tracks, arrange audio and MIDI, and work on effects. A desktop app
handles sessions alongside REAPER.

## Getting started

ReaMesh runs on macOS. There are no packaged builds yet, so
[build the extension and app from source](#building-from-source) first.

1. Copy `reaper_reamesh.dylib` into
   `~/Library/Application Support/REAPER/UserPlugins/` on each Mac.
2. Restart REAPER so it loads the extension.
3. Open the ReaMesh app. It connects to REAPER automatically.
4. Save a project in its own folder, then host it from the app. You can set a
   passphrase when creating the session.
5. On the other computer, join from the lobby. ReaMesh downloads the project and
   opens it in REAPER.

**Hosting shares the project folder**, including files that aren't used in the
project. Dotfiles, `.rpp-bak` backups and `.reapeaks` caches are excluded.
Referenced media outside the folder is copied into it for sharing.

Sessions are discovered automatically on the LAN. ReaMesh has no relay or NAT
traversal, so everyone needs to be on the same local network.

## What syncs

| Part of the project | Shared edits |
| --- | --- |
| Tracks | Add, remove, reorder, names, volume, pan, mute and solo |
| Audio and MIDI items | Move, split, trim, fades, takes, take FX and MIDI content |
| Effects and automation | Track FX, plugin state and track envelopes |
| Project settings | Tempo, time signatures, markers and regions |
| Media files | New recordings and imported files transfer in the background |

Selection, open FX windows and window layout stay local. Master-track state
isn't synced.

The app shows everyone in the session by the name and color they picked, and
tells you when people join or leave.

## Working together

The host has priority. If the host and a guest change the same field within
about three seconds of each other, the host's change wins and the guest's snaps
back. A guest's change that the host doesn't confirm in that time also snaps
back. Whole items, FX chains and sets of track envelopes each count as one
field, so working on different tracks or items avoids conflicts.

Each remote edit arrives as one undo point, so Ctrl+Z steps back through
received changes one at a time. Undoing one syncs like any other edit.

When the host stops the session, everyone keeps their local project copy.

To use the project's effects, you'll need the plugins installed locally.

Only join people you trust: shared projects can load plugins on your computer.
Passphrase-protected sessions restrict downloading. Session names remain public
for discovery, and session messages aren't encrypted with the passphrase.

## Building from source

Install Rust, Bun, CMake 3.20 or newer, the Xcode command-line tools and the
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/) for
macOS.

From the repository root:

```sh
./build.sh
```

This builds the Rust core and REAPER extension. `cbindgen` is optional; without
it, the build uses the checked-in FFI header.

| Output | Location |
| --- | --- |
| macOS extension | `extension/build/reaper_reamesh.dylib` |

A [CI workflow](.github/workflows/build.yml) also builds
`reaper_reamesh.dll` for Windows, but that build hasn't been tested yet. On
Windows with Rust, CMake and MSVC, run `cargo build --release` in `rust/`, then
CMake on `extension/`.

After building the extension, build the companion app:

```sh
cd app
bun install
bun run tauri build
```

This produces `app/src-tauri/target/release/bundle/macos/ReaMesh.app`. Use
`bun run tauri dev` instead to run it in development mode. REAPER must be
running with the extension installed for live session data. The app uses Bun
for package management.

### Checks

Run these from the repository root:

```sh
cargo test --manifest-path rust/Cargo.toml
cd app
bun install
bun run build
```

The Rust tests run a host and a guest in one process over real sockets, plus a
round trip through the app's control connection. There are also
[C ABI smoke tests](tools/ffi_smoke.cpp) and
[state-chunk tests](tools/chunk_test.cpp).

## Code layout

- [`extension/`](extension/): C++ extension that reads and applies REAPER state
  on its main thread.
- [`rust/`](rust/): networking, file transfers and shared state using libp2p,
  Tokio and yrs. The host coordinates edits.
- [`app/`](app/): desktop companion built with Tauri 2, React and shadcn/ui.

The extension and Rust core communicate through a C ABI. The companion app
talks to the extension over a localhost WebSocket.
