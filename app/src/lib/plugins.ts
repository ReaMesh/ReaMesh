// Match manifest plugin names against the local REAPER cache. Compare core
// names without case or vendor differences; report unsupported or unavailable
// checks as unknown, since they cannot establish that a plugin is missing.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface FxRef {
  /** Format prefix without the colon ("VST", "VST3", "VSTi", "CLAP", "AU",
   *  "JS", …): "" when the display name carries none. */
  fmt: string;
  /** Core plugin name, vendor parenthetical stripped. */
  name: string;
  /** Vendor from the trailing "(...)", "" when absent. */
  vendor: string;
  raw: string;
}

/** Parse an RPP FX display name into format / name / vendor. */
export function parseFxName(raw: string): FxRef {
  let fmt = "";
  let rest = raw.trim();
  const m = rest.match(/^([A-Za-z0-9]{2,5}i?):\s*(.*)$/);
  if (m) {
    fmt = m[1];
    rest = m[2];
  }
  let name = rest;
  let vendor = "";
  const v = rest.match(/^(.*?)\s*\(([^()]*)\)\s*$/);
  if (v && v[1].trim()) {
    name = v[1].trim();
    vendor = v[2].trim();
  }
  return { fmt, name: name.trim(), vendor, raw };
}

export type PluginStatus = "have" | "missing" | "unknown";

/** Lower-cased lookup keys for everything the local REAPER knows: the raw
 *  cache names, the core names, and "name (vendor)" pairs. */
export function buildInstalledIndex(installed: string[]): Set<string> {
  const set = new Set<string>();
  for (let s of installed) {
    // REAPER's VST cache marks instruments with a "!!!VSTi" prefix on the
    // display name. The backend strips it, but be tolerant of a raw one.
    if (s.startsWith("!!!VSTi")) s = s.slice("!!!VSTi".length);
    const low = s.toLowerCase().trim();
    if (!low) continue;
    set.add(low);
    const p = parseFxName(s);
    if (p.name) set.add(p.name.toLowerCase());
    if (p.name && p.vendor) set.add(`${p.name.toLowerCase()} (${p.vendor.toLowerCase()})`);
  }
  return set;
}

/** Whether a host-claimed plugin is installed on this machine. `index` null
 *  (or empty) means the scan yielded nothing usable → "unknown", never
 *  "missing". */
export function pluginStatus(claimed: string, index: Set<string> | null): PluginStatus {
  if (!index || index.size === 0) return "unknown";
  const p = parseFxName(claimed);
  const name = p.name.toLowerCase();
  if (p.fmt.toUpperCase() === "JS") {
    // JS entries index as exact "js: path" from the Effects-dir walk.
    return index.has(claimed.toLowerCase().trim()) || index.has(`js: ${name}`)
      ? "have"
      : "missing";
  }
  if (index.has(claimed.toLowerCase().trim())) return "have";
  if (p.vendor && index.has(`${name} (${p.vendor.toLowerCase()})`)) return "have";
  if (index.has(name)) return "have";
  // REAPER's own bundled plugins are always present, whatever the caches say.
  if (p.vendor.toLowerCase() === "cockos") return "have";
  return "missing";
}

// Module-level cache: the installed set changes only when plugins are
// (un)installed, which involves restarting REAPER anyway: once per app run
// is plenty. `undefined` = not fetched yet, `null` = fetch failed (no backend
// or unreadable caches) → everything renders as "unknown".
let cachedIndex: Set<string> | null | undefined;
let inflight: Promise<void> | null = null;

export function fetchInstalledIndex(): Promise<Set<string> | null> {
  if (cachedIndex !== undefined) return Promise.resolve(cachedIndex);
  inflight ??= invoke<string[]>("installed_plugins")
    .then((list) => {
      cachedIndex = buildInstalledIndex(Array.isArray(list) ? list : []);
    })
    .catch(() => {
      cachedIndex = null;
    })
    .finally(() => {
      inflight = null;
    });
  return inflight.then(() => cachedIndex ?? null);
}

/** The installed-plugin index, fetched once per run. Null until known (and
 *  forever when there is no backend): callers treat null as "can't tell". */
export function useInstalledPlugins(): Set<string> | null {
  const [idx, setIdx] = useState<Set<string> | null>(cachedIndex ?? null);
  useEffect(() => {
    let alive = true;
    fetchInstalledIndex().then((v) => {
      if (alive) setIdx(v);
    });
    return () => {
      alive = false;
    };
  }, []);
  return idx;
}
