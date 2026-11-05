// The Lobby's "last session" block: which session just ended, why, and where the
// guest's copy of it is. A module store, since the Lobby remounts on every screen
// change and the frame that says why is gone by the next one. One per REAPER
// instance. Cleared by a new session or the block's X.

import { useSyncExternalStore } from "react";
import type { ExtState, LogEntry } from "./types";

export interface LastSession {
  name: string;
  /** Why it ended; null = the guest left by themselves. */
  reason: LogEntry | null;
  /** The guest's copy (absolute folder); "" when there is none to offer. */
  dir: string;
  /** That folder is a linked copy of the user's, not a temporary one. */
  linked: boolean;
}

/** A way back that ended with the session's folder itself: the block shows why. */
const COPY_REFUSALS = ["local_copy_changed", "linked_copy_missing", "linked_copy_foreign"];
const RETRY_ENDINGS = new Set([
  "reconnect_failed",
  "host_lost",
  "linked_copy_busy",
  "linked_copy_unsaved",
  "linked_copy_open",
  ...COPY_REFUSALS,
]);

/** The core won't take that folder as it is: only the trust dialog can go on. */
export const isCopyRefusal = (reason: LogEntry | null) =>
  !!reason && COPY_REFUSALS.includes(reason.code);

/** What the extension is doing at first when the host vanishes: retrying, with
 *  no error to read the reason from. It shows when that fails some other way. */
const HOST_LOST: LogEntry = { code: "host_lost", params: {}, text: "Lost the connection to the host." };

/** The block's answer to one state frame. `before` is the previous frame of this
 *  instance, null when there is none to compare with. */
export function lastSessionAfter(
  before: ExtState | null,
  now: ExtState,
  block: LastSession | null,
): LastSession | null {
  if (now.mode === "hosting" || now.mode === "client") return null;
  const wasGuest = before?.mode === "client";
  if (wasGuest || before?.mode === "hosting") {
    const lost = now.reconnecting || now.mode === "downloading";
    const reason = now.error ?? (lost ? HOST_LOST : null);
    // A host stopping is no news; only a session that ended on its own is.
    if (!wasGuest && !reason) return null;
    // Still loading: that folder isn't a whole copy of anything yet.
    const whole = !before.loading && !before.placeholders;
    return {
      name: before.session,
      reason,
      dir: wasGuest && whole ? (before.session_dir ?? "") : "",
      linked: wasGuest && !!before.session_linked,
    };
  }
  // A join of something new. A rejoin comes straight from the retry.
  if (now.mode === "downloading" && before?.mode === "browsing" && !before.reconnecting) return null;
  // The retry gave up or was cancelled: same session, newer reason.
  if (block && now.error && RETRY_ENDINGS.has(now.error.code)) {
    return { ...block, reason: now.error };
  }
  return block;
}

const byPort = new Map<number, LastSession>();
const listeners = new Set<() => void>();

export function getLastSession(port: number): LastSession | null {
  return byPort.get(port) ?? null;
}

export function setLastSession(port: number, value: LastSession | null) {
  if (value) byPort.set(port, value);
  else byPort.delete(port);
  listeners.forEach((l) => l());
}

export function useLastSession(port: number | null): LastSession | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => (port == null ? null : getLastSession(port)),
  );
}
