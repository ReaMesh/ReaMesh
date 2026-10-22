import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { toast } from "sonner";
import { ArcRing } from "@/components/ArcRing";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import { restartToUpdate, useUpdate } from "@/lib/updater";
import { cn, focusRing } from "@/lib/utils";

// The update's only presence on screen outside Settings, next to the wordmark:
// a small ring while a newer build downloads in the background, then a quiet
// "Update" chip that opens into a panel with the restart. Nothing at all while
// idle, checking or up to date, and never a toast or a dialog of its own.
export function UpdateIndicator({ className }: { className?: string }) {
  const update = useUpdate();
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
            <ReadyChip version={update.version} canRestart={update.can_restart} />
          </motion.span>
        )}
      </AnimatePresence>
    </div>
  );
}

// The chip grows into its own panel: size, corners and fill ride one spring,
// the contents come into focus once it is under way. Folding back has no
// spring, so the panel never wobbles on its way into the chip.
const GROW = { type: "spring", stiffness: 380, damping: 34, mass: 0.9 } as const;
const FOLD = { duration: 0.24, ease: [0.32, 0.72, 0, 1] } as const;
const EASE_OUT = [0.16, 1, 0.3, 1] as const;
const PANEL_W = 240;
const EDGE = 12; // keep the panel this far inside the window
// Literal colours: Motion can't tween between CSS variables.
const CHIP_FILL = "rgba(106, 166, 240, 0.13)"; // --found at 13 %
const PANEL_FILL = "#1c1c21"; // --popover
const CHIP_EDGE = "inset 0 0 0 1px rgba(255, 255, 255, 0), 0 16px 40px rgba(0, 0, 0, 0)";
const PANEL_EDGE = "inset 0 0 0 1px rgba(255, 255, 255, 0.08), 0 16px 40px rgba(0, 0, 0, 0.45)";

const chipLabel = "flex h-[22px] items-center gap-1.5 pr-2 pl-[7px] text-[11.5px] font-medium text-found";

function ReadyChip({ version, canRestart }: { version: string; canRestart: boolean }) {
  const reduce = useReducedMotion();
  const chip = useRef<HTMLButtonElement>(null);
  const surface = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  // Where the chip sat when it opened; set for as long as the surface is up,
  // which outlasts `open` by the fold back into the chip.
  const [from, setFrom] = useState<DOMRect | null>(null);
  const [open, setOpen] = useState(false);
  const [restarting, setRestarting] = useState(false);

  const show = () => {
    const r = chip.current?.getBoundingClientRect();
    if (!r) return;
    setFrom(r);
    setOpen(true);
  };
  const hide = () => {
    if (surface.current?.contains(document.activeElement)) chip.current?.focus({ preventScroll: true });
    setOpen(false);
  };

  useEffect(() => {
    if (!open) return;
    panel.current?.focus({ preventScroll: true });
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && hide();
    const onDown = (e: PointerEvent) => {
      if (!surface.current?.contains(e.target as Node)) hide();
    };
    // The panel is placed from where the chip was: a resized window closes it.
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("resize", hide);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("resize", hide);
    };
  }, [open]);

  const restart = async () => {
    setRestarting(true);
    try {
      await restartToUpdate();
    } catch (e) {
      setRestarting(false);
      toast.error(String(e));
    }
  };

  const title = t("updateReadyTitle", { version });
  // Open from the chip's left edge, nudged inward when that would run past
  // the window (the centred macOS wordmark leaves little room on the right).
  const dx = from
    ? Math.min(Math.max(from.left, EDGE), window.innerWidth - EDGE - PANEL_W) - from.left
    : 0;
  const folded = from && {
    width: from.width,
    height: from.height,
    x: 0,
    borderRadius: from.height / 2,
    backgroundColor: CHIP_FILL,
    boxShadow: CHIP_EDGE,
  };
  const grown = {
    width: PANEL_W,
    height: "auto",
    x: dx,
    borderRadius: 12,
    backgroundColor: PANEL_FILL,
    boxShadow: PANEL_EDGE,
  };

  return (
    <>
      <button
        ref={chip}
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={show}
        // Hidden, not removed, while the surface stands in for it, so focus
        // can come back to it.
        className={cn(
          chipLabel,
          "rounded-full bg-found/[0.13] transition-colors hover:bg-found/20",
          from && "opacity-0",
          focusRing,
        )}
      >
        <span aria-hidden="true" className="size-1.5 rounded-full bg-found" />
        {t("updateChip")}
      </button>
      {from &&
        folded &&
        createPortal(
          <motion.div
            ref={surface}
            className="fixed z-50 overflow-hidden"
            style={{ left: from.left, top: from.top }}
            initial={folded}
            animate={open ? grown : folded}
            transition={reduce ? { duration: 0.12 } : open ? GROW : FOLD}
            onAnimationComplete={() => !open && setFrom(null)}
          >
            {/* The chip's own label, fading out of the way as it grows. */}
            <motion.span
              aria-hidden="true"
              className={cn(chipLabel, "pointer-events-none absolute top-0 left-0")}
              initial={false}
              animate={
                open
                  ? { opacity: 0, filter: "blur(4px)" }
                  : { opacity: 1, filter: "blur(0px)" }
              }
              transition={open ? { duration: 0.12 } : { duration: 0.16, delay: 0.1 }}
            >
              <span className="size-1.5 rounded-full bg-found" />
              {t("updateChip")}
            </motion.span>
            <AnimatePresence initial={false}>
              {open && (
                <motion.div
                  key="panel"
                  ref={panel}
                  role="dialog"
                  aria-label={title}
                  tabIndex={-1}
                  style={{ width: PANEL_W }}
                  className="origin-top-left p-3 outline-none"
                  initial={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.92, filter: "blur(6px)" }}
                  animate={{
                    opacity: 1,
                    scale: 1,
                    filter: "blur(0px)",
                    transition: { duration: 0.24, ease: EASE_OUT, delay: reduce ? 0 : 0.07 },
                  }}
                  exit={{ opacity: 0, filter: "blur(4px)", transition: { duration: 0.12 } }}
                >
                  <p className="text-[13px] font-semibold">{title}</p>
                  <p className="mt-0.5 text-[12px] text-muted-foreground">
                    {canRestart ? t("updateReloadHint") : t("updateAfterSession")}
                  </p>
                  {canRestart && (
                    <div className="mt-3 flex justify-end gap-1.5">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 px-2.5 text-[12px] text-muted-foreground"
                        onClick={hide}
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
                </motion.div>
              )}
            </AnimatePresence>
          </motion.div>,
          document.body,
        )}
    </>
  );
}
