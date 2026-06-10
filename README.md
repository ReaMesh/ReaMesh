# ReaMesh

Work on a REAPER project together. ReaMesh keeps track edits in sync between
computers on a local network. No account or server needed.

Everyone opens the same project, and changes to its tracks show up on the other
computers within about a second. For now, ReaMesh syncs the track list and basic
track settings only.

## Getting started

ReaMesh runs on macOS. There are no packaged builds yet, so
[build the extension from source](#building-from-source) first.

1. Copy `reaper_reamesh.dylib` into
   `~/Library/Application Support/REAPER/UserPlugins/` on each Mac.
2. Restart REAPER. The REAPER console shows a short message when ReaMesh loads.
3. Open the same project on each computer. Computers find each other by the
   project's file name, so save it under the same name everywhere.
4. Edit tracks on one computer. The changes appear on the others.

Each computer needs its own copy of the project and its media. ReaMesh doesn't
transfer files.

Computers find each other automatically on the LAN. ReaMesh has no relay or NAT
traversal, so everyone needs to be on the same local network.

## What syncs

| Part of the project | Shared edits |
| --- | --- |
| Tracks | Add, remove, reorder, names, volume, pan, mute and solo |

Media items, effects, automation, tempo, markers and master-track state aren't
synced yet.

## Working together

When two people change the same field at the same time, every computer settles
on the same value. Changes to different fields or tracks all survive.

Remote edits don't create undo points. Undoing an earlier local action can still
roll back received changes because REAPER uses snapshots for undo. That rollback
syncs too.

Sessions are matched by project file name, so two different projects with the
same name will sync into each other. Unsaved projects all share one session.

## Building from source

Install Rust, CMake 3.20 or newer and the Xcode command-line tools. ReaMesh
builds on macOS only for now.

From the repository root:

```sh
./build.sh
```

This builds the Rust core and REAPER extension. `cbindgen` is optional; without
it, the build uses the checked-in FFI header.

| Output | Location |
| --- | --- |
| macOS extension | `extension/build/reaper_reamesh.dylib` |

### Checks

Run this from the repository root:

```sh
cargo test --manifest-path rust/Cargo.toml
```

The Rust tests run two complete peers in one process over real sockets. There
are also [C ABI smoke tests](tools/ffi_smoke.cpp).

## Code layout

- [`extension/`](extension/): C++ extension that reads and applies REAPER state
  on its main thread.
- [`rust/`](rust/): networking and shared state using libp2p, Tokio and yrs.

The extension and Rust core communicate through a C ABI.
