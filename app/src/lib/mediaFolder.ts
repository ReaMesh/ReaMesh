// Where hosting puts media it copies into the project (files the project uses
// from outside its folder). The extension does the copying; this is the
// user's choice, kept in localStorage (shared by every window, Settings on
// macOS included) and sent to each extension on connect and on change.

import { useSyncExternalStore } from "react";

export type MediaFolderMode = "reaper" | "beside" | "custom";

export interface MediaFolder {
  /** reaper: the project's own media path in REAPER, else `DEFAULT_MEDIA_FOLDER`;
   *  beside: next to the project file; custom: `folder`, inside the project. */
  mode: MediaFolderMode;
  /** For `custom`: a folder relative to the project. Empty or invalid falls
   *  back to `DEFAULT_MEDIA_FOLDER`. */
  folder: string;
}

export const DEFAULT_MEDIA_FOLDER = "Media";

const KEY = "reamesh.mediaFolder";
const DEFAULTS: MediaFolder = { mode: "reaper", folder: "" };
const MODES: MediaFolderMode[] = ["reaper", "beside", "custom"];

function read(): MediaFolder {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<MediaFolder> | null;
    if (!v || typeof v !== "object") return DEFAULTS;
    return {
      mode: MODES.includes(v.mode as MediaFolderMode) ? (v.mode as MediaFolderMode) : "reaper",
      folder: typeof v.folder === "string" ? v.folder : "",
    };
  } catch {
    return DEFAULTS;
  }
}

let current = read();
const listeners = new Set<() => void>();

function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

// The other window changed it: follow, so this window's screen and its next
// connect agree with what was chosen there.
if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== KEY) return;
    current = read();
    listeners.forEach((l) => l());
  });
}

export function subscribeMediaFolder(cb: () => void): () => void {
  return subscribe(cb);
}

/** For sending: as stored, in case the other window changed it last. */
export function getMediaFolder(): MediaFolder {
  return read();
}

export function useMediaFolder(): MediaFolder {
  return useSyncExternalStore(subscribe, () => current);
}

export function setMediaFolder(patch: Partial<MediaFolder>) {
  current = { ...current, ...patch };
  try {
    localStorage.setItem(KEY, JSON.stringify(current));
  } catch {
    // Private window or full storage: the choice lasts until the app closes.
  }
  listeners.forEach((l) => l());
}

/** A folder relative to the project, '/'-joined, or "" when `s` isn't one.
 *  Mirrors `cleanRelativeFolder` in the extension, which has the last word. */
export function cleanRelativeFolder(s: string): string {
  if (!s || s.length > 240 || s[0] === "/" || s[0] === "\\") return "";
  const parts: string[] = [];
  for (const part of s.split(/[/\\]/)) {
    if (part === "") continue;
    if (part === "." || part === ".." || part.endsWith(".") || part.endsWith(" ")) return "";
    // Control characters and what Windows can't put in a name.
    // eslint-disable-next-line no-control-regex
    if (/[\u0000-\u001f:*?"<>|]/.test(part)) return "";
    parts.push(part);
  }
  return parts.join("/");
}
