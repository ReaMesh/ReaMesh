import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// The in-app updater, mirrored from the backend (src-tauri/src/updater.rs),
// which owns it so the native macOS settings window shows the same state. A
// newer build downloads in the background; nothing installs until the user
// clicks restart. Outside Tauri (plain-browser dev) every call fails quietly
// and the state stays idle.

export type UpdatePhase = "idle" | "checking" | "current" | "downloading" | "ready" | "failed";

export interface UpdateState {
  phase: UpdatePhase;
  /** The version on offer ("" until a check found one). */
  version: string;
  /** 0..1 while downloading (0 when the size is unknown). */
  progress: number;
  /** Restart may install it now (not mid-session with an incompatible build). */
  can_restart: boolean;
}

/** Beta gets every release, Stable only the ones marked stable. */
export type UpdateChannel = "beta" | "stable";

export interface ChannelInfo {
  channel: UpdateChannel;
  /** False until the first stable release exists: Stable shows, greyed out. */
  stable_available: boolean;
}

let state: UpdateState = { phase: "idle", version: "", progress: 0, can_restart: false };
let auto: boolean | undefined;
let channel: ChannelInfo = { channel: "stable", stable_available: true };
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());
let started = false;

function start() {
  if (started) return;
  started = true;
  invoke<UpdateState>("update_state")
    .then((s) => {
      if (!s) return;
      state = s;
      emit();
    })
    .catch(() => {});
  invoke<boolean>("get_update_auto")
    .then((v) => {
      if (typeof v !== "boolean") return;
      auto = v;
      emit();
    })
    .catch(() => {});
  invoke<ChannelInfo>("get_update_channel")
    .then((c) => {
      if (!c) return;
      channel = c;
      emit();
    })
    .catch(() => {});
  listen<UpdateChannel>("update-channel-changed", (e) => {
    channel = { ...channel, channel: e.payload };
    emit();
  }).catch(() => {});
  listen<UpdateState>("update-state", (e) => {
    state = e.payload;
    emit();
  }).catch(() => {});
  listen<boolean>("update-auto-changed", (e) => {
    auto = e.payload;
    emit();
  }).catch(() => {});
}

function subscribe(cb: () => void) {
  start();
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function getUpdate(): UpdateState {
  return state;
}

export function useUpdate(): UpdateState {
  return useSyncExternalStore(subscribe, () => state);
}

/** Whether checks run on their own. `undefined` until the backend answers. */
export function useUpdateAuto(): boolean | undefined {
  return useSyncExternalStore(subscribe, () => auto);
}

export function getUpdateChannel(): ChannelInfo {
  return channel;
}

export function useUpdateChannel(): ChannelInfo {
  return useSyncExternalStore(subscribe, () => channel);
}

/** How the app names a channel, the same in every language. */
export function channelLabel(c: UpdateChannel): string {
  return c === "stable" ? "Stable" : "Beta";
}

export async function setUpdateChannel(next: UpdateChannel) {
  if (next === "stable" && !channel.stable_available) return;
  channel = { ...channel, channel: next };
  emit();
  await invoke("set_update_channel", { channel: next }).catch(() => {});
}

export function checkForUpdate() {
  invoke("update_check").catch(() => {});
}

export async function setUpdateAuto(enabled: boolean) {
  auto = enabled;
  emit();
  await invoke("set_update_auto", { enabled }).catch(() => {});
}

/** Install the download and relaunch. The backend first sends
 *  `update:restarting` (see lib/resume.ts), so the window fades out and comes
 *  back on the same screen. */
export async function restartToUpdate() {
  await invoke("update_restart");
}

let appVersion = "";
const versionReady = import("@tauri-apps/api/app")
  .then((m) => m.getVersion())
  .then((v) => {
    appVersion = v;
    emit();
  })
  .catch(() => {});
void versionReady;

/** This app's version ("" until known, and in plain-browser dev). */
export function useAppVersion(): string {
  return useSyncExternalStore(subscribe, () => appVersion);
}
