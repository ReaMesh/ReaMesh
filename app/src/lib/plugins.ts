// Match manifest plugin names against the local REAPER cache. Compare core
// names without case or vendor differences, but keep the format: a project
// saved with the VST3 of a plugin doesn't load its VST2 or AU. Report
// unsupported or unavailable checks as unknown, since they cannot establish
// that a plugin is missing.

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

// REAPER appends the channel layout to some names: "ReaSynDr (Cockos) (4 out)".
const CHANNELS = /\s*\((?:mono|\d+\s*out|\d+\s*ch|\d+->\d+\s*ch)\)\s*$/i;

/** Parse an RPP FX display name into format / name / vendor. */
export function parseFxName(raw: string): FxRef {
  let fmt = "";
  let rest = raw.trim();
  const m = rest.match(/^([A-Za-z0-9]{2,5}i?):\s*(.*)$/);
  if (m) {
    fmt = m[1];
    rest = m[2];
  }
  rest = rest.replace(CHANNELS, "");
  let name = rest;
  let vendor = "";
  const v = rest.match(/^(.*?)\s*\(([^()]*)\)\s*$/);
  if (v && v[1].trim()) {
    name = v[1].trim();
    vendor = v[2].trim();
  }
  return { fmt, name: name.trim(), vendor, raw };
}

/** The format without the instrument marker: "VST3i" and "VST3" are one
 *  plugin format, "VSTi" and "VST" another. "" when the name carries none. */
export function fxFormat(fmt: string): string {
  const up = fmt.toUpperCase();
  return up.length > 2 && up.endsWith("I") ? up.slice(0, -1) : up;
}

/** A format as people say it: REAPER's bare "VST" is VST2. */
export function formatLabel(format: string): string {
  return format === "VST" ? "VST2" : format;
}

/** `format`: installed, but only in another plugin format. */
export type PluginStatus = "have" | "format" | "missing" | "unknown";

/** Everything the local REAPER knows, keyed in lower case. */
export interface InstalledIndex {
  /** JS effects as "js: path". */
  js: Set<string>;
  /** Core plugin name → the formats it is installed in. "" stands for an
   *  entry whose format is not known, which then matches any. */
  formats: Map<string, Set<string>>;
}

export function buildInstalledIndex(installed: string[]): InstalledIndex {
  const index: InstalledIndex = { js: new Set(), formats: new Map() };
  for (let s of installed) {
    // The VST cache marks instruments with "!!!VSTi". The backend turns that
    // into the format prefix, but be tolerant of a raw one on either end.
    s = s.replace("!!!VSTi", "").trim();
    if (!s) continue;
    const p = parseFxName(s);
    const format = fxFormat(p.fmt);
    if (format === "JS") {
      index.js.add(s.toLowerCase());
      continue;
    }
    const key = p.name.toLowerCase();
    if (!key) continue;
    let formats = index.formats.get(key);
    if (!formats) index.formats.set(key, (formats = new Set()));
    formats.add(format);
  }
  return index;
}

function isEmpty(index: InstalledIndex | null): index is null {
  return !index || (index.js.size === 0 && index.formats.size === 0);
}

/** Whether a host-claimed plugin is installed on this machine. `index` null
 *  (or empty) means the scan yielded nothing usable → "unknown", never
 *  "missing". */
export function pluginStatus(claimed: string, index: InstalledIndex | null): PluginStatus {
  if (isEmpty(index)) return "unknown";
  const p = parseFxName(claimed);
  const name = p.name.toLowerCase();
  const format = fxFormat(p.fmt);
  if (format === "JS") {
    // JS entries index as exact "js: path" from the Effects-dir walk.
    return index.js.has(claimed.toLowerCase().trim()) || index.js.has(`js: ${name}`) ? "have" : "missing";
  }
  const formats = index.formats.get(name);
  if (!formats) {
    // REAPER's own bundled plugins are always present, whatever the caches say.
    return p.vendor.toLowerCase() === "cockos" ? "have" : "missing";
  }
  return !format || formats.has(format) || formats.has("") ? "have" : "format";
}

/** For a plugin installed only in another format: the format the project
 *  needs and the ones this machine has, as labels. Null in every other case. */
export function formatMismatch(
  claimed: string,
  index: InstalledIndex | null,
): { need: string; have: string } | null {
  if (isEmpty(index) || pluginStatus(claimed, index) !== "format") return null;
  const p = parseFxName(claimed);
  const have = [...(index.formats.get(p.name.toLowerCase()) ?? [])].filter(Boolean).sort();
  return { need: formatLabel(fxFormat(p.fmt)), have: have.map(formatLabel).join(", ") };
}

// Module-level cache: the installed set changes only when plugins are
// (un)installed, which involves restarting REAPER anyway: once per app run
// is plenty. `undefined` = not fetched yet, `null` = fetch failed (no backend
// or unreadable caches) → everything renders as "unknown".
let cachedIndex: InstalledIndex | null | undefined;
let inflight: Promise<void> | null = null;

export function fetchInstalledIndex(): Promise<InstalledIndex | null> {
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
export function useInstalledPlugins(): InstalledIndex | null {
  const [idx, setIdx] = useState<InstalledIndex | null>(cachedIndex ?? null);
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
