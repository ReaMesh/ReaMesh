import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Whether the bundled REAPER extension is installed and kept up to date
// automatically (the default) or left to the manual install/update button.
//
// The source of truth is the Tauri backend (a tiny flag file next to the
// language/overlay prefs), so the in-app Settings screen and the native macOS
// settings window agree. The backend does the actual installing: on launch,
// after the REAPER folder changes, and the moment this flips on; here we only
// mirror the flag so the checkbox renders and follows changes made elsewhere.
// `undefined` until the backend answers (treat as the on-by-default value);
// invoking throws in plain-browser dev, where we just assume on.

let cache: boolean | undefined;
const listeners = new Set<() => void>();

function set(v: boolean) {
  cache = v;
  for (const l of listeners) l();
}

export function getAutoInstall(): boolean | undefined {
  return cache;
}

export async function setAutoInstall(enabled: boolean): Promise<void> {
  set(enabled);
  try {
    await invoke("set_auto_install", { enabled });
  } catch {
    /* plain-browser dev: no backend: keep the optimistic value */
  }
}

void invoke<boolean>("get_auto_install")
  .then((v) => set(v))
  .catch(() => set(true));
void listen<boolean>("auto-install-changed", (e) => set(e.payload));

/** Subscribe a component to the auto-install preference. */
export function useAutoInstall(): boolean | undefined {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => cache,
  );
}
