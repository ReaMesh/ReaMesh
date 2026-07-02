import { useEffect, useRef, useState } from "react";
import { motion } from "motion/react";
import { Button } from "@/components/ui/button";
import { send } from "@/lib/connection";
import { resolveLang, t } from "@/lib/i18n";
import type { ExtState } from "@/lib/types";

function formatBytes(n: number, lang: string): string {
  const fmt = (v: number, unit: string, digits: number) =>
    `${v.toLocaleString(lang, {
      minimumFractionDigits: digits,
      maximumFractionDigits: digits,
    })} ${unit}`;
  if (n >= 1e9) return fmt(n / 1e9, "GB", 2);
  if (n >= 1e6) return fmt(n / 1e6, "MB", 0);
  if (n >= 1e3) return fmt(n / 1e3, "kB", 0);
  return fmt(n, "B", 0);
}

/** Smoothed transfer rate (bytes/s) from the monotonically growing byte
 *  counter the extension streams. EMA over the per-update deltas, so the
 *  display doesn't flicker with every timer tick. */
function useRate(done: number): number {
  const ref = useRef({ last: 0, at: 0, ema: 0 });
  const [rate, setRate] = useState(0);
  useEffect(() => {
    const now = performance.now();
    const r = ref.current;
    if (r.at > 0 && done > r.last) {
      const dt = (now - r.at) / 1000;
      if (dt > 0.05) {
        const inst = (done - r.last) / dt;
        r.ema = r.ema === 0 ? inst : r.ema * 0.8 + inst * 0.2;
        r.last = done;
        r.at = now;
        setRate(r.ema);
      }
    } else if (r.at === 0 || done < r.last) {
      // First sample, or a restarted download: re-baseline.
      ref.current = { last: done, at: now, ema: 0 };
      setRate(0);
    }
  }, [done]);
  return rate;
}

// Project transfer in progress. The bar is spring-animated toward the real
// progress the extension reports, so it glides instead of stepping.
export function Downloading({ state }: { state: ExtState }) {
  const pct = Math.round((state.progress || 0) * 100);
  const lang = resolveLang();
  const done = state.done_bytes || 0;
  const total = state.total_bytes || 0;
  const rate = useRate(done);

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-8 px-10">
      <div className="w-full space-y-3 text-center">
        <p className="text-[13px] text-muted-foreground">{t("joiningLabel")}</p>
        <h2 className="truncate text-lg font-semibold tracking-tight">
          {state.session || t("sessionFallback")}
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
          <span className="tabular-nums">
            {total > 0
              ? `${formatBytes(done, lang)} / ${formatBytes(total, lang)}`
              : t("downloadingProject")}
          </span>
          <span className="tabular-nums">
            {rate > 0 && `${formatBytes(rate, lang)}/s · `}
            {pct}%
          </span>
        </div>
      </div>
      <Button
        variant="ghost"
        size="sm"
        className="text-muted-foreground"
        onClick={() => send({ cmd: "leave" })}
      >
        {t("cancel")}
      </Button>
    </div>
  );
}
