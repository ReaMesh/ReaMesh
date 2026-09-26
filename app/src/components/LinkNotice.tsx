import { useEffect, useState } from "react";
import { t } from "@/lib/i18n";
import type { Peer } from "@/lib/types";

// Slow connections, from the extension's ping RTT samples. RTT measures
// latency; a transfer rate exists only during downloads. The Session roster
// says "Slow connection" at the person it concerns (the host on a client, any
// guest on the host); the download screen says it about the host.

/** RTT threshold (ms) above which a link counts as slow. */
const SLOW_ON = 400;
/** Lower threshold for dropping it again, so a value hovering around the
 *  threshold doesn't flicker. */
const SLOW_OFF = 250;

/** The peer whose link matters, or null when nothing is measured yet. */
export function slowestLink(peers: Peer[], hosting: boolean): { peer: Peer; rtt: number } | null {
  let worst: { peer: Peer; rtt: number } | null = null;
  for (const p of peers) {
    if (p.is_self) continue;
    if (!hosting && !p.is_owner) continue;
    const rtt = p.rtt ?? 0;
    if (rtt <= 0) continue; // 0 = not measured (yet)
    if (!worst || rtt > worst.rtt) worst = { peer: p, rtt };
  }
  return worst;
}

/** Whether a round trip counts as slow, with hysteresis. 0 = not measured. */
export function useSlowLink(rtt: number): boolean {
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    if (rtt >= SLOW_ON) setSlow(true);
    else if (rtt < SLOW_OFF) setSlow(false);
  }, [rtt]);
  return slow;
}

/** One amber line while the connection to the host is slow. */
export function SlowHostLine({ peers }: { peers: Peer[] }) {
  const link = slowestLink(peers, false);
  const rtt = link?.rtt ?? 0;
  const slow = useSlowLink(rtt);
  if (!slow) return null;
  return (
    <p role="status" className="text-[12px] text-amber-500" title={t("slowLinkRtt", { ms: Math.round(rtt) })}>
      {t("slowLinkToHost")}
    </p>
  );
}
