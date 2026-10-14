// Writes the updater manifest (latest.json) for a release from its signed
// updater artifacts:
//
//   bun scripts/release-manifest.ts <artifacts-dir> <version> <download-base-url>
//
// Looks for `*.app.tar.gz` (macOS, universal: Apple silicon and Intel get the
// same entry) and `*-setup.exe` (Windows)
// with their `.sig` files, as `tauri build --config src-tauri/tauri.release.conf.json`
// leaves them. Next to Tauri's own fields it records the release's PROTO: the
// app offers a restart in the middle of a session only when that matches the
// running build (see src-tauri/src/updater.rs).
//
// It also writes `extension.json`, the same format for the extension binaries
// (`reaper_reamesh.dylib` / `.dll` with their `.sig`): the app fetches it next
// to the downloaded update to install the new extension before it restarts.

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
type Platforms = Record<string, { signature: string; url: string }>;
const platforms: Platforms = {};
const extension: Platforms = {};
const entry = (key: string, test: (f: string) => boolean, into = platforms) => {
  const file = files.find((f) => test(f) && files.includes(`${f}.sig`));
  if (!file) return;
  into[key] = {
    signature: readFileSync(join(dir, `${file}.sig`), "utf8").trim(),
    url: `${base.replace(/\/$/, "")}/${encodeURIComponent(file)}`,
  };
};
for (const mac of ["darwin-aarch64", "darwin-x86_64"]) {
  entry(mac, (f) => f.endsWith(".app.tar.gz"));
  entry(mac, (f) => f === "reaper_reamesh.dylib", extension);
}
entry("windows-x86_64", (f) => f.endsWith("-setup.exe"));
entry("windows-x86_64", (f) => f === "reaper_reamesh.dll", extension);
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

// Optional: a release without it still updates; the extension then only
// changes when the app restarts into the update.
if (Object.keys(extension).length) {
  const ext = { ...manifest, platforms: extension };
  writeFileSync(join(dir, "extension.json"), JSON.stringify(ext, null, 2) + "\n");
  console.log(`extension.json: ${Object.keys(extension).join(", ")}`);
}
