// Reject an extension binary older than any of its sources before bundling.
// The app installs this bundled copy, so a stale binary would omit extension
// changes even when the frontend is current.
//
// --warn only reports the mismatch during development.
// REAMESH_SKIP_EXT_CHECK=1 bypasses the check.

import { readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const warnOnly = process.argv.includes("--warn");

const binary =
  process.platform === "win32"
    ? join(root, "extension/build/Release/reaper_reamesh.dll")
    : process.platform === "darwin"
      ? join(root, "extension/build/reaper_reamesh.dylib")
      : join(root, "extension/build/reaper_reamesh.so");

/** Newest mtime under `dir`, skipping build output; returns [mtime, path]. */
function newest(dir: string, skip: Set<string>): [number, string] {
  let best: [number, string] = [0, ""];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (skip.has(entry.name) || entry.name.startsWith(".")) continue;
    const p = join(dir, entry.name);
    if (entry.isDirectory()) {
      const sub = newest(p, skip);
      if (sub[0] > best[0]) best = sub;
    } else if (entry.isFile()) {
      const m = statSync(p).mtimeMs;
      if (m > best[0]) best = [m, p];
    }
  }
  return best;
}

function fail(msg: string): never {
  const hint = "Rebuild it first: ./build.sh (or cmake --build extension/build --config Release).";
  if (warnOnly) {
    console.warn(`\n[check-extension] WARNING: ${msg}\n[check-extension] ${hint}\n`);
    process.exit(0);
  }
  console.error(`\n[check-extension] ${msg}\n[check-extension] ${hint}`);
  console.error("[check-extension] Set REAMESH_SKIP_EXT_CHECK=1 to bundle it anyway.\n");
  process.exit(1);
}

if (process.env.REAMESH_SKIP_EXT_CHECK === "1") process.exit(0);

let built: number;
try {
  built = statSync(binary).mtimeMs;
} catch {
  fail(`the REAPER extension binary is missing: ${binary}`);
}

const sources = [
  newest(join(root, "rust"), new Set(["target"])),
  newest(join(root, "extension"), new Set(["build"])),
].sort((a, b) => b[0] - a[0])[0];

if (sources[0] > built) {
  const age = ((sources[0] - built) / 60000).toFixed(1);
  fail(
    `the REAPER extension binary is older than its sources (${age} min behind ${sources[1]})`,
  );
}
