// What the overlay shows, toggled from the HUD's gear and persisted in
// localStorage. Both overlay windows (the HUD and the full-window arrange
// overlay) share the same origin, so a change in one reaches the other through
// the browser `storage` event; the writing window is nudged by a custom event
// (storage only fires in *other* documents). The hook below is built on
// useSyncExternalStore so components re-render the moment a toggle flips.

import { useSyncExternalStore } from "react";

export interface OverlaySettings {
  /** The participant roster (avatars + names + track numbers). */
  roster: boolean;
  /** The one-line activity feed. */
  activity: boolean;
  /** Peer playheads drawn over the arrange view. */
  playheads: boolean;
  /** Peer selection "ghosts" (tinted item/track outlines). */
  ghosts: boolean;
}

const KEY = "reamesh.overlay";
const EVENT = "reamesh:overlay-settings";
const DEFAULTS: OverlaySettings = {
  roster: true,
  activity: true,
  playheads: true,
  ghosts: true,
};

let cache: OverlaySettings = read();

function read(): OverlaySettings {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) return { ...DEFAULTS, ...(JSON.parse(raw) as Partial<OverlaySettings>) };
  } catch {
    // localStorage unavailable / malformed: fall back to defaults
  }
  return { ...DEFAULTS };
}

export function getOverlaySettings(): OverlaySettings {
  return cache;
}

export function setOverlaySetting(key: keyof OverlaySettings, value: boolean) {
  cache = { ...cache, [key]: value };
  try {
    localStorage.setItem(KEY, JSON.stringify(cache));
  } catch {
    // keep the in-memory value even if persistence fails
  }
  // Notify this window (storage only fires in other documents) and let the
  // storage event carry it to the other overlay window.
  window.dispatchEvent(new Event(EVENT));
}

const listeners = new Set<() => void>();

function notify() {
  cache = read();
  for (const l of listeners) l();
}

// Cross-window: another overlay window wrote the key.
window.addEventListener("storage", (e) => {
  if (e.key === KEY) notify();
});
// Same-window: our own setOverlaySetting fired the custom event.
window.addEventListener(EVENT, notify);

/** Subscribe a component to overlay-settings changes. */
export function useOverlaySettings(): OverlaySettings {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => cache,
  );
}
