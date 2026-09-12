# Contributing to ReaMesh

Thanks for wanting to help. This page covers what you need to know before
opening a pull request.

## Licensing

- ReaMesh is published under the **GPL-3.0** (see [LICENSE](LICENSE)).
  Contributions are accepted under the same licence.
- The name **ReaMesh** and its logo are not covered by the licence. See the
  trademark note in the [README](README.md#license-and-trademark).

## Before you start

Talk first for anything bigger than a bug fix. Open an issue describing what
you want to change and why. Sync coverage, the wire format, and the
concurrency model have a lot of invariants that are easy to break without
noticing, so read the comments around the code you change, especially in
`reamesh.cpp` and `lib.rs`.

## Building

| Part | Command |
| --- | --- |
| Everything (Rust core + extension) | `./build.sh` (macOS, or Windows from Git Bash) |
| Rust core only | `cd rust && cargo build --release` |
| Rust tests | `cd rust && cargo test -- --test-threads=1` |
| Companion app | `cd app && bun install && bun run build` |
| App unit tests | `cd app && bun run test` |
| Live app | `cd app && bun run tauri dev` (needs REAPER with the extension) |

The Rust session tests use real sockets and mDNS and **must run serially**;
in parallel about half of them fail spuriously. The app is managed with
**bun**, never npm or pnpm.

Both macOS and Windows are first-class. Keep the Rust core free of
`cfg(target_os)`, and keep platform-specific C++ behind small helpers rather
than `#ifdef`s in the capture/apply logic. Anything that changes what goes on
the wire has to round-trip identically on both platforms.

## Pull requests

- One topic per pull request. Small is good.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `refactor:`, `docs:`).
- Run the relevant build and tests before pushing. CI runs the Rust tests
  serially, the app's `tsc` + vitest gate, and a dependency-licence check.
- New user-facing extension messages need a `code` and an i18n entry in
  `app/src/lib/log.ts`; the extension never writes to the REAPER console.
- Don't add dependencies under GPL, LGPL, AGPL, SSPL or other strong-copyleft
  licences. `cargo deny check licenses` and `bun tools/check-npm-licenses.ts`
  enforce this in CI; see [deny.toml](deny.toml) for the allow-list.
- Don't edit vendored code (`extension/sdk/`, `extension/WDL/`,
  `extension/include/json.hpp`, `extension/include/reamesh_ffi.h`).

## Reporting bugs

Use GitHub issues. Include your REAPER version, OS, whether you were host or
guest, and the Activity feed from the companion app. Network-level logs go to
stderr; launching REAPER from a terminal shows them.

Security issues go through [SECURITY.md](SECURITY.md), not public issues.
