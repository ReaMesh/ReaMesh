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

/** No bytes for this long and the transfer is treated as stalled: the rate
 *  decays and the UI stops promising a time. The Rust side gives a stall a full
 *  60s before failing the join, which is a long time to sit under a confident
 *  "About 7s to go". */
export const STALL_AFTER_MS = 6000;

export interface Transfer {
  /** Smoothed bytes/s. Decays toward 0 while nothing arrives, so it can't sit
   *  frozen at the last optimistic value through an entire stall. */
  rate: number;
  /** True once nothing has arrived for `STALL_AFTER_MS`. */
  stalled: boolean;
}

/** Smooth byte-counter deltas with an EMA. A timer decays the rate during
 *  stalls even when the counter stops changing. The first update after the
 *  manifest establishes the baseline, since it includes reused files that
 *  were never transferred over the network. */
export function useTransfer(done: number, total: number): Transfer {
  const ref = useRef({ last: 0, at: 0, ema: 0, seenTotal: false });
  const [state, setState] = useState<Transfer>({ rate: 0, stalled: false });

  useEffect(() => {
    const now = performance.now();
    const r = ref.current;

    // New (or restarted) download: rebaseline and report nothing yet.
    if (r.at === 0 || done < r.last) {
      ref.current = { last: done, at: now, ema: 0, seenTotal: total > 0 };
      setState({ rate: 0, stalled: false });
      return;
    }
    // The manifest just arrived and brought the deduped/already-present bytes
    // with it: a baseline, not a transfer.
    if (!r.seenTotal && total > 0) {
      r.seenTotal = true;
      r.last = done;
      r.at = now;
      setState({ rate: 0, stalled: false });
      return;
    }
    if (done > r.last) {
      const dt = (now - r.at) / 1000;
      if (dt > 0.05) {
        const inst = (done - r.last) / dt;
        r.ema = r.ema === 0 ? inst : r.ema * 0.8 + inst * 0.2;
        r.last = done;
        r.at = now;
        setState({ rate: r.ema, stalled: false });
      }
    }
  }, [done, total]);

  // Age the last sample even when nothing arrives.
  useEffect(() => {
    const id = setInterval(() => {
      const r = ref.current;
      if (r.at === 0) return;
      const idle = performance.now() - r.at;
      if (idle < STALL_AFTER_MS / 2) return;
      // Decay the rate with a half-life of STALL_AFTER_MS/2; set it to zero
      // and report a stall once the threshold is reached.
      const stalled = idle >= STALL_AFTER_MS;
      r.ema = stalled ? 0 : r.ema * 0.5;
      setState({ rate: r.ema, stalled });
    }, 1000);
    return () => clearInterval(id);
  }, []);

  return state;
}
