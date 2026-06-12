// Thin wrappers over the Tauri backend that opens/closes the floating presence
// overlay window. Both swallow errors so the plain-browser dev build (no Tauri
// backend) and any transient window race never surface to the user.

import { invoke } from "@tauri-apps/api/core";

export function openOverlay(port: number) {
  invoke("open_overlay", { port }).catch(() => {});
}

export function closeOverlay() {
  invoke("close_overlay").catch(() => {});
}
