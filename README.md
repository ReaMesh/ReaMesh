# ReaMesh

Work on a REAPER project together. ReaMesh shares project files and keeps edits
in sync between computers on a local network. No account or server needed.

One person hosts a project, the others join and get a local copy. Everyone can
then add, remove and reorder tracks and change their basic settings. A panel
inside REAPER handles hosting and joining.

## Getting started

ReaMesh runs on macOS. The panel needs ReaImGui, which you can install with
[ReaPack](https://reapack.com). There are no packaged builds yet, so
[build the extension from source](#building-from-source) first.

1. Copy `reaper_reamesh.dylib` into
   `~/Library/Application Support/REAPER/UserPlugins/` on each Mac.
2. Restart REAPER. ReaMesh installs its panel script on startup.
3. Run the action "ReaMesh: Open panel (GUI)". Save a project in its own
   folder, then click Host current project.
4. On the other computer, open the panel and join the session from the list.
   ReaMesh downloads the project and opens it in REAPER.

The panel shows download progress and how many people are connected, and stops
hosting or leaves the session. Without ReaImGui, the actions "ReaMesh: Host
session", "ReaMesh: Join session" and "ReaMesh: Leave session" still work and
report to the REAPER console.

**Hosting shares the project folder**, including files that aren't used in the
project. Dotfiles, `.rpp-bak` backups and `.reapeaks` caches are excluded.
Media outside the folder isn't shared, so save with "Copy all media into project
directory" before hosting.

Sessions are discovered automatically on the LAN. ReaMesh has no relay or NAT
traversal, so everyone needs to be on the same local network.

## What syncs

| Part of the project | Shared edits |
| --- | --- |
| Tracks | Add, remove, reorder, names, volume, pan, mute and solo |

Everything else arrives with the initial download but isn't synced afterwards.
That includes media items, effects, automation, tempo and markers. Media added
during a session doesn't transfer, and master-track state isn't synced.

## Working together

The host has priority. If the host and a guest change the same field within
about three seconds of each other, the host's change wins and the guest's snaps
back. A guest's change that the host doesn't confirm in that time also snaps
back.

Remote edits don't create undo points. Undoing an earlier local action can still
roll back received changes because REAPER uses snapshots for undo. That rollback
syncs too.

When the host stops the session, everyone keeps their local project copy.

To use the project's effects, you'll need the plugins installed locally.

Only join people you trust: shared projects can load plugins on your computer.
Sessions have no passphrase yet, so anyone on the network can join.

## Building from source

Install Rust, CMake 3.20 or newer and the Xcode command-line tools.

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

### Checks

Run this from the repository root:

```sh
cargo test --manifest-path rust/Cargo.toml
```

The Rust tests run a host and a guest in one process over real sockets. There
are also [C ABI smoke tests](tools/ffi_smoke.cpp).

## Code layout

- [`extension/`](extension/): C++ extension that reads and applies REAPER state
  on its main thread.
- [`rust/`](rust/): networking, file transfers and shared state using libp2p,
  Tokio and yrs. The host coordinates edits.
- [`scripts/`](scripts/): ReaImGui panel, embedded into the extension at build
  time.

The extension and Rust core communicate through a C ABI. The panel talks to the
extension through its ReaScript API.
