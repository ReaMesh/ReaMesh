import { useState } from "react";
import { Check, ExternalLink, TriangleAlert } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { BarMark } from "@/components/BarMark";
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
// restart REAPER. All three steps stay visible; only the current one opens.

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
          "mt-px grid size-[22px] shrink-0 place-items-center rounded-full text-[11px] font-semibold tabular-nums",
          state === "done" && "bg-emerald-500/15 text-emerald-400",
          state === "current" && "bg-foreground text-background",
          state === "todo" && "bg-white/[0.07] text-muted-foreground",
        )}
      >
        {state === "done" ? <Check className="size-3" strokeWidth={3} /> : n}
      </span>
      <div className="min-w-0 flex-1 space-y-2.5">
        <h2
          className={cn(
            "text-[13px] leading-[22px] font-semibold",
            state === "todo" && "text-muted-foreground",
          )}
        >
          {title}
        </h2>
        {state !== "todo" && children}
      </div>
    </li>
  );
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
    <div className="flex flex-1 flex-col [justify-content:safe_center] gap-5 overflow-y-auto px-4 pb-6">
      <div className="space-y-3 px-1">
        <BarMark height={38} />
        <div className="space-y-1">
          <h1 className="text-[21px] leading-tight font-semibold tracking-[-0.015em]">{t("welcome")}</h1>
          <p className="text-[13px] text-muted-foreground">{t("tagline")}</p>
        </div>
      </div>

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
            {!found && (
              <div className="flex gap-2">
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
            )}
          </Step>
          <Step
            n={2}
            state={done ? "done" : found ? "current" : "todo"}
            title={done ? t("allSet") : t("installExtTitle")}
          >
            {!done && (
              <>
                {!status.isReaperDir && (
                  <p className="flex items-start gap-2 rounded-[10px] bg-amber-500/10 px-2.5 py-2 text-[12px] leading-snug text-amber-500">
                    <TriangleAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
                    {t("notReaperDirWarning")}
                  </p>
                )}
                <Button size="sm" className="h-8 w-full text-[13px]" disabled={busy} onClick={install}>
                  {busy ? t("installing") : t("installExtension")}
                </Button>
              </>
            )}
          </Step>
          <Step n={3} state={done ? "current" : "todo"} title={t("restartReaperStep")}>
            <FirewallNotice />
            <Button size="sm" className="h-8 w-full text-[13px]" onClick={() => refreshStatus()}>
              {t("finish")}
            </Button>
          </Step>
        </ol>
      </Group>
    </div>
  );
}
