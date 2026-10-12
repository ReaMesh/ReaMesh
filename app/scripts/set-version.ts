// Sets the one version app, extension and core share, in every manifest:
//
//   bun scripts/set-version.ts 0.2.0
//
// They release together (see PROTO in rust/src/lib.rs), so they never carry
// different numbers; src/lib/version.test.ts fails when they drift. Bump PROTO
// by hand as well when the release changes anything on the wire or the
// control server.

import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const v = process.argv[2];
if (!v || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(v)) {
  console.error("usage: bun scripts/set-version.ts <major.minor.patch>");
  process.exit(1);
}
const repo = resolve(import.meta.dir, "../..");
const edit = (p: string, f: (s: string) => string) => {
  const path = resolve(repo, p);
  const before = readFileSync(path, "utf8");
  const after = f(before);
  if (after === before) throw new Error(`no version found in ${p}`);
  writeFileSync(path, after);
  console.log(`  ${p}`);
};
const json = (s: string) => s.replace(/("version":\s*)"[^"]+"/, `$1"${v}"`);
const cargo = (s: string) => s.replace(/^version = "[^"]+"/m, `version = "${v}"`);

console.log(`ReaMesh ${v}:`);
edit("app/package.json", json);
edit("app/src-tauri/tauri.conf.json", json);
edit("app/src-tauri/Cargo.toml", cargo);
edit("rust/Cargo.toml", cargo);
console.log("Cargo.lock files follow on the next cargo build.");
