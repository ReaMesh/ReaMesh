import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Bridges the restart after an update so it reads as a short reload: the
// window fades out, the screen the server doesn't know about (Settings, the
// acknowledgements) is remembered, and the new process starts hidden, puts
// that screen back and only shows its window once something is painted.

const KEY = "reamesh.resume";
/** Older than this, a leftover note is from some other exit: ignore it. */
const MAX_AGE_MS = 60_000;

export type ResumeScreen = "settings" | "credits" | null;

let current: () => ResumeScreen = () => null;

/** App.tsx registers which local screen is open, for the note. */
export function trackScreen(get: () => ResumeScreen) {
  current = get;
}

/** The note left by the process the update replaced (read once, then gone). */
export function takeResume(): { screen: ResumeScreen } | null {
  try {
    const raw = localStorage.getItem(KEY);
    localStorage.removeItem(KEY);
    if (!raw) return null;
    const note = JSON.parse(raw) as { at: number; screen: ResumeScreen };
    if (Date.now() - note.at > MAX_AGE_MS) return null;
    return { screen: note.screen ?? null };
  } catch {
    return null;
  }
}

/** Fade the whole window to its background colour, or back in. */
function setFaded(faded: boolean) {
  const root = document.getElementById("root");
  if (!root) return;
  root.style.transition = "opacity 160ms ease-out";
  root.style.opacity = faded ? "0" : "1";
}

export function listenForRestart() {
  listen("update:restarting", () => {
    try {
      localStorage.setItem(KEY, JSON.stringify({ at: Date.now(), screen: current() }));
    } catch {
      // Storage unavailable: the restart still works, it just lands on the default screen.
    }
    setFaded(true);
  }).catch(() => {});
  // Installing failed after all: come back.
  listen("update:restart-failed", () => setFaded(false)).catch(() => {});
}

/** Show the (initially hidden) main window. A resumed launch starts faded and
 *  fades in, so the restart reads as one continuous reload. Timers, not
 *  requestAnimationFrame: a webview whose window is still hidden may never
 *  run animation frames. */
export function revealWindow(resumed: boolean) {
  if (resumed) {
    const root = document.getElementById("root");
    if (root) {
      root.style.transition = "none";
      root.style.opacity = "0";
    }
  }
  invoke("show_main_window").catch(() => {});
  if (resumed) setTimeout(() => setFaded(false), 40);
}
