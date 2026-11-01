// A `reamesh://relay/…` code the OS handed the app: clicked where the relay
// printed it (terminals with links), in a chat or in an email. It waits here
// until the main window has asked whether to add it; nothing is stored or
// dialed before the user says yes.
import { useSyncExternalStore } from "react";

let pending = "";
const listeners = new Set<() => void>();

/** Park a freshly received code. A newer one replaces an unanswered one. */
export function offerRelayLink(code: string) {
  pending = code;
  listeners.forEach((l) => l());
}

export function clearRelayLink() {
  offerRelayLink("");
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** The parked code, "" when there is none. */
export function useRelayLink(): string {
  return useSyncExternalStore(subscribe, () => pending);
}
