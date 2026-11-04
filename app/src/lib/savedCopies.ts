// "Save as" for a downloaded copy of a joined session. The backend copies on a
// thread of its own and reports through events (src-tauri/src/copies.rs); this
// keeps the one running copy for the buttons that show it and says how it ended.
// A finished copy is a linked one from then on: the backend's list (links.rs) is
// the one record of those, and this mirrors it for the screens that show them.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import type { LinkedCopy } from "./copyLink";
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
  linked: boolean;
}

let running: RunningCopy | null = null;
let linkedCopies: LinkedCopy[] | null = null;
// Which linked folder a staging folder was last saved to, in this run.
const savedAs = new Map<string, string>();
const listeners = new Set<() => void>();
// Set in the window that asked for this copy: only that one reports the end.
let asked = false;

const notify = () => listeners.forEach((l) => l());
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
};

function setRunning(next: RunningCopy | null) {
  running = next;
  notify();
}

const lastPart = (path: string) => path.split(/[/\\]/).filter(Boolean).pop() ?? path;

/** A folder's own name, for the rows that show one. */
export const folderName = lastPart;

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
    const { dir, path, linked } = e.payload;
    if (path && linked) savedAs.set(dir, path);
    setRunning(null);
    if (asked) report(e.payload);
    asked = false;
  }).catch(() => {});
  listen("linked-projects-changed", () => void refreshLinkedCopies()).catch(() => {});
}

/** Read the backend's list anew. Rejects nowhere: no backend means no links. */
export async function refreshLinkedCopies(): Promise<LinkedCopy[]> {
  wire();
  const list = await invoke<LinkedCopy[] | undefined>("linked_projects").catch(() => undefined);
  linkedCopies = list ?? [];
  notify();
  return linkedCopies;
}

/** Take a folder off the list; nothing on the disk changes. */
export async function unlinkCopy(path: string) {
  await invoke("unlink_project", { path });
  await refreshLinkedCopies();
}

/** The linked copies, null until the first answer. */
export function useLinkedCopies(): LinkedCopy[] | null {
  wire();
  return useSyncExternalStore(subscribe, () => linkedCopies);
}

/** Where this staging folder was saved to as a linked copy, if it was. */
export function useSavedAs(dir: string): string | null {
  wire();
  return useSyncExternalStore(subscribe, () => savedAs.get(dir) ?? null);
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
  return useSyncExternalStore(subscribe, () => running);
}

/** Whole percent of a running copy. Files can grow while being read. */
export function copyPercent(copy: RunningCopy): number {
  return copy.total > 0 ? Math.min(100, Math.floor((copy.done / copy.total) * 100)) : 0;
}
