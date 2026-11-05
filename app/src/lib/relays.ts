import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MAX_RELAYS, parseRelayCode, type RelayCode } from "./relay";
import type { RelayAbout } from "./relayInfo";

// The relays configured in Settings → Relay. The backend stores them, every
// connected extension gets them as `set_relays` (lib/connection.ts) and uses
// them from its next hosting run on. Outside Tauri the list stays empty.

let codes: RelayCode[] = [];
let loaded = false;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

function load() {
  invoke<string[]>("get_relays")
    .then((list) => {
      codes = (list ?? []).map(parseRelayCode).filter((c): c is RelayCode => c !== null);
      emit();
    })
    .catch(() => {});
}

function start() {
  if (loaded) return;
  loaded = true;
  load();
  // Changed in the other window (Settings on macOS).
  listen("relays-changed", load).catch(() => {});
}

export function subscribeRelays(cb: () => void): () => void {
  start();
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function getRelays(): RelayCode[] {
  start();
  return codes;
}

export function useRelays(): RelayCode[] {
  return useSyncExternalStore(subscribeRelays, () => codes);
}

async function store(next: RelayCode[]) {
  codes = next;
  emit();
  await invoke("set_relays", { codes: next.map((c) => c.code) });
}

/** Add a pasted code at the end of the list. `false` when it doesn't parse,
 *  is already there, or the list is full. */
export async function addRelay(raw: string): Promise<boolean> {
  const c = parseRelayCode(raw);
  if (!c || codes.length >= MAX_RELAYS || codes.some((o) => o.peer === c.peer)) return false;
  await store([...codes, c]);
  return true;
}

export async function removeRelay(peer: string): Promise<void> {
  await store(codes.filter((c) => c.peer !== peer));
  const cached = readCache();
  delete cached[peer];
  writeCache(cached);
}

// What a relay last said, kept across restarts: a notice about downtime is most
// useful exactly when the relay can't be asked.
const CACHE_KEY = "reamesh.relayInfo";

function readCache(): Record<string, RelayAbout> {
  try {
    const v = JSON.parse(localStorage.getItem(CACHE_KEY) ?? "{}");
    return v && typeof v === "object" && !Array.isArray(v) ? v : {};
  } catch {
    return {};
  }
}

function writeCache(v: Record<string, RelayAbout>) {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(v));
  } catch {
    // Only a convenience.
  }
}

/** A relay's answers came in: remember them, and take over a changed address.
 *  Nothing else about the list changes, so no "applies next time" toast. */
export function noteRelayInfo(info: Record<string, RelayAbout>) {
  const peers = Object.keys(info);
  if (peers.length === 0) return;
  const cached = readCache();
  let next: RelayCode[] | null = null;
  for (const peer of peers) {
    // `new_code` and the transient flags are no use after a restart.
    const { new_code, stopping: _stopping, ...keep } = info[peer];
    cached[peer] = keep;
    const fresh = new_code ? parseRelayCode(new_code) : null;
    if (fresh?.peer === peer && codes.some((c) => c.peer === peer && c.code !== fresh.code)) {
      next = (next ?? codes).map((c) => (c.peer === peer ? fresh : c));
    }
  }
  writeCache(cached);
  if (next) store(next).catch(() => {});
}

/** What is known about a relay: live from `live`, else from the last time. */
export function relayAbout(live: Record<string, RelayAbout> | undefined, peer: string): RelayAbout | undefined {
  return live?.[peer] ?? readCache()[peer];
}
