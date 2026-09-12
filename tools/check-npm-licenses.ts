// Dependency-licence gate for the companion app's bun packages (app/).
//
//   bun tools/check-npm-licenses.ts app
//
// Walks node_modules of the given package and fails when any *runtime*
// dependency (transitive closure of "dependencies", not devDependencies)
// carries a strong-copyleft or missing licence. ReaMesh is GPL-3.0; keeping
// GPL/LGPL/AGPL dependencies out of the shipped bundle keeps its own licensing
// in the maintainer's hands. Build-time tools (devDeps) are not shipped and
// are only reported, never fatal.

import { readFileSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(process.argv[2] ?? ".");
const pkgJson = (dir: string) => JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));

// Licences that would infect the shipped program.
const DENY = /\b(A?GPL|SSPL|EUPL|OSL|CPAL|CC-BY-SA)\b/i;
const UNKNOWN = /^(undefined|null|none|unlicensed|see license.*)$/i;

function licenseOf(pkg: any): string {
  let lic = pkg.license;
  if (lic && typeof lic === "object") lic = lic.type;
  if (!lic && Array.isArray(pkg.licenses)) lic = pkg.licenses.map((l: any) => l.type ?? "?").join(" OR ");
  return String(lic ?? "undefined");
}

// Resolve a dependency the way node does: nearest node_modules upward.
function locate(name: string, from: string): string | null {
  let dir = from;
  for (;;) {
    const cand = join(dir, "node_modules", name);
    if (existsSync(join(cand, "package.json"))) return cand;
    if (dir === root) return null;
    const parent = resolve(dir, "..");
    if (parent === dir) return null;
    dir = parent;
  }
}

// An OR expression passes if any alternative is acceptable.
function acceptable(lic: string): boolean {
  if (UNKNOWN.test(lic)) return false;
  const alts = lic.replace(/[()]/g, "").split(/\s+OR\s+/i);
  return alts.some((a) => !DENY.test(a));
}

const seen = new Map<string, { lic: string; runtime: boolean }>();
function walk(dir: string, runtime: boolean) {
  const pkg = pkgJson(dir);
  const deps = Object.keys(pkg.dependencies ?? {});
  const optional = Object.keys(pkg.optionalDependencies ?? {});
  for (const name of [...deps, ...optional]) {
    const where = locate(name, dir);
    if (!where) continue; // optional platform binary not installed here
    const p = pkgJson(where);
    const key = `${p.name}@${p.version}`;
    const prev = seen.get(key);
    if (prev) { prev.runtime ||= runtime; continue; }
    let lic = licenseOf(p);
    // napi-style platform binaries (`@foo/binding-win32-x64`) are published
    // from the same repo as their parent and often omit the licence field;
    // they inherit the parent's licence.
    if (UNKNOWN.test(lic) && optional.includes(name) && !UNKNOWN.test(licenseOf(pkg))) {
      lic = `${licenseOf(pkg)} (inherited from ${pkg.name})`;
    }
    seen.set(key, { lic, runtime });
    walk(where, runtime);
  }
}

const top = pkgJson(root);
walk(root, true);
for (const name of Object.keys(top.devDependencies ?? {})) {
  const where = locate(name, root);
  if (!where) continue;
  const p = pkgJson(where);
  const key = `${p.name}@${p.version}`;
  if (!seen.has(key)) { seen.set(key, { lic: licenseOf(p), runtime: false }); walk(where, false); }
}

const fatal: string[] = [];
const noted: string[] = [];
for (const [key, { lic, runtime }] of seen) {
  if (acceptable(lic)) continue;
  (runtime ? fatal : noted).push(`${key}: ${lic}`);
}

const runtimeCount = [...seen.values()].filter((v) => v.runtime).length;
console.log(`${top.name}: ${seen.size} packages checked, ${runtimeCount} at runtime`);
if (noted.length) console.log(`build-time only (not shipped, informational):\n  ${noted.join("\n  ")}`);
if (fatal.length) {
  console.error(`runtime dependencies with disallowed or missing licence:\n  ${fatal.join("\n  ")}`);
  process.exit(1);
}
console.log("no copyleft runtime dependencies");
