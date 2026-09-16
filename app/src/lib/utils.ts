import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** Keyboard focus indicator for hand-written controls.
 *
 *  The global `outline-ring/50` from index.css is not enough on its own: it is
 *  a translucent ring halved again by the `/50`, which lands well under the 3:1
 *  contrast a focus indicator needs. `--ring` is opaque now, and every
 *  app-authored button carries this so tabbing through the app is actually
 *  visible. `focus-visible` (not `focus`) keeps it off mouse clicks. */
export const focusRing =
  "outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background";

/** Swallow the webview's native context menu (Back / Reload / Inspect…) on
 *  chrome that has nothing to offer a right-click, such as the rosters. */
export function noContextMenu(e: { preventDefault(): void }) {
  e.preventDefault();
}
