import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { Check, ExternalLink } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { RMark } from "@/components/RMark";
import { FirewallNotice } from "@/components/FirewallNotice";
import { Group } from "@/components/Group";
import { PathField } from "@/components/PathField";
import { t } from "@/lib/i18n";
import {
  installExtension,
  refreshStatus,
  setReaperPath,
  type ReaperStatus,
} from "@/lib/install";
import { cn } from "@/lib/utils";

// First-run setup as one checklist: find REAPER, install the extension,
// restart (or open) REAPER. All three steps stay visible; only the current one opens.

const FOLD = { type: "spring", stiffness: 400, damping: 38 } as const;

/** Unfolds and folds its content on the spring. Spacing lives inside the
 *  content (padding, not a gap or margin), so it travels with the fold. */
function Fold({ show, children }: { show: boolean; children: React.ReactNode }) {
  return (
    <AnimatePresence initial={false}>
      {show && (
        <motion.div
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={FOLD}
          className="overflow-hidden"
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

// The header and the checklist rise in one after the other on first show.
const enter = {
  hidden: { opacity: 0, y: 8 },
  shown: { opacity: 1, y: 0, transition: { duration: 0.32, ease: "easeOut" } },
} as const;

function Step({
  n,
  state,
  title,
  children,
}: {
  n: number;
  state: "done" | "current" | "todo";
  title: string;
  children?: React.ReactNode;
}) {
  return (
    <li className="flex gap-3 px-3 py-3">
      <span
        aria-hidden="true"
        className={cn(
          "mt-px grid size-[22px] shrink-0 place-items-center rounded-full text-[11px] font-semibold tabular-nums transition-colors duration-300",
          state === "done" && "bg-foreground/10 text-foreground",
          state === "current" && "bg-foreground text-background",
          state === "todo" && "bg-white/[0.07] text-muted-foreground",
        )}
      >
        <AnimatePresence initial={false}>
          <motion.span
            key={state === "done" ? "done" : "n"}
            className="col-start-1 row-start-1"
            initial={{ opacity: 0, scale: 0.4 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.4 }}
            transition={{ type: "spring", stiffness: 500, damping: 26 }}
          >
            {state === "done" ? <Check className="size-3" strokeWidth={3} /> : n}
          </motion.span>
        </AnimatePresence>
      </span>
      <div className="min-w-0 flex-1">
        <h2
          className={cn(
            "grid text-[13px] leading-[22px] font-semibold transition-colors duration-300",
            state === "todo" && "text-muted-foreground",
          )}
        >
          <AnimatePresence initial={false}>
            <motion.span
              key={title}
              className="col-start-1 row-start-1"
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -4 }}
              transition={{ duration: 0.2, ease: "easeOut" }}
            >
              {title}
            </motion.span>
          </AnimatePresence>
        </h2>
        <Fold show={state !== "todo" && !!children}>
          <div className="pt-2.5">{children}</div>
        </Fold>
      </div>
    </li>
  );
}

/** Whether REAPER is running, polled while the setup is on screen. Unknown
 *  (no backend, or no answer yet) reads as running, the common case. */
function useReaperRunning(): boolean {
  const [running, setRunning] = useState(true);
  useEffect(() => {
    let alive = true;
    const check = () =>
      invoke<boolean>("reaper_running")
        .then((v) => {
          if (alive) setRunning(v);
        })
        .catch(() => {});
    check();
    const id = setInterval(check, 1500);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, []);
  return running;
}

export function Onboarding({ status }: { status: ReaperStatus }) {
  // Local "installed just now" flag so the last step stays visible until
  // Finish refreshes the status and the App-level gate moves on. Dev-only:
  // ?step=done jumps straight to it for preview (see the ?mock=onboarding
  // harness in main.tsx).
  const [done, setDone] = useState(
    import.meta.env.DEV &&
      new URLSearchParams(window.location.search).get("step") === "done",
  );
  const [busy, setBusy] = useState(false);
  const reaperRunning = useReaperRunning();
  const found = status.reaperFound;

  const pick = async () => {
    const dir = await open({ directory: true, title: t("locateReaperFolder") });
    if (typeof dir === "string") {
      try {
        await setReaperPath(dir);
      } catch (e) {
        toast.error(String(e));
        return;
      }
      await refreshStatus();
    }
  };
  const install = async () => {
    // Installing takes a moment at most, so the button shows no busy state;
    // the flag only swallows a double click.
    if (busy) return;
    setBusy(true);
    try {
      await installExtension();
      setDone(true);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <motion.div
      className="flex flex-1 flex-col [justify-content:safe_center] gap-5 overflow-y-auto px-4 pb-6"
      initial="hidden"
      animate="shown"
      transition={{ staggerChildren: 0.07, delayChildren: 0.05 }}
    >
      <motion.div className="space-y-3 px-1" variants={{ hidden: {}, shown: {} }} transition={{ staggerChildren: 0.07 }}>
        <motion.div variants={enter}>
          <RMark height={40} />
        </motion.div>
        <motion.div className="space-y-1" variants={enter}>
          <h1 className="text-[21px] leading-tight font-semibold tracking-[-0.015em]">{t("welcome")}</h1>
          <p className="text-[13px] text-muted-foreground">{t("tagline")}</p>
        </motion.div>
      </motion.div>

      <motion.div variants={enter}>
        <Group>
          <ol className="divide-y divide-white/[0.07]">
            <Step
              n={1}
              state={found ? "done" : "current"}
              title={found ? t("reaperFound") : t("reaperNotFound")}
            >
              <PathField
                path={status.resourcePath}
                action={
                  <Button variant="ghost" size="sm" className="h-7 shrink-0 px-2.5 text-[12px]" onClick={pick}>
                    {t("change")}
                  </Button>
                }
              />
              <Fold show={!found}>
                <div className="flex gap-2 pt-2.5">
                  <Button
                    size="sm"
                    className="h-8 flex-1 gap-1.5 text-[12px]"
                    onClick={() => openUrl("https://www.reaper.fm/download.php")}
                  >
                    {t("downloadReaper")}
                    <ExternalLink className="size-3" aria-hidden="true" />
                  </Button>
                  <Button
                    variant="secondary"
                    size="sm"
                    className="h-8 flex-1 text-[12px]"
                    onClick={() => refreshStatus()}
                  >
                    {t("checkAgain")}
                  </Button>
                </div>
              </Fold>
            </Step>
            <Step
              n={2}
              state={done ? "done" : found ? "current" : "todo"}
              title={done ? t("allSet") : t("installExtTitle")}
            >
              {!done && (
                <>
                  <Fold show={!status.isReaperDir}>
                    <p className="pb-2.5 text-[12px] leading-normal text-amber-500">{t("notReaperDirWarning")}</p>
                  </Fold>
                  <Button
                    size="sm"
                    className="h-8 w-full text-[13px]"
                    onClick={install}
                  >
                    {t("installExtension")}
                  </Button>
                </>
              )}
            </Step>
            <Step
              n={3}
              state={done ? "current" : "todo"}
              title={reaperRunning ? t("restartReaperStep") : t("openReaper")}
            >
              <FirewallNotice className="pb-2.5" />
              <Button size="sm" className="h-8 w-full text-[13px]" onClick={() => refreshStatus()}>
                {t("finish")}
              </Button>
            </Step>
          </ol>
        </Group>
      </motion.div>
    </motion.div>
  );
}
