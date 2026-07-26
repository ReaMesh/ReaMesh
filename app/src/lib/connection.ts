// Connection manager: keeps a WebSocket to every running REAPER instance's
// ReaMesh control server (ports 47415-47418, one per instance) and exposes
// them as a tiny external store for React.
//
// Session state stays in the extension; this module only transports it.
// Commands go out as JSON ({cmd:"host"|"join"|"leave"|"browse"|"set_profile"});
// the extension streams back one `state` message whenever anything changes.

import { useSyncExternalStore } from "react";
import type { ExtState, Instance } from "./types";
import { getProfile } from "./profile";
import { getOverlaySettings, subscribeOverlaySettings } from "./overlaySettings";

const ALL_PORTS = [47415, 47416, 47417, 47418];
const RETRY_MS = 2000;

// VITE_CTL_PORT pins this window to one REAPER instance. Without it,
// scan all control ports and offer an instance switcher.
const pinnedPort = Number(import.meta.env.VITE_CTL_PORT) || 0;
const PORTS = pinnedPort ? [pinnedPort] : ALL_PORTS;

interface StoreState {
  instances: Instance[];
  activePort: number | null;
}

const sockets = new Map<number, WebSocket>();
const live = new Map<number, Instance>();
let activePort: number | null = null;
let snapshot: StoreState = { instances: [], activePort: null };
const listeners = new Set<() => void>();
let started = false;

function emit() {
  // Stable instance order (by port) so the UI never reshuffles.
  snapshot = {
    instances: [...live.values()].sort((a, b) => a.port - b.port),
    activePort,
  };
  listeners.forEach((l) => l());
}

function ensureActive() {
  if (activePort !== null && live.has(activePort)) return;
  const first = [...live.keys()].sort((a, b) => a - b)[0];
  activePort = first ?? null;
}

function connect(port: number) {
  if (sockets.has(port)) return;
  let ws: WebSocket;
  try {
    ws = new WebSocket(`ws://127.0.0.1:${port}`);
  } catch {
    return;
  }
  sockets.set(port, ws);

  ws.onopen = () => {
    live.set(port, { port });
    ensureActive();
    sendProfile(port);
    sendTint(port);
    emit();
  };
  ws.onmessage = (ev) => {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(String(ev.data));
    } catch {
      return;
    }
    const inst = live.get(port) ?? { port };
    if (msg.type === "hello") {
      inst.pid = Number(msg.pid) || undefined;
    } else if (msg.type === "state") {
      inst.state = msg as unknown as ExtState;
    } else if (msg.type === "busy") {
      // Watchdog from the control server: REAPER's main thread stopped
      // ticking (usually a modal dialog waiting for an answer over there).
      inst.busy = Boolean(msg.busy);
    }
    live.set(port, inst);
    ensureActive();
    emit();
  };
  ws.onclose = () => {
    sockets.delete(port);
    if (live.delete(port)) {
      ensureActive();
      emit();
    }
  };
  ws.onerror = () => ws.close();
}

export function startScanning() {
  if (started) return;
  started = true;
  const scan = () => PORTS.forEach(connect);
  scan();
  setInterval(scan, RETRY_MS);
  // The lock tint is painted by the extension, so push the toggle to every
  // connected instance whenever it changes (the initial value rides ws.onopen).
  subscribeOverlaySettings(() => {
    const tint = getOverlaySettings().tint;
    if (tint === lastTintSent) return;
    lastTintSent = tint;
    for (const p of sockets.keys()) sendTint(p);
  });
}

let lastTintSent: boolean | null = null;

/** Tell the extension whether to paint the dark-grey lock tint. */
export function sendTint(port?: number) {
  const cmd = { cmd: "set_tint", on: getOverlaySettings().tint };
  if (port !== undefined) {
    send(cmd, port);
  } else {
    for (const p of sockets.keys()) send(cmd, p);
  }
}

export function send(cmd: Record<string, unknown>, port?: number) {
  const target = port ?? activePort;
  if (target === null) return;
  const ws = sockets.get(target);
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(cmd));
}

export function sendProfile(port?: number) {
  const { name, color } = getProfile();
  const cmd = { cmd: "set_profile", name, color };
  if (port !== undefined) {
    send(cmd, port);
  } else {
    for (const p of sockets.keys()) send(cmd, p);
  }
}

export function getActivePort(): number | null {
  return activePort;
}

export function setActivePort(port: number) {
  if (live.has(port)) {
    activePort = port;
    emit();
  }
}

export function useConnections(): StoreState {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => snapshot,
  );
}

export function useActiveInstance(): Instance | null {
  const { instances, activePort: active } = useConnections();
  return instances.find((i) => i.port === active) ?? null;
}
