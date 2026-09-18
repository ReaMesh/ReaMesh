import { useEffect, useRef } from "react";
import {
  animate,
  motion,
  useAnimationFrame,
  useMotionValue,
  useReducedMotion,
} from "motion/react";

// Indeterminate motion, in turns: the whole arc drifts round once per
// BASE_TURN_MS while, once per CYCLE_MS, its head runs SWEEP ahead and its tail
// then catches up, so the arc grows and shrinks as it travels.
const BASE_TURN_MS = 2000;
const CYCLE_MS = 1400;
const SWEEP = 0.68;
const MIN_ARC = 0.03;
const STILL_ARC = 0.75; // reduced motion: a resting open arc
const BLEND_MS = 400; // eases whatever the ring showed into the spin

const easeInOut = (x: number) => (x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2);

/** Shared progress ring. While spinning, the arc grows and shrinks as it turns.
 *  On transition to determinate progress, finish the rotation at 12 o'clock
 *  and animate the arc length to the progress value. */
export function ArcRing({
  size,
  radius,
  strokeWidth,
  spinning,
  progress = 0,
  color,
  track = true,
}: {
  size: number;
  radius: number;
  strokeWidth: number;
  spinning: boolean;
  progress?: number;
  color: string;
  track?: boolean;
}) {
  const reduce = useReducedMotion();
  const circ = 2 * Math.PI * radius;
  const c = size / 2;
  const rotate = useMotionValue(0);
  const dashoffset = useMotionValue(circ); // circ * (1 - visible share)
  const spin = useRef({ clock: 0, fromRotate: 0, fromArc: 0 });
  const moving = spinning && !reduce;

  useEffect(() => {
    if (!moving) return;
    spin.current = {
      clock: 0,
      fromRotate: rotate.get(),
      fromArc: 1 - dashoffset.get() / circ,
    };
  }, [moving, rotate, dashoffset, circ]);

  useAnimationFrame((_, delta) => {
    if (!moving) return;
    const s = spin.current;
    s.clock += delta;
    const cycles = s.clock / CYCLE_MS;
    const u = cycles % 1;
    const head = easeInOut(Math.min(u * 2, 1));
    const tail = easeInOut(Math.max(u * 2 - 1, 0));
    const turns = s.clock / BASE_TURN_MS + (Math.floor(cycles) + tail) * SWEEP;
    const arc = MIN_ARC + (head - tail) * SWEEP;
    const blend = Math.min(1, s.clock / BLEND_MS);
    rotate.set(s.fromRotate + turns * 360);
    dashoffset.set(circ * (1 - (s.fromArc + (arc - s.fromArc) * blend)));
  });

  useEffect(() => {
    if (moving) return;
    const settle = animate(rotate, Math.ceil(rotate.get() / 360) * 360, {
      duration: 0.5,
      ease: "easeOut",
    });
    return () => settle.stop();
  }, [moving, rotate]);

  const fill = spinning ? STILL_ARC : Math.min(1, Math.max(0, progress));
  useEffect(() => {
    if (moving) return;
    const glide = animate(dashoffset, circ * (1 - fill), {
      type: "spring",
      stiffness: 80,
      damping: 20,
    });
    return () => glide.stop();
  }, [moving, fill, dashoffset, circ]);

  return (
    <motion.div
      className="absolute inset-0"
      style={{ rotate }}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.35, ease: "easeOut" }}
    >
      <svg viewBox={`0 0 ${size} ${size}`} className="size-full -rotate-90">
        {track && (
          <motion.circle
            cx={c}
            cy={c}
            r={radius}
            fill="none"
            strokeWidth={strokeWidth}
            className="stroke-secondary"
            initial={{ opacity: 0 }}
            animate={{ opacity: spinning ? 0 : 1 }}
            transition={{ duration: 0.35, ease: "easeOut" }}
          />
        )}
        <motion.circle
          cx={c}
          cy={c}
          r={radius}
          fill="none"
          strokeWidth={strokeWidth}
          strokeLinecap="round"
          strokeDasharray={circ}
          style={{ strokeDashoffset: dashoffset }}
          initial={{ stroke: color }}
          animate={{ stroke: color }}
          transition={{ duration: 0.45, ease: "easeOut" }}
        />
      </svg>
    </motion.div>
  );
}
