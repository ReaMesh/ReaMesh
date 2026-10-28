import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MAX_RELAYS, parseRelayCode, type RelayCode } from "./relay";

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
}
