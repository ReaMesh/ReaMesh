// Builds the Windows installer, ReaMesh Setup (../../setup):
//
//   bun run tauri build    # the app itself (src-tauri/target/release/app.exe)
//   bun run setup          # this script
//
// 1. the Setup window's page (vite.setup.config.ts → dist-setup/),
// 2. the Setup program, which embeds that page,
// 3. the installer: Setup with the app and the extension appended as its
//    payload, at src-tauri/target/release/bundle/setup/ReaMesh_<version>_x64-setup.exe.
//
// The same .exe is the in-app update: the updater runs it with `/S /UPDATE /R`.
// It is signed for the updater in release.yml, never here.

import { existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

if (process.platform !== "win32") {
  console.error("ReaMesh Setup is built on Windows only.");
  process.exit(1);
}

const app = resolve(import.meta.dir, "..");
const repo = resolve(app, "..");
const setup = join(repo, "setup");
const version: string = (await Bun.file(join(app, "package.json")).json()).version;

function run(cmd: string[], cwd: string) {
  console.log(`$ ${cmd.join(" ")}`);
  const r = Bun.spawnSync(cmd, { cwd, stdout: "inherit", stderr: "inherit" });
  if (r.exitCode !== 0) process.exit(r.exitCode ?? 1);
}

const appExe = join(app, "src-tauri/target/release/app.exe");
const dll = join(repo, "extension/build/Release/reaper_reamesh.dll");
for (const [what, p] of [
  ["the app (bun run tauri build)", appExe],
  ["the extension (./build.sh, or the cmake step)", dll],
] as const) {
  if (!existsSync(p)) {
    console.error(`Missing ${what}: ${p}`);
    process.exit(1);
  }
}
// The app build already refuses an extension older than its sources; the one
// thing left to catch is an app binary older than that extension.
if (statSync(appExe).mtimeMs < statSync(dll).mtimeMs) {
  console.warn("warning: app.exe is older than reaper_reamesh.dll; run `bun run tauri build` first if the app changed.");
}

run(["bun", "x", "vite", "build", "--config", "vite.setup.config.ts"], app);
run(["cargo", "build", "--release", "--locked", "--features", "custom-protocol"], setup);

const out = join(app, "src-tauri/target/release/bundle/setup");
mkdirSync(out, { recursive: true });
for (const f of readdirSync(out)) {
  if (f.endsWith("-setup.exe")) rmSync(join(out, f));
}
const installer = join(out, `ReaMesh_${version}_x64-setup.exe`);
run(
  [
    join(setup, "target/release/pack.exe"),
    join(setup, "target/release/reamesh-setup.exe"),
    installer,
    version,
    `app.exe=${appExe}`,
    `reaper_reamesh.dll=${dll}`,
  ],
  setup,
);
console.log(`\nBuilt: ${installer}`);
