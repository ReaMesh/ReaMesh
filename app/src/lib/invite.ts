// A `reamesh://` invite link the OS handed the app: clicked in a chat, an
// email or a browser, delivered by the deep-link plugin either at launch or
// into the already-running instance. The Lobby's invite field takes it from
// here; while the app is elsewhere (in a session, REAPER closed) it waits
// until the Lobby is back on screen.
import { useSyncExternalStore } from "react";

let pending = "";
let seq = 0;
const listeners = new Set<() => void>();

/** Park a freshly received link for the Lobby. A newer link replaces an
 *  older one that was never used. */
export function offerInvite(link: string) {
  pending = link;
  seq++;
  listeners.forEach((l) => l());
}

/** Hand out the parked link (once) and clear it. */
export function takeInvite(): string {
  const link = pending;
  pending = "";
  return link;
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** Bumps every time a link arrives, so a mounted Lobby can react to it. */
export function useInviteSeq(): number {
  return useSyncExternalStore(subscribe, () => seq);
}
