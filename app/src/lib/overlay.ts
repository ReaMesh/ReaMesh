// Thin wrappers over the Tauri backend that opens/closes the floating presence
// overlay window. Both swallow errors so the plain-browser dev build (no Tauri
// backend) and any transient window race never surface to the user.

import { invoke } from "@tauri-apps/api/core";

/** Open (or re-target) the presence HUD for one REAPER instance.
 *
 *  `port` selects the roster source. `pid` selects the matching REAPER window
 *  when multiple instances are open. */
export function openOverlay(port: number, pid: number | null) {
  invoke("open_overlay", { port, pid }).catch(() => {});
}

export function closeOverlay() {
  invoke("close_overlay").catch(() => {});
}

/** Tell the backend whether we're hosting and on which control port, so the
 *  quit handler can leave/stop the session (and confirm, if hosting) before the
 *  app exits. */
export function setSessionState(hosting: boolean, port: number | null) {
  invoke("set_session_state", { hosting, port }).catch(() => {});
}
