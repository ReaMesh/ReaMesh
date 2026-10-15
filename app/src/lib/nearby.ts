import { useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { send } from "./connection";
import { ticketAddrs } from "./ticket";
import { PROTO } from "./version";
import type { Mode } from "./types";

// Nearby sessions over Bluetooth LE, a discovery fallback for networks that
// block the usual LAN search (src-tauri/src/nearby.rs owns the radio). While
// hosting, the app advertises the host's network addresses, taken from its own
// invite link; in the Lobby it scans, and each address it reads goes to the
// extension as a `hint` to dial. The session then shows up in the Lobby the
// normal way, from its signed announce over the network, or not at all.
// Outside Tauri every call fails quietly and the feature reads unsupported.

export type NearbyAuth = "unsupported" | "undetermined" | "restricted" | "denied" | "allowed";

export interface NearbyState {
  supported: boolean;
  /** `null` while the user never decided either way. */
  enabled: boolean | null;
  auth: NearbyAuth;
}

let state: NearbyState = { supported: false, enabled: false, auth: "unsupported" };
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());
let started = false;

function refresh() {
  invoke<NearbyState>("nearby_state")
    .then((s) => {
      if (!s) return;
      state = s;
      emit();
    })
    .catch(() => {});
}

function start() {
  if (started) return;
  started = true;
  refresh();
  // Turned on or off in the native settings window.
  listen("nearby-changed", refresh).catch(() => {});
  // Back from the system's permission prompt or System Settings.
  window.addEventListener("focus", refresh);
}

function subscribe(cb: () => void) {
  start();
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function useNearby(): NearbyState {
  return useSyncExternalStore(subscribe, () => state);
}

export function setNearby(on: boolean) {
  invoke<NearbyState>("set_nearby", { on })
    .then((s) => {
      if (!s) return;
      state = s;
      emit();
    })
    .catch(() => {});
}

/** Whether the permission is out of the app's hands. */
export function nearbyBlocked(n: NearbyState): boolean {
  return n.auth === "denied" || n.auth === "restricted";
}

/** Run the radio for what is on screen: scan in the Lobby, advertise while
 *  hosting (once the invite link, and so the addresses, are known). */
export function useNearbySync(mode: Mode | undefined, ticket: string | undefined, port: number | undefined) {
  const on = useNearby().enabled === true;
  const scanning = on && mode === "browsing";
  useEffect(() => {
    invoke("nearby_scan", { on: scanning }).catch(() => {});
  }, [scanning]);

  const addrs = on && mode === "hosting" && ticket ? ticketAddrs(ticket) : null;
  const payload = addrs ? JSON.stringify({ v: PROTO, ...addrs }) : null;
  useEffect(() => {
    invoke("nearby_advertise", { payload }).catch(() => {});
  }, [payload]);

  useEffect(() => {
    if (!scanning || port == null) return;
    const un = listen<string>("nearby:found", (e) => {
      try {
        const p = JSON.parse(e.payload) as { ips?: unknown; tcp?: unknown; quic?: unknown };
        send({ cmd: "hint", ips: p.ips, tcp: p.tcp, quic: p.quic }, port);
      } catch {
        // The backend only forwards clean payloads; nothing to do otherwise.
      }
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, [scanning, port]);
}
