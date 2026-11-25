// The Setup window's side of its backend (../../../setup/src/main.rs).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Mode = "install" | "uninstall";
export type Relation = "fresh" | "update" | "same" | "downgrade";
export type Phase = "verify" | "close" | "copy" | "register" | "remove" | "done";

export type Info = {
  mode: Mode;
  version: string | null;
  installedVersion: string | null;
  relation: Relation;
  dir: string;
  defaultDir: string;
  appRunning: boolean;
  passive: boolean;
  update: boolean;
  needBytes: number;
  freeBytes: number | null;
  extensionInstalled: boolean;
  dataExists: boolean;
};

export type Done = { ok: boolean; error: string | null; restartReaper: boolean };

export const setupInfo = () => invoke<Info>("setup_info");
export const chooseDir = (path: string) => invoke<string>("choose_dir", { path });
export const startInstall = (desktop: boolean) => invoke("start_install", { args: { desktop } });
export const startUninstall = (removeExtension: boolean, removeData: boolean) =>
  invoke("start_uninstall", { args: { removeExtension, removeData } });
export const launchApp = () => invoke("launch_app");
export const showWindow = () => invoke("show_window").catch(() => {});
export const quit = () => invoke("quit").catch(() => {});

export function onProgress(cb: (phase: Phase, value: number) => void) {
  return listen<{ phase: Phase; value: number }>("setup:progress", (e) =>
    cb(e.payload.phase, e.payload.value),
  );
}

export function onDone(cb: (d: Done) => void) {
  return listen<Done>("setup:done", (e) => cb(e.payload));
}

/** How far along the whole job is, from one phase's own progress. Copying is
 *  nearly all of an install; the steps around it are quick. */
export function overall(mode: Mode, phase: Phase, value: number): number {
  const spans: Record<Mode, Partial<Record<Phase, [number, number]>>> = {
    install: { verify: [0, 0.06], close: [0.06, 0.1], copy: [0.1, 0.92], register: [0.92, 1], done: [1, 1] },
    uninstall: { close: [0, 0.15], remove: [0.15, 1], done: [1, 1] },
  };
  const [a, b] = spans[mode][phase] ?? [0, 0];
  return a + (b - a) * Math.min(1, Math.max(0, value));
}
