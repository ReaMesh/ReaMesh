// Shared slot-text wrapper with the reduced-motion fallback missing upstream.

import { useEffect, useRef } from "react";
import { useReducedMotion } from "motion/react";
import { SlotText } from "slot-text/react";

export function Rolling({
  text,
  direction = "up",
  className,
}: {
  text: string;
  direction?: "up" | "down";
  className?: string;
}) {
  const reduce = useReducedMotion();
  if (reduce) return <span className={className}>{text}</span>;
  return (
    <SlotText text={text} options={{ direction, duration: 260 }} className={className} />
  );
}

/** A peer's current track number, right-aligned and rolling *toward* the
 *  change: moving to a higher track rolls up, to a lower one rolls down.
 *  Shared by the Session peer list and the overlay HUD roster. */
export function TrackNumber({ track }: { track?: number | null }) {
  const cur = track != null && track > 0 ? track : null;
  // Previous value updated in an effect (not during render) so StrictMode's
  // double render can't swallow the direction of the very change it belongs to.
  const prevRef = useRef<number | null>(null);
  const prev = prevRef.current;
  useEffect(() => {
    prevRef.current = cur;
  });
  if (cur == null) return <span className="w-4" />;
  return (
    <Rolling
      text={String(cur)}
      direction={prev != null && cur < prev ? "down" : "up"}
      className="w-4 shrink-0 text-right text-[12px] tabular-nums text-muted-foreground"
    />
  );
}
