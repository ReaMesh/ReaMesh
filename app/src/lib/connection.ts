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
/** Retry unused ports less often after repeated refusals. This avoids
 *  unnecessary wakeups and loopback TIME_WAIT entries when fewer than four
 *  REAPER instances are running. */
const BACKOFF_AFTER = 3;
const BACKOFF_MS = 10000;

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
/** Consecutive failed connects per port, and the wall-clock time we may next
 *  retry it. Reset the moment a port answers. */
const failures = new Map<number, { count: number; nextAt: number }>();

/** A `state` frame, with the array fields forced to arrays.
 *
 *  The frame is JSON from another process that ships and versions separately:
 *  an older or newer extension, a partial publish, a renamed field. Everything
 *  downstream reads `state.peers`, `state.sessions` and `state.log` without
 *  guarding, and an uncaught render throw takes the whole window down to a grey
 *  rectangle with no way to reload (see components/ErrorBoundary). The boundary
 *  is the backstop; this is the part that keeps skew from reaching it at all. */
function normalizeState(msg: Record<string, unknown>): ExtState {
  const arr = <T,>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
  return {
    ...(msg as unknown as ExtState),
    peers: arr(msg.peers),
    sessions: arr(msg.sessions),
    log: arr(msg.log),
  };
}

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
  const fail = failures.get(port);
  if (fail && fail.count >= BACKOFF_AFTER && Date.now() < fail.nextAt) return;
  let ws: WebSocket;
  try {
    ws = new WebSocket(`ws://127.0.0.1:${port}`);
  } catch {
    noteFailure(port);
    return;
  }
  sockets.set(port, ws);

  ws.onopen = () => {
    failures.delete(port);
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
      inst.state = normalizeState(msg);
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
    // Only a port that never answered counts toward the backoff; an instance
    // that was live and went away deserves the fast cadence back.
    if (live.delete(port)) {
      failures.delete(port);
      ensureActive();
      emit();
    } else {
      noteFailure(port);
    }
  };
  ws.onerror = () => ws.close();
}

function noteFailure(port: number) {
  const count = (failures.get(port)?.count ?? 0) + 1;
  failures.set(port, { count, nextAt: Date.now() + BACKOFF_MS });
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
