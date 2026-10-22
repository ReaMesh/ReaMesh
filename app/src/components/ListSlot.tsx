import type { ReactNode } from "react";
import { motion, useReducedMotion } from "motion/react";

// One entry's place in a list that changes while you look at it: the session
// roster, the Lobby's sessions, blocked devices. Something arriving opens its
// row and comes into focus out of a slight blur; something leaving goes the
// same way back. Use inside an AnimatePresence, keyed by the entry.
export const LIST_SPRING = { type: "spring", stiffness: 360, damping: 32, mass: 0.6 } as const;

export function ListSlot({ children }: { children: ReactNode }) {
  const reduce = useReducedMotion();
  const away = reduce ? { opacity: 0 } : { height: 0, opacity: 0, filter: "blur(6px)" };
  return (
    <motion.div
      layout="position"
      initial={away}
      animate={reduce ? { opacity: 1 } : { height: "auto", opacity: 1, filter: "blur(0px)" }}
      exit={away}
      transition={reduce ? { duration: 0.15 } : LIST_SPRING}
      className="overflow-hidden"
    >
      {children}
    </motion.div>
  );
}
