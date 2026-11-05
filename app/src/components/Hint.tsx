// The app's tooltips (no native `title`, which WebKit draws in the system's own
// style after a long wait): shared Radix tooltips with a hover delay and an
// optional collision boundary. The overlay window fits its card exactly, so its
// tooltips must stay inside that card to avoid being clipped by the native window.

import { createContext, useContext, type ReactElement, type ReactNode, type RefObject } from "react";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

const BoundaryCtx = createContext<RefObject<HTMLElement | null> | null>(null);

/** For a whole row: it stays out of the way while the pointer crosses a list. */
const SLOW_DELAY = 900;
/** The rosters answer at once: they are read while hovering, the rest of the app isn't. */
export const ROSTER_DELAY = 120;

export function HintProvider({
  boundary,
  delay = 500,
  children,
}: {
  /** Element the tooltips must stay inside (the overlay card). */
  boundary?: RefObject<HTMLElement | null>;
  delay?: number;
  children: ReactNode;
}) {
  return (
    <BoundaryCtx.Provider value={boundary ?? null}>
      {/* Close when the pointer leaves the trigger, including when it enters the tooltip. */}
      <TooltipProvider delayDuration={delay} skipDelayDuration={300} disableHoverableContent>
        {children}
      </TooltipProvider>
    </BoundaryCtx.Provider>
  );
}

/** Text that actually ran out of room, on the element or a truncating child. */
function isClipped(el: Element) {
  return [el, ...el.querySelectorAll(".truncate")].some((n) => n.scrollWidth > n.clientWidth);
}

/** Wraps one element and shows `label` on hover / keyboard focus. The child must
 *  accept a ref and DOM props (a plain span/button or a motion element). Without
 *  a label nothing shows, and the child stays mounted when the label comes and
 *  goes. Between nested hints the innermost one wins. */
export function Hint({
  label,
  side = "bottom",
  slow,
  overflowOnly,
  className,
  children,
}: {
  label?: string;
  side?: "top" | "bottom" | "left" | "right";
  /** Wait longer before showing, for a hint on a whole row. */
  slow?: boolean;
  /** Only when the text is cut off, for a label that repeats the text itself. */
  overflowOnly?: boolean;
  className?: string;
  children: ReactElement;
}) {
  const boundary = useContext(BoundaryCtx);
  const wanted = (el: Element) => !!label && (!overflowOnly || isClipped(el));
  return (
    <Tooltip delayDuration={slow ? SLOW_DELAY : undefined}>
      {/* A prevented event is one Radix ignores. Focus counts only from the
          keyboard: a dialog's autofocus or the window coming back to the front
          must not pop a tooltip up on its own. */}
      <TooltipTrigger
        asChild
        onPointerMove={(e) => {
          if (wanted(e.currentTarget)) e.stopPropagation();
          else e.preventDefault();
        }}
        onFocus={(e) => {
          if (!wanted(e.currentTarget) || !e.currentTarget.matches(":focus-visible")) e.preventDefault();
        }}
      >
        {children}
      </TooltipTrigger>
      {label && (
        <TooltipContent
          side={side}
          sideOffset={6}
          collisionBoundary={boundary?.current ?? undefined}
          collisionPadding={6}
          className={cn(
            "pointer-events-none max-w-[min(20rem,calc(100vw-1.5rem))] px-2 py-1 text-[11px] font-medium wrap-anywhere",
            className,
          )}
        >
          {label}
        </TooltipContent>
      )}
    </Tooltip>
  );
}
