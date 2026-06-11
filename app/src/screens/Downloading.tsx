import { motion } from "motion/react";
import { Button } from "@/components/ui/button";
import { send } from "@/lib/connection";
import type { ExtState } from "@/lib/types";

// Project transfer in progress. The bar is spring-animated toward the real
// progress the extension reports, so it glides instead of stepping.
export function Downloading({ state }: { state: ExtState }) {
  const pct = Math.round((state.progress || 0) * 100);

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-8 px-10">
      <div className="w-full space-y-3 text-center">
        <p className="text-[13px] text-muted-foreground">Joining</p>
        <h2 className="truncate text-lg font-semibold tracking-tight">
          {state.session || "session"}
        </h2>
      </div>
      <div className="w-full space-y-2">
        <div className="h-1.5 w-full overflow-hidden rounded-full bg-secondary">
          <motion.div
            className="h-full rounded-full bg-foreground"
            initial={{ width: 0 }}
            animate={{ width: `${pct}%` }}
            transition={{ type: "spring", stiffness: 80, damping: 20 }}
          />
        </div>
        <div className="flex justify-between text-[12px] text-muted-foreground">
          <span>Downloading the project…</span>
          <span className="tabular-nums">{pct}%</span>
        </div>
      </div>
      <Button
        variant="ghost"
        size="sm"
        className="text-muted-foreground"
        onClick={() => send({ cmd: "leave" })}
      >
        Cancel
      </Button>
    </div>
  );
}
