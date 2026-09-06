import { useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { CheckCircle2, ExternalLink, TriangleAlert } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { FirewallNotice } from "@/components/FirewallNotice";
import { t } from "@/lib/i18n";
import {
  installExtension,
  refreshStatus,
  setReaperPath,
  type ReaperStatus,
} from "@/lib/install";

// First-run setup for locating REAPER, installing the extension, and handing
// control back to the main app.

const BAR_COLORS = ["#f0846b", "#e8d35c", "#7bd88f", "#5b8def"];
const BAR_HEIGHTS = [22, 34, 28, 38];

function LocateStep({ status }: { status: ReaperStatus }) {
  const pick = async () => {
    const dir = await open({
      directory: true,
      title: t("locateReaperFolder"),
    });
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

  return (
    <div className="space-y-3 rounded-xl bg-card p-3.5">
      <div className="space-y-1">
        <h2 className="text-[13px] font-semibold">{t("reaperNotFound")}</h2>
        <p className="text-[12px] leading-relaxed text-muted-foreground">
          {t("reaperNotFoundBody")}
        </p>
      </div>
      <div className="flex items-center gap-2">
        <p className="min-w-0 flex-1 break-all rounded-md bg-secondary/60 px-2.5 py-1.5 text-[12px] leading-relaxed text-foreground/80">
          {status.resourcePath}
        </p>
        <Button
          variant="outline"
          size="sm"
          className="h-7 shrink-0 text-[12px]"
          onClick={pick}
        >
          {t("change")}
        </Button>
      </div>
      <div className="flex gap-2">
        <Button
          size="sm"
          className="h-7 flex-1 gap-1.5 text-[12px]"
          onClick={() => openUrl("https://www.reaper.fm/download.php")}
        >
          {t("downloadReaper")}
          <ExternalLink className="size-3" />
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-7 flex-1 text-[12px]"
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
    <div className="space-y-3 rounded-xl bg-card p-3.5">
      <div className="space-y-1">
        <h2 className="text-[13px] font-semibold">{t("installExtTitle")}</h2>
        <p className="text-[12px] leading-relaxed text-muted-foreground">
          {t("installExtBody")}{" "}
          <span className="break-all text-foreground/80">
            {status.userPlugins}
          </span>
          .
        </p>
      </div>
      {!status.isReaperDir && (
        <p className="flex items-start gap-2 rounded-lg bg-amber-500/10 p-2.5 text-[12px] leading-relaxed text-amber-500">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
          {t("notReaperDirWarning")}
        </p>
      )}
      <Button
        size="sm"
        className="h-7 w-full text-[12px]"
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
    <div className="space-y-3 rounded-xl bg-card p-3.5">
      <div className="space-y-1">
        <h2 className="flex items-center gap-1.5 text-[13px] font-semibold">
          <CheckCircle2 className="size-4 shrink-0 text-emerald-500" />
          {t("allSet")}
        </h2>
        <p className="text-[12px] leading-relaxed text-muted-foreground">
          {t("allSetBody")}
        </p>
      </div>
      <FirewallNotice />
      <Button
        size="sm"
        className="h-7 w-full text-[12px]"
        onClick={() => refreshStatus()}
      >
        {t("finish")}
      </Button>
    </div>
  );
}

export function Onboarding({ status }: { status: ReaperStatus }) {
  // Local "installed just now" flag so the success step stays visible until
  // Finish refreshes the status and the App-level gate moves on. Dev-only:
  // ?step=done jumps straight to the "All set" step for preview (see the
  // ?mock=onboarding harness in main.tsx).
  const [done, setDone] = useState(
    import.meta.env.DEV &&
      new URLSearchParams(window.location.search).get("step") === "done",
  );
  const step = done ? "done" : status.reaperFound ? "install" : "locate";

  return (
    <div className="flex flex-1 flex-col [justify-content:safe_center] overflow-y-auto gap-5 px-6 pb-8">
      <div className="space-y-3 px-0.5">
        <div className="flex h-10 items-end gap-1.5">
          {BAR_COLORS.map((c, i) => (
            <div
              key={c}
              className="w-2.5 rounded-full"
              style={{ backgroundColor: c, height: BAR_HEIGHTS[i] }}
            />
          ))}
        </div>
        <div className="space-y-1">
          <h1 className="text-[15px] font-semibold">{t("welcome")}</h1>
          <p className="text-[13px] text-muted-foreground">{t("tagline")}</p>
        </div>
      </div>
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
