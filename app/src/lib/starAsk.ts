// The Lobby's one-time "a star on GitHub helps" line. It shows once this
// machine has had a few real sessions (other people in them, long enough to
// have been work), and never again once it was clicked or dismissed. Kept in
// localStorage, which survives app updates; the Settings row is always there.

import { useSyncExternalStore } from "react";
import type { ExtState } from "./types";

const KEY = "reamesh.starAsk";

/** Sessions it takes before the Lobby asks. */
export const STAR_ASK_AFTER = 3;
/** Time with at least one other person that makes a session count. */
export const STAR_SESSION_MS = 10 * 60_000;
/** A session not seen for this long is not the one still running. */
const STALE_MS = 2 * 60_000;
/** How often the running session's "still here" time is written down. */
const SEEN_EVERY_MS = 30_000;

export interface StarAsk {
  /** Sessions that counted so far. */
  sessions: number;
  /** Clicked or dismissed: never ask again. */
  done: boolean;
  /** The session being timed: since when others were in it, last time seen. */
  current?: { key: string; since: number; seen: number; counted: boolean } | null;
}

const EMPTY: StarAsk = { sessions: 0, done: false, current: null };

function read(): StarAsk {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return EMPTY;
    const v = JSON.parse(raw);
    if (!v || typeof v !== "object") return EMPTY;
    return {
      sessions: typeof v.sessions === "number" && v.sessions > 0 ? v.sessions : 0,
      done: v.done === true,
      current:
        v.current && typeof v.current.key === "string" && typeof v.current.since === "number"
          ? {
              key: v.current.key,
              since: v.current.since,
              seen: typeof v.current.seen === "number" ? v.current.seen : v.current.since,
              counted: v.current.counted === true,
            }
          : null,
    };
  } catch {
    return EMPTY;
  }
}

let current = read();
const listeners = new Set<() => void>();

function write(next: StarAsk) {
  current = next;
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Storage blocked: the line simply never shows.
  }
  listeners.forEach((l) => l());
}

// The Settings window marks it done too: follow.
if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== KEY) return;
    current = read();
    listeners.forEach((l) => l());
  });
}

/** The record after one state frame of the instance on `port`. Pure. */
export function starAskAfter(rec: StarAsk, state: ExtState, port: number, now: number): StarAsk {
  if (rec.done) return rec.current ? { ...rec, current: null } : rec;
  const inSession = state.mode === "hosting" || state.mode === "client";
  const others = inSession && state.peers.some((p) => !p.is_self);
  if (!others) {
    // Out of the session: forget it. Alone in it: keep the clock, it may
    // still count once someone is back.
    return !inSession && rec.current ? { ...rec, current: null } : rec;
  }
  const key = `${port}:${state.session}`;
  const cur = rec.current;
  if (!cur || cur.key !== key || now - cur.seen > STALE_MS) {
    return { ...rec, current: { key, since: now, seen: now, counted: false } };
  }
  if (!cur.counted && now - cur.since >= STAR_SESSION_MS) {
    return { ...rec, sessions: rec.sessions + 1, current: { ...cur, seen: now, counted: true } };
  }
  if (now - cur.seen >= SEEN_EVERY_MS) return { ...rec, current: { ...cur, seen: now } };
  return rec;
}

/** Feed one state frame (main window only). */
export function noteStarAskFrame(state: ExtState, port: number, now = Date.now()) {
  const next = starAskAfter(current, state, port, now);
  if (next !== current) write(next);
}

/** The Lobby line is due. */
export function starAskDue(rec: StarAsk = current): boolean {
  return !rec.done && rec.sessions >= STAR_ASK_AFTER;
}

/** Clicked or dismissed, here or through the Settings row: never ask again. */
export function finishStarAsk() {
  if (!current.done) write({ ...current, done: true, current: null });
}

export function useStarAsk(): StarAsk {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}

export const REPO_URL = "https://github.com/ReaMesh/ReaMesh";
