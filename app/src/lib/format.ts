import { useEffect, useRef, useState } from "react";
import { resolveLang } from "./i18n";

/** Compact human-readable byte size ("412 MB", "1.30 GB"), locale-formatted. */
export function formatBytes(n: number, lang = resolveLang()): string {
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

/** Smoothed transfer rate (bytes/s) from a monotonically growing byte counter.
 *  EMA over the per-update deltas, so the display doesn't flicker with every
 *  extension timer tick. Resets when the counter restarts (new download). */
export function useRate(done: number): number {
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
      ref.current = { last: done, at: now, ema: 0 };
      setRate(0);
    }
  }, [done]);
  return rate;
}
