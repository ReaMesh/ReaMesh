// Shared Radix tooltips with a hover delay and an optional collision boundary.
// The overlay window fits its card exactly, so its tooltips must stay inside
// that card to avoid being clipped by the native window.

import { createContext, useContext, type ReactElement, type ReactNode, type RefObject } from "react";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

const BoundaryCtx = createContext<RefObject<HTMLElement | null> | null>(null);

export function HintProvider({
  boundary,
  delay = 120,
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

/** Wraps one element and shows `label` on hover / focus. The child must accept
 *  a ref and DOM props (a plain span/button or a motion element). An empty
 *  label renders the child alone. */
export function Hint({
  label,
  side = "bottom",
  className,
  children,
}: {
  label?: string;
  side?: "top" | "bottom" | "left" | "right";
  className?: string;
  children: ReactElement;
}) {
  const boundary = useContext(BoundaryCtx);
  if (!label) return children;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent
        side={side}
        sideOffset={6}
        collisionBoundary={boundary?.current ?? undefined}
        collisionPadding={6}
        className={cn("pointer-events-none px-2 py-1 text-[11px] font-medium", className)}
      >
        {label}
      </TooltipContent>
    </Tooltip>
  );
}
