// Devices this host has blocked, one row each with "Unblock". Blocks belong to
// the REAPER process and survive re-hosting until they expire or are lifted,
// so the Lobby lists them as well as the hosting screen. Nothing renders while
// the list is empty.

import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { Button } from "@/components/ui/button";
import { Divider, Group, SectionLabel } from "@/components/Group";
import { ListSlot } from "@/components/ListSlot";
import { PeerAvatar } from "@/components/PeerAvatar";
import { send } from "@/lib/connection";
import { t } from "@/lib/i18n";
import type { BanEntry } from "@/lib/types";

function remainingText(expiresAt: number, now: number): string {
  const minutes = Math.ceil((expiresAt - now) / 60_000);
  return minutes >= 1 ? t("blockExpiresIn", { minutes }) : t("blockExpiresSoon");
}

// The section folds in and out on the spring the Lobby's cards unfold on.
const FOLD = { type: "spring", stiffness: 400, damping: 38 } as const;

/** `gap` is the flex gap above the section in its screen: folded away, the
 *  section pulls it in too, so nothing below jumps when it finally goes. */
export function BanList({
  bans,
  port,
  gap,
}: {
  bans: BanEntry[];
  port: number | null;
  gap: number;
}) {
  // Re-render the "N min left" labels: the state frame carries absolute
  // stamps and never changes on its own.
  const [now, setNow] = useState(() => Date.now());
  const any = bans.length > 0;
  useEffect(() => {
    if (!any) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), 15_000);
    return () => clearInterval(id);
  }, [any]);

  const away = { height: 0, marginTop: -gap, opacity: 0, filter: "blur(4px)" };
  return (
    <AnimatePresence initial={false}>
      {any && (
        <motion.section
          key="bans"
          initial={away}
          animate={{ height: "auto", marginTop: 0, opacity: 1, filter: "blur(0px)" }}
          exit={away}
          transition={FOLD}
          className="shrink-0 overflow-hidden"
        >
          <div className="flex flex-col gap-2">
            <SectionLabel>{t("blockedSection")}</SectionLabel>
            <BanRows bans={bans} now={now} port={port} />
          </div>
        </motion.section>
      )}
    </AnimatePresence>
  );
}

function BanRows({ bans, now, port }: { bans: BanEntry[]; now: number; port: number | null }) {
  return (
    <Group role="list" aria-label={t("blockedSection")}>
      {/* A lifted block closes its row the way someone leaves the roster. */}
      <AnimatePresence initial={false}>
        {bans.map((b, i) => (
          <ListSlot key={b.id}>
            <div role="listitem">
              {i > 0 && <Divider />}
              <div className="flex min-h-[52px] items-center gap-3 px-3 py-2">
                {/* Same avatar as the roster, so the host recognises who this was. */}
                <PeerAvatar
                  peer={{ id: b.id, name: b.name, color: b.color, is_self: false, is_owner: false }}
                />
                <div className="flex min-w-0 flex-1 flex-col gap-px">
                  <span className="truncate text-[13px] font-semibold">{b.name || t("guest")}</span>
                  <span className="text-[12px] tabular-nums text-muted-foreground">
                    {remainingText(b.expires_at, now)}
                  </span>
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 shrink-0 px-3 text-[12px]"
                  onClick={() => send({ cmd: "unban", peer_id: b.id }, port ?? undefined)}
                >
                  {t("unblock")}
                </Button>
              </div>
            </div>
          </ListSlot>
        ))}
      </AnimatePresence>
    </Group>
  );
}
