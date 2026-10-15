// The compatibility number this app speaks, mirrored from the core's `PROTO`
// (rust/src/lib.rs) and the app backend's (src-tauri/src/lib.rs); version.test.ts
// checks the three agree. App, extension and core release together, so a
// different number on the other end means a different build: the app refuses
// to drive such an extension (REAPER still has the old one loaded) and the
// Lobby refuses to join such a host, instead of anyone testing the mix.
export const PROTO = 1;

/** How a peer's or extension's `proto` relates to ours. A build from before
 *  `proto` existed reports none and counts as 0, i.e. older. */
export function protoSkew(proto: number | undefined): "same" | "newer" | "older" {
  const p = typeof proto === "number" ? proto : 0;
  return p === PROTO ? "same" : p > PROTO ? "newer" : "older";
}

/** How an extension build's `version` relates to this app's. App and extension
 *  ship as one release, so any difference means REAPER still runs another
 *  build's copy; the PROTO check alone would let a same-protocol mix through.
 *  Unknown on either side (no hello yet, version not loaded) counts as same. */
export function versionSkew(
  ext: string | undefined,
  app: string | undefined,
): "same" | "newer" | "older" {
  if (!ext || !app || ext === app) return "same";
  return compareVersions(ext, app) > 0 ? "newer" : "older";
}

/** Semver order: numeric core first, then a pre-release sorts before its
 *  release and pre-release tags compare part by part. */
export function compareVersions(a: string, b: string): number {
  const split = (v: string) => {
    const [core, ...pre] = v.trim().replace(/^v/, "").split("+")[0].split("-");
    return { core: core.split(".").map((n) => Number(n) || 0), pre: pre.join("-") };
  };
  const x = split(a);
  const y = split(b);
  for (let i = 0; i < Math.max(x.core.length, y.core.length); i++) {
    const d = (x.core[i] ?? 0) - (y.core[i] ?? 0);
    if (d) return Math.sign(d);
  }
  if (x.pre === y.pre) return 0;
  if (!x.pre) return 1;
  if (!y.pre) return -1;
  const xs = x.pre.split(".");
  const ys = y.pre.split(".");
  for (let i = 0; i < Math.max(xs.length, ys.length); i++) {
    if (xs[i] === undefined) return -1;
    if (ys[i] === undefined) return 1;
    if (xs[i] === ys[i]) continue;
    const xn = /^\d+$/.test(xs[i]);
    const yn = /^\d+$/.test(ys[i]);
    if (xn && yn) return Math.sign(Number(xs[i]) - Number(ys[i]));
    if (xn !== yn) return xn ? -1 : 1;
    return xs[i] < ys[i] ? -1 : 1;
  }
  return 0;
}
