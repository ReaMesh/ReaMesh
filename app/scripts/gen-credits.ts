// Writes src/assets/credits.json: every third-party package that ships in
// ReaMesh (the core + extension, the app backend and the app's runtime npm
// packages, plus the vendored C++ code), for the Acknowledgements screen.
//
//   bun scripts/gen-credits.ts          regenerate
//   bun scripts/gen-credits.ts --check  fail when the committed file is stale
//
// Only what is shipped counts: normal Rust dependencies (not dev/build) and
// the npm runtime closure (not devDependencies). Runs before every release
// build (beforeBuildCommand); when cargo isn't reachable it keeps the
// committed file instead of failing the build.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const app = resolve(import.meta.dir, "..");
const repo = resolve(app, "..");
const out = join(app, "src/assets/credits.json");
const check = process.argv.includes("--check");

interface Credit {
  /** Package name. */
  n: string;
  /** SPDX licence expression (as published). */
  l: string;
  /** Homepage or repository, when the package names one. */
  u?: string;
}

const credits = new Map<string, Credit>();
function add(c: Credit) {
  const key = c.n.toLowerCase();
  const prev = credits.get(key);
  if (!prev) credits.set(key, c);
  else if (!prev.u && c.u) prev.u = c.u;
}

function cleanUrl(u: unknown): string | undefined {
  if (typeof u !== "string" || !u) return undefined;
  return u
    .replace(/^git\+/, "")
    .replace(/^git:\/\//, "https://")
    .replace(/\.git$/, "")
    .replace(/^github:/, "https://github.com/");
}

// ---------- Rust ----------

interface CargoPackage {
  id: string;
  name: string;
  license: string | null;
  repository: string | null;
  homepage: string | null;
  source: string | null;
}

function rust(manifestDir: string): boolean {
  let meta: {
    packages: CargoPackage[];
    resolve: { root: string | null; nodes: { id: string; deps: { pkg: string; dep_kinds: { kind: string | null }[] }[] }[] };
    workspace_members: string[];
  };
  try {
    const raw = execFileSync("cargo", ["metadata", "--format-version", "1", "--locked"], {
      cwd: manifestDir,
      encoding: "utf8",
      maxBuffer: 256 * 1024 * 1024,
      stdio: ["ignore", "pipe", "ignore"],
    });
    meta = JSON.parse(raw);
  } catch {
    return false;
  }
  const byId = new Map(meta.packages.map((p) => [p.id, p]));
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
  // Walk normal edges only, from the workspace members.
  const seen = new Set<string>();
  const stack = [...meta.workspace_members];
  while (stack.length) {
    const id = stack.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    for (const d of nodes.get(id)?.deps ?? []) {
      if (d.dep_kinds.some((k) => k.kind === null)) stack.push(d.pkg);
    }
  }
  for (const id of seen) {
    const p = byId.get(id);
    if (!p || !p.source) continue; // our own crates
    add({ n: p.name, l: p.license ?? "see source", u: cleanUrl(p.repository ?? p.homepage) });
  }
  return true;
}

// ---------- npm ----------

const pkgJson = (dir: string) => JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));

function locate(name: string, from: string): string | null {
  let dir = from;
  for (;;) {
    const cand = join(dir, "node_modules", name);
    if (existsSync(join(cand, "package.json"))) return cand;
    if (dir === app) return null;
    const parent = resolve(dir, "..");
    if (parent === dir) return null;
    dir = parent;
  }
}

function licenseOf(pkg: { license?: unknown; licenses?: unknown }): string {
  let lic = pkg.license;
  if (lic && typeof lic === "object") lic = (lic as { type?: string }).type;
  if (!lic && Array.isArray(pkg.licenses)) {
    lic = pkg.licenses.map((l: { type?: string }) => l.type ?? "?").join(" OR ");
  }
  return String(lic ?? "see source");
}

function npm(): boolean {
  if (!existsSync(join(app, "node_modules"))) return false;
  const seen = new Set<string>();
  const walk = (dir: string) => {
    const pkg = pkgJson(dir);
    for (const name of Object.keys(pkg.dependencies ?? {})) {
      const where = locate(name, dir);
      if (!where) continue;
      const p = pkgJson(where);
      const key = `${p.name}@${p.version}`;
      if (seen.has(key)) continue;
      seen.add(key);
      const repoUrl = typeof p.repository === "string" ? p.repository : p.repository?.url;
      add({ n: p.name, l: licenseOf(p), u: cleanUrl(p.homepage ?? repoUrl) });
      walk(where);
    }
  };
  walk(app);
  return true;
}

// ---------- vendored ----------

const VENDORED: Credit[] = [
  { n: "REAPER extension SDK", l: "Cockos (permissive)", u: "https://github.com/justinfrankel/reaper-sdk" },
  { n: "WDL / SWELL", l: "zlib", u: "https://www.cockos.com/wdl/" },
  { n: "nlohmann/json", l: "MIT", u: "https://github.com/nlohmann/json" },
];

// ---------- write ----------

const ok =
  rust(join(repo, "rust")) && rust(join(app, "src-tauri")) && npm();
if (!ok) {
  if (check) {
    console.error("gen-credits: cargo or node_modules unavailable, can't check");
    process.exit(1);
  }
  console.warn("gen-credits: cargo or node_modules unavailable, keeping the committed list");
  process.exit(0);
}
VENDORED.forEach(add);

const list = [...credits.values()].sort((a, b) =>
  a.n.localeCompare(b.n, "en", { sensitivity: "base" }),
);
const text = JSON.stringify(list, null, 0).replace(/\},\{/g, "},\n{") + "\n";

if (check) {
  const current = existsSync(out) ? readFileSync(out, "utf8") : "";
  if (current !== text) {
    console.error("gen-credits: src/assets/credits.json is stale, run `bun scripts/gen-credits.ts`");
    process.exit(1);
  }
  console.log(`gen-credits: ${list.length} packages, up to date`);
} else {
  writeFileSync(out, text);
  console.log(`gen-credits: ${list.length} packages`);
}
