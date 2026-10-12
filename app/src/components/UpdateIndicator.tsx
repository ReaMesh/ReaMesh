import { useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { Popover } from "radix-ui";
import { toast } from "sonner";
import { ArcRing } from "@/components/ArcRing";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import { restartToUpdate, useUpdate } from "@/lib/updater";
import { cn, focusRing } from "@/lib/utils";

// The update's only presence on screen outside Settings, next to the wordmark:
// a small ring while a newer build downloads in the background, then a quiet
// "Update" chip that opens a popover with the restart. Nothing at all while
// idle, checking or up to date, and never a toast or a dialog of its own.
export function UpdateIndicator({ className }: { className?: string }) {
  const update = useUpdate();
  const [open, setOpen] = useState(false);
  const [restarting, setRestarting] = useState(false);

  const restart = async () => {
    setRestarting(true);
    try {
      await restartToUpdate();
    } catch (e) {
      setRestarting(false);
      toast.error(String(e));
    }
  };

  const pct = Math.round(update.progress * 100);
  return (
    <div className={cn("flex items-center", className)}>
      <AnimatePresence mode="wait" initial={false}>
        {update.phase === "downloading" && (
          <motion.span
            key="ring"
            role="status"
            aria-label={t("updateDownloadingTip", { version: update.version, pct })}
            title={t("updateDownloadingTip", { version: update.version, pct })}
            className="relative block size-3.5"
            initial={{ opacity: 0, scale: 0.6 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.6 }}
            transition={{ type: "spring", stiffness: 400, damping: 30 }}
          >
            <ArcRing
              size={14}
              radius={5.5}
              strokeWidth={1.6}
              spinning={update.progress <= 0}
              progress={update.progress}
              color="var(--muted-foreground)"
            />
          </motion.span>
        )}
        {update.phase === "ready" && (
          <motion.span
            key="chip"
            initial={{ opacity: 0, scale: 0.8 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.8 }}
            transition={{ type: "spring", stiffness: 400, damping: 30 }}
          >
            <Popover.Root open={open} onOpenChange={setOpen}>
              <Popover.Trigger asChild>
                <button
                  type="button"
                  className={cn(
                    "flex h-[22px] items-center gap-1.5 rounded-full bg-found/[0.13] pr-2 pl-[7px] text-[11.5px] font-medium text-found transition-colors hover:bg-found/20",
                    focusRing,
                  )}
                >
                  <span aria-hidden="true" className="size-1.5 rounded-full bg-found" />
                  {t("updateChip")}
                </button>
              </Popover.Trigger>
              <Popover.Portal>
                <Popover.Content
                  sideOffset={8}
                  collisionPadding={12}
                  className="z-50 w-60 rounded-xl border bg-popover p-3 shadow-xl outline-none data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95"
                >
                  <p className="text-[13px] font-semibold">
                    {t("updateReadyTitle", { version: update.version })}
                  </p>
                  <p className="mt-0.5 text-[12px] text-muted-foreground">
                    {update.can_restart ? t("updateReloadHint") : t("updateAfterSession")}
                  </p>
                  {update.can_restart && (
                    <div className="mt-3 flex justify-end gap-1.5">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 px-2.5 text-[12px] text-muted-foreground"
                        onClick={() => setOpen(false)}
                      >
                        {t("later")}
                      </Button>
                      <Button
                        size="sm"
                        className="h-7 px-3 text-[12px]"
                        disabled={restarting}
                        onClick={restart}
                      >
                        {t("restartNow")}
                      </Button>
                    </div>
                  )}
                </Popover.Content>
              </Popover.Portal>
            </Popover.Root>
          </motion.span>
        )}
      </AnimatePresence>
    </div>
  );
}
