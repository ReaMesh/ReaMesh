// Writes the updater manifest (latest.json) for a release from its signed
// updater artifacts:
//
//   bun scripts/release-manifest.ts <artifacts-dir> <version> <download-base-url>
//
// Looks for `*.app.tar.gz` (macOS, Apple silicon) and `*-setup.exe` (Windows)
// with their `.sig` files, as `tauri build --config src-tauri/tauri.release.conf.json`
// leaves them. Next to Tauri's own fields it records the release's PROTO: the
// app offers a restart in the middle of a session only when that matches the
// running build (see src-tauri/src/updater.rs).

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const [dir, version, base] = process.argv.slice(2);
if (!dir || !version || !base) {
  console.error("usage: bun scripts/release-manifest.ts <artifacts-dir> <version> <download-base-url>");
  process.exit(1);
}
const repo = resolve(import.meta.dir, "../..");
const proto = Number(
  /const PROTO: u32 = (\d+);/.exec(readFileSync(join(repo, "rust/src/lib.rs"), "utf8"))?.[1],
);
if (!proto) throw new Error("PROTO not found in rust/src/lib.rs");

const files = readdirSync(dir);
const platforms: Record<string, { signature: string; url: string }> = {};
const entry = (key: string, test: (f: string) => boolean) => {
  const file = files.find((f) => test(f) && files.includes(`${f}.sig`));
  if (!file) return;
  platforms[key] = {
    signature: readFileSync(join(dir, `${file}.sig`), "utf8").trim(),
    url: `${base.replace(/\/$/, "")}/${encodeURIComponent(file)}`,
  };
};
entry("darwin-aarch64", (f) => f.endsWith(".app.tar.gz"));
entry("windows-x86_64", (f) => f.endsWith("-setup.exe"));
if (!Object.keys(platforms).length) throw new Error(`no signed updater artifacts in ${dir}`);

const manifest = {
  version,
  proto,
  notes: "",
  pub_date: new Date().toISOString(),
  platforms,
};
writeFileSync(join(dir, "latest.json"), JSON.stringify(manifest, null, 2) + "\n");
console.log(`latest.json: ${version}, proto ${proto}, ${Object.keys(platforms).join(", ")}`);
