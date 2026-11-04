// "Save as" for a downloaded copy of a joined session. The backend copies on a
// thread of its own and reports through events (src-tauri/src/copies.rs); this
// keeps the one running copy for the buttons that show it and says how it ended.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { t } from "./i18n";
import { isWindows } from "./platform";

export interface RunningCopy {
  /** Folder name inside the staging root. */
  dir: string;
  done: number;
  total: number;
}

interface CopyFinished {
  dir: string;
  path: string | null;
  error: string | null;
  trashed: boolean;
  kept: string | null;
}

let running: RunningCopy | null = null;
const listeners = new Set<() => void>();
// Set in the window that asked for this copy: only that one reports the end.
let asked = false;

function setRunning(next: RunningCopy | null) {
  running = next;
  listeners.forEach((l) => l());
}

const lastPart = (path: string) => path.split(/[/\\]/).filter(Boolean).pop() ?? path;

/** The folder name a session's folder goes by in the backend's commands. */
export const copyDirName = lastPart;

function reveal(path: string) {
  return {
    label: t(isWindows ? "showInExplorer" : "showInFinder"),
    onClick: () => void revealItemInDir(path).catch(() => {}),
  };
}

function report(end: CopyFinished) {
  if (end.path) {
    toast.success(t("copySaved", { name: lastPart(end.path) }), { action: reveal(end.path) });
    return;
  }
  const why =
    end.error === "no_space"
      ? t("copyNoSpace")
      : end.error === "inside_staging"
        ? t("copyInsideStaging")
        : t("copyFailed", { reason: end.error ?? "" });
  // A failed copy is never left looking finished: say where its remains went.
  // The OS's own words may come without a full stop.
  const then = `${why.replace(/\.?$/, ".")} `;
  if (end.kept) {
    toast.error(then + t("copyPartialKept"), { action: reveal(end.kept) });
  } else {
    toast.error(end.trashed ? then + t(isWindows ? "copyPartialTrashedWin" : "copyPartialTrashed") : why);
  }
}

let wired = false;
function wire() {
  if (wired) return;
  wired = true;
  // Rejects outside Tauri (plain-browser dev), where nothing copies anyway.
  listen<RunningCopy>("copy-progress", (e) => setRunning(e.payload)).catch(() => {});
  listen<CopyFinished>("copy-finished", (e) => {
    setRunning(null);
    if (asked) report(e.payload);
    asked = false;
  }).catch(() => {});
}

/** Ask for a folder and copy there. `dir` is the folder name in the staging root. */
export async function saveCopyAs(dir: string) {
  wire();
  if (running) return;
  const dest = await open({ directory: true, title: t("saveCopyAs") }).catch(() => null);
  if (typeof dest !== "string" || running) return;
  asked = true;
  setRunning({ dir, done: 0, total: 0 });
  try {
    await invoke("save_joined_copy", { dir, dest });
  } catch (e) {
    asked = false;
    setRunning(null);
    toast.error(t("copyFailed", { reason: String(e) }));
  }
}

/** The running copy, in any window. */
export function useRunningCopy(): RunningCopy | null {
  wire();
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => running,
  );
}

/** Whole percent of a running copy. Files can grow while being read. */
export function copyPercent(copy: RunningCopy): number {
  return copy.total > 0 ? Math.min(100, Math.floor((copy.done / copy.total) * 100)) : 0;
}
