import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

// Extension-install plumbing: the Tauri backend checks REAPER's resource
// folder and copies the bundled reaper_reamesh.dylib into UserPlugins.
// A custom REAPER folder (portable installs) persists in localStorage and
// is passed along with every command.

export interface ReaperStatus {
  reaperFound: boolean;
  isReaperDir: boolean;
  resourcePath: string;
  userPlugins: string;
  installed: boolean;
  upToDate: boolean;
}

const PATH_KEY = "reamesh.reaperPath";

export function getReaperPath(): string | null {
  try {
    return localStorage.getItem(PATH_KEY);
  } catch {
    return null;
  }
}

export function setReaperPath(path: string | null) {
  try {
    if (path) localStorage.setItem(PATH_KEY, path);
    else localStorage.removeItem(PATH_KEY);
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
    status = await invoke<ReaperStatus>("reaper_status", {
      customPath: getReaperPath(),
    });
  } catch {
    status = null;
  }
  emit();
  return status;
}

export async function installExtension(): Promise<ReaperStatus> {
  const s = await invoke<ReaperStatus>("install_extension", {
    customPath: getReaperPath(),
  });
  status = s;
  emit();
  return s;
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
