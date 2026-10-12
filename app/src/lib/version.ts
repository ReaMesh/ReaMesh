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
