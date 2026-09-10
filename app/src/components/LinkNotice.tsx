import { useEffect, useState } from "react";
import { Wifi } from "lucide-react";
import { t } from "@/lib/i18n";
import { displayName, type Peer } from "@/lib/types";

// Show a slow-connection notice from the extension's ping RTT samples.
// Clients use the host's RTT; hosts use the slowest guest's. RTT measures
// latency, while transfer rate is available only during downloads.

/** RTT threshold (ms) for showing the notice. */
const SLOW_ON = 400;
/** Lower dismissal threshold to avoid flicker near the activation threshold. */
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

export function LinkNotice({
  peers,
  hosting,
  /** Downloading: word the consequence as a slow transfer, not laggy edits. */
  downloading = false,
}: {
  peers: Peer[];
  hosting: boolean;
  downloading?: boolean;
}) {
  const link = slowestLink(peers, hosting);
  const rtt = link?.rtt ?? 0;
  const [shown, setShown] = useState(false);
  useEffect(() => {
    if (rtt >= SLOW_ON) setShown(true);
    else if (rtt < SLOW_OFF) setShown(false);
  }, [rtt]);
  if (!shown || !link) return null;

  const who = link.peer.is_owner ? t("slowLinkHost") : displayName(link.peer);
  return (
    <div
      role="status"
      className="space-y-1 rounded-lg bg-sky-500/10 p-2.5 text-left"
      title={t("slowLinkRtt", { ms: Math.round(rtt) })}
    >
      <p className="flex items-start gap-2 text-[12px] font-semibold leading-relaxed text-sky-300">
        <Wifi className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
        {t("slowLinkTitle", { who })}
      </p>
      <p className="text-[12px] leading-relaxed text-sky-300/90">
        {downloading ? t("slowLinkBodyDownload") : t("slowLinkBody")}
      </p>
    </div>
  );
}
