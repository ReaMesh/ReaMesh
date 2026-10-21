import { useState } from "react";
import { motion, useReducedMotion } from "motion/react";
import { cn, focusRing } from "@/lib/utils";

// A heavy, well-damped thumb: it travels with some weight but never wobbles.
const THUMB = { type: "spring", stiffness: 800, damping: 80, mass: 4 } as const;

/** An on/off switch in the app's accent colour. */
export function Switch({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  const reduce = useReducedMotion();
  // The thumb gives a little under the pointer while it is held down.
  const [pressed, setPressed] = useState(false);
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      onPointerDown={() => setPressed(true)}
      onPointerUp={() => setPressed(false)}
      onPointerLeave={() => setPressed(false)}
      className={cn(
        "relative h-[22px] w-[38px] shrink-0 rounded-full transition-colors disabled:pointer-events-none",
        checked ? "bg-found" : "bg-white/15",
        focusRing,
      )}
    >
      <motion.span
        className="absolute top-0.5 left-0.5 size-[18px] rounded-full bg-white"
        initial={false}
        animate={{ x: checked ? 16 : 0, scale: pressed && !reduce ? 0.9 : 1 }}
        transition={reduce ? { duration: 0 } : THUMB}
      />
    </button>
  );
}
