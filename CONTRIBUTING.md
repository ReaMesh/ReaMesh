# Contributing to ReaMesh

Thanks for wanting to help. Pull requests are welcome straight away, no issue
needed first; rough edges get sorted out in review.

## Ground rules

1. **Run the build and tests before pushing.** CI runs the same checks.
2. **Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)**
   (`feat:`, `fix:`, `refactor:`, `docs:`).
3. **Changed the wire format or the app↔extension protocol? Bump `PROTO`**
   (`rust/src/lib.rs`, `app/src-tauri/src/lib.rs`, `app/src/lib/version.ts`).
4. **Keep both platforms working.** macOS and Windows peers share sessions,
   so no `cfg(target_os)` in the Rust core and no `#ifdef`s in the
   capture/apply logic.
5. **No copyleft dependencies** (GPL, LGPL, AGPL, SSPL, …). CI enforces the
   [allow-list](deny.toml).
6. **Don't edit vendored code**: `extension/sdk/`, `extension/WDL/`,
   `extension/include/json.hpp`, `extension/include/reamesh_ffi.h`.

Sync, the wire format and the concurrency model carry invariants that break
quietly. Read the comments around what you change, especially in
`reamesh.cpp` and `lib.rs`.

## Building

| Part | Command |
| --- | --- |
| Everything | `./build.sh` (macOS, or Windows from Git Bash) |
| Rust tests | `cd rust && cargo test -- --test-threads=1` |
| App build | `cd app && bun install && bun run build` |
| App tests | `cd app && bun run test` |
| Live app | `cd app && bun run tauri dev` (needs REAPER with the extension) |
| Two instances | `cd app && bun run dev:ab` (start two REAPER instances first) |

The Rust tests must run serially; in parallel they fail spuriously. The app
uses **bun**, never npm or pnpm.

## Reporting bugs

Open a GitHub issue with your REAPER version, OS, whether you hosted or
joined, and the output of **Settings → Copy diagnostics** in the app.

Security issues go through [SECURITY.md](SECURITY.md), not public issues.

## Licensing

ReaMesh is [GPL-3.0](LICENSE); contributions are accepted under the same
licence. The ReaMesh name, logo and everything in [`proprietary/`](proprietary/)
are not covered by it (see the
[trademark note](README.md#license-and-trademark)).
