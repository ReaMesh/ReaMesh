// Connection manager: keeps a WebSocket to every running REAPER instance's
// ReaMesh control server (ports 47415-47418, one per instance) and exposes
// them as a tiny external store for React.
//
// Session state stays in the extension; this module only transports it.
// Commands go out as JSON ({cmd:"host"|"join"|"leave"|"browse"|"set_profile"});
// the extension streams back one `state` message whenever anything changes.

import { useSyncExternalStore } from "react";
import { normalizeState, type Instance } from "./types";
import { getProfile } from "./profile";
import { getOverlaySettings, subscribeOverlaySettings } from "./overlaySettings";
import { openCtlSocket } from "./ctlSocket";
import { PROTO } from "./version";

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
/** Ports whose connection attempt is waiting for its token. */
const opening = new Set<number>();
const live = new Map<number, Instance>();
let activePort: number | null = null;
let snapshot: StoreState = { instances: [], activePort: null };
const listeners = new Set<() => void>();
let started = false;
/** Consecutive failed connects per port, and the wall-clock time we may next
 *  retry it. Reset the moment a port answers. */
const failures = new Map<number, { count: number; nextAt: number }>();

// `normalizeState` (version-skew hardening of the state frame) lives in
// lib/types.ts next to the frame's shape, where it is unit-testable without
// pulling this module's Tauri wiring into the test runner.

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
  if (sockets.has(port) || opening.has(port)) return;
  const fail = failures.get(port);
  if (fail && fail.count >= BACKOFF_AFTER && Date.now() < fail.nextAt) return;
  // Fetching the port's token is async; `opening` keeps the next scan from
  // starting a second attempt meanwhile.
  opening.add(port);
  openCtlSocket(port)
    .then((ws) => {
      opening.delete(port);
      attach(port, ws);
    })
    .catch(() => {
      opening.delete(port);
      noteFailure(port);
    });
}

function attach(port: number, ws: WebSocket) {
  sockets.set(port, ws);

  ws.onopen = () => {
    failures.delete(port);
    live.set(port, { port });
    ensureActive();
    sendProfile(port);
    sendTint(port);
    sendCursors(port);
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
      // Greeted without the run token (`auth: false`): the extension only
      // says which build it is. A build of another protocol stays, so the
      // version-mismatch screen can name it; one of ours is useless without
      // the token, so drop it and fetch the token again on the next scan.
      if (msg.auth === false && msg.proto === PROTO) {
        ws.close();
        return;
      }
      inst.pid = Number(msg.pid) || undefined;
      inst.version = typeof msg.version === "string" ? msg.version : undefined;
      inst.proto = typeof msg.proto === "number" ? msg.proto : undefined;
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
  // Lock marks and cursors are drawn by the extension, so push those toggles to
  // every connected instance whenever they change (the initial values ride
  // ws.onopen).
  subscribeOverlaySettings(() => {
    const { tint, cursors, shareCursor } = getOverlaySettings();
    if (tint !== lastTintSent) {
      lastTintSent = tint;
      for (const p of sockets.keys()) sendTint(p);
    }
    const c = `${cursors}:${shareCursor}`;
    if (c !== lastCursorsSent) {
      lastCursorsSent = c;
      for (const p of sockets.keys()) sendCursors(p);
    }
  });
}

let lastTintSent: boolean | null = null;
let lastCursorsSent: string | null = null;

/** Tell the extension whether to show and share mouse cursors in REAPER. */
export function sendCursors(port: number) {
  const { cursors, shareCursor } = getOverlaySettings();
  send({ cmd: "set_cursors", show: cursors, share: shareCursor }, port);
}

/** Tell the extension whether to mark items someone else is editing. */
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

export function getConnections(): StoreState {
  return snapshot;
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
