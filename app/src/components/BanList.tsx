// Manage blocked devices from the hosting screen or Lobby. Blocks belong to
// the REAPER process and survive re-hosting until they expire or are revoked.
// Hide the control when the list is empty.

import { useEffect, useState } from "react";
import { Gavel } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Hint } from "@/components/Hint";
import { PeerAvatar } from "@/components/PeerAvatar";
import { send } from "@/lib/connection";
import { t } from "@/lib/i18n";
import type { BanEntry } from "@/lib/types";
import { cn, focusRing } from "@/lib/utils";

function remainingText(expiresAt: number, now: number): string {
  const minutes = Math.ceil((expiresAt - now) / 60_000);
  return minutes >= 1 ? t("blockExpiresIn", { minutes }) : t("blockExpiresSoon");
}

export function BanList({
  bans,
  port,
  variant = "line",
}: {
  bans: BanEntry[];
  port: number | null;
  /** "line": a one-line text link (Lobby). "icon": a ban-hammer button with a
   *  count, for the bottom-right corner of the hosting screen. */
  variant?: "line" | "icon";
}) {
  const [open, setOpen] = useState(false);
  // Re-render the "N min left" labels while the dialog is open; the state
  // frame itself carries absolute stamps and never changes on its own.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!open) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), 15_000);
    return () => clearInterval(id);
  }, [open]);
  // Close when the last block lapses or is lifted.
  useEffect(() => {
    if (open && bans.length === 0) setOpen(false);
  }, [open, bans.length]);
  if (bans.length === 0) return null;
  const label = bans.length === 1 ? t("blockedOne") : t("blockedMany", { count: bans.length });

  return (
    <>
      {variant === "icon" ? (
        <Hint label={label} side="top">
          <button
            onClick={() => setOpen(true)}
            aria-label={label}
            className={cn(
              "relative flex size-8 shrink-0 items-center justify-center rounded-lg bg-secondary text-muted-foreground transition-colors hover:text-foreground",
              focusRing,
            )}
          >
            <Gavel className="size-4" aria-hidden="true" />
            <span
              aria-hidden="true"
              className="absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-red-500 px-1 text-[10px] font-semibold tabular-nums text-white"
            >
              {bans.length}
            </span>
          </button>
        </Hint>
      ) : (
        <button
          onClick={() => setOpen(true)}
          className={cn(
            "flex shrink-0 items-center gap-1.5 self-start rounded px-1 text-[11px] text-muted-foreground transition-colors hover:text-foreground",
            focusRing,
          )}
        >
          <Gavel className="size-3" aria-hidden="true" />
          <span>{label}</span>
        </button>
      )}

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-[360px] gap-4 rounded-2xl">
          <DialogHeader>
            <DialogTitle className="text-[15px]">{t("blockedTitle")}</DialogTitle>
            <DialogDescription className="text-[13px] leading-relaxed">
              {t("blockedBody")}
            </DialogDescription>
          </DialogHeader>
          <ul className="flex max-h-64 flex-col gap-1 overflow-y-auto" aria-label={t("blockedTitle")}>
            {bans.map((b) => {
              const name = b.name || t("guest");
              return (
                <li key={b.id} className="flex items-center gap-3 rounded-lg px-2 py-1.5">
                  {/* Same avatar as the roster, so the host recognises who this was. */}
                  <PeerAvatar
                    peer={{ id: b.id, name: b.name, color: b.color, is_self: false, is_owner: false }}
                  />
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="truncate text-[13px] font-medium">{name}</span>
                    <span className="text-[11px] tabular-nums text-muted-foreground">
                      {remainingText(b.expires_at, now)}
                    </span>
                  </span>
                  <Button
                    variant="outline"
                    size="xs"
                    onClick={() => send({ cmd: "unban", peer_id: b.id }, port ?? undefined)}
                  >
                    {t("unblock")}
                  </Button>
                </li>
              );
            })}
          </ul>
          <DialogFooter className="gap-2 sm:gap-2">
            <Button variant="outline" className="flex-1" onClick={() => setOpen(false)}>
              {t("close")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
