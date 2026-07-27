// Store previous join decisions by host peer id. The extension persists its
// keypair across sessions, so the trust dialog can recognize returning hosts.
// Use the transport-verified peer id, never the self-reported display name.

const KEY = "reamesh.knownHosts";

function load(): Record<string, number> {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return {};
    const v = JSON.parse(raw);
    return v && typeof v === "object" ? (v as Record<string, number>) : {};
  } catch {
    return {};
  }
}

/** How many times this machine has joined the given host peer id. */
export function hostSeenCount(peerId: string): number {
  if (!peerId) return 0;
  const n = load()[peerId];
  return typeof n === "number" && n > 0 ? n : 0;
}

/** Record a join. Called when the user commits to joining, not on success:
 *  the point is "you have trusted this identity before". */
export function recordHostJoin(peerId: string): void {
  if (!peerId) return;
  try {
    const map = load();
    map[peerId] = (map[peerId] ?? 0) + 1;
    // Bound the map so a hostile lobby can't grow localStorage forever.
    const entries = Object.entries(map);
    if (entries.length > 200) {
      entries.sort((a, b) => b[1] - a[1]);
      localStorage.setItem(KEY, JSON.stringify(Object.fromEntries(entries.slice(0, 200))));
    } else {
      localStorage.setItem(KEY, JSON.stringify(map));
    }
  } catch {
    // localStorage unavailable: the dialog just stays on "first time".
  }
}
