import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  CheckCircle2,
  ExternalLink,
  FolderSearch,
  Puzzle,
  TriangleAlert,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import {
  installExtension,
  refreshStatus,
  setReaperPath,
  type ReaperStatus,
} from "@/lib/install";

// First-run onboarding: shown until the bundled REAPER extension is
// installed. Two entry states: REAPER's folder wasn't found (point us at
// it), or it was (one-click install), plus a success step that hands off
// to the normal app on Finish.

const BAR_COLORS = ["#f0846b", "#e8d35c", "#7bd88f", "#5b8def"];
const BAR_HEIGHTS = [22, 34, 28, 38];

function Wordmark() {
  // After the entrance springs land, the bars sway through a few slow,
  // desynced equalizer passes (this is a DAW tool, after all) and settle.
  const [settled, setSettled] = useState(false);
  const reduce = useReducedMotion();
  useEffect(() => {
    const id = setTimeout(() => setSettled(true), 1100);
    return () => clearTimeout(id);
  }, []);
  return (
    <div className="flex flex-col items-center gap-4">
      <div className="flex h-10 items-end gap-1.5">
        {BAR_COLORS.map((c, i) => (
          <motion.div
            key={c}
            className="w-2.5 rounded-full"
            style={{ backgroundColor: c }}
            initial={{ height: 8, opacity: 0 }}
            animate={
              settled && !reduce
                ? {
                    height: [
                      BAR_HEIGHTS[i],
                      BAR_HEIGHTS[i] * 0.82,
                      BAR_HEIGHTS[i] * 1.1,
                      BAR_HEIGHTS[i] * 0.92,
                      BAR_HEIGHTS[i],
                    ],
                    opacity: 1,
                  }
                : { height: BAR_HEIGHTS[i], opacity: 1 }
            }
            transition={
              settled && !reduce
                ? {
                    height: {
                      duration: 2.2 + i * 0.25,
                      ease: "easeInOut",
                      repeat: 2,
                      delay: i * 0.15,
                    },
                    opacity: { duration: 0.2 },
                  }
                : { type: "spring", stiffness: 350, damping: 28, delay: 0.08 * i }
            }
          />
        ))}
      </div>
      <div className="space-y-1 text-center">
        <h1 className="text-lg font-semibold tracking-tight">{t("welcome")}</h1>
        <p className="text-[13px] text-muted-foreground">{t("tagline")}</p>
      </div>
    </div>
  );
}

function LocateStep({ status }: { status: ReaperStatus }) {
  const pick = async () => {
    const dir = await open({
      directory: true,
      title: t("locateReaperFolder"),
    });
    if (typeof dir === "string") {
      await setReaperPath(dir);
      await refreshStatus();
    }
  };

  return (
    <div className="space-y-4 rounded-xl bg-card p-4">
      <div className="flex items-start gap-3">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-amber-500/15">
          <FolderSearch className="size-4.5 text-amber-500" />
        </div>
        <div className="min-w-0 space-y-1">
          <h2 className="text-[14px] font-semibold">{t("reaperNotFound")}</h2>
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            {t("reaperNotFoundBody")}
          </p>
        </div>
      </div>
      <div className="flex items-center gap-2">
        <p className="min-w-0 flex-1 break-all rounded-md bg-secondary/60 px-2.5 py-1.5 text-[12px] leading-relaxed text-foreground/80">
          {status.resourcePath}
        </p>
        <Button
          variant="outline"
          size="sm"
          className="h-8 shrink-0 text-[12px]"
          onClick={pick}
        >
          {t("change")}
        </Button>
      </div>
      <div className="flex gap-2">
        <Button
          size="sm"
          className="h-8 flex-1 gap-1.5 text-[12px]"
          onClick={() => openUrl("https://www.reaper.fm/download.php")}
        >
          {t("downloadReaper")}
          <ExternalLink className="size-3" />
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-8 flex-1 text-[12px]"
          onClick={() => refreshStatus()}
        >
          {t("checkAgain")}
        </Button>
      </div>
    </div>
  );
}

function InstallStep({
  status,
  onInstalled,
}: {
  status: ReaperStatus;
  onInstalled: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const install = async () => {
    setBusy(true);
    try {
      await installExtension();
      onInstalled();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4 rounded-xl bg-card p-4">
      <div className="flex items-start gap-3">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-secondary">
          <Puzzle className="size-4.5 text-foreground/80" />
        </div>
        <div className="min-w-0 space-y-1">
          <h2 className="text-[14px] font-semibold">{t("installExtTitle")}</h2>
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            {t("installExtBody")}{" "}
            <span className="break-all text-foreground/80">
              {status.userPlugins}
            </span>
            .
          </p>
        </div>
      </div>
      {!status.isReaperDir && (
        <p className="flex items-start gap-2 rounded-lg bg-amber-500/10 p-2.5 text-[12px] leading-relaxed text-amber-500">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
          {t("notReaperDirWarning")}
        </p>
      )}
      <Button
        size="sm"
        className="h-8 w-full text-[12px]"
        disabled={busy}
        onClick={install}
      >
        {busy ? t("installing") : t("installExtension")}
      </Button>
    </div>
  );
}

function DoneStep() {
  return (
    <div className="space-y-4 rounded-xl bg-card p-4">
      <div className="flex items-start gap-3">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-emerald-500/15">
          <CheckCircle2 className="size-4.5 text-emerald-500" />
        </div>
        <div className="min-w-0 space-y-1">
          <h2 className="text-[14px] font-semibold">{t("allSet")}</h2>
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            {t("allSetBody")}
          </p>
        </div>
      </div>
      <Button
        size="sm"
        className="h-8 w-full text-[12px]"
        onClick={() => refreshStatus()}
      >
        {t("finish")}
      </Button>
    </div>
  );
}

export function Onboarding({ status }: { status: ReaperStatus }) {
  // Local "installed just now" flag so the success step stays visible until
  // Finish refreshes the status and the App-level gate moves on.
  const [done, setDone] = useState(false);
  const step = done ? "done" : status.reaperFound ? "install" : "locate";

  return (
    <div className="flex flex-1 flex-col justify-center gap-8 px-6 pb-10">
      <Wordmark />
      <AnimatePresence mode="wait">
        <motion.div
          key={step}
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -8 }}
          transition={{ duration: 0.18, ease: "easeOut" }}
        >
          {step === "done" ? (
            <DoneStep />
          ) : step === "install" ? (
            <InstallStep status={status} onInstalled={() => setDone(true)} />
          ) : (
            <LocateStep status={status} />
          )}
        </motion.div>
      </AnimatePresence>
    </div>
  );
}
