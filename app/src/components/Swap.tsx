// Trades one icon or label for another inside a control with a short blur
// crossfade, so a state change ("Copied") reads as the same control changing
// rather than as new content popping in.

import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion, type Variants } from "motion/react";

const EASE_OUT = [0.16, 1, 0.3, 1] as const;
// The arriving layer springs in; the leaving one is a little quicker so the
// two cross as one morph.
const SWAP = { type: "spring", stiffness: 460, damping: 30, mass: 0.55 } as const;
const LEAVE = { duration: 0.16, ease: EASE_OUT } as const;
const RESIZE = { type: "spring", stiffness: 360, damping: 32, mass: 0.6 } as const;

function variants(from: number, to: number, reduce: boolean | null): Variants {
  if (reduce) {
    return {
      initial: { opacity: 0 },
      animate: { opacity: 1, transition: { duration: 0.12 } },
      exit: { opacity: 0, transition: { duration: 0.12 } },
    };
  }
  return {
    initial: { opacity: 0, scale: from, filter: "blur(2px)" },
    animate: { opacity: 1, scale: 1, filter: "blur(0px)", transition: SWAP },
    exit: { opacity: 0, scale: to, filter: "blur(2px)", transition: LEAVE },
  };
}

/** An icon slot of a fixed size; `value` keys which icon is showing. */
export function SwapIcon({
  value,
  children,
  className,
}: {
  value: string;
  children: ReactNode;
  className?: string;
}) {
  const v = variants(0.25, 0.25, useReducedMotion());
  return (
    <span className={`relative inline-block shrink-0 ${className ?? ""}`}>
      <AnimatePresence initial={false}>
        <motion.span
          key={value}
          variants={v}
          initial="initial"
          animate="animate"
          exit="exit"
          className="absolute inset-0 grid place-items-center"
        >
          {children}
        </motion.span>
      </AnimatePresence>
    </span>
  );
}

/** A label slot whose width follows the label on the same kind of spring. */
export function SwapText({ value, children }: { value: string; children: ReactNode }) {
  const reduce = useReducedMotion();
  const inner = useRef<HTMLSpanElement>(null);
  const [width, setWidth] = useState<number | "auto">("auto");
  useLayoutEffect(() => {
    const el = inner.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.offsetWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const v = variants(0.9, 0.96, reduce);
  return (
    <motion.span
      className="inline-block whitespace-nowrap"
      // Clip at the slot's sides while it resizes, so a longer label never
      // spills past the control (or the card it sits at the edge of); the
      // blur may still bleed above and below.
      style={{ clipPath: "inset(-0.4em 0)" }}
      initial={false}
      animate={{ width }}
      transition={reduce ? { duration: 0 } : RESIZE}
    >
      {/* popLayout lifts the leaving label out of the flow, so this span
          measures only the arriving one. */}
      <span ref={inner} className="relative inline-block">
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span
            key={value}
            variants={v}
            initial="initial"
            animate="animate"
            exit="exit"
            className="inline-block"
          >
            {children}
          </motion.span>
        </AnimatePresence>
      </span>
    </motion.span>
  );
}
