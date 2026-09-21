// Local profile: a display name and identity color, persisted on this
// machine only (no accounts). Sent to every REAPER instance on connect and
// whenever it changes; the extension gossips it to session peers as a
// presence beacon.

import { useSyncExternalStore } from "react";

export interface Profile {
  name: string;
  color: string;
}

/** Identity palette for the dark theme. */
export const PALETTE = [
  "#f06464", // coral
  "#f0a05a", // amber
  "#e8d35c", // gold
  "#7ed27e", // green
  "#5cc8c8", // teal
  "#6aa6f0", // blue
  "#9d8cf0", // violet
  "#e87ec8", // pink
];

const KEY = "reamesh.profile";

export function getProfile(): Profile {
  if (current) return current;
  return (current = readProfile());
}

/** Subscribe a component to profile edits made anywhere in the window. */
export function useProfile(): Profile {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    getProfile,
  );
}

function readProfile(): Profile {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const p = JSON.parse(raw);
      if (typeof p.name === "string" && typeof p.color === "string") return p;
    }
  } catch {
    // fall through to default
  }
  // Persist the initial random color so reconnects and other REAPER instances
  // use the same identity color.
  const p: Profile = {
    name: "",
    color: PALETTE[Math.floor(Math.random() * PALETTE.length)],
  };
  try {
    saveProfile(p);
  } catch {
    // localStorage unavailable: at least this call is self-consistent
  }
  return p;
}

// One in-memory copy so the title-bar chip and the first-run card stay in step.
let current: Profile | null = null;
const listeners = new Set<() => void>();

export function saveProfile(p: Profile) {
  current = p;
  listeners.forEach((l) => l());
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    // storage unavailable/full: the in-memory profile still reaches the extension
  }
}
