import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

// Extension-install plumbing: the Tauri backend checks REAPER's resource
// folder and copies the bundled reaper_reamesh.dylib into UserPlugins.
// The custom REAPER folder (portable installs) is persisted backend-side so
// the native macOS settings window and this webview share one source of truth;
// we cache it here for synchronous reads in render.

export interface ReaperStatus {
  reaperFound: boolean;
  isReaperDir: boolean;
  resourcePath: string;
  userPlugins: string;
  installed: boolean;
  upToDate: boolean;
}

let cachedPath: string | null = null;

// Loads the persisted override into the cache. Call once before the first
// refreshStatus, and again whenever the backend reports a change.
export async function loadReaperPath(): Promise<void> {
  try {
    cachedPath = (await invoke<string | null>("get_reaper_path")) ?? null;
  } catch {
    cachedPath = null;
  }
}

export function getReaperPath(): string | null {
  return cachedPath;
}

export async function setReaperPath(path: string | null): Promise<void> {
  cachedPath = path;
  try {
    await invoke("set_reaper_path", { path });
  } catch {
    /* ignore */
  }
}

// undefined = not checked yet; null = backend unavailable (plain-browser dev),
// which skips onboarding rather than blocking the app.
let status: ReaperStatus | null | undefined;
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

export async function refreshStatus(): Promise<ReaperStatus | null> {
  try {
    status = await invoke<ReaperStatus>("reaper_status");
  } catch {
    status = null;
  }
  emit();
  return status;
}

// Copies the bundled dylib into UserPlugins and returns the fresh status, but
// deliberately does NOT publish it into the shared store: during onboarding the
// App-level gate keys off `installed`, and flipping it here would unmount the
// Onboarding screen before its "All set" step could show. Callers refresh the
// shared status themselves when they're ready to move on (Onboarding on Finish,
// Settings right after, to refresh the badge).
export async function installExtension(): Promise<ReaperStatus> {
  return invoke<ReaperStatus>("install_extension");
}

/** Delete the installed extension (and any displaced copies), and turn
 *  auto-install off so the next launch doesn't quietly put it back. */
export async function removeExtension(): Promise<ReaperStatus> {
  return invoke<ReaperStatus>("remove_extension");
}

export function useReaperStatus(): ReaperStatus | null | undefined {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => status,
  );
}
