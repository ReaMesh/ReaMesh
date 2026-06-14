// Overlay settings are stored as a bitmask in the Tauri backend and shared
// by the native Settings window and webviews. Backend writes emit
// `overlay-settings-changed`; localStorage caches the value for first render
// and provides persistence in browser previews.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface OverlaySettings {
  /** The participant roster (avatars + names + track numbers). */
  roster: boolean;
  /**
   * The dark-grey tint painted on media items another peer is editing (their
   * editing lock is live). Rendered by the REAPER extension, so flipping this
   * is pushed to it over the control server (see connection.ts). Bit 2: reused
   * from the retired "activity feed" toggle, so existing installs (where the
   * activity bit was on by default) keep the tint on without a migration.
   */
  tint: boolean;
}

const KEY = "reamesh.overlay";
const EVENT = "reamesh:overlay-settings";
const DEFAULTS: OverlaySettings = {
  roster: true,
  tint: true,
};

// Bit layout: must match settings_window.h (kReaMeshOverlay*) and the Rust
// `OVERLAY_ALL` mask. Bits 4/8 (peer playheads / time-selection) are retired.
const BITS: Record<keyof OverlaySettings, number> = {
  roster: 1,
  tint: 2,
};

function maskToSettings(mask: number): OverlaySettings {
  return {
    roster: !!(mask & BITS.roster),
    tint: !!(mask & BITS.tint),
  };
}

function settingsToMask(s: OverlaySettings): number {
  let m = 0;
  for (const k of Object.keys(BITS) as (keyof OverlaySettings)[]) {
    if (s[k]) m |= BITS[k];
  }
  return m;
}

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

function persistLocal(s: OverlaySettings) {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // keep the in-memory value even if persistence fails
  }
}

const listeners = new Set<() => void>();

function set(next: OverlaySettings) {
  cache = next;
  persistLocal(cache);
  for (const l of listeners) l();
}

export function getOverlaySettings(): OverlaySettings {
  return cache;
}

export function setOverlaySetting(key: keyof OverlaySettings, value: boolean) {
  set({ ...cache, [key]: value });
  // Push to the backend (authoritative + reaches the other windows via the
  // change event). In plain-browser dev this throws and we lean on the
  // localStorage `storage` event below instead.
  void invoke("set_overlay_settings", { mask: settingsToMask(cache) }).catch(() => {
    // Notify other browser windows; set() already notified this window.
    window.dispatchEvent(new Event(EVENT));
  });
}

// Pull the authoritative value from the backend on load, and follow every
// later change (including ones made in the native Settings window).
void invoke<number>("get_overlay_settings")
  .then((mask) => set(maskToSettings(mask)))
  .catch(() => {
    /* plain-browser dev: no backend; localStorage already seeded the cache */
  });
void listen<number>("overlay-settings-changed", (e) => set(maskToSettings(e.payload)));

// Cross-window fallback for plain-browser dev (same-origin documents).
window.addEventListener("storage", (e) => {
  if (e.key === KEY) set(read());
});
window.addEventListener(EVENT, () => set(read()));

/**
 * Subscribe to overlay-settings changes outside React (e.g. the connection
 * manager, which forwards the tint toggle to the extension). Returns an
 * unsubscribe function.
 */
export function subscribeOverlaySettings(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

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
